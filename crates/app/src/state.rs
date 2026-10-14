//! App state and the pure `update` function.
//!
//! `update` only mutates state, queues commands without blocking, and returns
//! [`Task`]s. It never awaits, never touches the network and never sees an SDK
//! type.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use iced::theme;
use iced::widget::{operation, scrollable};
use iced::{Element, Subscription, Task, Theme, clipboard, system, window};
use tokio::sync::mpsc;
use twicecat_core::api::{
    ActionKind, Command, CoreError, CoreEvent, Draft, EventId, ListDiff, MediaBytes, MediaKind,
    MediaSize, MessageDetails, MessageText, NotificationKind, NotificationSettingsView,
    NotificationView, OutgoingMessage, Password, ReauthAction, RoomId, RoomListQuery, RoomListTab,
    RoomNotificationModeView, RoomSummary, SyncState, TimelineItem, TimelineItemKind, TypingUser,
    UiSettings, UserId, UserTrust,
};

use crate::actions::{self, ActionButtons, ActionState, NOTICE_DURATION, Notice, Picker};
use crate::avatars::Avatars;
use crate::blocking;
use crate::bridge::{self, Bridge, EventsSlot};
use crate::calls::{self, CallMessage, CallModel};
use crate::composer::{self, Composer, ComposerKey, EditorAction, SearchStep};
use crate::dialogs::{self, Clipboard, ClipboardContent, Dialogs, SystemClipboard, SystemDialogs};
use crate::emoji;
use crate::keys;
use crate::links::{self, LinkBar};
use crate::media::{self, Decoded, MediaModel, SavePath};
use crate::notifications::{self, Folding, ModeChoice, NoNotifier, Notifier};
use crate::ptt::{PttMessage, PttModel, PttStarter, ServiceStarter};
use crate::rich_text::{LinkTarget, MAX_REVEALED, Revealed};
use crate::room_list;
use crate::rooms::{RoomsMessage, RoomsModel};
use crate::screens::login::{self, BrowserUrl, LoginForm};
use crate::screens::{self, loading, main_view};
use crate::security::{Busy, SecurityMessage, SecurityModel};
use crate::sessions::{SessionsMessage, SessionsModel, logout_needs_warning};
use crate::settings::{NoSettingsStore, SettingsMessage, SettingsModel, SettingsStore};
use crate::spaces::{SpacesMessage, SpacesModel};
use crate::text_safety;
use crate::threads::{ThreadsMessage, ThreadsModel};
use crate::timeline::{self, Effects, TimelineView};
use crate::tray::{self, NoTrayFactory, SystemTrayFactory, TrayAction, TrayFactory, TrayModel};
use crate::upload::{self, Outcome, UploadModel, UploadPaths};
use crate::verification::{VerificationMessage, VerificationModel};

/// How many users' trust the app keeps: the selected peer, and a stale one or
/// two from watchers the core replaced.
pub const MAX_TRUST_KEPT: usize = 8;

/// How many room mode choices the app keeps until the list shows them.
pub const MAX_LOCAL_MODES: usize = 64;

/// How long to wait for the core to close after `Shutdown` (plan Q18).
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);
/// Pause after the last composer change before its draft is stored.
pub const DRAFT_SAVE_DELAY: Duration = Duration::from_secs(1);
/// The notice for a command without its text, such as `/me` alone
/// (Element's wording).
pub const COMMAND_ERROR_TEXT: &str = "Command error: Unable to handle slash command.";
/// The notice for command `name` sent without its text, in Element's words:
/// its usage line, except for `/me`, whose Element error is the generic one.
pub fn command_error_text(name: &str) -> String {
    if name == "me" {
        COMMAND_ERROR_TEXT.to_owned()
    } else {
        format!("Command error: Usage: /{name} <message>")
    }
}

/// The notice when a reply's original is not found.
pub const JUMP_FAILED_TEXT: &str =
    "Could not find the original message (searched about 1,000 earlier messages)";

/// Starts the system handler for a checked link, away from `update`.
/// Brings the window to the front: un-minimises it and takes the focus.
fn focus_window() -> Task<Message> {
    window::oldest()
        .and_then(|id| Task::batch([window::minimize(id, false), window::gain_focus(id)]))
}

/// Asks the window system to flag the window (taskbar, dock) without taking
/// the focus.
fn attention() -> Task<Message> {
    attention_at(window::UserAttention::Informational)
}

/// [`attention`] at a chosen level: `Critical` keeps flagging until the
/// window is focused (a ringing call).
pub(crate) fn attention_at(level: window::UserAttention) -> Task<Message> {
    window::oldest().and_then(move |id| window::request_user_attention(id, Some(level)))
}

fn open_link_task(url: links::CheckedUrl) -> Task<Message> {
    Task::perform(async move { links::open_link(&url) }, |ok| {
        Message::LinkOpened { ok }
    })
}

/// Where commands go. The [`Bridge`] in production; a plain channel sender in
/// tests, so `update` can be tested without a runtime.
pub trait CommandSink: Send {
    /// Queues a command without blocking. `None` when queued; `Some` task when
    /// the channel was full (the task queues it later).
    fn send(&self, command: Command) -> Option<Task<Message>>;

    /// A task that yields [`Message::ShutdownDeadline`] after `after`.
    fn deadline(&self, after: Duration) -> Task<Message>;

    /// A task that yields `message` after `after`.
    fn timer(&self, after: Duration, message: Message) -> Task<Message>;

    /// The core's command channel, for push-to-talk's forwarding thread,
    /// which sends outside `update`.
    fn command_sender(&self) -> Option<mpsc::Sender<Command>>;

    /// The core's runtime, where the shortcuts portal runs.
    fn runtime(&self) -> Option<tokio::runtime::Handle>;
}

impl CommandSink for Bridge {
    fn send(&self, command: Command) -> Option<Task<Message>> {
        Bridge::send(self, command)
    }

    fn deadline(&self, after: Duration) -> Task<Message> {
        Bridge::deadline(self, after)
    }

    fn timer(&self, after: Duration, message: Message) -> Task<Message> {
        Bridge::timer(self, after, message)
    }

    fn command_sender(&self) -> Option<mpsc::Sender<Command>> {
        Some(Bridge::command_sender(self))
    }

    fn runtime(&self) -> Option<tokio::runtime::Handle> {
        Some(Bridge::runtime(self))
    }
}

impl CommandSink for mpsc::Sender<Command> {
    fn send(&self, command: Command) -> Option<Task<Message>> {
        bridge::try_send(self, command)
            .err()
            .map(bridge::Pending::into_task)
    }

    /// Tests have no runtime to time on: no deadline.
    fn deadline(&self, _after: Duration) -> Task<Message> {
        Task::none()
    }

    /// Likewise: tests call the timer's handler themselves.
    fn timer(&self, _after: Duration, _message: Message) -> Task<Message> {
        Task::none()
    }

    fn command_sender(&self) -> Option<mpsc::Sender<Command>> {
        Some(self.clone())
    }

    /// Tests start no real service.
    fn runtime(&self) -> Option<tokio::runtime::Handle> {
        None
    }
}

/// Which screen the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Waiting for the answer to `RestoreSession`.
    Starting,
    /// Logged out; the login form.
    Login,
    /// The OAuth URL was handed to the browser; waiting for the user.
    WaitingForBrowser,
    /// Logged in, waiting for the first room list.
    Loading,
    /// The two-pane main layout.
    Main,
    /// The window was closed; waiting for the core to stop.
    ShuttingDown,
    /// The core stopped on its own.
    Disconnected,
}

/// Everything that can happen to the app.
#[derive(Debug, Clone)]
pub enum Message {
    /// An event from the core.
    Core(Box<CoreEvent>),
    /// The core's event channel closed.
    CoreClosed,
    /// The system theme mode, at boot or after a change.
    ThemeMode(theme::Mode),
    /// The server field changed.
    ServerChanged(String),
    /// The user name field changed.
    UsernameChanged(String),
    /// The password field changed; its `Debug` is redacted.
    PasswordChanged(Password),
    /// The user submitted the login form (Enter or **Log in**).
    LoginSubmitted,
    /// The user pressed **Sign in with browser**.
    OAuthRequested,
    /// The user pressed **Use another server**.
    ToggleServerField,
    /// The user pressed **Cancel** while waiting for the browser.
    CancelOAuth,
    /// The browser launcher finished.
    BrowserOpened {
        /// Whether the launcher started without error.
        ok: bool,
    },
    /// The user pressed **Copy link**.
    CopyLink,
    /// An action in the composer's text editor; its `Debug` shows only the
    /// kind of action.
    Composer(EditorAction),
    /// A key command in the composer (send, suggestion keys, escape).
    ComposerKey(ComposerKey),
    /// The user clicked suggestion `index` under the composer.
    SuggestionPicked(usize),
    /// The member search timer fired.
    MembersDue,
    /// The pointer entered a message.
    HoverEnter(String),
    /// The pointer left a message.
    HoverExit(String),
    /// **Reply** on a message.
    ReplyRequested(String),
    /// **Edit** on a message.
    EditRequested(String),
    /// **Copy** on a message.
    CopyRequested(String),
    /// **Delete** on a message (asks for confirmation).
    DeleteRequested(String),
    /// The user confirmed the pending delete.
    DeleteConfirmed,
    /// The user backed out of the pending delete.
    DeleteCancelled,
    /// The user pressed the reply bar's cross (reply or edit mode).
    ModeCancelled,
    /// **React** on a message opens the picker.
    PickerOpened(String),
    /// The picker's search field changed; the text is not logged.
    PickerQuery(MessageText),
    /// The user pressed Enter in the picker's search field.
    PickerSubmitted,
    /// The user picked an emoji in the picker; the key is not logged.
    PickerPicked(MessageText),
    /// The user closed the picker.
    PickerClosed,
    /// The user clicked a reaction chip; the key is not logged.
    ReactionToggled {
        /// The message.
        item: String,
        /// The reaction.
        key: MessageText,
    },
    /// The failure notice with this sequence number timed out.
    NoticeExpired(u64),
    /// A reply's quote was clicked: jump to the quoted message.
    ReplyQuoteClicked(EventId),
    /// The highlight of the row jumped to timed out.
    FlashEnded(String),
    /// The pause after a composer change ended: store the draft if this is
    /// still the last change.
    DraftDue(u64),
    /// The user pressed **Log out**.
    LogoutRequested,
    /// The user picked a room in the list.
    SelectRoom(RoomId),
    /// The user picked a tab above the room list.
    TabSelected(RoomListTab),
    /// The room list search field changed.
    SearchChanged(String),
    /// The room list scrolled, or its size changed.
    RoomListScrolled {
        /// Pixels scrolled from the top.
        offset_y: f32,
        /// Height of the visible part, in pixels.
        viewport_h: f32,
    },
    /// The pane measured a timeline row.
    RowMeasured {
        /// The item's ID.
        id: String,
        /// Its height in pixels.
        height: f32,
    },
    /// The timeline scrolled, or its size changed.
    TimelineScrolled(scrollable::Viewport),
    /// The user clicked a link, pill or spoiler in a message.
    LinkClicked(LinkTarget),
    /// The user confirmed opening the link in the confirmation bar.
    LinkConfirmed,
    /// The user closed the link bar.
    LinkDismissed,
    /// "Send as message" on the unknown-command bar.
    CommandSendAnyway,
    /// The unknown-command bar was closed; the text stays in the composer.
    CommandCancelled,
    /// The system handler was started (or failed to start) for a link.
    LinkOpened {
        /// Whether the handler started without error.
        ok: bool,
    },
    /// The window gained or lost focus.
    WindowFocus(bool),
    /// The wait of a newly shown incoming-call card ended (its sequence).
    /// View state only: not a call message.
    RingArmed(u64),
    /// A key of Ctrl+Shift+A or Ctrl+Shift+X came up.
    RingChordReleased,
    /// The window was resized to this logical width.
    WindowResized(f32),
    /// The timer of a delayed read receipt ended.
    ReadDue(RoomId),
    /// **Retry** on a failed local echo.
    RetryRequested(String),
    /// **Discard** on a failed local echo.
    DiscardRequested(String),
    /// The user asked to close the window.
    CloseRequested,
    /// The core did not close in time after `Shutdown`.
    ShutdownDeadline,
    /// **Verify** in the header of a direct message.
    VerifyPeerRequested(UserId),
    /// A message of the security banner and screens.
    Security(SecurityMessage),
    /// A button of the verification dialog.
    Verification(VerificationMessage),
    /// A button of the Sessions screen or the logout warning.
    Sessions(SessionsMessage),
    /// An inline image was clicked: open the full-size viewer.
    MediaClicked(String),
    /// The viewer was closed (**Close**, Esc).
    ViewerClosed,
    /// **Save...** on a media message.
    SaveRequested(String),
    /// The save dialog answered; `None` when the user cancelled. The path is
    /// redacted in `Debug`.
    SavePicked {
        /// Room of the item.
        room_id: RoomId,
        /// The media message.
        item_id: String,
        /// The path the user chose.
        path: Option<SavePath>,
    },
    /// **Open** on a saved file (asks first).
    OpenRequested(String),
    /// The user confirmed opening the saved file.
    OpenConfirmed,
    /// The user backed out of opening the saved file.
    OpenDismissed,
    /// The system handler was started (or failed to start) for a saved file.
    MediaOpened {
        /// Whether the handler started without error.
        ok: bool,
    },
    /// **Attach** or Ctrl+U: choose files to send.
    AttachRequested,
    /// The composer's **Emoji** button: start a `:shortcode` at the cursor.
    EmojiRequested,
    /// The file picker answered; the paths are redacted in `Debug`.
    FilesChosen(UploadPaths),
    /// Files are being dragged over the window.
    FileHovered,
    /// The files left the window without being dropped.
    FilesHoveredLeft,
    /// A file was dropped on the window.
    FileDropped(UploadPaths),
    /// The pause after the last dropped file ended; the number says which
    /// drop started it, so only the last one opens the confirmation.
    DropsDue(u64),
    /// A file of the confirmation was read and checked.
    UploadPrepared {
        /// The line of the confirmation.
        entry: u64,
        /// What was found.
        outcome: Box<Outcome>,
    },
    /// Ctrl+V in the composer.
    PasteRequested,
    /// What the clipboard held; redacted in `Debug`.
    Pasted(ClipboardContent),
    /// The window system's text paste, after the clipboard could not be read.
    PasteFallback(Option<MessageText>),
    /// **Send** in the confirmation.
    UploadConfirmed,
    /// **Cancel** in the confirmation (or Esc).
    UploadCancelled,
    /// A window opened (the first one creates the tray).
    WindowOpened(window::Id),
    /// The desktop gave the room's notification this ID.
    NotificationShown {
        /// The room.
        room_id: RoomId,
        /// The desktop's ID, used to update the notification.
        id: u32,
    },
    /// The user clicked a desktop notification.
    OpenRoomFromNotification(RoomId),
    /// A choice in the room header's notification menu.
    RoomModeChosen(ModeChoice),
    /// **Show Twicecat** or **Quit** in the tray, or a click on the icon.
    Tray(TrayAction),
    /// A message of the room dialogs, panels, user card and join bar.
    Rooms(RoomsMessage),
    /// A call control: the header's call button, the call bar, the
    /// participants panel, the incoming-call card, the Voice & audio card.
    Call(CallMessage),
    /// **More** on a person's row of the participants panel: shows or hides
    /// the row's volume and cleaning controls. View state only, so it is not
    /// a [`CallMessage`] (those are the call's own controls).
    CallRowMore(UserId),
    /// A control of the Settings screen, or Ctrl+,.
    Settings(SettingsMessage),
    /// A control of the Voice & audio card.
    Audio(crate::audio_settings::AudioMessage),
    /// Push-to-talk: the service's state, and keys in the window.
    Ptt(PttMessage),
    /// A button of the spaces sidebar.
    Spaces(SpacesMessage),
    /// A thread summary row or a control of the thread panel.
    Threads(ThreadsMessage),
    /// A keyboard shortcut (see `keys`); `captured` says a widget used the
    /// key already.
    Shortcut {
        /// What the keys stand for.
        shortcut: crate::keys::Shortcut,
        /// A widget used the key.
        captured: bool,
    },
    /// The room switcher's field, list and rows.
    Switcher(crate::switcher::SwitcherMessage),
    /// The task that decoded fetched image bytes finished.
    MediaDecoded {
        /// Room of the item.
        room_id: RoomId,
        /// The media message.
        item_id: String,
        /// Thumbnail or full image.
        size: MediaSize,
        /// The pixels, or `None` when the bytes were refused.
        decoded: Option<Decoded>,
    },
}

/// What `main` hands to Iced's boot function.
pub struct Startup {
    /// Where commands go.
    pub sink: Box<dyn CommandSink>,
    /// The core's events.
    pub events: EventsSlot,
}

/// The app state.
pub struct App {
    sink: Box<dyn CommandSink>,
    events: EventsSlot,
    /// The screen shown.
    pub screen: Screen,
    /// The room list, kept in step with the core's diffs.
    pub rooms: Vec<RoomSummary>,
    /// How many rooms the server reported, once the first load is done.
    pub total_rooms: Option<u32>,
    /// The room list query last sent to the core (the search is trimmed).
    pub query: RoomListQuery,
    /// The search field as typed, untrimmed.
    pub search_input: String,
    /// Scroll offset of the room list, in pixels.
    pub list_offset_y: f32,
    /// Height of the room list viewport, in pixels.
    pub list_viewport_h: f32,
    /// The window's logical width, from `window::resize_events`.
    pub window_width: f32,
    /// The side panel opened last (see [`crate::layout::layout_for`]).
    pub newest_panel: crate::layout::SidePanel,
    /// The row height the list was last laid out with, so a change of the
    /// list as shown keeps the place in it.
    pub(crate) shown_row_height: f32,
    /// The loaded length for which `LoadMoreRooms` was last sent.
    pub load_more_at: Option<usize>,
    /// Avatar requests and answers of this session (plan Q11).
    pub avatars: Avatars,
    /// The room whose timeline is open.
    pub selected: Option<RoomId>,
    /// The selected room's timeline, kept in step with the core's diffs.
    pub timeline: TimelineView,
    /// The latest trust the core reported per user, for the DM header. The
    /// core says nothing about which room a `UserTrust` belongs to, and an
    /// aborted watcher can still deliver one for the previous peer after the
    /// new one, so the header looks up the selected peer by ID
    /// ([`App::dm_peer_trust`]). Capped at [`MAX_TRUST_KEPT`].
    pub user_trust: Vec<(UserId, UserTrust)>,
    /// Thumbnails, the viewer and saves (plan Q7, Q8).
    pub media: MediaModel,
    /// Files being sent: the confirmation, drops and the server's limit.
    pub uploads: UploadModel,
    /// Notification, tray and theme settings kept on this device. Until the
    /// local settings storage exists these are the defaults.
    pub ui_settings: UiSettings,
    /// The account's notification defaults and keywords, as the core last
    /// reported them.
    pub notification_settings: Option<NotificationSettingsView>,
    /// Notification modes the user picked that the room list has not shown
    /// yet (`None` is "use default"). The SDK does not refresh a room's
    /// summary right after a mode change, so the menu reads these first.
    pub room_modes: HashMap<RoomId, Option<RoomNotificationModeView>>,
    /// The tray icon and the unread count it shows.
    pub tray: TrayModel,
    /// Room dialogs, settings, members, the user card and the join bar.
    pub rooms_ui: RoomsModel,
    /// The Settings screen.
    pub settings_ui: SettingsModel,
    pub(crate) settings_store: Box<dyn SettingsStore>,
    /// The spaces the user is in (the sidebar).
    pub spaces: SpacesModel,
    /// The thread panel.
    pub threads: ThreadsModel,
    /// The room switcher (Ctrl+K).
    pub switcher: crate::switcher::SwitcherModel,
    /// The composer's own Esc just did something; the window listener's
    /// copy of the same key press must not close another layer.
    pub(crate) escape_spent: bool,
    notifier: Box<dyn Notifier>,
    folding: Folding,
    pub(crate) dialogs: Box<dyn Dialogs>,
    pub(crate) clipboard: Box<dyn Clipboard>,
    /// The signed-in account.
    pub user_id: Option<UserId>,
    /// The last sync state.
    pub sync_state: SyncState,
    /// The login form.
    pub login: LoginForm,
    /// A login request was sent and not answered yet.
    pub login_in_flight: bool,
    /// The app ended an OAuth attempt itself and sent `CancelOAuth`; the core
    /// answers with one `LoginFailed(Cancelled)`, which must not touch the
    /// notice or a newer attempt.
    pub pending_cancel: bool,
    /// A non-secret line shown above the login form: why the last login
    /// failed, or why the user is logged out.
    pub notice: Option<String>,
    /// The checked OAuth URL, kept only for **Copy link**; never shown.
    pub browser_link: Option<BrowserUrl>,
    /// `None` while the browser launcher runs, then whether it worked.
    pub browser_opened: Option<bool>,
    /// The composer of the selected room.
    pub composer: Composer,
    /// A non-secret line about the last failed send.
    pub send_error: Option<String>,
    /// Hover, delete confirmation, reaction picker and failure notice.
    pub actions: ActionState,
    /// The system theme mode.
    pub theme_mode: theme::Mode,
    /// Spoilers the user revealed in the open room.
    pub revealed: Revealed,
    /// Who is typing in the open room (other users, at most
    /// [`timeline::MAX_TYPING_KEPT`]).
    pub typing: Vec<TypingUser>,
    /// The window has focus (read receipts wait for it).
    pub window_focused: bool,
    /// The bar above the composer about a clicked link.
    pub link_bar: Option<LinkBar>,
    /// The name of an unknown `/command` waiting for "Send as message".
    pub command_bar: Option<String>,
    /// Counts composer changes, so only the last draft timer saves.
    pub draft_seq: u64,
    /// The composer changed since the room was opened: only then is its
    /// draft stored (the room's own draft may still be on its way).
    draft_dirty: bool,
    /// The security banner and screens.
    pub security: SecurityModel,
    /// The verification dialog, while a flow is open.
    pub verification: Option<VerificationModel>,
    /// The Sessions screen and the logout warning.
    pub sessions: SessionsModel,
    /// The call, call counts, rings and audio settings.
    pub calls: CallModel,
    /// The Voice & audio card.
    pub audio_ui: crate::audio_settings::AudioUi,
    /// Push-to-talk: the service while it is needed, and what it reports.
    pub ptt: PttModel,
    /// Set when the app decided to exit (the task itself is opaque to tests).
    pub exit_requested: bool,
    /// Clipboard reads sent by `drop_secrets` whose answer is still due.
    pub(crate) flushes_pending: u32,
    /// Counts the waits of the verification card's agreeing buttons, so a
    /// timer of an earlier wait (or flow) does nothing.
    pub(crate) verify_arm_counter: u64,
    /// The core closed while the window was closing; exit waits for the
    /// pending clipboard flushes.
    pub(crate) core_closed: bool,
}

// Manual impl: the sink and the event slot are not `Debug`, and the composer
// and password fields stay redacted by their own types (plan Q19).
impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("screen", &self.screen)
            .field("rooms", &self.rooms.len())
            .field("timeline", &self.timeline)
            .field("selected", &self.selected)
            .field("media", &self.media)
            .field("uploads", &self.uploads)
            .field("rooms_ui", &self.rooms_ui)
            .field("spaces", &self.spaces)
            .field("threads", &self.threads)
            .field("settings_ui", &self.settings_ui)
            .field("switcher", &self.switcher)
            .field("notification_settings", &self.notification_settings)
            .field("login", &self.login)
            .field("login_in_flight", &self.login_in_flight)
            .field("composer", &self.composer)
            .field("actions", &self.actions)
            .field("security", &self.security)
            .field("verification", &self.verification)
            .field("sessions", &self.sessions)
            .field("calls", &self.calls)
            .field("audio_ui", &self.audio_ui.open)
            .field("ptt", &self.ptt)
            .finish_non_exhaustive()
    }
}

/// Core error text shown to the user: one line, at most 512 characters.
fn safe_line(text: &str) -> String {
    text_safety::one_line_capped(text, 512)
}

impl App {
    /// Creates the app and the boot task: ask the core to restore the session
    /// and read the system theme.
    pub fn new(startup: Startup) -> (Self, Task<Message>) {
        let Startup { sink, events } = startup;
        let app = Self {
            sink,
            events,
            screen: Screen::Starting,
            rooms: Vec::new(),
            total_rooms: None,
            query: RoomListQuery::default(),
            search_input: String::new(),
            list_offset_y: 0.0,
            list_viewport_h: room_list::DEFAULT_VIEWPORT_HEIGHT,
            window_width: crate::layout::DEFAULT_WINDOW_WIDTH,
            newest_panel: crate::layout::SidePanel::default(),
            shown_row_height: 0.0,
            load_more_at: None,
            avatars: Avatars::default(),
            selected: None,
            timeline: TimelineView::default(),
            user_trust: Vec::new(),
            media: MediaModel::default(),
            uploads: UploadModel::default(),
            ui_settings: UiSettings::default(),
            notification_settings: None,
            room_modes: HashMap::new(),
            // Inert until `booter` installs the real ones, so tests never
            // touch the desktop.
            tray: TrayModel::new(Box::new(NoTrayFactory)),
            rooms_ui: RoomsModel::default(),
            settings_ui: SettingsModel::default(),
            settings_store: Box::new(NoSettingsStore),
            spaces: SpacesModel::default(),
            threads: ThreadsModel::default(),
            switcher: crate::switcher::SwitcherModel::default(),
            escape_spent: false,
            notifier: Box::new(NoNotifier),
            folding: Folding::default(),
            dialogs: Box::new(SystemDialogs),
            clipboard: Box::new(SystemClipboard),
            user_id: None,
            sync_state: SyncState::Idle,
            login: LoginForm::new(),
            login_in_flight: false,
            pending_cancel: false,
            notice: None,
            browser_link: None,
            browser_opened: None,
            composer: Composer::new(),
            send_error: None,
            actions: ActionState::default(),
            theme_mode: theme::Mode::None,
            revealed: Revealed::new(),
            typing: Vec::new(),
            window_focused: true,
            link_bar: None,
            command_bar: None,
            draft_seq: 0,
            draft_dirty: false,
            security: SecurityModel::default(),
            verification: None,
            sessions: SessionsModel::default(),
            calls: CallModel::default(),
            audio_ui: crate::audio_settings::AudioUi::default(),
            // Inert until `booter` installs the real service.
            ptt: PttModel::default(),
            exit_requested: false,
            flushes_pending: 0,
            verify_arm_counter: 0,
            core_closed: false,
        };
        let restore = app.send(Command::RestoreSession);
        let boot = Task::batch([restore, system::theme().map(Message::ThemeMode)]);
        (app, boot)
    }

    /// A boot function for `iced::application`; Iced calls it once.
    ///
    /// `ui_settings` are the local settings read from disk before the core
    /// started; they decide the theme, notifications, previews and the tray
    /// badge from the first frame. `store` writes them back on each change.
    pub fn booter(
        startup: Startup,
        ui_settings: UiSettings,
        store: impl SettingsStore + Clone + 'static,
    ) -> impl Fn() -> (App, Task<Message>) {
        let startup = Mutex::new(Some(startup));
        move || {
            let startup = startup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .expect("iced boots the application exactly once");
            let (mut app, task) = App::new(startup);
            app.set_ui_settings(ui_settings);
            app.set_settings_store(Box::new(store.clone()));
            app.set_notifier(Box::new(notifications::SystemNotifier));
            app.set_tray_factory(Box::new(SystemTrayFactory));
            if let Some(runtime) = app.sink.runtime() {
                app.set_ptt_starter(
                    Box::new(ServiceStarter(runtime)),
                    twicecat_ptt::KeyboardBackend::detect(),
                );
            }
            (app, task)
        }
    }

    /// Replaces the local settings (read from the settings file at boot).
    pub fn set_ui_settings(&mut self, settings: UiSettings) {
        self.ui_settings = settings;
        self.timeline.set_density(self.density());
    }

    /// Replaces where the local settings are written (tests use a fake).
    pub fn set_settings_store(&mut self, store: Box<dyn SettingsStore>) {
        self.settings_store = store;
    }

    /// The theme the window uses: the user's override, else the system's.
    pub fn current_theme(&self) -> Theme {
        crate::theme::theme_for_choice(
            self.theme_mode,
            self.ui_settings.theme,
            crate::theme::resolve_family(self.ui_settings.palette),
        )
    }

    /// Replaces the desktop notification service (tests use a fake).
    pub fn set_notifier(&mut self, notifier: Box<dyn Notifier>) {
        self.notifier = notifier;
    }

    /// Installs what starts push-to-talk (tests use a fake) for a session
    /// with `keyboard`. A running service is stopped.
    pub fn set_ptt_starter(
        &mut self,
        starter: Box<dyn PttStarter>,
        keyboard: twicecat_ptt::KeyboardBackend,
    ) {
        self.ptt.set_starter(starter, keyboard);
    }

    /// Runs push-to-talk while it is needed (plan Q1): in push-to-talk or
    /// toggle mode, during a call, the mic test, or with the Voice & audio
    /// card open. Called after every message, from `update`, so the service
    /// is started and dropped on the main thread.
    fn sync_ptt(&mut self) {
        let in_call = self.calls.call.is_some();
        let mic_test = self.audio_ui.mic_test.is_some();
        let needed = self.screen == Screen::Main && (in_call || mic_test || self.audio_ui.open);
        let wanted = if needed {
            self.calls
                .settings
                .as_ref()
                .and_then(|s| crate::ptt::setup_for(s, self.ptt.keyboard()))
        } else {
            None
        };
        let sink = &self.sink;
        self.ptt
            .sync(wanted, (in_call, mic_test), || sink.command_sender());
    }

    /// Replaces the tray factory (tests use a fake). Only before the tray is
    /// created.
    pub fn set_tray_factory(&mut self, factory: Box<dyn TrayFactory>) {
        self.tray = TrayModel::new(factory);
    }

    /// The choice the room header's menu shows for `room`: what the user
    /// picked and the list has not caught up with, else the list's mode.
    pub fn room_mode_choice(&self, room: &RoomId) -> ModeChoice {
        ModeChoice::from_mode(self.effective_room_mode(room))
    }

    /// The mode of `room` for the list and the menu.
    pub fn effective_room_mode(&self, room: &RoomId) -> Option<RoomNotificationModeView> {
        match self.room_modes.get(room) {
            Some(mode) => *mode,
            None => self
                .rooms
                .iter()
                .find(|r| r.id == *room)
                .and_then(|r| r.notification_mode),
        }
    }

    /// Forgets local mode choices that the room list now agrees with.
    fn reconcile_room_modes(&mut self) {
        let rooms = &self.rooms;
        self.room_modes.retain(|id, mode| {
            rooms
                .iter()
                .find(|r| r.id == *id)
                .is_none_or(|r| r.notification_mode != *mode)
        });
    }

    fn choose_room_mode(&mut self, choice: ModeChoice) -> Task<Message> {
        let Some(room) = self.selected.clone() else {
            return Task::none();
        };
        if self.room_mode_choice(&room) == choice {
            return Task::none();
        }
        if self.room_modes.len() >= MAX_LOCAL_MODES && !self.room_modes.contains_key(&room) {
            self.room_modes.clear();
        }
        self.room_modes.insert(room.clone(), choice.mode());
        self.send(Command::SetRoomNotificationMode {
            room_id: room,
            mode: choice.mode(),
        })
    }

    /// Creates the tray once, on the main thread.
    fn ensure_tray(&mut self) {
        if self.tray.ensure(self.ui_settings.tray_badge) {
            eprintln!("twicecat: the tray icon is not available");
        }
    }

    fn on_notification(&mut self, mut view: NotificationView) -> Task<Message> {
        if !notifications::should_show(self.window_focused, &self.ui_settings) {
            return Task::none();
        }
        let show_content = self.ui_settings.show_message_content;
        if !show_content {
            // The core always sends a preview; it stops here.
            view.preview = None;
        }
        let (folded, replace) = match self.folding.admit(&view.room_id, Instant::now()) {
            notifications::Delivery::New => (None, None),
            notifications::Delivery::Fold { count, replace } => (Some(count), replace),
        };
        let toast = notifications::build_toast(&view, show_content, folded);
        let show = self.notifier.show(notifications::Request {
            room_id: view.room_id,
            toast,
            replace,
        });
        if view.kind == NotificationKind::Mention {
            Task::batch([show, attention()])
        } else {
            show
        }
    }

    fn open_room_from_notification(&mut self, room: RoomId) -> Task<Message> {
        let select =
            if self.selected.as_ref() != Some(&room) && room_list::contains(&self.rooms, &room) {
                self.update_inner(Message::SelectRoom(room))
            } else {
                Task::none()
            };
        Task::batch([select, focus_window()])
    }

    /// Shows a desktop notification built elsewhere (an incoming call), when
    /// the settings and the window's focus allow it.
    pub(crate) fn notify(&mut self, request: notifications::Request) -> Task<Message> {
        if notifications::should_show(self.window_focused, &self.ui_settings) {
            self.notifier.show(request)
        } else {
            Task::none()
        }
    }

    /// Replaces the system dialogs (tests use a fake).
    pub fn set_dialogs(&mut self, dialogs: Box<dyn Dialogs>) {
        self.dialogs = dialogs;
    }

    /// Replaces the system clipboard (tests use a fake).
    pub fn set_clipboard(&mut self, clipboard: Box<dyn Clipboard>) {
        self.clipboard = clipboard;
    }

    /// The selected direct message's other person and the core's latest word
    /// on their trust: `None` unless the core named a peer for the room and
    /// reported that user. A report for anyone else is ignored.
    pub fn dm_peer_trust(&self) -> Option<(&UserId, UserTrust)> {
        let (peer, _) = self.dm_peer()?;
        self.user_trust
            .iter()
            .find(|(user, _)| user == peer)
            .map(|(user, trust)| (user, *trust))
    }

    /// The selected direct message's other person, as the core names them in
    /// the room's summary (`RoomSummary::dm_peer`), with that summary.
    pub fn dm_peer(&self) -> Option<(&UserId, &RoomSummary)> {
        let room = self
            .selected
            .as_ref()
            .and_then(|id| self.rooms.iter().find(|room| room.id == *id))?;
        Some((room.dm_peer.as_ref()?, room))
    }

    /// Keeps the core's word on `user`'s trust, newest last.
    fn remember_trust(&mut self, user: UserId, trust: UserTrust) {
        self.user_trust.retain(|(known, _)| *known != user);
        if self.user_trust.len() >= MAX_TRUST_KEPT {
            self.user_trust.remove(0);
        }
        self.user_trust.push((user, trust));
    }

    pub(crate) fn send(&self, command: Command) -> Task<Message> {
        self.sink.send(command).unwrap_or_else(Task::none)
    }

    /// A task that delivers `message` after `after`.
    pub(crate) fn timer(&self, after: Duration, message: Message) -> Task<Message> {
        self.sink.timer(after, message)
    }

    /// Applies a message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        let touches_composer = matches!(
            message,
            Message::Composer(_)
                | Message::ComposerKey(_)
                | Message::SuggestionPicked(_)
                | Message::ReplyRequested(_)
                | Message::EditRequested(_)
                | Message::ModeCancelled
                | Message::EmojiRequested
                | Message::Pasted(_)
                | Message::PasteFallback(_)
        );
        let thread_before = self.threads.panel.is_some();
        let call_before = self.calls.panel_shown(self.selected.as_ref());
        let resized = matches!(message, Message::WindowResized(_));
        let fills_before = self.pane_layout().panel_fills;
        let task = self.update_inner(message);
        self.note_panels(thread_before, call_before);
        // The reaction picker takes the keyboard: it must never stay open in
        // a pane the user cannot see.
        if self
            .actions
            .picker
            .as_ref()
            .is_some_and(|p| !self.pane_shows_item(p.in_thread))
        {
            self.actions.picker = None;
        }
        // The room pane is shown again: what it could not report while it
        // was hidden (the receipt for what arrived) is due now.
        let task = if fills_before && !self.pane_layout().panel_fills {
            Task::batch([task, self.read_receipt(Instant::now())])
        } else {
            task
        };
        // The list as shown may have changed (a panel opened in a medium
        // window): keep the first visible room at the top. A resize only
        // records the new height: it must not start a task, and the offset
        // stays what the scrollable really has.
        let task = if resized {
            self.shown_row_height = self.room_row_height();
            task
        } else {
            let place = self.keep_list_place(self.shown_row_height);
            Task::batch([task, place])
        };
        self.sync_ptt();
        // A card that just showed, or changed to another ring, waits before
        // it answers anything.
        let task = match self.calls.sync_arming() {
            Some(seq) => Task::batch([
                task,
                self.timer(crate::calls::RING_ARMING, Message::RingArmed(seq)),
            ]),
            None => task,
        };
        if touches_composer && self.selected.is_some() {
            self.draft_dirty = true;
            self.draft_seq = self.draft_seq.wrapping_add(1);
            let timer = self
                .sink
                .timer(DRAFT_SAVE_DELAY, Message::DraftDue(self.draft_seq));
            Task::batch([task, timer])
        } else {
            task
        }
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Core(event) => self.on_core(*event),
            Message::CoreClosed => {
                if self.screen == Screen::ShuttingDown {
                    self.core_closed = true;
                    self.exit_if_flushed(Task::none())
                } else {
                    self.screen = Screen::Disconnected;
                    self.drop_secrets()
                }
            }
            Message::ThemeMode(mode) => {
                self.theme_mode = mode;
                Task::none()
            }
            Message::ServerChanged(server) => {
                self.login.server = server;
                Task::none()
            }
            Message::UsernameChanged(username) => {
                self.login.username = username;
                Task::none()
            }
            Message::PasswordChanged(password) => {
                self.login.password = password;
                Task::none()
            }
            Message::ToggleServerField => {
                // A field that holds something, or that a notice is about,
                // stays open; the button then does nothing.
                if !login::server_held(&self.login.server, self.notice.as_deref()) {
                    self.login.server_open = !self.login.server_open;
                }
                Task::none()
            }
            Message::LoginSubmitted => self.submit_login(),
            Message::OAuthRequested => self.request_oauth(),
            Message::CancelOAuth => {
                if self.screen == Screen::WaitingForBrowser {
                    self.send(Command::CancelOAuth)
                } else {
                    Task::none()
                }
            }
            Message::BrowserOpened { ok } => {
                if self.screen == Screen::WaitingForBrowser {
                    self.browser_opened = Some(ok);
                }
                Task::none()
            }
            Message::CopyLink => match &self.browser_link {
                Some(link) => clipboard::write(link.as_str().to_owned()),
                None => Task::none(),
            },
            Message::Composer(EditorAction(action)) => {
                self.composer_action_at(action, Instant::now())
            }
            Message::ComposerKey(key) => self.composer_key_at(key, Instant::now()),
            Message::EmojiRequested => self.emoji_requested(Instant::now()),
            Message::SuggestionPicked(index) => {
                self.pick_suggestion_at(Some(index), Instant::now())
            }
            Message::MembersDue => self.members_due_at(Instant::now()),
            // The room pane behind a panel has no width: what it reports then
            // is not what the user will see.
            Message::RowMeasured { .. } | Message::TimelineScrolled(_)
                if self.pane_layout().panel_fills =>
            {
                Task::none()
            }
            Message::RowMeasured { id, height } => {
                let effects = self.timeline.row_measured(&id, height);
                self.timeline_effects(effects)
            }
            Message::TimelineScrolled(viewport) => {
                let bounds = viewport.bounds();
                // The scrollable is anchored at the bottom, so its stored
                // offset is the distance from the bottom edge.
                self.on_timeline_scrolled(viewport.absolute_offset().y, bounds.height, bounds.width)
            }
            Message::LinkClicked(target) => self.link_clicked(target),
            Message::LinkConfirmed => match self.link_bar.take() {
                Some(LinkBar::Confirm(url)) => open_link_task(url),
                other => {
                    self.link_bar = other;
                    Task::none()
                }
            },
            Message::LinkDismissed => {
                self.link_bar = None;
                Task::none()
            }
            Message::CommandSendAnyway => {
                self.command_bar = None;
                match self.selected.clone() {
                    Some(room_id) => match composer::as_text(&self.composer, &room_id) {
                        Some(message) => self.send_composed(message),
                        None => Task::none(),
                    },
                    None => Task::none(),
                }
            }
            Message::CommandCancelled => {
                self.command_bar = None;
                Task::none()
            }
            Message::LinkOpened { ok } => {
                if !ok {
                    self.link_bar = Some(LinkBar::Failed);
                }
                Task::none()
            }
            Message::HoverEnter(id) => {
                self.actions.hovered = Some(id);
                Task::none()
            }
            Message::HoverExit(id) => {
                if self.actions.hovered.as_deref() == Some(id.as_str()) {
                    self.actions.hovered = None;
                }
                Task::none()
            }
            Message::WindowResized(width) => {
                self.window_width = width;
                Task::none()
            }
            Message::WindowOpened(_) => {
                self.ensure_tray();
                Task::none()
            }
            Message::NotificationShown { room_id, id } => {
                self.folding.shown(&room_id, id);
                Task::none()
            }
            Message::OpenRoomFromNotification(room) => self.open_room_from_notification(room),
            Message::RoomModeChosen(choice) => self.choose_room_mode(choice),
            Message::Tray(TrayAction::Show) => focus_window(),
            Message::Tray(TrayAction::Quit) => self.update_inner(Message::CloseRequested),
            Message::WindowFocus(focused) => {
                self.window_focused = focused;
                // A key release may happen where this window does not see it.
                self.calls.chord_held = false;
                if !focused {
                    self.ptt.window_unfocused();
                    self.stop_capture();
                }
                // A second chance to create the tray, should the first
                // window event not have reached the app.
                self.ensure_tray();
                self.read_receipt(Instant::now())
            }
            Message::RingArmed(seq) => {
                self.calls.arm_ended(seq);
                Task::none()
            }
            Message::RingChordReleased => {
                self.calls.chord_held = false;
                Task::none()
            }
            Message::ReadDue(room_id) => {
                if self.selected.as_ref() != Some(&room_id) {
                    return Task::none();
                }
                self.timeline.read_timer_fired();
                self.read_receipt(Instant::now())
            }
            Message::RetryRequested(id) => self.echo_command(id, false),
            Message::DiscardRequested(id) => self.echo_command(id, true),
            Message::ReplyRequested(id) => self.request_reply(id),
            Message::EditRequested(id) => self.request_edit(id),
            Message::CopyRequested(id) => match self.copy_source(&id) {
                Some(body) => clipboard::write(body.to_owned()),
                None => Task::none(),
            },
            Message::DeleteRequested(id) => {
                if self.buttons_for(&id).is_some_and(|b| b.delete) {
                    self.actions.confirm_delete = Some(id);
                }
                Task::none()
            }
            Message::DeleteConfirmed => self.confirm_delete(),
            Message::DeleteCancelled => {
                self.actions.confirm_delete = None;
                Task::none()
            }
            Message::ModeCancelled => {
                let was_edit = matches!(self.composer.mode, composer::Mode::Edit(_));
                self.composer.cancel_mode();
                self.after_edit_left(was_edit)
            }
            Message::PickerOpened(id) => {
                let in_thread = self.is_thread_item(&id);
                if self.buttons_for(&id).is_some_and(|b| b.react) && self.pane_shows_item(in_thread)
                {
                    self.actions.picker = Some(Picker {
                        target: id,
                        in_thread,
                        query: String::new(),
                    });
                    return operation::focus(actions::PICKER_INPUT_ID);
                }
                Task::none()
            }
            Message::PickerQuery(query) => {
                if let Some(picker) = &mut self.actions.picker {
                    query.expose().clone_into(&mut picker.query);
                }
                Task::none()
            }
            Message::PickerSubmitted => {
                let first = self
                    .actions
                    .picker
                    .as_ref()
                    .and_then(|p| emoji::picker_results(&p.query).first().copied());
                match first {
                    Some(glyph) => self.pick_reaction(glyph),
                    None => Task::none(),
                }
            }
            Message::PickerPicked(key) => self.pick_reaction(key.expose()),
            Message::PickerClosed => {
                self.actions.picker = None;
                Task::none()
            }
            Message::ReactionToggled { item, key } => self.toggle_reaction(&item, key.expose()),
            Message::NoticeExpired(seq) => {
                if self.actions.notice.as_ref().is_some_and(|n| n.seq == seq) {
                    self.actions.notice = None;
                }
                Task::none()
            }
            Message::ReplyQuoteClicked(event_id) => {
                if self.selected.is_none() {
                    return Task::none();
                }
                let effects = self.timeline.jump_to(event_id);
                self.timeline_effects(effects)
            }
            Message::FlashEnded(id) => {
                self.timeline.flash_ended(&id);
                Task::none()
            }
            Message::DraftDue(seq) => {
                if seq == self.draft_seq {
                    self.save_draft()
                } else {
                    Task::none()
                }
            }
            Message::LogoutRequested => {
                if logout_needs_warning(&self.security.status, self.sessions.other_devices()) {
                    self.sessions.logout_warning = true;
                    Task::none()
                } else {
                    self.logout()
                }
            }
            // Only for the peer the header drew: a stale click for anyone
            // else (the room or its peer changed) is dropped.
            Message::VerifyPeerRequested(user_id) => match self.dm_peer() {
                Some((peer, _)) if *peer == user_id => {
                    self.send(Command::RequestUserVerification { user_id })
                }
                _ => Task::none(),
            },
            Message::Security(message) => self.on_security(message),
            Message::Verification(message) => self.on_verification(message),
            Message::Sessions(message) => self.on_sessions(message),
            Message::Rooms(message) => self.on_rooms(message),
            Message::Settings(message) => self.on_settings(message),
            Message::Call(message) => self.on_call(message),
            Message::CallRowMore(user) => {
                self.calls.toggle_row(&user);
                Task::none()
            }
            Message::Audio(message) => self.on_audio(message),
            Message::Ptt(PttMessage::Update(update)) => {
                self.ptt.update(update);
                Task::none()
            }
            Message::Ptt(PttMessage::WindowKey(key)) => {
                // While the card waits for a new key, the old one does not
                // press; its release still lets go.
                if !self.audio_ui.capturing || !key.pressed {
                    self.ptt.window_key(key);
                }
                Task::none()
            }
            Message::Spaces(SpacesMessage::Select(space)) => self.select_space(space),
            Message::Threads(message) => self.on_threads(message),
            Message::Shortcut { shortcut, captured } => self.on_shortcut(shortcut, captured),
            Message::Switcher(message) => self.on_switcher(message),
            // Ctrl+U and drops are for the room: with its pane hidden behind
            // a panel the user would pick files for a room they cannot see.
            Message::AttachRequested | Message::FileHovered | Message::FileDropped(_)
                if self.pane_layout().panel_fills =>
            {
                Task::none()
            }
            Message::AttachRequested => {
                self.uploads_for_room();
                self.attach_requested()
            }
            Message::FilesChosen(paths) => self.files_added(paths.0),
            Message::FileHovered => {
                self.uploads.hovering = self.upload_room().is_some();
                Task::none()
            }
            Message::FilesHoveredLeft => {
                self.uploads.hovering = false;
                Task::none()
            }
            Message::FileDropped(paths) => {
                self.uploads_for_room();
                self.files_dropped(paths.0)
            }
            Message::DropsDue(seq) => self.drops_due(seq),
            Message::UploadPrepared { entry, outcome } => {
                self.upload_prepared(entry, *outcome);
                Task::none()
            }
            Message::PasteRequested => {
                self.uploads_for_room();
                self.paste_requested()
            }
            Message::Pasted(content) => self.pasted(content),
            Message::PasteFallback(text) => self.paste_fallback(text),
            Message::UploadConfirmed => self.upload_confirmed(),
            Message::UploadCancelled => {
                self.uploads.dialog = None;
                Task::none()
            }
            Message::MediaClicked(item_id) => self.open_viewer(item_id),
            Message::ViewerClosed => {
                self.media.close_viewer();
                Task::none()
            }
            Message::SaveRequested(item_id) => self.request_save(&item_id),
            Message::SavePicked {
                room_id,
                item_id,
                path,
            } => self.save_picked(room_id, item_id, path),
            Message::OpenRequested(item_id) => {
                if let (Some(room_id), Some(name)) =
                    (self.selected.clone(), self.media_filename(&item_id))
                {
                    let name = name.to_owned();
                    self.media.ask_open(&(room_id, item_id), &name);
                }
                Task::none()
            }
            Message::OpenConfirmed => match self.media.open_bar.take() {
                Some(bar) => {
                    let path = bar.path().to_path_buf();
                    Task::perform(async move { dialogs::open_file(&path) }, |ok| {
                        Message::MediaOpened { ok }
                    })
                }
                None => Task::none(),
            },
            Message::OpenDismissed => {
                self.media.open_bar = None;
                Task::none()
            }
            Message::MediaOpened { ok } => {
                if ok {
                    Task::none()
                } else {
                    self.show_notice_text("The file could not be opened.".to_owned())
                }
            }
            Message::MediaDecoded {
                room_id,
                item_id,
                size,
                decoded,
            } => {
                let key = (room_id, item_id);
                match size {
                    MediaSize::Thumbnail => self.media.thumb_decoded(&key, decoded),
                    MediaSize::Full => self.media.viewer_decoded(&key, decoded),
                }
                Task::none()
            }
            Message::SelectRoom(id) => {
                if !room_list::contains(&self.rooms, &id) {
                    return Task::none();
                }
                let stop_typing = self.stop_typing();
                let marker = self.move_read_marker();
                // Before the core opens the next room (commands run in order).
                let save = self.save_draft();
                self.draft_dirty = false;
                self.draft_seq = self.draft_seq.wrapping_add(1);
                // The core closes the thread with the old room; this is for
                // picking the open room again.
                let close_thread = self.close_thread_panel();
                self.selected = Some(id.clone());
                self.timeline.reset();
                self.media.reset_view();
                self.uploads.reset_view();
                self.user_trust.clear();
                self.typing.clear();
                self.actions.reset();
                self.clear_draft();
                self.reset_link_state();
                self.rooms_ui.reset_room();
                Task::batch([
                    stop_typing,
                    marker,
                    save,
                    close_thread,
                    self.send(Command::SelectRoom(id)),
                ])
            }
            Message::TabSelected(tab) => {
                let query = RoomListQuery {
                    tab,
                    search: self.query.search.clone(),
                    space: self.query.space.clone(),
                };
                self.set_query(query)
            }
            Message::SearchChanged(search) => {
                let query = RoomListQuery {
                    tab: self.query.tab,
                    search: search.trim().to_owned(),
                    space: self.query.space.clone(),
                };
                self.search_input = search;
                self.set_query(query)
            }
            Message::RoomListScrolled {
                offset_y,
                viewport_h,
            } => {
                self.list_offset_y = offset_y;
                self.list_viewport_h = viewport_h;
                self.after_list_change()
            }
            Message::CloseRequested => {
                if self.screen == Screen::ShuttingDown {
                    return Task::none();
                }
                let save = self.save_draft();
                // The app stays alive until the core closes, so the read can
                // still be answered.
                let flush = self.drop_secrets();
                self.screen = Screen::ShuttingDown;
                Task::batch([
                    flush,
                    save,
                    self.send(Command::Shutdown),
                    self.sink.deadline(SHUTDOWN_WAIT),
                ])
            }
            Message::ShutdownDeadline => {
                if self.screen == Screen::ShuttingDown {
                    self.exit()
                } else {
                    Task::none()
                }
            }
        }
    }

    /// Exits once the core has closed and every clipboard flush has been
    /// answered, so the clear write is issued first (`ShutdownDeadline`
    /// stays the hard limit). Otherwise returns `task` unchanged.
    pub(crate) fn exit_if_flushed(&mut self, task: Task<Message>) -> Task<Message> {
        if self.screen == Screen::ShuttingDown && self.core_closed && self.flushes_pending == 0 {
            self.exit_requested = true;
            task.chain(iced::exit())
        } else {
            task
        }
    }

    fn exit(&mut self) -> Task<Message> {
        self.exit_requested = true;
        iced::exit()
    }

    fn on_core(&mut self, event: CoreEvent) -> Task<Message> {
        if self.screen == Screen::ShuttingDown {
            return Task::none();
        }
        let mut task = Task::none();
        match event {
            CoreEvent::NoSession => {
                self.pending_cancel = false;
                self.end_login_attempt();
                self.screen = Screen::Login;
            }
            CoreEvent::LoggedIn {
                user_id, restored, ..
            } => {
                // Any cancel reply would have come before this login.
                self.pending_cancel = false;
                // A copy still on the clipboard is checked, not forgotten.
                task = Task::batch([self.drop_secrets(), self.calls_after_login()]);
                self.security = SecurityModel::default();
                self.security.fresh_login = !restored;
                self.room_modes.clear();
                self.folding.clear();
                self.user_id = Some(user_id);
                self.notice = None;
                self.end_login_attempt();
                self.screen = Screen::Loading;
            }
            CoreEvent::LoginFailed(CoreError::Cancelled)
                if self.pending_cancel || !self.login_in_flight =>
            {
                // The reply to a cancel the app sent after ending the attempt
                // itself: one reply, consumed once; or one with no attempt.
                self.pending_cancel = false;
            }
            CoreEvent::LoginFailed(error) => {
                if !matches!(error, CoreError::Cancelled) {
                    self.pending_cancel = false;
                }
                self.notice = Some(match error {
                    CoreError::Cancelled => "Sign-in cancelled.".to_owned(),
                    other => safe_line(&other.to_string()),
                });
                self.end_login_attempt();
                self.screen = Screen::Login;
            }
            CoreEvent::LoggedOut => task = self.back_to_login("You were logged out."),
            CoreEvent::SessionExpired => {
                task = self.back_to_login("Your session expired. Please log in again.");
            }
            CoreEvent::OAuthUrl(url) => task = self.on_oauth_url(url.expose()),
            CoreEvent::SyncState(state) => self.sync_state = state,
            CoreEvent::RoomListChanged(diffs) => {
                room_list::apply_room_diffs(&mut self.rooms, &diffs);
                self.reconcile_room_modes();
                // A search or tab filters the list, so an open room missing
                // from it is only hidden: keep it and its draft. Without a
                // filter, a missing room has really gone.
                let selection_gone = self.query == RoomListQuery::default()
                    && self
                        .selected
                        .as_ref()
                        .is_some_and(|id| !room_list::contains(&self.rooms, id));
                if selection_gone {
                    self.deselect();
                }
                if self.rooms.is_empty() {
                    // No scrollable is built for an empty list; it restarts
                    // at the top.
                    self.list_offset_y = 0.0;
                }
                if self.screen == Screen::Loading && !self.rooms.is_empty() {
                    self.screen = Screen::Main;
                }
                task = Task::batch([self.after_list_change(), self.rooms_after_list_change()]);
            }
            CoreEvent::RoomListLoaded { total_rooms } => {
                self.total_rooms = total_rooms;
                if self.screen == Screen::Loading {
                    self.screen = Screen::Main;
                }
                task = self.check_paging();
            }
            CoreEvent::TimelineDiff { room_id, diffs } => {
                task = self.apply_timeline(&room_id, diffs);
            }
            CoreEvent::DraftLoaded { room_id, draft } => {
                if self.selected.as_ref() == Some(&room_id) {
                    // A reply whose message is not loaded comes back as a
                    // plain draft; the text is never lost.
                    let reply = draft.reply_to.as_ref().and_then(|event| {
                        self.timeline
                            .items
                            .iter()
                            .find(|i| i.event_id.as_ref() == Some(event))
                            .map(|i| i.id.clone())
                    });
                    self.composer
                        .load_draft(draft.text.expose(), draft.mentions, reply);
                }
            }
            CoreEvent::Pagination { room_id, state } => {
                if self.selected.as_ref() == Some(&room_id) {
                    let effects = self.timeline.pagination_changed(state);
                    task = self.timeline_effects(effects);
                }
            }
            CoreEvent::SendFailed { room_id, error } => {
                if self.selected.as_ref() == Some(&room_id) {
                    self.send_error = Some(safe_line(&error.to_string()));
                }
            }
            CoreEvent::Error(error)
                if calls::is_call_error(&error)
                    && matches!(self.screen, Screen::Main | Screen::Loading) =>
            {
                self.calls.on_error(&error);
                self.audio_card_error(&error);
            }
            CoreEvent::Error(error) => task = self.on_core_error(&error),
            CoreEvent::Avatar { id, image } => self.avatars.answer(&id, image),
            CoreEvent::Members {
                room_id,
                query,
                members,
            } => {
                if self.selected.as_ref() == Some(&room_id) {
                    self.composer.set_members(query.expose(), members);
                }
            }
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Send | ActionKind::Edit,
                error,
            } => {
                if self.selected.as_ref() == Some(&room_id) {
                    self.send_error = Some(safe_line(&error.to_string()));
                }
            }
            CoreEvent::ActionFailed {
                room_id,
                action: action @ (ActionKind::Redact | ActionKind::React),
                error,
            } => {
                if self.selected.as_ref() == Some(&room_id) {
                    task = self.show_notice(action, &error);
                }
            }
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Thread,
                ..
            } => {
                if self.selected.as_ref() == Some(&room_id) {
                    task = self.show_notice_text("The thread could not be loaded.".to_owned());
                }
            }
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Media,
                ..
            } => {
                self.media.fail_saves(&room_id);
                if self.selected.as_ref() == Some(&room_id) {
                    task = self.show_notice_text("The file could not be saved.".to_owned());
                }
            }
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Notifications,
                ..
            } => {
                // The change did not happen: show what the list says again.
                self.room_modes.remove(&room_id);
                if room_id.as_str().is_empty() {
                    self.on_account_notification_failed();
                }
                if self.selected.as_ref() == Some(&room_id) {
                    task = self.show_notice_text(
                        "The notification setting could not be changed.".to_owned(),
                    );
                }
            }
            CoreEvent::ActionFailed {
                action: ActionKind::Profile,
                ..
            } => self.on_profile_failed(),
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Rooms,
                error,
            } => task = self.on_rooms_failed(&room_id, &error),
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Discard,
                ..
            } => {
                if self.selected.as_ref() == Some(&room_id) {
                    task = self.show_notice_text(upload::CANCEL_REFUSED.to_owned());
                }
            }
            CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Upload,
                error,
            } => {
                if self.selected.as_ref() == Some(&room_id) {
                    task = self.show_notice_text(upload::failure_text(&error).to_owned());
                }
            }
            CoreEvent::Typing { room_id, mut users } => {
                if self.selected.as_ref() == Some(&room_id) {
                    users.truncate(timeline::MAX_TYPING_KEPT);
                    self.typing = users;
                }
            }
            CoreEvent::ActionFailed {
                action: ActionKind::Verification,
                error,
                ..
            } => task = self.on_verification_failed(&error),
            CoreEvent::ActionFailed {
                action: ActionKind::Devices,
                error,
                ..
            } => self.on_devices_failed(&error),
            CoreEvent::ActionFailed {
                action: ActionKind::SignOutDevices,
                error,
                ..
            } => self.on_sign_out_failed(&error),
            CoreEvent::ReauthRequired {
                action: ReauthAction::DeleteDevices,
            } => task = self.on_sessions_reauth(),
            // A sign-out in flight owns the page, even with its screen closed
            // (a reset is refused while one runs).
            CoreEvent::OpenAccountManagement(url)
                if self.sessions.pending.is_some()
                    || self.security.busy != Some(Busy::Resetting) =>
            {
                task = self.on_sessions_url(&url);
            }
            // Recovery and cross-signing failures belong to the security
            // screens; other actions have no screen yet.
            event @ (CoreEvent::ActionFailed { .. }
            | CoreEvent::Security(_)
            | CoreEvent::RecoveryProgress(_)
            | CoreEvent::RecoveryKeyReady(_)
            | CoreEvent::Recovered
            | CoreEvent::IdentityReset
            | CoreEvent::ReauthRequired { .. }
            | CoreEvent::OpenAccountManagement(_)) => {
                task = self.on_security_event(event);
            }
            CoreEvent::VerificationRequested { id, peer } => {
                task = self.on_verification_requested(id, &peer);
            }
            CoreEvent::Verification { id, phase } => {
                task = self.on_verification_phase(&id, phase);
            }
            CoreEvent::UserTrust { user_id, trust } => self.remember_trust(user_id, trust),
            CoreEvent::Devices(list) => self.on_devices_event(list),
            // Phase 5 events: the app screens that show them arrive with the
            // UI tasks of the Phase 5 plan; until then they are ignored.
            CoreEvent::Media {
                room_id,
                item_id,
                size,
                result,
            } => task = self.on_media(room_id, item_id, size, result),
            CoreEvent::MediaSaved { room_id, item_id } => {
                self.media.finish_save(&(room_id, item_id));
            }
            CoreEvent::MediaLimits { max_upload_bytes } => {
                self.uploads.max_upload_bytes = max_upload_bytes;
            }
            CoreEvent::Notification(view) => task = self.on_notification(view),
            CoreEvent::NotificationSettings(settings) => self.on_notification_settings(settings),
            CoreEvent::UnreadTotals { notifications, .. } => {
                self.tray
                    .set_unread(notifications, self.ui_settings.tray_badge);
            }
            CoreEvent::RoomCreated { room_id } => task = self.on_room_created(room_id),
            CoreEvent::RoomJoined { room_id } => task = self.on_room_joined(room_id),
            CoreEvent::RoomDetails(details) => task = self.on_room_details(details),
            CoreEvent::RoomMembers {
                room_id,
                members,
                truncated,
            } => task = self.on_room_members(room_id, members, truncated),
            CoreEvent::Spaces(spaces) => task = self.on_spaces(spaces),
            event @ (CoreEvent::ThreadDiff { .. } | CoreEvent::ThreadPagination { .. }) => {
                task = self.thread_event(event);
            }
            CoreEvent::OwnProfile(view) => self.on_own_profile(&view),
            event @ (CoreEvent::CallState(_)
            | CoreEvent::CallParticipants { .. }
            | CoreEvent::ActiveCall { .. }
            | CoreEvent::IncomingCall { .. }
            | CoreEvent::RingingStopped { .. }
            | CoreEvent::CallDeclined { .. }
            | CoreEvent::AudioDevices(_)
            | CoreEvent::AudioSettings(_)
            | CoreEvent::InputLevel(_)
            | CoreEvent::AudioStats(_)
            | CoreEvent::GpuEnhancement(_)) => {
                self.audio_card_saw(&event);
                task = self.on_call_event(event);
            }
        }
        task
    }

    /// A plain `Error` strands the user unless it is handled: a failed restore
    /// (the core emits `Error`, not `NoSession`) leaves `Starting`, and an
    /// error during a login ends that attempt. `CoreError`'s `Display` never
    /// carries secrets, so it is safe to show. Other screens ignore it.
    ///
    /// Ending an attempt that waits for the browser also cancels it in the
    /// core, or the core stays busy and its redirect listener stays open.
    fn on_core_error(&mut self, error: &CoreError) -> Task<Message> {
        let waiting = self.screen == Screen::WaitingForBrowser;
        match self.screen {
            Screen::Starting => {
                self.notice = Some(safe_line(&format!(
                    "Could not restore your session: {error}"
                )));
                self.end_login_attempt();
                self.screen = Screen::Login;
                Task::none()
            }
            Screen::Login | Screen::WaitingForBrowser if self.login_in_flight => {
                self.notice = Some(safe_line(&format!("Sign-in failed: {error}")));
                self.end_login_attempt();
                self.screen = Screen::Login;
                if waiting {
                    self.cancel_in_core()
                } else {
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }

    /// Sends `CancelOAuth` for an attempt the app ended itself, and expects
    /// one `LoginFailed(Cancelled)` back.
    fn cancel_in_core(&mut self) -> Task<Message> {
        self.pending_cancel = true;
        self.send(Command::CancelOAuth)
    }

    /// Sends the password login from the form. The password moves into the
    /// command and the field is cleared (plan Q19).
    fn submit_login(&mut self) -> Task<Message> {
        if self.screen != Screen::Login || self.login_in_flight {
            return Task::none();
        }
        if self.refuse_hidden_server() {
            return Task::none();
        }
        let username = self.login.username.trim().to_owned();
        if username.is_empty() || self.login.password.expose().is_empty() {
            self.notice = Some("Enter a user name and password.".to_owned());
            return Task::none();
        }
        let server = login::server_to_send(&self.login.server);
        let password = std::mem::replace(&mut self.login.password, Password::from(String::new()));
        self.notice = None;
        self.login_in_flight = true;
        self.send(Command::LoginPassword {
            username,
            password,
            server,
        })
    }

    /// Sets the notice and returns true when the Server field holds an
    /// address that is refused: one that hides its real server (userinfo and
    /// the like), a name with a character that is not allowed, or plain
    /// `http://` to a host that is not this computer. Nothing is sent for it,
    /// and the password stays in the form (it has not been moved yet).
    fn refuse_hidden_server(&mut self) -> bool {
        match login::server_refusal(&self.login.server) {
            Some(notice) => {
                self.notice = Some(notice.to_owned());
                true
            }
            None => false,
        }
    }

    /// Asks the core for an OAuth login at the server named in the form.
    fn request_oauth(&mut self) -> Task<Message> {
        if self.screen != Screen::Login || self.login_in_flight {
            return Task::none();
        }
        if self.refuse_hidden_server() {
            return Task::none();
        }
        let Some(server) = login::server_to_send(&self.login.server) else {
            self.notice = Some("Enter a server name to sign in with the browser.".to_owned());
            return Task::none();
        };
        self.notice = None;
        self.login_in_flight = true;
        self.browser_opened = None;
        self.send(Command::LoginOAuth { server })
    }

    /// Re-checks the OAuth URL, then opens it in a task (plan Q21).
    fn on_oauth_url(&mut self, url: &str) -> Task<Message> {
        if self.screen != Screen::Login || !self.login_in_flight {
            return Task::none();
        }
        match login::check_browser_url(url) {
            Ok(link) => {
                self.screen = Screen::WaitingForBrowser;
                self.browser_opened = None;
                self.browser_link = Some(link.clone());
                Task::perform(async move { login::open_in_browser(&link) }, |ok| {
                    Message::BrowserOpened { ok }
                })
            }
            Err(refused) => {
                self.notice = Some(format!("Sign-in stopped: {refused}."));
                self.end_login_attempt();
                self.cancel_in_core()
            }
        }
    }

    /// Keeps `query` and asks the core for it, unless it is the query the core
    /// already has. The list comes back as a `Reset`, so paging may ask again.
    pub(crate) fn set_query(&mut self, query: RoomListQuery) -> Task<Message> {
        if query == self.query {
            return Task::none();
        }
        self.query = query.clone();
        self.load_more_at = None;
        self.send(Command::SetRoomListQuery(query))
    }

    /// The density the app draws in.
    pub fn density(&self) -> crate::ui::tokens::Density {
        self.ui_settings.density.into()
    }

    /// What the main window shows at its width.
    pub fn pane_layout(&self) -> crate::layout::PaneLayout {
        crate::layout::layout_for(
            self.window_width,
            self.ui_settings.room_list,
            self.threads.panel.is_some(),
            self.calls.panel_shown(self.selected.as_ref()),
            self.newest_panel,
        )
    }

    /// Remembers which side panel was opened last, given which were open
    /// before the message. Closing one leaves the other as it was.
    fn note_panels(&mut self, thread_before: bool, call_before: bool) {
        let thread = self.threads.panel.is_some();
        let call = self.calls.panel_shown(self.selected.as_ref());
        if call && !call_before {
            self.newest_panel = crate::layout::SidePanel::Call;
        }
        if thread && !thread_before {
            self.newest_panel = crate::layout::SidePanel::Thread;
        }
    }

    /// The height of a row of the room list as shown now: the density's, or
    /// the rail's fixed one.
    pub fn room_row_height(&self) -> f32 {
        room_list::list_row_height(self.density(), self.pane_layout().list)
    }

    /// What follows every change of the list or of the viewport: paging and
    /// avatar requests.
    pub(crate) fn after_list_change(&mut self) -> Task<Message> {
        Task::batch([self.check_paging(), self.request_avatars()])
    }

    /// Asks the core for the avatars of the built rows that were never
    /// requested this session (plan Q11): one `FetchAvatars`, or nothing when
    /// the set is empty. An ID is requested once per session whatever the
    /// answer, so a failing homeserver is not hit again by scrolling.
    fn request_avatars(&mut self) -> Task<Message> {
        let len = self.rooms.len();
        let range = room_list::visible_range(
            self.list_offset_y,
            self.list_viewport_h,
            self.room_row_height(),
            len,
            room_list::OVERSCAN_ROWS,
        );
        let ids = self.avatars.take_unrequested(
            self.rooms[range]
                .iter()
                .filter_map(|room| room.avatar.clone()),
        );
        if ids.is_empty() {
            return Task::none();
        }
        self.send(Command::FetchAvatars(ids))
    }

    /// The paging bookkeeping (plan Q8): asks for the next page when the
    /// built rows reach the end of the loaded list, at most once per loaded
    /// length.
    fn check_paging(&mut self) -> Task<Message> {
        let len = self.rooms.len();
        if self.load_more_at == Some(len) {
            return Task::none();
        }
        let range = room_list::visible_range(
            self.list_offset_y,
            self.list_viewport_h,
            self.room_row_height(),
            len,
            room_list::OVERSCAN_ROWS,
        );
        if !room_list::should_load_more(range.end, len, self.total_rooms) {
            return Task::none();
        }
        self.load_more_at = Some(len);
        self.send(Command::LoadMoreRooms)
    }

    /// The **Emoji** button: types the opener of a shortcode and puts the
    /// focus back in the editor. A composer that is out of sight (no room, or
    /// a panel fills the window) takes nothing.
    fn emoji_requested(&mut self, now: Instant) -> Task<Message> {
        if self.selected.is_none() || self.pane_layout().panel_fills {
            return Task::none();
        }
        let mut tasks = Vec::new();
        for c in self.composer.emoji_opener().chars() {
            tasks.push(self.composer_action_at(
                iced::widget::text_editor::Action::Edit(iced::widget::text_editor::Edit::Insert(c)),
                now,
            ));
        }
        tasks.push(operation::focus(composer::COMPOSER_ID));
        Task::batch(tasks)
    }

    /// Applies an editor action to the composer of the open room and sends
    /// what that asks for: typing notices and member searches.
    pub(crate) fn composer_action_at(
        &mut self,
        action: iced::widget::text_editor::Action,
        now: Instant,
    ) -> Task<Message> {
        if self.selected.is_none() {
            return Task::none();
        }
        let effects = self.composer.apply(action, now);
        self.run_effects(effects.search, effects.typing)
    }

    /// Sends a `SetTyping` and starts or runs a member search step.
    fn run_effects(&self, search: SearchStep, typing: Option<bool>) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        if let Some(typing) = typing {
            tasks.push(self.send(Command::SetTyping {
                room_id: room_id.clone(),
                typing,
            }));
        }
        match search {
            SearchStep::Idle => {}
            SearchStep::Send(query) => {
                tasks.push(self.send(Command::SearchMembers { room_id, query }));
            }
            SearchStep::Arm(after) => tasks.push(self.sink.timer(after, Message::MembersDue)),
        }
        Task::batch(tasks)
    }

    fn members_due_at(&mut self, now: Instant) -> Task<Message> {
        let step = self.composer.search.timer_fired(now);
        self.run_effects(step, None)
    }

    fn pick_suggestion_at(&mut self, index: Option<usize>, now: Instant) -> Task<Message> {
        match self.composer.pick(index, now) {
            Some(effects) if self.selected.is_some() => {
                self.run_effects(effects.search, effects.typing)
            }
            _ => Task::none(),
        }
    }

    fn composer_key_at(&mut self, key: ComposerKey, now: Instant) -> Task<Message> {
        // A dialog, card or screen is over the room: the editor may still
        // have the keyboard focus, but its keys are not for it.
        if self.any_overlay_open() {
            return Task::none();
        }
        match key {
            ComposerKey::Send => self.submit_composer(),
            ComposerKey::Pick => self.pick_suggestion_at(None, now),
            ComposerKey::Next => {
                self.composer.step(1);
                Task::none()
            }
            ComposerKey::Previous => {
                self.composer.step(-1);
                Task::none()
            }
            ComposerKey::Escape => {
                self.escape_spent = self.composer_has_escape_work();
                let was_edit = matches!(self.composer.mode, composer::Mode::Edit(_));
                self.composer.escape();
                self.after_edit_left(was_edit)
            }
        }
    }

    /// The message `id` of the open timeline: its plain body, whether it is
    /// the user's own, and its details. `None` for other kinds of item.
    fn find_message(&self, id: &str) -> Option<(&str, bool, &MessageDetails)> {
        // Thread item IDs carry their root and never equal a room item's,
        // so one search over both lists finds the right item.
        let thread = self.threads.panel.iter().flat_map(|p| &p.timeline.items);
        self.timeline
            .items
            .iter()
            .chain(thread)
            .find(|item| item.id == id)
            .and_then(|item| match &item.kind {
                TimelineItemKind::Message {
                    body,
                    is_own,
                    details,
                    ..
                } => Some((body.as_str(), *is_own, details.as_ref())),
                _ => None,
            })
    }

    /// Whether the pane that draws an item of the thread panel (`in_thread`)
    /// or of the room's timeline is on screen: the room pane has no width
    /// while a panel fills the window, and the thread panel is out of sight
    /// when the call panel has the turn.
    fn pane_shows_item(&self, in_thread: bool) -> bool {
        let layout = self.pane_layout();
        if in_thread {
            layout.thread && self.threads.panel.is_some()
        } else {
            !layout.panel_fills
        }
    }

    /// Whether `id` is an item of the thread panel.
    fn is_thread_item(&self, id: &str) -> bool {
        self.threads
            .panel
            .as_ref()
            .is_some_and(|p| p.timeline.items.iter().any(|item| item.id == id))
    }

    /// The buttons message `id` offers right now, or `None` when it is gone.
    fn buttons_for(&self, id: &str) -> Option<ActionButtons> {
        self.find_message(id)
            .map(|(_, is_own, details)| actions::action_buttons(details, is_own))
    }

    /// The text **Copy** puts on the clipboard: the plain body, when the core
    /// allows copying.
    pub fn copy_source(&self, id: &str) -> Option<&str> {
        let (body, is_own, details) = self.find_message(id)?;
        actions::action_buttons(details, is_own)
            .copy
            .then_some(body)
    }

    /// After a reply or an edit mode ended: an edit that leaves an empty
    /// composer ends the typing notice it started.
    fn after_edit_left(&mut self, was_edit: bool) -> Task<Message> {
        if was_edit
            && matches!(self.composer.mode, composer::Mode::Normal)
            && self.composer.is_empty()
        {
            self.stop_typing()
        } else {
            Task::none()
        }
    }

    /// **Reply**: quote message `id` in the next send.
    fn request_reply(&mut self, id: String) -> Task<Message> {
        // The room's composer cannot quote a thread item.
        if self.selected.is_none()
            || self.is_thread_item(&id)
            || !self.buttons_for(&id).is_some_and(|b| b.reply)
        {
            return Task::none();
        }
        let was_edit = matches!(self.composer.mode, composer::Mode::Edit(_));
        if was_edit {
            self.composer.cancel_mode();
        }
        self.composer.mode = composer::Mode::Reply(id);
        Task::batch([
            self.after_edit_left(was_edit),
            operation::focus(composer::COMPOSER_ID),
        ])
    }

    /// **Edit**: load the plain body of the user's own message `id`.
    fn request_edit(&mut self, id: String) -> Task<Message> {
        if self.selected.is_none()
            || self.is_thread_item(&id)
            || !self.buttons_for(&id).is_some_and(|b| b.edit)
        {
            return Task::none();
        }
        let Some(body) = self.find_message(&id).map(|(body, ..)| body.to_owned()) else {
            return Task::none();
        };
        self.composer.begin_edit(id, &body);
        operation::focus(composer::COMPOSER_ID)
    }

    /// The user confirmed **Delete**: redact the message, if it is still
    /// there and the core still allows it.
    fn confirm_delete(&mut self) -> Task<Message> {
        let Some(id) = self.actions.confirm_delete.take() else {
            return Task::none();
        };
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        if !self.buttons_for(&id).is_some_and(|b| b.delete) {
            return Task::none();
        }
        self.send(Command::Redact {
            room_id,
            item_id: id,
            reason: None,
        })
    }

    /// Sends `ToggleReaction` for message `id` when the core allows reacting.
    fn toggle_reaction(&self, id: &str, key: &str) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        if !self.buttons_for(id).is_some_and(|b| b.react) {
            return Task::none();
        }
        self.send(Command::ToggleReaction {
            room_id,
            item_id: id.to_owned(),
            key: key.to_owned(),
        })
    }

    /// An emoji was picked: react with it and close the picker.
    fn pick_reaction(&mut self, key: &str) -> Task<Message> {
        match self.actions.picker.take() {
            // Defence in depth: `update` closes a hidden picker, so this
            // only refuses what no one could have seen.
            Some(picker) if self.pane_shows_item(picker.in_thread) => {
                self.toggle_reaction(&picker.target, key)
            }
            Some(_) => Task::none(),
            None => Task::none(),
        }
    }

    /// Stores the open room's draft, or clears it when the composer is empty;
    /// nothing when the composer was not touched since the room was opened.
    fn save_draft(&mut self) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        if !self.draft_dirty {
            return Task::none();
        }
        let draft = self.composer.draft_parts().map(|(text, mentions, reply)| {
            // The reply target is kept by event; an item not sent yet has
            // none, and the draft then stays a plain message.
            let reply_to = reply.and_then(|item| {
                self.timeline
                    .items
                    .iter()
                    .find(|i| i.id == item)
                    .and_then(|i| i.event_id.clone())
            });
            Draft {
                text: MessageText::from(text),
                mentions,
                reply_to,
            }
        });
        self.send(Command::SaveDraft { room_id, draft })
    }

    /// Shows the failure line of a delete or reaction for 5 seconds.
    fn show_notice(&mut self, action: ActionKind, error: &CoreError) -> Task<Message> {
        let Some(text) = actions::notice_text(action, &safe_line(&error.to_string())) else {
            return Task::none();
        };
        self.show_notice_text(text)
    }

    /// Shows a failure line for 5 seconds.
    pub(crate) fn show_notice_text(&mut self, text: String) -> Task<Message> {
        let seq = self.actions.notice_seq.saturating_add(1);
        self.actions.notice_seq = seq;
        self.actions.notice = Some(Notice { seq, text });
        self.sink
            .timer(NOTICE_DURATION, Message::NoticeExpired(seq))
    }

    /// `SetTyping(false)` for the open room when a notice is outstanding.
    fn stop_typing(&mut self) -> Task<Message> {
        match &self.selected {
            Some(room_id) if self.composer.typing.stop() => self.send(Command::SetTyping {
                room_id: room_id.clone(),
                typing: false,
            }),
            _ => Task::none(),
        }
    }

    /// Sends the composer text to the selected room (a new message, a reply
    /// or an edit) and resets the composer. A blank text sends nothing.
    fn submit_composer(&mut self) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        let message = match composer::outgoing(&self.composer, &room_id) {
            composer::Outgoing::Send(message) => message,
            composer::Outgoing::UnknownCommand(name) => {
                self.command_bar = Some(name);
                return Task::none();
            }
            composer::Outgoing::CommandError(name) => {
                return self.show_notice_text(command_error_text(&name));
            }
            composer::Outgoing::Nothing => return Task::none(),
        };
        self.send_composed(message)
    }

    /// Sends `message` as a new message, a reply or an edit, as the composer
    /// mode says, and resets the composer.
    fn send_composed(&mut self, message: OutgoingMessage) -> Task<Message> {
        self.command_bar = None;
        let command = match &self.composer.mode {
            composer::Mode::Edit(item_id) => Command::Edit {
                item_id: item_id.clone(),
                message,
            },
            composer::Mode::Normal | composer::Mode::Reply(_) => Command::Send(message),
        };
        self.composer.finish_send();
        self.send_error = None;
        let sent = self.send(command);
        // The stored draft becomes what is left: nothing, or after an edit
        // the draft it set aside. Marked dirty here because "Send as message"
        // reaches this without a composer message.
        self.draft_dirty = true;
        let save = self.save_draft();
        Task::batch([sent, save, self.stop_typing()])
    }

    /// Forgets everything about a login attempt: the flag, the browser link
    /// and the password.
    fn end_login_attempt(&mut self) {
        self.login_in_flight = false;
        self.browser_link = None;
        self.browser_opened = None;
        self.login.password = Password::from(String::new());
    }

    /// Empties the composer without telling the room (the caller does that
    /// where the room is still open and reachable).
    fn clear_draft(&mut self) {
        self.composer.clear();
        self.composer.typing.reset();
        self.send_error = None;
    }

    fn back_to_login(&mut self, notice: &str) -> Task<Message> {
        self.pending_cancel = false;
        self.user_id = None;
        self.rooms.clear();
        self.total_rooms = None;
        self.load_more_at = None;
        self.avatars.clear();
        self.media.clear();
        self.uploads.clear();
        self.list_offset_y = 0.0;
        self.deselect();
        self.sync_state = SyncState::Idle;
        let flush = self.drop_secrets();
        self.security = SecurityModel::default();
        self.verification = None;
        self.sessions = SessionsModel::default();
        self.calls = CallModel::default();
        self.audio_ui = crate::audio_settings::AudioUi::default();
        self.rooms_ui = RoomsModel::default();
        self.settings_ui = SettingsModel::default();
        self.notification_settings = None;
        self.spaces.clear();
        self.reset_threads();
        self.notice = Some(notice.to_owned());
        self.end_login_attempt();
        self.screen = Screen::Login;
        flush
    }

    pub(crate) fn deselect(&mut self) {
        self.selected = None;
        self.reset_threads();
        self.timeline.reset();
        self.media.reset_view();
        self.uploads.reset_view();
        self.user_trust.clear();
        self.typing.clear();
        self.actions.reset();
        self.clear_draft();
        self.reset_link_state();
        self.rooms_ui.reset_room();
    }

    /// Forgets revealed spoilers, the link bar and the unknown-command bar
    /// of the room left.
    fn reset_link_state(&mut self) {
        self.revealed.clear();
        self.link_bar = None;
        self.command_bar = None;
    }

    /// A click on a link, pill or spoiler (plan Q19). Nothing here opens
    /// anything itself: opening is a `Task`.
    fn link_clicked(&mut self, target: LinkTarget) -> Task<Message> {
        match target {
            // Every link asks first; opening happens on `LinkConfirmed`.
            LinkTarget::Url(url) => {
                self.link_bar = Some(match links::check_link_url(&url) {
                    Ok(checked) => LinkBar::Confirm(checked),
                    Err(_) => LinkBar::Refused,
                });
                Task::none()
            }
            LinkTarget::Room(room) => {
                let id = RoomId::from(room.as_str());
                if room_list::contains(&self.rooms, &id) {
                    self.update(Message::SelectRoom(id))
                } else {
                    self.room_pill_clicked(&room)
                }
            }
            LinkTarget::User(user) => self.user_pill_clicked(user),
            LinkTarget::Spoiler { item, index } => {
                let key = (item, index);
                if !self.revealed.remove(&key) && self.revealed.len() < MAX_REVEALED {
                    self.revealed.insert(key);
                }
                Task::none()
            }
        }
    }

    /// Applies timeline diffs of the selected room; other rooms are ignored.
    fn apply_timeline(
        &mut self,
        room_id: &RoomId,
        diffs: Vec<ListDiff<TimelineItem>>,
    ) -> Task<Message> {
        if self.selected.as_ref() != Some(room_id) {
            return Task::none();
        }
        let effects = self.timeline.apply_diffs(diffs);
        let members = self.members_for_pills();
        Task::batch([self.timeline_effects(effects), members])
    }

    /// The timeline scrolled to `offset_from_bottom` pixels from the bottom
    /// in a viewport of `viewport_h` by `width` pixels.
    pub fn on_timeline_scrolled(
        &mut self,
        offset_from_bottom: f32,
        viewport_h: f32,
        width: f32,
    ) -> Task<Message> {
        let effects = self
            .timeline
            .scrolled(offset_from_bottom, viewport_h, width);
        self.timeline_effects(effects)
    }

    /// Turns what the timeline model asks for into a scroll task and a
    /// `PaginateBackwards` command (plan Q14, Q16).
    fn timeline_effects(&mut self, effects: Effects) -> Task<Message> {
        let mut tasks = Vec::new();
        if let Some(y) = effects.scroll_to {
            tasks.push(operation::scroll_to(
                timeline::TIMELINE_ID,
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(y),
                },
            ));
        }
        if effects.paginate
            && let Some(room_id) = self.selected.clone()
        {
            tasks.push(self.send(Command::PaginateBackwards { room_id }));
        }
        if let Some(id) = effects.flash {
            tasks.push(
                self.sink
                    .timer(timeline::FLASH_DURATION, Message::FlashEnded(id)),
            );
        }
        if effects.jump_failed {
            tasks.push(self.show_notice_text(JUMP_FAILED_TEXT.to_owned()));
        }
        tasks.push(self.fetch_visible_media());
        tasks.push(self.read_receipt(Instant::now()));
        Task::batch(tasks)
    }

    /// Asks for the thumbnail of every image within one screen of the
    /// viewport that is neither cached nor on its way (plan Q7). Rows far
    /// away ask for nothing, and an answer already in the cache is never
    /// asked for again.
    fn fetch_visible_media(&mut self) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        let wanted = visible_images(&self.timeline);
        self.fetch_thumbnails(&room_id, wanted)
    }

    /// The same for the thread panel's timeline, whose item IDs
    /// (`thread:{root}:{n}`) share the media cache with the room's.
    pub(crate) fn fetch_visible_thread_media(&mut self) -> Task<Message> {
        let Some(panel) = &self.threads.panel else {
            return Task::none();
        };
        let room_id = panel.room_id.clone();
        let wanted = visible_images(&panel.timeline);
        self.fetch_thumbnails(&room_id, wanted)
    }

    fn fetch_thumbnails(&mut self, room_id: &RoomId, wanted: Vec<String>) -> Task<Message> {
        let mut tasks = Vec::new();
        for item_id in wanted {
            if self.media.needs_fetch(&(room_id.clone(), item_id.clone())) {
                tasks.push(self.send(Command::FetchMedia {
                    room_id: room_id.clone(),
                    item_id,
                    size: MediaSize::Thumbnail,
                }));
            }
        }
        Task::batch(tasks)
    }

    /// The core answered a `FetchMedia`. The bytes are untrusted: they are
    /// decoded in a task, never here, and only if somebody still waits.
    fn on_media(
        &mut self,
        room_id: RoomId,
        item_id: String,
        size: MediaSize,
        result: Result<MediaBytes, CoreError>,
    ) -> Task<Message> {
        let key = (room_id.clone(), item_id.clone());
        let wanted = match size {
            MediaSize::Thumbnail => self.media.is_pending(&key),
            MediaSize::Full => self.media.viewer_wants(&key),
        };
        if !wanted {
            return Task::none();
        }
        let Ok(bytes) = result else {
            match size {
                MediaSize::Thumbnail => self.media.thumb_failed(&key),
                MediaSize::Full => self.media.viewer_decoded(&key, None),
            }
            return Task::none();
        };
        // Decoding untrusted bytes is CPU work: off the executor, and only
        // a couple at a time.
        Task::perform(
            async move {
                blocking::run(move || match size {
                    MediaSize::Thumbnail => media::decode_thumbnail(bytes.as_bytes()),
                    MediaSize::Full => media::decode_full(bytes.as_bytes()),
                })
                .await
                .flatten()
            },
            move |decoded| Message::MediaDecoded {
                room_id: room_id.clone(),
                item_id: item_id.clone(),
                size,
                decoded,
            },
        )
    }

    /// The sender-given file name of media message `item_id` in the open
    /// room.
    fn media_filename(&self, item_id: &str) -> Option<&str> {
        let (_, _, details) = self.find_message(item_id)?;
        details.media.as_deref().map(|m| m.filename.as_str())
    }

    /// When the message `item_id` of the room or the thread panel was sent.
    fn message_timestamp_ms(&self, item_id: &str) -> Option<u64> {
        let thread = self.threads.panel.iter().flat_map(|p| &p.timeline.items);
        self.timeline
            .items
            .iter()
            .chain(thread)
            .find(|item| item.id == item_id)
            .and_then(|item| match &item.kind {
                TimelineItemKind::Message { timestamp_ms, .. } => Some(*timestamp_ms),
                _ => None,
            })
    }

    /// Opens the viewer on an image of the open room and asks for the file.
    fn open_viewer(&mut self, item_id: String) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        let is_image = self
            .find_message(&item_id)
            .and_then(|(_, _, details)| details.media.as_deref())
            .is_some_and(|m| m.kind == MediaKind::Image);
        if !is_image {
            return Task::none();
        }
        self.media.open_viewer((room_id.clone(), item_id.clone()));
        self.send(Command::FetchMedia {
            room_id,
            item_id,
            size: MediaSize::Full,
        })
    }

    /// **Save...**: the dialog proposes a name made from the sender's and
    /// the sent time by [`media::save_name`]; the answer comes back as
    /// `SavePicked`.
    fn request_save(&mut self, item_id: &str) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        let Some(filename) = self.media_filename(item_id) else {
            return Task::none();
        };
        let sent = self
            .message_timestamp_ms(item_id)
            .and_then(|ms| i64::try_from(ms).ok())
            .and_then(|ms| chrono::TimeZone::timestamp_millis_opt(&chrono::Local, ms).single());
        let suggested = match sent {
            Some(sent) => media::save_name(filename, &sent),
            None => media::suggested_file_name(filename),
        };
        let item_id = item_id.to_owned();
        self.dialogs
            .save_file(&suggested)
            .map(move |path| Message::SavePicked {
                room_id: room_id.clone(),
                item_id: item_id.clone(),
                path: path.map(SavePath::from),
            })
    }

    /// The user picked a path: send it to the core as it is. The path is
    /// never built from the sender's file name.
    fn save_picked(
        &mut self,
        room_id: RoomId,
        item_id: String,
        path: Option<SavePath>,
    ) -> Task<Message> {
        let Some(path) = path else {
            return Task::none();
        };
        let path = path.as_path();
        if self.selected.as_ref() != Some(&room_id) || self.media_filename(&item_id).is_none() {
            return Task::none();
        }
        if !media::save_path_usable(path) {
            return self.show_notice_text("The file could not be saved there.".to_owned());
        }
        self.media
            .begin_save((room_id.clone(), item_id.clone()), path.to_path_buf());
        self.send(Command::SaveMedia {
            room_id,
            item_id,
            path: path.to_path_buf(),
        })
    }

    /// Sends the read receipt when one is due, or starts the timer that
    /// asks again (plan Q12). `now` is a parameter for the tests.
    fn read_receipt(&mut self, now: Instant) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        // Nothing of a room pane that has no width is seen, so nothing is
        // marked read; the resize that shows it again asks once more.
        if self.pane_layout().panel_fills {
            return Task::none();
        }
        match self.timeline.read_step(self.window_focused, now) {
            timeline::ReadStep::Idle => Task::none(),
            timeline::ReadStep::Send => self.send(Command::MarkRead {
                room_id,
                fully_read: false,
            }),
            timeline::ReadStep::Wait(after) => self.sink.timer(after, Message::ReadDue(room_id)),
        }
    }

    /// `MoveReadMarker` for the room being left, when its bottom was seen
    /// with the window focused. Never a public receipt.
    fn move_read_marker(&mut self) -> Task<Message> {
        match &self.selected {
            // Not for a pane that was hidden when the room was left: what
            // arrived meanwhile was not seen.
            Some(room_id) if self.timeline.bottom_seen && !self.pane_layout().panel_fills => self
                .send(Command::MoveReadMarker {
                    room_id: room_id.clone(),
                }),
            _ => Task::none(),
        }
    }

    /// **Retry** or **Discard** for item `id`, only while it is a failed
    /// local echo.
    fn echo_command(&mut self, item_id: String, discard: bool) -> Task<Message> {
        // A failed echo can be retried or discarded; an upload that is on
        // its way can be cancelled, which is a discard.
        let allowed = self.find_message(&item_id).is_some_and(|(_, _, details)| {
            details.send_failure.is_some()
                || (discard
                    && details
                        .media
                        .as_deref()
                        .is_some_and(|media| media.upload.is_some()))
        });
        let Some(room_id) = self.selected.clone().filter(|_| allowed) else {
            return Task::none();
        };
        self.send(if discard {
            Command::DiscardSend { room_id, item_id }
        } else {
            Command::RetrySend { room_id, item_id }
        })
    }

    /// The window content.
    pub fn view(&self) -> Element<'_, Message> {
        match self.screen {
            Screen::Starting => {
                screens::centered_state("Starting\u{2026}", "Looking for your saved session.")
            }
            Screen::Login => login::view(self),
            Screen::WaitingForBrowser => login::waiting_view(self),
            Screen::Loading => {
                let base = screens::security::layer(
                    loading::view(self),
                    screens::security::top_layer(self),
                );
                screens::verification::layer(base, self)
            }
            Screen::Main => screens::call::ring_layer(
                screens::verification::layer(
                    screens::switcher::layer(
                        screens::upload_dialog::layer(
                            screens::media_viewer::layer(
                                screens::audio_settings::layer(
                                    screens::settings::layer(
                                        screens::room_dialogs::layer(main_view::view(self), self),
                                        self,
                                    ),
                                    self,
                                ),
                                self,
                            ),
                            self,
                        ),
                        self,
                    ),
                    self,
                ),
                self,
            ),
            Screen::ShuttingDown => {
                screens::centered_state("Closing\u{2026}", "Saving your drafts and settings.")
            }
            Screen::Disconnected => screens::centered_state(
                "The core stopped",
                "Close the window and start Twicecat again.",
            ),
        }
    }

    /// The core's events and window close requests.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            bridge::events_subscription(self.events.clone()),
            window::close_requests().map(|_| Message::CloseRequested),
            // While the Voice & audio card waits for the push-to-talk key,
            // keys go to it and no shortcut (Esc included) is made of them:
            // one listener, so the two never race.
            if self.audio_ui.capturing {
                iced::event::listen_with(|event, _status, _id| match event {
                    iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocus(true)),
                    iced::Event::Window(window::Event::Unfocused) => {
                        Some(Message::WindowFocus(false))
                    }
                    other => crate::audio_settings::capture_message(&other),
                })
            } else {
                iced::event::listen_with(|event, status, _id| match event {
                    iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocus(true)),
                    iced::Event::Window(window::Event::Unfocused) => {
                        Some(Message::WindowFocus(false))
                    }
                    other => upload::event_message(&other).or_else(|| {
                        keys::event_message(&other, status == iced::event::Status::Captured)
                    }),
                })
            },
            system::theme_changes().map(Message::ThemeMode),
            window::open_events().map(Message::WindowOpened),
            window::resize_events().map(|(_, size)| Message::WindowResized(size.width)),
            tray::subscription(),
            self.ptt.subscription(),
        ])
    }

    /// The theme for the current system mode (plan Q13).
    pub fn theme(&self) -> Option<Theme> {
        Some(self.current_theme())
    }
}

/// The IDs of the image messages within one screen of `view`'s viewport.
fn visible_images(view: &TimelineView) -> Vec<String> {
    let range = timeline::visible_window(
        &view.prefix,
        view.offset_from_bottom,
        view.viewport_h,
        view.viewport_h,
    );
    view.items
        .get(range)
        .unwrap_or_default()
        .iter()
        .filter(|item| {
            matches!(
                &item.kind,
                TimelineItemKind::Message { details, .. }
                    if details.media.as_ref().is_some_and(|m| m.kind == MediaKind::Image)
            )
        })
        .map(|item| item.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iced::widget::text_editor::{Action, Edit};
    use twicecat_core::api::{DeviceId, ListDiff, RoomId};

    use super::*;

    fn app_with(capacity: usize) -> (App, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(capacity);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        (app, rx)
    }

    fn core(event: CoreEvent) -> Message {
        Message::Core(Box::new(event))
    }

    #[test]
    fn boot_sends_restore_session() {
        let (app, mut rx) = app_with(4);
        assert!(matches!(rx.try_recv(), Ok(Command::RestoreSession)));
        assert_eq!(app.screen, Screen::Starting);
    }

    #[test]
    fn close_request_sends_shutdown_once_and_moves_to_shutting_down() {
        let (mut app, mut rx) = app_with(8);
        let _ = rx.try_recv();
        let _ = app.update(Message::CloseRequested);
        assert_eq!(app.screen, Screen::ShuttingDown);
        assert!(matches!(rx.try_recv(), Ok(Command::Shutdown)));
        let _ = app.update(Message::CloseRequested);
        assert!(rx.try_recv().is_err(), "a second close sends nothing");
        assert!(!app.exit_requested);
    }

    #[test]
    fn core_closed_while_shutting_down_exits() {
        let (mut app, _rx) = app_with(8);
        let _ = app.update(Message::CloseRequested);
        assert!(!app.exit_requested);
        let _ = app.update(Message::CoreClosed);
        assert!(app.exit_requested);
    }

    #[test]
    fn shutdown_deadline_exits_only_while_shutting_down() {
        let (mut app, _rx) = app_with(8);
        let _ = app.update(Message::ShutdownDeadline);
        assert!(!app.exit_requested);
        let _ = app.update(Message::CloseRequested);
        let _ = app.update(Message::ShutdownDeadline);
        assert!(app.exit_requested);
    }

    #[test]
    fn core_closed_on_its_own_is_disconnected_not_an_exit() {
        let (mut app, _rx) = app_with(8);
        let _ = app.update(Message::CoreClosed);
        assert_eq!(app.screen, Screen::Disconnected);
        assert!(!app.exit_requested);
    }

    #[test]
    fn full_sink_does_not_block_and_falls_back_to_a_task() {
        // Capacity 1, already holding the boot command.
        let (mut app, mut rx) = app_with(1);
        let started = Instant::now();
        let task = app.update(Message::CloseRequested);
        assert!(started.elapsed() < Duration::from_millis(50));
        assert!(task.units() > 0, "the fallback task carries the command");
        assert!(matches!(rx.try_recv(), Ok(Command::RestoreSession)));
        assert!(
            rx.try_recv().is_err(),
            "Shutdown waits in the fallback task"
        );
    }

    #[test]
    fn session_events_move_between_screens() {
        let (mut app, _rx) = app_with(8);
        let _ = app.update(core(CoreEvent::NoSession));
        assert_eq!(app.screen, Screen::Login);
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }));
        assert_eq!(app.screen, Screen::Loading);
        assert_eq!(app.user_id, Some(UserId::from("@a:x")));
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert_eq!(app.screen, Screen::Login);
        assert!(app.user_id.is_none());
    }

    #[test]
    fn first_room_list_diff_opens_the_main_screen_and_fills_the_list() {
        let (mut app, _rx) = app_with(8);
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: true,
        }));
        let room = RoomSummary::new(RoomId::from("!r:x"), "Lounge".into());
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
            values: vec![room],
        }])));
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.rooms.len(), 1);
    }

    #[test]
    fn theme_follows_the_mode() {
        let (mut app, _rx) = app_with(8);
        let name = |app: &App| app.theme().expect("always themed").to_string();
        let light = crate::theme::theme_for(theme::Mode::Light).to_string();
        let dark = crate::theme::theme_for(theme::Mode::Dark).to_string();
        assert_eq!(name(&app), light);
        let _ = app.update(Message::ThemeMode(theme::Mode::Dark));
        assert_eq!(name(&app), dark);
        assert!(app.theme().expect("themed").extended_palette().is_dark);
        let _ = app.update(Message::ThemeMode(theme::Mode::Light));
        assert_eq!(name(&app), light);
        let _ = app.update(Message::ThemeMode(theme::Mode::None));
        assert_eq!(name(&app), light);
    }

    // ---- Task 9: screens ----

    use twicecat_core::api::{AuthUrl, AvatarId, AvatarImage, CoreError};

    /// Types into the login form through `update`.
    fn fill(app: &mut App, server: &str, user: &str, password: &str) {
        let _ = app.update(Message::ServerChanged(server.into()));
        let _ = app.update(Message::UsernameChanged(user.into()));
        let _ = app.update(Message::PasswordChanged(Password::from(password)));
    }

    /// An app on the login screen with the boot command drained.
    fn login_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = app_with(16);
        let _ = rx.try_recv();
        let _ = app.update(core(CoreEvent::NoSession));
        (app, rx)
    }

    fn logged_in(app: &mut App) {
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }));
    }

    fn room_event(ids: &[&str]) -> CoreEvent {
        CoreEvent::RoomListChanged(vec![ListDiff::Reset {
            values: ids
                .iter()
                .map(|n| RoomSummary::new(RoomId::from(format!("!{n}:x")), (*n).to_owned()))
                .collect(),
        }])
    }

    #[test]
    fn password_login_sends_the_form_clears_the_password_and_blocks_a_second_submit() {
        let (mut app, mut rx) = login_app();
        fill(&mut app, "  localhost  ", "alice", "hunter2-x");
        let _ = app.update(Message::LoginSubmitted);
        match rx.try_recv() {
            Ok(Command::LoginPassword {
                username,
                password,
                server,
            }) => {
                assert_eq!(username, "alice");
                assert_eq!(password.expose(), "hunter2-x");
                assert_eq!(server.as_deref(), Some("localhost"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(app.login.password.expose(), "", "cleared on submit (Q19)");
        assert!(app.login_in_flight);
        // Typing the password again and submitting while in flight sends nothing.
        let _ = app.update(Message::PasswordChanged(Password::from("hunter2-x")));
        let _ = app.update(Message::LoginSubmitted);
        let _ = app.update(Message::OAuthRequested);
        assert!(rx.try_recv().is_err(), "a second submit sends nothing");
    }

    #[test]
    fn empty_server_is_sent_as_none_and_empty_fields_send_nothing() {
        let (mut app, mut rx) = login_app();
        fill(&mut app, "", "alice", "");
        let _ = app.update(Message::LoginSubmitted);
        assert!(rx.try_recv().is_err(), "no password: nothing sent");
        assert!(!app.login_in_flight);
        assert!(app.notice.is_some());
        fill(&mut app, "", "", "pw");
        let _ = app.update(Message::LoginSubmitted);
        assert!(rx.try_recv().is_err(), "no user name: nothing sent");
        fill(&mut app, "", "alice", "pw");
        let _ = app.update(Message::LoginSubmitted);
        assert!(matches!(
            rx.try_recv(),
            Ok(Command::LoginPassword { server: None, .. })
        ));
    }

    #[test]
    fn login_failed_returns_to_login_with_a_notice_and_a_cleared_password() {
        let (mut app, mut rx) = login_app();
        fill(&mut app, "", "alice", "hunter2-x");
        let _ = app.update(Message::LoginSubmitted);
        let _ = rx.try_recv();
        // The user typed another password meanwhile.
        let _ = app.update(Message::PasswordChanged(Password::from("hunter2-x")));
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Login(
            "wrong user name or password".into(),
        ))));
        assert_eq!(app.screen, Screen::Login);
        assert_eq!(app.login.password.expose(), "");
        assert!(!app.login_in_flight);
        let notice = app.notice.clone().expect("a notice");
        assert!(notice.contains("wrong user name or password"), "{notice}");
        assert!(!notice.contains("hunter2-x"));
        // The form works again.
        fill(&mut app, "", "alice", "pw2");
        let _ = app.update(Message::LoginSubmitted);
        assert!(matches!(rx.try_recv(), Ok(Command::LoginPassword { .. })));
    }

    #[test]
    fn logged_out_and_session_expired_show_a_notice_on_the_login_screen() {
        for event in [CoreEvent::LoggedOut, CoreEvent::SessionExpired] {
            let (mut app, _rx) = login_app();
            logged_in(&mut app);
            assert_eq!(app.screen, Screen::Loading);
            let _ = app.update(core(event));
            assert_eq!(app.screen, Screen::Login);
            assert!(app.notice.is_some());
            assert!(!app.login_in_flight);
        }
        let (mut app, _rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(CoreEvent::LoggedOut));
        let first = app.notice.clone();
        logged_in(&mut app);
        let _ = app.update(core(CoreEvent::SessionExpired));
        assert_ne!(first, app.notice, "the two notices differ");
    }

    #[test]
    fn logged_in_clears_the_notice_and_the_in_flight_flag() {
        let (mut app, _rx) = login_app();
        fill(&mut app, "", "alice", "pw");
        let _ = app.update(Message::LoginSubmitted);
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Cancelled)));
        assert!(app.notice.is_some());
        fill(&mut app, "", "alice", "pw");
        let _ = app.update(Message::LoginSubmitted);
        logged_in(&mut app);
        assert!(app.notice.is_none());
        assert!(!app.login_in_flight);
    }

    #[test]
    fn loading_becomes_main_on_loaded_or_a_non_empty_list_only() {
        let (mut app, _rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(room_event(&[])));
        assert_eq!(app.screen, Screen::Loading, "an empty list is not enough");
        let _ = app.update(core(CoreEvent::RoomListLoaded {
            total_rooms: Some(0),
        }));
        assert_eq!(app.screen, Screen::Main);

        let (mut app, _rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a"])));
        assert_eq!(app.screen, Screen::Main);
    }

    #[test]
    fn restored_session_goes_through_loading() {
        let (mut app, _rx) = app_with(8);
        assert_eq!(app.screen, Screen::Starting);
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: true,
        }));
        assert_eq!(app.screen, Screen::Loading);
    }

    #[test]
    fn browser_login_needs_a_server_and_sends_login_oauth_once() {
        let (mut app, mut rx) = login_app();
        let _ = app.update(Message::OAuthRequested);
        assert!(rx.try_recv().is_err(), "empty server: nothing sent");
        assert!(!app.login_in_flight);
        let _ = app.update(Message::ServerChanged("   ".into()));
        let _ = app.update(Message::OAuthRequested);
        assert!(rx.try_recv().is_err(), "blank server: nothing sent");
        let _ = app.update(Message::ServerChanged(" matrix.org ".into()));
        let _ = app.update(Message::OAuthRequested);
        match rx.try_recv() {
            Ok(Command::LoginOAuth { server }) => assert_eq!(server, "matrix.org"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(app.login_in_flight);
        let _ = app.update(Message::OAuthRequested);
        let _ = app.update(Message::LoginSubmitted);
        assert!(rx.try_recv().is_err(), "a second request sends nothing");
    }

    #[test]
    fn an_address_that_hides_its_server_sends_nothing_and_keeps_the_password() {
        let hidden = format!("https://matrix.org{}@evil.example", "a".repeat(300));
        let servers = [hidden.as_str(), "https://matrix.org@evil.example"]
            .into_iter()
            .chain(login::tests::LOOKALIKES);
        for server in servers {
            let (mut app, mut rx) = login_app();
            fill(&mut app, server, "alice", "hunter2-x");
            let _ = app.update(Message::LoginSubmitted);
            assert!(rx.try_recv().is_err(), "password login sent a command");
            assert!(!app.login_in_flight);
            let notice = login::server_refusal(server).expect("refused");
            assert_eq!(app.notice.as_deref(), Some(notice), "{server:?}");
            assert_eq!(app.login.password.expose(), "hunter2-x");
            app.notice = None;
            let _ = app.update(Message::OAuthRequested);
            assert!(rx.try_recv().is_err(), "browser login sent a command");
            assert!(!app.login_in_flight);
            assert_eq!(app.notice.as_deref(), Some(notice), "{server:?}");
        }
    }

    #[test]
    fn a_refused_but_honest_name_and_plain_http_keep_the_password_and_send_nothing() {
        for (server, notice) in [
            ("matrix.org..", login::UNSUPPORTED_SERVER),
            ("my_synapse", login::UNSUPPORTED_SERVER),
            ("b\u{fc}cher.de", login::UNSUPPORTED_SERVER),
            ("http://matrix.org", login::INSECURE_SERVER),
            ("http://localhost.evil.com", login::INSECURE_SERVER),
        ] {
            let (mut app, mut rx) = login_app();
            fill(&mut app, server, "alice", "hunter2-x");
            let _ = app.update(Message::LoginSubmitted);
            assert!(rx.try_recv().is_err(), "{server:?}: a command was sent");
            assert_eq!(app.notice.as_deref(), Some(notice), "{server:?}");
            assert_eq!(app.login.password.expose(), "hunter2-x", "{server:?}");
            assert!(!app.login_in_flight);
            let _ = app.update(Message::OAuthRequested);
            assert!(rx.try_recv().is_err(), "{server:?}: browser login sent");
        }
    }

    #[test]
    fn a_trailing_dot_is_dropped_from_what_is_sent() {
        let (mut app, mut rx) = login_app();
        fill(&mut app, " matrix.org. ", "alice", "hunter2-x");
        let _ = app.update(Message::LoginSubmitted);
        match rx.try_recv() {
            Ok(Command::LoginPassword { server, .. }) => {
                assert_eq!(server.as_deref(), Some("matrix.org"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    fn waiting_app(url: &str) -> (App, mpsc::Receiver<Command>, Task<Message>) {
        let (mut app, mut rx) = login_app();
        let _ = app.update(Message::ServerChanged("matrix.org".into()));
        let _ = app.update(Message::OAuthRequested);
        let _ = rx.try_recv();
        let task = app.update(core(CoreEvent::OAuthUrl(AuthUrl::from(url))));
        (app, rx, task)
    }

    #[test]
    fn an_allowed_oauth_url_opens_the_browser_through_a_task() {
        let (app, mut rx, task) = waiting_app("https://auth.example.org/a?state=secret-state-x");
        assert_eq!(app.screen, Screen::WaitingForBrowser);
        assert!(task.units() > 0, "the launcher runs in a task");
        assert!(app.browser_link.is_some());
        assert_eq!(app.browser_opened, None);
        assert!(rx.try_recv().is_err(), "nothing is cancelled");
    }

    #[test]
    fn a_refused_oauth_url_is_never_opened_and_cancels_the_login() {
        for url in [
            "http://auth.example.org/a?state=secret-state-x",
            "https://user:pw@auth.example.org/a?state=secret-state-x",
            "javascript:alert('secret-state-x')",
            "file:///etc/passwd?secret-state-x",
        ] {
            let (app, mut rx, task) = waiting_app(url);
            assert_eq!(app.screen, Screen::Login, "{url}");
            assert_eq!(task.units(), 0, "no task, so no launcher: {url}");
            assert!(app.browser_link.is_none());
            assert!(matches!(rx.try_recv(), Ok(Command::CancelOAuth)));
            let notice = app.notice.clone().expect("a notice");
            assert!(!notice.contains("secret-state-x"), "{notice}");
            assert!(!notice.contains("auth.example.org"), "{notice}");
        }
    }

    #[test]
    fn an_unsolicited_oauth_url_is_ignored() {
        let (mut app, mut rx) = app_with(8);
        let _ = rx.try_recv();
        logged_in(&mut app);
        let task = app.update(core(CoreEvent::OAuthUrl(AuthUrl::from(
            "https://auth.example.org/a",
        ))));
        assert_eq!(app.screen, Screen::Loading);
        assert_eq!(task.units(), 0);
        assert!(app.browser_link.is_none());
    }

    #[test]
    fn cancel_sends_cancel_oauth_and_the_cancelled_failure_returns_to_login() {
        let (mut app, mut rx, _task) = waiting_app("https://auth.example.org/a?state=s");
        let _ = app.update(Message::CancelOAuth);
        assert!(matches!(rx.try_recv(), Ok(Command::CancelOAuth)));
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Cancelled)));
        assert_eq!(app.screen, Screen::Login);
        assert!(app.browser_link.is_none());
        assert!(!app.login_in_flight);
    }

    #[test]
    fn cancel_does_nothing_outside_the_browser_screen() {
        let (mut app, mut rx) = login_app();
        let _ = app.update(Message::CancelOAuth);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn copy_link_appears_when_the_browser_fails_and_copies_the_checked_link() {
        let (mut app, _rx, _task) = waiting_app("https://auth.example.org/a?state=s");
        let _ = app.update(Message::BrowserOpened { ok: false });
        assert_eq!(app.browser_opened, Some(false));
        assert!(app.update(Message::CopyLink).units() > 0);
        let _ = app.update(Message::BrowserOpened { ok: true });
        assert_eq!(app.browser_opened, Some(true));
        // Without a link there is nothing to copy.
        let (mut app, _rx) = login_app();
        assert_eq!(app.update(Message::CopyLink).units(), 0);
    }

    #[test]
    fn logged_in_after_the_browser_clears_the_link() {
        let (mut app, _rx, _task) = waiting_app("https://auth.example.org/a?state=s");
        logged_in(&mut app);
        assert_eq!(app.screen, Screen::Loading);
        assert!(app.browser_link.is_none());
        assert_eq!(app.browser_opened, None);
        assert!(!app.login_in_flight);
    }

    #[test]
    fn update_never_calls_the_browser_launcher() {
        let source = include_str!("state.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("source before the tests");
        assert!(
            !production.contains("open::"),
            "state.rs must not use `open`"
        );
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        assert!(
            !code.contains("twicecat_app::open") && !code.contains("that_detached"),
            "no direct launcher call"
        );
    }

    #[test]
    fn debug_output_never_shows_the_password_or_the_composer_text() {
        let (mut app, _rx) = login_app();
        let message = Message::PasswordChanged(Password::from("hunter2-x"));
        assert!(!format!("{message:?}").contains("hunter2-x"));
        let _ = app.update(message);
        assert!(!format!("{app:?}").contains("hunter2-x"));

        let paste = Message::Composer(EditorAction(Action::Edit(Edit::Paste(Arc::new(
            "secret-body-x".to_owned(),
        )))));
        assert!(!format!("{paste:?}").contains("secret-body-x"));
        let _ = app.update(core(CoreEvent::LoggedOut));
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a"])));
        let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
        let _ = app.update(paste);
        assert_eq!(app.composer.text(), "secret-body-x");
        let dbg = format!("{app:?}");
        assert!(!dbg.contains("secret-body-x"), "{dbg}");
        assert!(dbg.contains("Main"), "the app Debug still says something");
    }

    /// An app on the main screen with room `a` selected.
    fn main_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a", "b"])));
        let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    /// Types `text` into the composer through `update`, like the editor does.
    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            let edit = if c == '\n' {
                Edit::Enter
            } else {
                Edit::Insert(c)
            };
            let _ = app.update(Message::Composer(EditorAction(Action::Edit(edit))));
        }
    }

    /// Everything queued so far.
    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn send_commands(commands: &[Command]) -> Vec<&Command> {
        commands
            .iter()
            .filter(|c| matches!(c, Command::Send(_) | Command::Edit { .. }))
            .collect()
    }

    #[test]
    fn composer_enter_sends_the_message_and_clears_the_field() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "hello");
        let _ = drain(&mut rx);
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        let commands = drain(&mut rx);
        match send_commands(&commands).as_slice() {
            [Command::Send(message)] => {
                assert_eq!(message.text.expose(), "hello");
                assert_eq!(message.room_id, RoomId::from("!a:x"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, Command::SetTyping { typing: false, .. })),
            "sending stops the typing notice"
        );
        assert_eq!(app.composer.text(), "");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert!(drain(&mut rx).is_empty(), "an empty composer sends nothing");
        type_in(&mut app, "  \t \n");
        let _ = drain(&mut rx);
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert!(
            send_commands(&drain(&mut rx)).is_empty(),
            "blank text sends nothing"
        );
    }

    #[test]
    fn the_emoji_button_types_a_colon_and_only_in_a_visible_composer() {
        let (mut app, mut rx) = main_app();
        let _ = app.update(Message::EmojiRequested);
        assert_eq!(app.composer.text(), ":");
        type_in(&mut app, "sm");
        assert!(app.composer.suggestions.is_open() || !app.composer.text().is_empty());
        assert!(
            send_commands(&drain(&mut rx)).is_empty(),
            "the button never sends"
        );
        let (mut app, _rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a"])));
        let _ = app.update(Message::EmojiRequested);
        assert_eq!(app.composer.text(), "", "no room, no input");
    }

    #[test]
    fn the_composer_is_disabled_without_a_selected_room() {
        let (mut app, mut rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a"])));
        let _ = drain(&mut rx);
        type_in(&mut app, "hello");
        assert_eq!(app.composer.text(), "", "input ignored");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn edit_mode_sends_an_edit_and_reply_mode_a_reply() {
        let (mut app, mut rx) = main_app();
        app.composer.mode = composer::Mode::Reply("item-1".into());
        type_in(&mut app, "answer");
        let _ = drain(&mut rx);
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        match send_commands(&drain(&mut rx)).as_slice() {
            [Command::Send(m)] => assert_eq!(m.reply_to.as_deref(), Some("item-1")),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(app.composer.mode, composer::Mode::Normal);

        app.composer.mode = composer::Mode::Edit("item-2".into());
        type_in(&mut app, "fixed");
        let _ = drain(&mut rx);
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        match send_commands(&drain(&mut rx)).as_slice() {
            [Command::Edit { item_id, message }] => {
                assert_eq!(item_id, "item-2");
                assert_eq!(message.text.expose(), "fixed");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(app.composer.mode, composer::Mode::Normal);
    }

    #[test]
    fn typing_notices_are_throttled_and_stop_on_clear_and_room_switch() {
        let (mut app, mut rx) = main_app();
        let t0 = Instant::now();
        let key = |c| Action::Edit(Edit::Insert(c));
        let _ = app.composer_action_at(key('a'), t0);
        let _ = app.composer_action_at(key('b'), t0 + Duration::from_secs(1));
        let _ = app.composer_action_at(key('c'), t0 + Duration::from_secs(3));
        let typing: Vec<bool> = drain(&mut rx)
            .into_iter()
            .filter_map(|c| match c {
                Command::SetTyping { typing, room_id } => {
                    assert_eq!(room_id, RoomId::from("!a:x"));
                    Some(typing)
                }
                _ => None,
            })
            .collect();
        assert_eq!(typing, [true, true]);

        // Emptying the buffer stops it.
        for _ in 0..3 {
            let _ =
                app.composer_action_at(Action::Edit(Edit::Backspace), t0 + Duration::from_secs(4));
        }
        assert!(matches!(
            drain(&mut rx).last(),
            Some(Command::SetTyping { typing: false, .. })
        ));

        // Switching room stops it in the room left.
        let _ = app.composer_action_at(key('x'), t0 + Duration::from_secs(5));
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.first(),
                Some(Command::SetTyping { room_id, typing: false }) if *room_id == RoomId::from("!a:x")
            ),
            "{commands:?}"
        );
        assert!(matches!(commands.last(), Some(Command::SelectRoom(_))));
        // Nothing outstanding: another switch sends no stop.
        let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::SetTyping { .. }))
        );
    }

    #[test]
    fn mention_queries_are_rate_limited_and_the_answer_fills_the_list() {
        let (mut app, mut rx) = main_app();
        let t0 = Instant::now();
        let key = |c| Action::Edit(Edit::Insert(c));
        let ms = |n| t0 + Duration::from_millis(n);
        let _ = app.composer_action_at(key('@'), ms(0));
        let _ = app.composer_action_at(key('a'), ms(0));
        let _ = app.composer_action_at(key('l'), ms(40));
        let _ = app.composer_action_at(key('i'), ms(80));
        let queries = |commands: Vec<Command>| -> Vec<String> {
            commands
                .into_iter()
                .filter_map(|c| match c {
                    Command::SearchMembers { query, .. } => Some(query),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            queries(drain(&mut rx)),
            ["a"],
            "the burst waits for the timer"
        );
        let _ = app.members_due_at(ms(150));
        assert_eq!(queries(drain(&mut rx)), ["ali"]);

        let answer = |query: &str| {
            core(CoreEvent::Members {
                room_id: RoomId::from("!a:x"),
                query: twicecat_core::api::MessageText::from(query),
                members: vec![twicecat_core::api::Member {
                    user_id: UserId::from("@alice:x"),
                    display_name: Some("Alice".into()),
                }],
            })
        };
        let _ = app.update(answer("al"));
        assert!(
            !app.composer.suggestions.is_open(),
            "a stale answer is ignored"
        );
        let _ = app.update(answer("ali"));
        assert!(app.composer.suggestions.is_open());
        let _ = app.update(Message::SuggestionPicked(0));
        assert_eq!(
            app.composer.text(),
            "@Alice: ",
            "starts the message: Element's colon"
        );
        assert_eq!(app.composer.mentions.len(), 1);
    }

    #[test]
    fn a_members_answer_for_another_room_is_ignored() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "@a");
        let _ = app.update(core(CoreEvent::Members {
            room_id: RoomId::from("!b:x"),
            query: twicecat_core::api::MessageText::from("a"),
            members: vec![twicecat_core::api::Member {
                user_id: UserId::from("@alice:x"),
                display_name: None,
            }],
        }));
        assert!(!app.composer.suggestions.is_open());
    }

    #[test]
    fn suggestion_keys_pick_move_and_escape() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, ":smi");
        assert!(app.composer.suggestions.is_open());
        let _ = app.update(Message::ComposerKey(ComposerKey::Next));
        assert_eq!(app.composer.suggestions.selected(), 1);
        let _ = app.update(Message::ComposerKey(ComposerKey::Previous));
        assert_eq!(app.composer.suggestions.selected(), 0);
        let _ = app.update(Message::ComposerKey(ComposerKey::Escape));
        assert!(!app.composer.suggestions.is_open());
        type_in(&mut app, "l");
        let _ = app.update(Message::ComposerKey(ComposerKey::Pick));
        assert!(!app.composer.suggestions.is_open());
        assert!(
            app.composer
                .text()
                .chars()
                .last()
                .is_some_and(|c| !c.is_ascii())
        );
    }

    #[test]
    fn a_failed_send_or_edit_shows_a_line_for_the_open_room() {
        let (mut app, _rx) = main_app();
        let failed = |room: &str, action| {
            core(CoreEvent::ActionFailed {
                room_id: RoomId::from(room),
                action,
                error: CoreError::Network("offline".into()),
            })
        };
        let _ = app.update(failed("!b:x", ActionKind::Send));
        assert!(app.send_error.is_none());
        let _ = app.update(failed("!a:x", ActionKind::Redact));
        assert!(app.send_error.is_none());
        let _ = app.update(failed("!a:x", ActionKind::Edit));
        assert!(app.send_error.is_some());
    }

    #[test]
    fn send_failed_shows_a_non_secret_error_for_the_selected_room() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "secret-body-x");
        let _ = app.update(core(CoreEvent::SendFailed {
            room_id: RoomId::from("!b:x"),
            error: CoreError::Network("offline".into()),
        }));
        assert!(
            app.send_error.is_none(),
            "another room's failure is ignored"
        );
        let _ = app.update(core(CoreEvent::SendFailed {
            room_id: RoomId::from("!a:x"),
            error: CoreError::Network("offline".into()),
        }));
        let line = app.send_error.clone().expect("an error line");
        assert!(line.contains("offline") && !line.contains("secret-body-x"));
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert!(app.send_error.is_none(), "selecting a room clears it");
    }

    #[test]
    fn changing_room_keeps_no_stale_composer_text() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "draft");
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert_eq!(app.composer.text(), "");
    }

    #[test]
    fn log_out_sends_logout() {
        let (mut app, mut rx) = main_app();
        let _ = app.update(Message::LogoutRequested);
        assert!(matches!(rx.try_recv(), Ok(Command::Logout)));
    }

    #[test]
    fn logged_out_clears_the_composer_and_errors() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "draft");
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert_eq!(app.composer.text(), "");
        assert!(app.send_error.is_none());
    }

    #[test]
    fn a_failed_restore_leaves_starting_for_the_login_screen_with_a_notice() {
        let (mut app, _rx) = app_with(8);
        assert_eq!(app.screen, Screen::Starting);
        let _ = app.update(core(CoreEvent::Error(CoreError::Storage(
            "secret store unavailable".into(),
        ))));
        assert_eq!(app.screen, Screen::Login);
        assert!(!app.login_in_flight);
        let notice = app.notice.clone().expect("a notice");
        assert!(
            notice.starts_with("Could not restore your session"),
            "{notice}"
        );
        assert!(notice.contains("secret store unavailable"), "{notice}");
    }

    #[test]
    fn an_error_during_a_login_returns_to_the_form_and_ends_the_attempt() {
        let (mut app, mut rx) = login_app();
        fill(&mut app, "", "alice", "hunter2-x");
        let _ = app.update(Message::LoginSubmitted);
        let _ = rx.try_recv();
        assert!(app.login_in_flight);
        let _ = app.update(core(CoreEvent::Error(CoreError::Internal(
            "could not set up syncing".into(),
        ))));
        assert_eq!(app.screen, Screen::Login);
        assert!(!app.login_in_flight);
        assert_eq!(app.login.password.expose(), "");
        let notice = app.notice.clone().expect("a notice");
        assert!(notice.contains("could not set up syncing"), "{notice}");
        assert!(!notice.contains("hunter2-x"));
        // The form works again.
        fill(&mut app, "", "alice", "pw2");
        let _ = app.update(Message::LoginSubmitted);
        assert!(matches!(rx.try_recv(), Ok(Command::LoginPassword { .. })));
    }

    #[test]
    fn an_error_while_waiting_for_the_browser_returns_to_the_form() {
        let (mut app, mut rx, _task) =
            waiting_app("https://auth.example.org/a?state=secret-state-x");
        let _ = app.update(core(CoreEvent::Error(CoreError::Network("down".into()))));
        assert_eq!(app.screen, Screen::Login);
        assert!(!app.login_in_flight);
        assert!(app.browser_link.is_none());
        let notice = app.notice.clone().expect("a notice");
        assert!(!notice.contains("secret-state-x"), "{notice}");
        assert!(matches!(rx.try_recv(), Ok(Command::CancelOAuth)));
    }

    #[test]
    fn the_cancel_reply_keeps_the_notice_of_a_refused_url() {
        let (mut app, mut rx, _task) = waiting_app("http://auth.example.org/a");
        assert!(matches!(rx.try_recv(), Ok(Command::CancelOAuth)));
        let before = app.notice.clone().expect("a notice");
        assert!(before.starts_with("Sign-in stopped"), "{before}");
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Cancelled)));
        assert_eq!(app.notice.as_deref(), Some(before.as_str()));
        assert_eq!(app.screen, Screen::Login);
    }

    #[test]
    fn the_cancel_reply_keeps_the_notice_of_an_error_while_waiting() {
        let (mut app, mut rx, _task) = waiting_app("https://auth.example.org/a");
        let _ = app.update(core(CoreEvent::Error(CoreError::Network("down".into()))));
        assert!(matches!(rx.try_recv(), Ok(Command::CancelOAuth)));
        let before = app.notice.clone().expect("a notice");
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Cancelled)));
        assert_eq!(app.notice.as_deref(), Some(before.as_str()));
    }

    #[test]
    fn a_late_cancel_reply_does_not_end_a_new_attempt() {
        let (mut app, mut rx, _task) = waiting_app("http://auth.example.org/a");
        assert!(matches!(rx.try_recv(), Ok(Command::CancelOAuth)));
        // The user starts a new attempt before the core answers the cancel.
        let _ = app.update(Message::OAuthRequested);
        assert!(matches!(rx.try_recv(), Ok(Command::LoginOAuth { .. })));
        assert!(app.login_in_flight);
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Cancelled)));
        assert!(app.login_in_flight, "the stale reply is consumed");
        assert_eq!(app.screen, Screen::Login);
        // A second Cancelled is a real one and ends the attempt.
        let _ = app.update(Message::CancelOAuth);
        let _ = app.update(core(CoreEvent::OAuthUrl(AuthUrl::from(
            "https://auth.example.org/b",
        ))));
        let _ = app.update(Message::CancelOAuth);
        let _ = app.update(core(CoreEvent::LoginFailed(CoreError::Cancelled)));
        assert!(!app.login_in_flight);
        assert_eq!(app.screen, Screen::Login);
    }

    #[test]
    fn error_and_send_error_text_is_one_capped_line() {
        let (mut app, _rx) = login_app();
        fill(&mut app, "", "alice", "pw");
        let _ = app.update(Message::LoginSubmitted);
        let long = format!("line1\nline2\u{202E}{}", "x".repeat(2000));
        let _ = app.update(core(CoreEvent::Error(CoreError::Internal(long.clone()))));
        let notice = app.notice.clone().expect("a notice");
        assert!(!notice.contains('\n') && !notice.contains('\u{202E}'));
        assert!(notice.chars().count() <= 512, "{}", notice.chars().count());

        let (mut app, _rx) = main_app();
        let room = app.selected.clone().expect("a selected room");
        let _ = app.update(core(CoreEvent::SendFailed {
            room_id: room,
            error: CoreError::Internal(long),
        }));
        let line = app.send_error.clone().expect("an error line");
        assert!(!line.contains('\n') && !line.contains('\u{202E}'));
        assert!(line.chars().count() <= 512);
    }

    #[test]
    fn an_error_on_other_screens_changes_nothing() {
        // Idle login form, no login in flight.
        let (mut app, _rx) = login_app();
        let _ = app.update(core(CoreEvent::Error(CoreError::NotLoggedIn)));
        assert_eq!(app.screen, Screen::Login);
        assert!(app.notice.is_none());
        // Loading and main.
        let (mut app, _rx) = main_app();
        let _ = app.update(core(CoreEvent::Error(CoreError::Internal("x".into()))));
        assert_eq!(app.screen, Screen::Main);
        assert!(app.notice.is_none());
        assert!(app.selected.is_some());
        let (mut app, _rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(CoreEvent::Error(CoreError::Internal("x".into()))));
        assert_eq!(app.screen, Screen::Loading);
        // Avatars stay ignored.
        let _ = app.update(core(CoreEvent::Avatar {
            id: AvatarId::from("mxc://x/a"),
            image: Some(AvatarImage::from(vec![1])),
        }));
        assert_eq!(app.screen, Screen::Loading);
    }

    #[test]
    fn search_keeps_the_open_room_and_its_draft_when_the_filtered_list_lacks_it() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "unsent");
        let _ = app.update(Message::SearchChanged("zzz".into()));
        let _ = app.update(core(room_event(&["z"])));
        assert_eq!(app.selected, Some(RoomId::from("!a:x")));
        assert_eq!(app.composer.text(), "unsent");
        // Emptied list, too.
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Clear])));
        assert_eq!(app.selected, Some(RoomId::from("!a:x")));
        assert_eq!(app.composer.text(), "unsent");
    }

    #[test]
    fn switching_tabs_keeps_the_open_room_and_its_draft() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "unsent");
        let _ = app.update(Message::TabSelected(RoomListTab::People));
        let _ = app.update(core(room_event(&["z"])));
        assert_eq!(app.selected, Some(RoomId::from("!a:x")));
        assert_eq!(app.composer.text(), "unsent");
    }

    #[test]
    fn the_open_room_is_deselected_when_the_default_query_loses_it() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "unsent");
        assert_eq!(app.query, RoomListQuery::default());
        let _ = app.update(core(room_event(&["b"])));
        assert_eq!(app.selected, None);
        assert_eq!(app.composer.text(), "");

        // Also after returning to the default query.
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::SearchChanged("zzz".into()));
        let _ = app.update(core(room_event(&["z"])));
        assert!(app.selected.is_some());
        let _ = app.update(Message::SearchChanged(String::new()));
        let _ = app.update(core(room_event(&["b"])));
        assert_eq!(app.selected, None);
    }

    #[test]
    fn the_open_room_survives_a_filtered_list_that_still_has_it_and_logout_clears_it() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::SearchChanged("zzz".into()));
        let _ = app.update(core(room_event(&["z"])));
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert_eq!(app.selected, None);
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::SearchChanged("zzz".into()));
        let _ = app.update(core(CoreEvent::SessionExpired));
        assert_eq!(app.selected, None);
    }

    #[test]
    fn a_pending_cancel_is_forgotten_when_the_session_state_changes() {
        for event in [
            CoreEvent::NoSession,
            CoreEvent::LoggedOut,
            CoreEvent::SessionExpired,
        ] {
            let (mut app, _rx) = login_app();
            app.pending_cancel = true;
            let _ = app.update(core(event.clone()));
            assert!(!app.pending_cancel, "{event:?}");
        }
    }

    fn clicked(url: &str) -> Message {
        Message::LinkClicked(LinkTarget::Url(url.to_owned()))
    }

    #[test]
    fn every_url_click_asks_first_and_opens_nothing_until_confirmed() {
        // Labels do not matter to the app: plain-word, matching,
        // mismatching, spoiler-wrapped and split-anchor links all reach
        // `update` as the same click (see the `rich_text` tests).
        for url in [
            "https://example.org/a",
            "http://example.org/",
            "https://Evil.example:8443/path?token=1",
            "mailto:alice@example.org",
        ] {
            let (mut app, mut rx) = main_app();
            let task = app.update(clicked(url));
            assert_eq!(task.units(), 0, "nothing opens before the answer: {url}");
            let Some(LinkBar::Confirm(checked)) = &app.link_bar else {
                panic!("expected a confirmation, got {:?}", app.link_bar);
            };
            assert!(!checked.host().is_empty());
            assert!(rx.try_recv().is_err());
            assert!(app.update(Message::LinkConfirmed).units() > 0, "{url}");
            assert!(app.link_bar.is_none());
        }
    }

    #[test]
    fn the_confirmation_names_the_host_or_mail_domain() {
        let (mut app, _rx) = main_app();
        let _ = app.update(clicked("https://Evil.example:8443/path?token=1"));
        let Some(LinkBar::Confirm(url)) = &app.link_bar else {
            panic!("expected a confirmation");
        };
        assert_eq!(url.host(), "evil.example");
        let _ = app.update(clicked("mailto:alice@Mail.example"));
        let Some(LinkBar::Confirm(url)) = &app.link_bar else {
            panic!("expected a confirmation");
        };
        assert_eq!(url.host(), "mail.example");
    }

    #[test]
    fn dismissing_the_bar_opens_nothing() {
        let (mut app, _rx) = main_app();
        let _ = app.update(clicked("https://evil.example/"));
        assert_eq!(app.update(Message::LinkDismissed).units(), 0);
        assert!(app.link_bar.is_none());
        // Confirming with no bar does nothing either.
        assert_eq!(app.update(Message::LinkConfirmed).units(), 0);
    }

    #[test]
    fn unsafe_links_are_refused_without_opening() {
        let (mut app, _rx) = main_app();
        for url in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,x",
            "https://user@evil.example/",
            "https://exa mple.org/",
            "https://exämple.org/",
        ] {
            let task = app.update(clicked(url));
            assert_eq!(task.units(), 0, "{url}");
            assert_eq!(app.link_bar, Some(LinkBar::Refused), "{url}");
            assert_eq!(app.update(Message::LinkConfirmed).units(), 0, "{url}");
            let _ = app.update(Message::LinkDismissed);
        }
    }

    #[test]
    fn a_failed_launch_is_reported() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::LinkOpened { ok: true });
        assert!(app.link_bar.is_none());
        let _ = app.update(Message::LinkOpened { ok: false });
        assert_eq!(app.link_bar, Some(LinkBar::Failed));
    }

    #[test]
    fn a_room_pill_selects_a_listed_room_only() {
        let (mut app, mut rx) = main_app();
        let task = app.update(Message::LinkClicked(LinkTarget::Room("!b:x".into())));
        assert_eq!(task.units(), 0);
        assert_eq!(app.selected, Some(RoomId::from("!b:x")));
        assert!(matches!(rx.try_recv(), Ok(Command::SelectRoom(id)) if id == RoomId::from("!b:x")));
        let _ = app.update(Message::LinkClicked(LinkTarget::Room("!nope:x".into())));
        let _ = app.update(Message::LinkClicked(LinkTarget::Room("#alias:x".into())));
        assert_eq!(app.selected, Some(RoomId::from("!b:x")));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_user_pill_does_nothing() {
        let (mut app, mut rx) = main_app();
        let task = app.update(Message::LinkClicked(LinkTarget::User(UserId::from("@a:x"))));
        assert_eq!(task.units(), 0);
        assert!(rx.try_recv().is_err());
        assert!(app.link_bar.is_none() && app.revealed.is_empty());
    }

    #[test]
    fn a_spoiler_click_toggles_it_per_item_and_index() {
        let (mut app, _rx) = main_app();
        let spoiler = |item: &str, index| {
            Message::LinkClicked(LinkTarget::Spoiler {
                item: item.to_owned(),
                index,
            })
        };
        let _ = app.update(spoiler("i1", 0));
        let _ = app.update(spoiler("i1", 1));
        let _ = app.update(spoiler("i2", 0));
        assert_eq!(app.revealed.len(), 3);
        let _ = app.update(spoiler("i1", 0));
        assert!(!app.revealed.contains(&("i1".to_owned(), 0)));
        assert!(app.revealed.contains(&("i1".to_owned(), 1)));
        assert!(app.revealed.contains(&("i2".to_owned(), 0)));
    }

    #[test]
    fn revealed_spoilers_are_capped() {
        let (mut app, _rx) = main_app();
        for index in 0..MAX_REVEALED + 50 {
            let _ = app.update(Message::LinkClicked(LinkTarget::Spoiler {
                item: "i".into(),
                index,
            }));
        }
        assert_eq!(app.revealed.len(), MAX_REVEALED);
    }

    #[test]
    fn changing_room_resets_spoilers_and_the_link_bar() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::LinkClicked(LinkTarget::Spoiler {
            item: "i".into(),
            index: 0,
        }));
        let _ = app.update(clicked("https://evil.example/"));
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert!(app.revealed.is_empty());
        assert!(app.link_bar.is_none());
    }

    #[test]
    fn links_never_reach_debug_output() {
        let (mut app, _rx) = main_app();
        let message = clicked("https://sentinel-host.example/sentinel-path");
        assert!(!format!("{message:?}").contains("sentinel"));
        let _ = app.update(message);
        let dbg = format!("{app:?} {:?}", app.link_bar);
        assert!(!dbg.contains("sentinel"), "{dbg}");
    }

    #[test]
    fn update_opens_links_only_through_the_links_module() {
        let source = include_str!("state.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production code");
        let code: String = production
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        assert!(!code.contains("open::"), "no direct launcher call");
        assert!(code.contains("links::open_link"), "the launcher is reached");
        assert!(code.contains("Task::perform"), "inside a task");
    }

    // ---- Task 12: message actions ----

    use crate::actions::NOTICE_DURATION;
    use twicecat_core::api::{
        ItemActions, MessageDetails, MessageText, ReactionGroup, SendStatus, TimelineItemKind,
    };

    const ALL_ACTIONS: ItemActions = ItemActions {
        can_reply: true,
        can_edit: true,
        can_redact: true,
        can_react: true,
        can_copy: true,
    };

    fn message_item(id: &str, body: &str, is_own: bool, actions: ItemActions) -> TimelineItem {
        TimelineItem::new(
            id.to_owned(),
            TimelineItemKind::Message {
                sender: UserId::from(if is_own { "@me:x" } else { "@them:x" }),
                body: body.to_owned(),
                is_own,
                timestamp_ms: 1,
                sent: SendStatus::Sent,
                details: Box::new(MessageDetails {
                    actions,
                    reactions: vec![ReactionGroup {
                        key: "\u{1F44D}".into(),
                        count: 1,
                        own: false,
                        senders: Vec::new(),
                    }],
                    ..MessageDetails::default()
                }),
            },
        )
    }

    /// `main_app` with an own message `mine`, someone else's `theirs`, and a
    /// message `locked` that allows nothing.
    fn actions_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: RoomId::from("!a:x"),
            diffs: vec![ListDiff::Append {
                values: vec![
                    message_item("mine", "my words\nsecond line", true, ALL_ACTIONS),
                    message_item("theirs", "their words", false, ALL_ACTIONS),
                    message_item("locked", "locked words", false, ItemActions::default()),
                ],
            }],
        }));
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    fn key(glyph: &str) -> MessageText {
        MessageText::from(glyph)
    }

    #[test]
    fn hover_tracks_one_message_and_a_stale_exit_changes_nothing() {
        let (mut app, _rx) = actions_app();
        let _ = app.update(Message::HoverEnter("mine".into()));
        assert_eq!(app.actions.hovered.as_deref(), Some("mine"));
        let _ = app.update(Message::HoverEnter("theirs".into()));
        let _ = app.update(Message::HoverExit("mine".into()));
        assert_eq!(app.actions.hovered.as_deref(), Some("theirs"));
        let _ = app.update(Message::HoverExit("theirs".into()));
        assert_eq!(app.actions.hovered, None);
    }

    #[test]
    fn reply_sets_the_mode_only_where_the_core_allows_it() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::ReplyRequested("locked".into()));
        assert_eq!(app.composer.mode, composer::Mode::Normal);
        let _ = app.update(Message::ReplyRequested("gone".into()));
        assert_eq!(app.composer.mode, composer::Mode::Normal);
        let _ = app.update(Message::ReplyRequested("theirs".into()));
        assert_eq!(app.composer.mode, composer::Mode::Reply("theirs".into()));
        assert!(drain(&mut rx).is_empty(), "nothing is sent yet");

        // The reply bar's cross leaves reply mode and keeps the draft.
        type_in(&mut app, "draft");
        let _ = app.update(Message::ModeCancelled);
        assert_eq!(app.composer.mode, composer::Mode::Normal);
        assert_eq!(app.composer.text(), "draft");

        // Escape leaves it as well.
        let _ = app.update(Message::ReplyRequested("theirs".into()));
        let _ = app.update(Message::ComposerKey(ComposerKey::Escape));
        assert_eq!(app.composer.mode, composer::Mode::Normal);
    }

    #[test]
    fn edit_loads_the_body_of_an_own_message_and_escape_restores_the_draft() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::EditRequested("theirs".into()));
        assert_eq!(app.composer.mode, composer::Mode::Normal, "not own");
        let _ = app.update(Message::EditRequested("locked".into()));
        assert_eq!(app.composer.mode, composer::Mode::Normal);

        type_in(&mut app, "my draft");
        let _ = drain(&mut rx);
        let _ = app.update(Message::EditRequested("mine".into()));
        assert_eq!(app.composer.mode, composer::Mode::Edit("mine".into()));
        assert_eq!(app.composer.text(), "my words\nsecond line");

        let _ = app.update(Message::ComposerKey(ComposerKey::Escape));
        assert_eq!(app.composer.mode, composer::Mode::Normal);
        assert_eq!(app.composer.text(), "my draft");
    }

    #[test]
    fn leaving_an_edit_to_an_empty_composer_stops_the_typing_notice() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::EditRequested("mine".into()));
        type_in(&mut app, "!");
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::SetTyping { typing: true, .. }))
        );
        let _ = app.update(Message::ComposerKey(ComposerKey::Escape));
        assert_eq!(app.composer.text(), "");
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::SetTyping { typing: false, .. })),
            "the notice must not linger"
        );

        let _ = app.update(Message::EditRequested("mine".into()));
        type_in(&mut app, "!");
        let _ = drain(&mut rx);
        let _ = app.update(Message::ModeCancelled);
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::SetTyping { typing: false, .. }))
        );
    }

    #[test]
    fn sending_an_edit_gives_command_edit_and_brings_the_draft_back() {
        let (mut app, mut rx) = actions_app();
        type_in(&mut app, "draft");
        let _ = app.update(Message::EditRequested("mine".into()));
        let _ = drain(&mut rx);
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        match send_commands(&drain(&mut rx)).as_slice() {
            [Command::Edit { item_id, message }] => {
                assert_eq!(item_id, "mine");
                assert_eq!(message.text.expose(), "my words\nsecond line");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(app.composer.mode, composer::Mode::Normal);
        assert_eq!(app.composer.text(), "draft");
    }

    #[test]
    fn reply_while_editing_leaves_the_edit_first() {
        let (mut app, _rx) = actions_app();
        type_in(&mut app, "draft");
        let _ = app.update(Message::EditRequested("mine".into()));
        let _ = app.update(Message::ReplyRequested("theirs".into()));
        assert_eq!(app.composer.mode, composer::Mode::Reply("theirs".into()));
        assert_eq!(app.composer.text(), "draft");
    }

    #[test]
    fn delete_asks_first_and_then_redacts_without_a_reason() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::DeleteRequested("theirs".into()));
        assert_eq!(app.actions.confirm_delete.as_deref(), Some("theirs"));
        assert!(drain(&mut rx).is_empty(), "no command before confirming");

        let _ = app.update(Message::DeleteCancelled);
        assert_eq!(app.actions.confirm_delete, None);
        let _ = app.update(Message::DeleteConfirmed);
        assert!(drain(&mut rx).is_empty(), "nothing to confirm");

        let _ = app.update(Message::DeleteRequested("theirs".into()));
        let _ = app.update(Message::DeleteConfirmed);
        match drain(&mut rx).as_slice() {
            [
                Command::Redact {
                    room_id,
                    item_id,
                    reason: None,
                },
            ] => {
                assert_eq!(*room_id, RoomId::from("!a:x"));
                assert_eq!(item_id, "theirs");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(app.actions.confirm_delete, None);
    }

    #[test]
    fn delete_is_refused_where_the_core_does_not_allow_it() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::DeleteRequested("locked".into()));
        assert_eq!(app.actions.confirm_delete, None);
        let _ = app.update(Message::DeleteRequested("gone".into()));
        assert_eq!(app.actions.confirm_delete, None);

        // The message vanished between the question and the answer.
        let _ = app.update(Message::DeleteRequested("theirs".into()));
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: RoomId::from("!a:x"),
            diffs: vec![ListDiff::Clear],
        }));
        let _ = app.update(Message::DeleteConfirmed);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn copy_uses_the_plain_body_and_only_where_allowed() {
        let (mut app, mut rx) = actions_app();
        assert_eq!(app.copy_source("theirs"), Some("their words"));
        assert_eq!(app.copy_source("locked"), None);
        assert_eq!(app.copy_source("gone"), None);
        let _ = app.update(Message::CopyRequested("theirs".into()));
        assert!(drain(&mut rx).is_empty(), "copying sends no command");
    }

    #[test]
    fn a_pick_in_the_picker_toggles_the_reaction_and_closes_it() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::PickerOpened("locked".into()));
        assert!(app.actions.picker.is_none(), "react not allowed");
        let _ = app.update(Message::PickerOpened("theirs".into()));
        let picker = app.actions.picker.clone().expect("open");
        assert_eq!(picker.target, "theirs");
        let _ = app.update(Message::PickerQuery(key("smile")));
        assert_eq!(
            app.actions.picker.as_ref().map(|p| p.query.as_str()),
            Some("smile")
        );

        let _ = app.update(Message::PickerPicked(key("\u{1F604}")));
        match drain(&mut rx).as_slice() {
            [
                Command::ToggleReaction {
                    room_id,
                    item_id,
                    key,
                },
            ] => {
                assert_eq!(*room_id, RoomId::from("!a:x"));
                assert_eq!(item_id, "theirs");
                assert_eq!(key, "\u{1F604}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(app.actions.picker.is_none());
    }

    #[test]
    fn enter_in_the_picker_picks_the_first_result() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::PickerOpened("theirs".into()));
        let _ = app.update(Message::PickerSubmitted);
        match drain(&mut rx).as_slice() {
            [Command::ToggleReaction { key, .. }] => assert_eq!(key, "\u{1F44D}"),
            other => panic!("unexpected {other:?}"),
        }
        let _ = app.update(Message::PickerOpened("theirs".into()));
        let _ = app.update(Message::PickerQuery(key("zzzzqqqq")));
        let _ = app.update(Message::PickerSubmitted);
        assert!(drain(&mut rx).is_empty(), "no result, nothing sent");
        let _ = app.update(Message::PickerClosed);
        assert!(app.actions.picker.is_none());
    }

    #[test]
    fn a_reaction_chip_toggles_and_a_pick_for_a_vanished_message_sends_nothing() {
        let (mut app, mut rx) = actions_app();
        let _ = app.update(Message::ReactionToggled {
            item: "theirs".into(),
            key: key("\u{1F44D}"),
        });
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::ToggleReaction { .. }]
        ));
        let _ = app.update(Message::ReactionToggled {
            item: "locked".into(),
            key: key("\u{1F44D}"),
        });
        assert!(drain(&mut rx).is_empty());

        let _ = app.update(Message::PickerOpened("theirs".into()));
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: RoomId::from("!a:x"),
            diffs: vec![ListDiff::Clear],
        }));
        let _ = app.update(Message::PickerPicked(key("\u{1F604}")));
        assert!(drain(&mut rx).is_empty());
        assert!(app.actions.picker.is_none());
    }

    #[test]
    fn a_failed_delete_or_reaction_shows_a_notice_that_expires() {
        let (mut app, _rx) = actions_app();
        let failed = |room: &str, action| {
            core(CoreEvent::ActionFailed {
                room_id: RoomId::from(room),
                action,
                error: CoreError::Network("offline".into()),
            })
        };
        let _ = app.update(failed("!b:x", ActionKind::Redact));
        assert!(app.actions.notice.is_none(), "another room");
        let _ = app.update(failed("!a:x", ActionKind::Typing));
        assert!(app.actions.notice.is_none(), "not reported");
        let _ = app.update(failed("!a:x", ActionKind::Redact));
        let first = app.actions.notice.clone().expect("a notice");
        assert!(first.text.contains("offline"));
        assert!(NOTICE_DURATION.as_secs() == 5);

        let _ = app.update(failed("!a:x", ActionKind::React));
        let second = app.actions.notice.clone().expect("a newer notice");
        assert_ne!(first.seq, second.seq);
        let _ = app.update(Message::NoticeExpired(first.seq));
        assert!(app.actions.notice.is_some(), "an old timer is stale");
        let _ = app.update(Message::NoticeExpired(second.seq));
        assert!(app.actions.notice.is_none());
    }

    #[test]
    fn changing_room_resets_the_action_state() {
        let (mut app, _rx) = actions_app();
        let _ = app.update(Message::HoverEnter("mine".into()));
        let _ = app.update(Message::DeleteRequested("mine".into()));
        let _ = app.update(Message::PickerOpened("mine".into()));
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert!(app.actions.hovered.is_none());
        assert!(app.actions.confirm_delete.is_none());
        assert!(app.actions.picker.is_none());
    }

    #[test]
    fn action_messages_never_print_reactions_or_queries() {
        let m = Message::ReactionToggled {
            item: "i".into(),
            key: key("secret-key-x"),
        };
        assert!(!format!("{m:?}").contains("secret-key-x"));
        let m = Message::PickerQuery(key("secret-query-x"));
        assert!(!format!("{m:?}").contains("secret-query-x"));
        let (mut app, _rx) = actions_app();
        let _ = app.update(Message::PickerOpened("theirs".into()));
        let _ = app.update(Message::PickerQuery(key("secret-query-x")));
        assert!(!format!("{app:?}").contains("secret-query-x"));
    }

    // ---- per-room drafts ----------------------------------------------------

    fn saved_drafts(commands: &[Command]) -> Vec<(String, Option<twicecat_core::api::Draft>)> {
        commands
            .iter()
            .filter_map(|c| match c {
                Command::SaveDraft { room_id, draft } => {
                    Some((room_id.as_str().to_owned(), draft.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn draft_text(draft: &Option<twicecat_core::api::Draft>) -> Option<String> {
        draft.as_ref().map(|d| d.text.expose().to_owned())
    }

    #[test]
    fn leaving_a_room_saves_its_draft_before_opening_the_next() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "half a thought");
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let commands = drain(&mut rx);
        let saved = saved_drafts(&commands);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].0, "!a:x");
        assert_eq!(draft_text(&saved[0].1).as_deref(), Some("half a thought"));
        let save_at = commands
            .iter()
            .position(|c| matches!(c, Command::SaveDraft { .. }))
            .unwrap();
        let select_at = commands
            .iter()
            .position(|c| matches!(c, Command::SelectRoom(_)))
            .unwrap();
        assert!(
            save_at < select_at,
            "saved before the core opens the next room"
        );
        assert_eq!(app.composer.text(), "");
    }

    #[test]
    fn leaving_a_room_after_emptying_the_composer_clears_its_draft() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "x");
        let _ = app.update(Message::Composer(EditorAction(Action::Edit(
            Edit::Backspace,
        ))));
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert_eq!(
            saved_drafts(&drain(&mut rx)),
            vec![("!a:x".to_owned(), None)]
        );
    }

    #[test]
    fn an_untouched_composer_leaves_the_stored_draft_alone() {
        // The room's draft may not have arrived yet: saving the empty
        // composer would wipe it.
        let (mut app, mut rx) = main_app();
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let _ = app.update(Message::CloseRequested);
        assert!(saved_drafts(&drain(&mut rx)).is_empty());
    }

    #[test]
    fn a_save_timer_from_the_previous_room_is_ignored() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "abc");
        let seq = app.draft_seq;
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let _ = drain(&mut rx);
        let _ = app.update(Message::DraftDue(seq));
        assert!(saved_drafts(&drain(&mut rx)).is_empty());
    }

    #[test]
    fn a_loaded_draft_fills_the_empty_composer_with_its_reply() {
        let (mut app, _rx) = actions_app();
        if let Some(item) = app.timeline.items.iter_mut().find(|i| i.id == "theirs") {
            item.event_id = Some(EventId::from("$theirs"));
        }
        let draft = twicecat_core::api::Draft {
            text: MessageText::from("@Bob: back"),
            mentions: vec![twicecat_core::api::MentionRef {
                label: "@Bob".into(),
                user_id: UserId::from("@bob:x"),
            }],
            reply_to: Some(EventId::from("$theirs")),
        };
        let _ = app.update(core(CoreEvent::DraftLoaded {
            room_id: RoomId::from("!b:x"),
            draft: draft.clone(),
        }));
        assert_eq!(app.composer.text(), "", "another room's draft is ignored");
        let _ = app.update(core(CoreEvent::DraftLoaded {
            room_id: RoomId::from("!a:x"),
            draft,
        }));
        assert_eq!(app.composer.text(), "@Bob: back");
        assert_eq!(app.composer.mentions.len(), 1);
        assert_eq!(app.composer.mode, composer::Mode::Reply("theirs".into()));
    }

    #[test]
    fn a_loaded_draft_never_replaces_typed_text() {
        let (mut app, _rx) = main_app();
        type_in(&mut app, "typed already");
        let _ = app.update(core(CoreEvent::DraftLoaded {
            room_id: RoomId::from("!a:x"),
            draft: twicecat_core::api::Draft {
                text: MessageText::from("old draft"),
                mentions: Vec::new(),
                reply_to: None,
            },
        }));
        assert_eq!(app.composer.text(), "typed already");
    }

    #[test]
    fn sending_clears_the_stored_draft() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "hello");
        let _ = drain(&mut rx);
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        let commands = drain(&mut rx);
        assert_eq!(send_commands(&commands).len(), 1);
        assert_eq!(saved_drafts(&commands), vec![("!a:x".to_owned(), None)]);
    }

    #[test]
    fn during_an_edit_the_set_aside_draft_is_what_is_kept() {
        let (mut app, mut rx) = actions_app();
        type_in(&mut app, "my draft");
        let _ = app.update(Message::EditRequested("mine".into()));
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let saved = saved_drafts(&drain(&mut rx));
        assert_eq!(draft_text(&saved[0].1).as_deref(), Some("my draft"));
    }

    #[test]
    fn closing_the_window_saves_the_draft_before_shutting_down() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "unsent");
        let _ = drain(&mut rx);
        let _ = app.update(Message::CloseRequested);
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [
                    Command::SaveDraft { draft: Some(_), .. },
                    ..,
                    Command::Shutdown
                ]
            ),
            "{commands:?}"
        );
    }

    #[test]
    fn typing_saves_the_draft_after_a_pause() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "abc");
        let _ = drain(&mut rx);
        let seq = app.draft_seq;
        let _ = app.update(Message::DraftDue(seq - 1));
        assert!(saved_drafts(&drain(&mut rx)).is_empty(), "a stale timer");
        let _ = app.update(Message::DraftDue(seq));
        let saved = saved_drafts(&drain(&mut rx));
        assert_eq!(draft_text(&saved[0].1).as_deref(), Some("abc"));
    }

    // ---- slash commands (as in Element) ----------------------------------

    #[test]
    fn an_unknown_command_asks_before_sending() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "/foo bar");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert!(
            send_commands(&drain(&mut rx)).is_empty(),
            "nothing sent yet"
        );
        assert_eq!(app.command_bar.as_deref(), Some("foo"));
        assert_eq!(app.composer.text(), "/foo bar", "the text stays");
        let _ = app.update(Message::CommandSendAnyway);
        let commands = drain(&mut rx);
        match send_commands(&commands).as_slice() {
            [Command::Send(m)] => {
                assert_eq!(m.text.expose(), "/foo bar");
                assert_eq!(m.format, twicecat_core::api::TextFormat::Markdown);
            }
            other => panic!("expected one send, got {other:?}"),
        }
        assert!(app.command_bar.is_none());
        assert_eq!(app.composer.text(), "");
    }

    #[test]
    fn me_alone_shows_elements_command_error() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "/me");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert!(send_commands(&drain(&mut rx)).is_empty());
        assert_eq!(
            app.actions.notice.as_ref().map(|n| n.text.as_str()),
            Some(COMMAND_ERROR_TEXT)
        );
        assert_eq!(app.composer.text(), "/me", "the text stays");
    }

    #[test]
    fn a_command_missing_its_text_shows_elements_usage_line() {
        assert_eq!(command_error_text("me"), COMMAND_ERROR_TEXT);
        assert_eq!(
            command_error_text("rainbow"),
            "Command error: Usage: /rainbow <message>"
        );
        let (mut app, _rx) = main_app();
        type_in(&mut app, "/rainbowme ");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert_eq!(
            app.actions.notice.as_ref().map(|n| n.text.as_str()),
            Some("Command error: Usage: /rainbowme <message>")
        );
    }

    #[test]
    fn cancelling_an_unknown_command_keeps_the_text() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "/foo");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        let _ = app.update(Message::CommandCancelled);
        assert!(app.command_bar.is_none());
        assert!(send_commands(&drain(&mut rx)).is_empty());
        assert_eq!(app.composer.text(), "/foo");
    }

    #[test]
    fn html_is_sent_as_html_and_the_bar_goes_with_the_room() {
        let (mut app, mut rx) = main_app();
        type_in(&mut app, "/html <b>hi</b>");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        let commands = drain(&mut rx);
        assert!(matches!(
            send_commands(&commands).as_slice(),
            [Command::Send(m)] if m.format == twicecat_core::api::TextFormat::Html
        ));
        type_in(&mut app, "/foo");
        let _ = app.update(Message::ComposerKey(ComposerKey::Send));
        assert!(app.command_bar.is_some());
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert!(app.command_bar.is_none());
    }

    // ---- jumping to a reply's original ------------------------------------

    fn with_event(mut item: TimelineItem, event: &str) -> TimelineItem {
        item.event_id = Some(EventId::from(event));
        item
    }

    #[test]
    fn clicking_a_quote_of_a_loaded_message_flashes_it() {
        let (mut app, mut rx) = main_app();
        append(
            &mut app,
            vec![
                with_event(message_item("q", "question", false, ALL_ACTIONS), "$q"),
                with_event(message_item("r", "answer", false, ALL_ACTIONS), "$r"),
            ],
        );
        let _ = drain(&mut rx);
        let _ = app.update(Message::ReplyQuoteClicked(EventId::from("$q")));
        assert_eq!(app.timeline.flash.as_deref(), Some("q"));
        assert!(app.actions.notice.is_none());
        let _ = app.update(Message::FlashEnded("q".into()));
        assert_eq!(app.timeline.flash, None);
    }

    #[test]
    fn clicking_a_quote_of_an_unloaded_message_pages_back() {
        let (mut app, mut rx) = main_app();
        append(
            &mut app,
            vec![message_item("r", "answer", false, ALL_ACTIONS)],
        );
        // Let any pagination of the short timeline finish first.
        let _ = app.update(core(CoreEvent::Pagination {
            room_id: RoomId::from("!a:x"),
            state: twicecat_core::api::PaginationState::Idle,
        }));
        let _ = drain(&mut rx);
        let _ = app.update(Message::ReplyQuoteClicked(EventId::from("$old")));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::PaginateBackwards { .. }))
        );
        assert!(app.timeline.jump.is_some());
    }

    #[test]
    fn a_jump_that_reaches_the_start_shows_a_notice() {
        let (mut app, mut rx) = main_app();
        append(
            &mut app,
            vec![message_item("r", "answer", false, ALL_ACTIONS)],
        );
        let _ = drain(&mut rx);
        let _ = app.update(Message::ReplyQuoteClicked(EventId::from("$gone")));
        let _ = app.update(core(CoreEvent::Pagination {
            room_id: RoomId::from("!a:x"),
            state: twicecat_core::api::PaginationState::ReachedStart,
        }));
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: RoomId::from("!a:x"),
            diffs: vec![ListDiff::PushFront {
                value: TimelineItem::new("start".into(), TimelineItemKind::TimelineStart),
            }],
        }));
        assert_eq!(
            app.actions.notice.as_ref().map(|n| n.text.as_str()),
            Some(JUMP_FAILED_TEXT)
        );
        assert!(app.timeline.jump.is_none());
    }

    // ---- Task 13: receipts, typing and local echoes ----

    fn append(app: &mut App, items: Vec<TimelineItem>) {
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: RoomId::from("!a:x"),
            diffs: vec![ListDiff::Append { values: items }],
        }));
    }

    fn marks(commands: &[Command]) -> Vec<bool> {
        commands
            .iter()
            .filter_map(|c| match c {
                Command::MarkRead { fully_read, .. } => Some(*fully_read),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_receipt_follows_the_bottom_item_and_never_repeats() {
        let (mut app, mut rx) = main_app();
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        assert_eq!(marks(&drain(&mut rx)), [false]);
        // The same item again (a re-measure, a scroll to the same place).
        let _ = app.update(Message::RowMeasured {
            id: "a".into(),
            height: 40.0,
        });
        let _ = app.on_timeline_scrolled(0.0, 600.0, 800.0);
        assert_eq!(marks(&drain(&mut rx)), Vec::<bool>::new());
    }

    #[test]
    fn no_receipt_while_scrolled_up_until_the_bottom_is_reached() {
        let (mut app, mut rx) = main_app();
        let _ = app.on_timeline_scrolled(500.0, 600.0, 800.0);
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        assert_eq!(marks(&drain(&mut rx)), Vec::<bool>::new());
        let _ = app.on_timeline_scrolled(0.0, 600.0, 800.0);
        assert_eq!(marks(&drain(&mut rx)), [false]);
    }

    #[test]
    fn no_receipt_without_window_focus_until_it_returns() {
        let (mut app, mut rx) = main_app();
        let _ = app.update(Message::WindowFocus(false));
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        assert_eq!(marks(&drain(&mut rx)), Vec::<bool>::new());
        let _ = app.update(Message::WindowFocus(true));
        assert_eq!(marks(&drain(&mut rx)), [false]);
    }

    #[test]
    fn leaving_a_room_whose_bottom_was_seen_moves_the_read_marker() {
        let (mut app, mut rx) = main_app();
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let commands = drain(&mut rx);
        assert!(commands.iter().any(|c| matches!(
            c,
            Command::MoveReadMarker { room_id } if *room_id == RoomId::from("!a:x")
        )));
        // Never a public receipt on the way out.
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, Command::MarkRead { .. }))
        );
        // Leaving again without having seen any bottom sends nothing.
        let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
        assert_eq!(marks(&drain(&mut rx)), Vec::<bool>::new());
    }

    #[test]
    fn leaving_an_unfocused_room_sends_no_marker() {
        let (mut app, mut rx) = main_app();
        let _ = app.update(Message::WindowFocus(false));
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let commands = drain(&mut rx);
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, Command::MarkRead { .. } | Command::MoveReadMarker { .. }))
        );
    }

    #[test]
    fn a_stale_read_timer_is_ignored() {
        let (mut app, mut rx) = main_app();
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        let _ = drain(&mut rx);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        let _ = drain(&mut rx);
        app.timeline.receipt_timer_armed = true;
        let _ = app.update(Message::ReadDue(RoomId::from("!a:x")));
        assert!(app.timeline.receipt_timer_armed, "another room's timer");
        let _ = app.update(Message::ReadDue(RoomId::from("!b:x")));
        assert!(!app.timeline.receipt_timer_armed);
    }

    #[test]
    fn leaving_a_room_scrolled_up_never_seen_sends_no_marker() {
        let (mut app, mut rx) = main_app();
        let _ = app.on_timeline_scrolled(500.0, 600.0, 800.0);
        append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert_eq!(marks(&drain(&mut rx)), Vec::<bool>::new());
    }

    fn typist(id: &str, name: Option<&str>) -> twicecat_core::api::TypingUser {
        twicecat_core::api::TypingUser {
            user_id: UserId::from(id),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn typing_events_apply_to_the_open_room_and_reset_on_switch() {
        let (mut app, _rx) = main_app();
        let _ = app.update(core(CoreEvent::Typing {
            room_id: RoomId::from("!b:x"),
            users: vec![typist("@x:x", None)],
        }));
        assert!(app.typing.is_empty(), "another room");
        let users: Vec<_> = (0..10).map(|i| typist(&format!("@u{i}:x"), None)).collect();
        let _ = app.update(core(CoreEvent::Typing {
            room_id: RoomId::from("!a:x"),
            users,
        }));
        assert_eq!(app.typing.len(), crate::timeline::MAX_TYPING_KEPT);
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert!(app.typing.is_empty());
    }

    fn failed_item(id: &str, recoverable: bool) -> TimelineItem {
        let mut item = message_item(id, "x", true, ALL_ACTIONS);
        if let TimelineItemKind::Message { sent, details, .. } = &mut item.kind {
            *sent = SendStatus::Failed;
            details.send_failure = Some(twicecat_core::api::SendFailure { recoverable });
        }
        item
    }

    #[test]
    fn retry_and_discard_send_commands_for_failed_echoes_only() {
        let (mut app, mut rx) = main_app();
        append(
            &mut app,
            vec![
                failed_item("bad", true),
                message_item("ok", "fine", true, ALL_ACTIONS),
            ],
        );
        let _ = drain(&mut rx);
        let _ = app.update(Message::RetryRequested("bad".into()));
        let _ = app.update(Message::DiscardRequested("bad".into()));
        let _ = app.update(Message::RetryRequested("ok".into()));
        let _ = app.update(Message::DiscardRequested("gone".into()));
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert!(matches!(
            &commands[0],
            Command::RetrySend { room_id, item_id } if *room_id == RoomId::from("!a:x") && item_id == "bad"
        ));
        assert!(matches!(
            &commands[1],
            Command::DiscardSend { room_id, item_id } if *room_id == RoomId::from("!a:x") && item_id == "bad"
        ));
    }

    // ---- Task 10: security banner and screens ----

    use crate::security::{Banner, Busy, Overlay, SecurityMessage, Visible};
    use twicecat_core::api::{
        ActionKind as Kind, CrossSigning, DeviceVerification, ReauthAction, RecoveryKey,
        RecoveryStatus, SecurityStatus,
    };

    fn sec(app: &mut App, message: SecurityMessage) -> Task<Message> {
        app.update(Message::Security(message))
    }

    #[test]
    fn saved_cannot_be_ticked_until_the_key_was_shown_but_can_be_unticked() {
        // The handler's guard, not only the view's missing press.
        let (mut app, _rx) = main_app();
        app.security.setup = Some(crate::security::RecoverySetup::new(
            RecoveryKey::from("secret-key-x"),
            false,
        ));
        let ready = |app: &App| {
            app.security
                .setup
                .as_ref()
                .is_some_and(crate::security::can_finish_setup)
        };
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        assert!(!ready(&app), "a hidden key cannot be marked as saved");
        let _ = sec(&mut app, SecurityMessage::Done);
        assert!(app.security.setup.is_some(), "and Done does nothing");
        let _ = sec(&mut app, SecurityMessage::RevealToggled(true));
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        assert!(ready(&app), "a shown key can");
        // Hiding it again keeps the tick; unticking works whatever is shown.
        let _ = sec(&mut app, SecurityMessage::RevealToggled(false));
        assert!(ready(&app));
        let _ = sec(&mut app, SecurityMessage::SavedToggled(false));
        assert!(!ready(&app));
    }

    fn status(device: DeviceVerification, recovery: RecoveryStatus) -> SecurityStatus {
        SecurityStatus {
            recovery,
            device,
            cross_signing: CrossSigning::Complete,
            has_devices_to_verify_against: true,
            identity_exists: true,
        }
    }

    fn set_status(app: &mut App, device: DeviceVerification, recovery: RecoveryStatus) {
        let _ = app.update(core(CoreEvent::Security(status(device, recovery))));
    }

    /// A freshly logged-in app whose device is unverified, on the identity
    /// screen, with the commands drained.
    fn identity_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    /// A verified app with recovery off, so the key can be set up.
    fn recovery_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    fn key_ready(app: &mut App, key: &str) {
        let _ = app.update(core(CoreEvent::RecoveryKeyReady(RecoveryKey::from(key))));
    }

    fn recovery_failed(error: CoreError) -> Message {
        core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: Kind::Recovery,
            error,
        })
    }

    #[test]
    fn a_fresh_login_with_an_unverified_device_opens_the_identity_screen() {
        let (app, _rx) = identity_app();
        assert_eq!(
            app.security.visible(),
            Visible::Overlay(Overlay::ConfirmIdentity)
        );
        assert_eq!(app.security.banner(), Some(Banner::DeviceUnverified));
    }

    #[test]
    fn a_restored_session_shows_the_banner_but_not_the_screen() {
        let (mut app, _rx) = app_with(16);
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: true,
        }));
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert_eq!(app.security.banner(), Some(Banner::DeviceUnverified));
    }

    #[test]
    fn skip_leaves_the_unverified_banner_and_verify_reopens_the_screen() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::SkipIdentity);
        assert!(app.security.identity_screen_skipped);
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert_eq!(app.security.banner(), Some(Banner::DeviceUnverified));
        // A dismiss message does not remove the persistent banner.
        let _ = sec(&mut app, SecurityMessage::DismissRecoveryBanner);
        assert_eq!(app.security.banner(), Some(Banner::DeviceUnverified));
        // Later status updates do not reopen the screen by themselves.
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(app.security.visible(), Visible::Nothing);
        let _ = sec(&mut app, SecurityMessage::OpenConfirmIdentity);
        assert_eq!(
            app.security.visible(),
            Visible::Overlay(Overlay::ConfirmIdentity)
        );
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_unverified_banner_goes_when_the_device_is_verified() {
        let (mut app, _rx) = identity_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(app.security.banner(), None);
        assert_eq!(app.security.visible(), Visible::Nothing);
    }

    #[test]
    fn use_another_device_sends_one_self_verification_request() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::UseOtherDevice);
        let _ = sec(&mut app, SecurityMessage::UseOtherDevice);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(matches!(commands[0], Command::RequestSelfVerification));
    }

    #[test]
    fn set_up_recovery_sends_enable_once_and_the_key_opens_the_show_once_screen() {
        let (mut app, mut rx) = recovery_app();
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(matches!(commands[0], Command::EnableRecovery));
        assert_eq!(app.security.busy, Some(Busy::SettingUp));
        key_ready(&mut app, "secret-key-x");
        assert_eq!(app.security.visible(), Visible::Key);
        assert_eq!(app.security.busy, None);
    }

    #[test]
    fn done_is_refused_until_the_box_is_ticked_and_then_drops_the_key() {
        let (mut app, _rx) = recovery_app();
        key_ready(&mut app, "secret-key-x");
        let _ = sec(&mut app, SecurityMessage::Done);
        assert_eq!(app.security.visible(), Visible::Key, "refused");
        // The box is for a key that was shown.
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        let _ = sec(&mut app, SecurityMessage::Done);
        assert_eq!(app.security.visible(), Visible::Key, "still refused");
        let _ = sec(&mut app, SecurityMessage::RevealToggled(true));
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        let _ = sec(&mut app, SecurityMessage::Done);
        assert!(app.security.setup.is_none());
        // Toggling with no key screen does nothing.
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        assert!(app.security.setup.is_none());
    }

    #[test]
    fn copy_arms_one_timer_and_the_clipboard_is_cleared_only_if_it_still_holds_the_key() {
        let (mut app, _rx) = recovery_app();
        key_ready(&mut app, "secret-key-x");
        assert!(sec(&mut app, SecurityMessage::CopyKey).units() > 0);
        assert_eq!(app.security.copy_seq, 1);
        // A timer of an older copy does nothing.
        assert_eq!(sec(&mut app, SecurityMessage::ClipboardDue(0)).units(), 0);
        // Done does not forget the copy: it is still checked.
        let _ = sec(&mut app, SecurityMessage::RevealToggled(true));
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        let _ = sec(&mut app, SecurityMessage::Done);
        assert!(app.security.setup.is_none());
        assert!(sec(&mut app, SecurityMessage::ClipboardDue(1)).units() > 0);
        let other = Some(RecoveryKey::from("something else"));
        assert_eq!(
            sec(&mut app, SecurityMessage::ClipboardRead(other)).units(),
            0
        );
        assert!(app.security.copied.is_none());

        // The clipboard still holds the key: cleared.
        let (mut app, _rx) = recovery_app();
        key_ready(&mut app, "secret-key-x");
        let _ = sec(&mut app, SecurityMessage::CopyKey);
        let _ = sec(&mut app, SecurityMessage::ClipboardDue(1));
        let held = Some(RecoveryKey::from("secret-key-x"));
        assert!(sec(&mut app, SecurityMessage::ClipboardRead(held)).units() > 0);
        assert!(app.security.copied.is_none());
    }

    #[test]
    fn restore_sends_the_trimmed_input_once_and_a_failure_keeps_it() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::OpenRestore);
        assert_eq!(app.security.visible(), Visible::Overlay(Overlay::Restore));
        // Nothing to send while the field is empty.
        let _ = sec(&mut app, SecurityMessage::RestoreSubmitted);
        assert!(drain(&mut rx).is_empty());
        let typed = RecoveryKey::from("  EsTc aaaa  \n");
        let _ = sec(&mut app, SecurityMessage::RestoreInput(typed));
        let _ = sec(&mut app, SecurityMessage::RestoreSubmitted);
        let _ = sec(&mut app, SecurityMessage::RestoreSubmitted);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        match &commands[0] {
            Command::Recover { key } => assert_eq!(key.expose(), "EsTc aaaa"),
            other => panic!("{other:?}"),
        }
        let _ = app.update(recovery_failed(CoreError::Internal(
            "that recovery key or passphrase is not correct".into(),
        )));
        assert_eq!(app.security.busy, None);
        let notice = app.security.notice.clone().expect("a notice");
        assert!(notice.contains("not correct"), "{notice}");
        assert_eq!(app.security.restore_input.expose(), "  EsTc aaaa  \n");
        assert_eq!(app.security.visible(), Visible::Overlay(Overlay::Restore));
        // A success empties the field and closes the screen.
        let _ = app.update(core(CoreEvent::Recovered));
        assert_eq!(app.security.restore_input.expose(), "");
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert_eq!(app.security.notice, None);
    }

    #[test]
    fn a_backup_that_already_exists_opens_the_restore_field() {
        let (mut app, mut rx) = recovery_app();
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        let _ = drain(&mut rx);
        let _ = app.update(recovery_failed(CoreError::Internal(
            "a key backup already exists; enter your recovery key".into(),
        )));
        assert_eq!(app.security.visible(), Visible::Overlay(Overlay::Restore));
        let notice = app.security.notice.clone().expect("a notice");
        assert!(notice.contains("backup already exists"), "{notice}");
        assert_eq!(app.security.busy, None);
    }

    #[test]
    fn a_recovery_failure_shows_one_capped_line() {
        let (mut app, _rx) = recovery_app();
        let _ = app.update(recovery_failed(CoreError::Internal(format!(
            "bad\n{}",
            "x".repeat(2000)
        ))));
        let notice = app.security.notice.clone().expect("a notice");
        assert!(!notice.contains('\n') && notice.chars().count() <= 512);
    }

    #[test]
    fn the_reset_warns_first_then_asks_for_the_password_and_resends_with_it() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        assert_eq!(
            app.security.visible(),
            Visible::Overlay(Overlay::ResetConfirm)
        );
        assert!(drain(&mut rx).is_empty(), "the warning sends nothing");
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(matches!(
            commands[0],
            Command::ResetIdentity { password: None }
        ));
        assert_eq!(app.security.busy, Some(Busy::Resetting));

        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::ResetIdentity,
        }));
        assert_eq!(app.security.visible(), Visible::Reauth);
        assert_eq!(app.security.busy, None);
        // An empty password sends nothing.
        let _ = sec(&mut app, SecurityMessage::ReauthSubmitted);
        assert!(drain(&mut rx).is_empty());
        let _ = sec(
            &mut app,
            SecurityMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let _ = sec(&mut app, SecurityMessage::ReauthSubmitted);
        let _ = sec(&mut app, SecurityMessage::ReauthSubmitted);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        match &commands[0] {
            Command::ResetIdentity {
                password: Some(password),
            } => assert_eq!(password.expose(), "secret-pw-x"),
            other => panic!("{other:?}"),
        }
        let prompt = app.security.reauth.as_ref().expect("prompt kept");
        assert_eq!(prompt.password.expose(), "", "the password is gone");
        assert!(prompt.waiting);
        assert_eq!(app.security.busy, Some(Busy::Resetting));

        // The server refused it: the prompt returns with the backup warning.
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: Kind::CrossSigning,
            error: CoreError::ReauthFailed,
        }));
        let prompt = app.security.reauth.as_ref().expect("prompt kept");
        assert!(prompt.failed && !prompt.waiting);
        assert_eq!(app.security.busy, None);
        let notice = app.security.notice.clone().expect("a notice");
        assert!(notice.contains("backup"), "{notice}");

        // A successful reset ends everything.
        let _ = sec(
            &mut app,
            SecurityMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let _ = sec(&mut app, SecurityMessage::ReauthSubmitted);
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        let _ = app.update(core(CoreEvent::IdentityReset));
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert_eq!(app.security.busy, None);
        assert_eq!(app.security.notice, None);
        assert_eq!(app.security.banner(), Some(Banner::RecoveryDisabled));
    }

    /// Only the core's answer ends a reset. On a new account the status from
    /// before the reset (verified, no backup) equals the one after it, and
    /// ending on it let the next action start while the reset still ran.
    #[test]
    fn a_status_from_before_the_reset_does_not_end_it() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let _ = drain(&mut rx);
        // A status already on its way when the reset began: still unverified.
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(app.security.busy, Some(Busy::Resetting));
        // Verified, but the backup still there: not the reset's result yet.
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(app.security.busy, Some(Busy::Resetting));
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        assert_eq!(app.security.busy, Some(Busy::Resetting));
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        assert!(drain(&mut rx).is_empty(), "nothing starts during the reset");
        let _ = app.update(core(CoreEvent::IdentityReset));
        assert_eq!(app.security.busy, None);
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::EnableRecovery]
        ));
    }

    #[test]
    fn a_failed_or_cancelled_reset_says_the_backup_is_gone() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let _ = drain(&mut rx);
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: Kind::CrossSigning,
            error: CoreError::Internal("identity reset could not be completed".into()),
        }));
        let notice = app.security.notice.clone().expect("a notice");
        assert!(notice.contains("backup"), "{notice}");
        assert_eq!(app.security.busy, None);
        assert!(app.security.reauth.is_none());
    }

    #[test]
    fn an_account_management_url_opens_only_when_it_is_https() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let _ = drain(&mut rx);
        let task = app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(
            "https://account.example.org/reset?x=1",
        ))));
        assert!(task.units() > 0);
        assert!(app.security.waiting_approval);
        assert_eq!(app.security.busy, Some(Busy::Resetting));

        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let _ = drain(&mut rx);
        let task = app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(
            "http://evil.example.org/reset",
        ))));
        assert_eq!(task.units(), 0);
        assert!(!app.security.waiting_approval);
        let commands = drain(&mut rx);
        assert!(matches!(commands[..], [Command::CancelIdentityReset]));
    }

    #[test]
    fn cancelling_the_approval_sends_one_cancel_and_says_the_backup_is_gone() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        let _ = app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(
            "https://account.example.org/reset",
        ))));
        let _ = drain(&mut rx);
        let _ = sec(&mut app, SecurityMessage::CancelApproval);
        let _ = sec(&mut app, SecurityMessage::CancelApproval);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(matches!(commands[0], Command::CancelIdentityReset));
        assert!(!app.security.waiting_approval);
        let notice = app.security.notice.clone().expect("a notice");
        assert!(notice.contains("backup"), "{notice}");
    }

    #[test]
    fn logout_and_window_close_drop_the_key_and_the_inputs() {
        for message in [Message::LogoutRequested, Message::CloseRequested] {
            let (mut app, _rx) = recovery_app();
            key_ready(&mut app, "secret-key-x");
            let _ = sec(&mut app, SecurityMessage::CopyKey);
            let _ = sec(
                &mut app,
                SecurityMessage::RestoreInput(RecoveryKey::from("secret-input-x")),
            );
            // With recovery set up, logout needs no warning first.
            set_status(
                &mut app,
                DeviceVerification::Verified,
                RecoveryStatus::Enabled,
            );
            let _ = app.update(message);
            assert!(app.security.setup.is_none());
            assert!(app.security.copied.is_none());
            assert_eq!(app.security.restore_input.expose(), "");
        }
    }

    #[test]
    fn logged_out_resets_the_security_state() {
        let (mut app, _rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::SkipIdentity);
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert_eq!(app.security.banner(), None);
        assert!(!app.security.identity_screen_skipped);
        assert!(!app.security.fresh_login);
    }

    #[test]
    fn a_sentinel_key_never_appears_in_the_app_debug_output() {
        let (mut app, _rx) = recovery_app();
        key_ready(&mut app, "secret-key-x");
        let _ = sec(&mut app, SecurityMessage::CopyKey);
        let _ = sec(
            &mut app,
            SecurityMessage::RestoreInput(RecoveryKey::from("secret-input-x")),
        );
        let _ = sec(
            &mut app,
            SecurityMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let dbg = format!("{app:?}");
        assert!(dbg.contains("security"), "{dbg}");
        for secret in ["secret-key-x", "secret-input-x", "secret-pw-x"] {
            assert!(!dbg.contains(secret), "{dbg}");
        }
        let messages = format!(
            "{:?} {:?} {:?}",
            Message::Core(Box::new(CoreEvent::RecoveryKeyReady(RecoveryKey::from(
                "secret-key-x"
            )))),
            Message::Security(SecurityMessage::RestoreInput(RecoveryKey::from(
                "secret-input-x"
            ))),
            Message::Security(SecurityMessage::ClipboardRead(Some(RecoveryKey::from(
                "secret-key-x"
            )))),
        );
        assert!(!messages.contains("secret-"), "{messages}");
    }

    /// An app that has copied its recovery key just now.
    fn copied_app() -> App {
        let (mut app, _rx) = recovery_app();
        key_ready(&mut app, "secret-key-x");
        assert!(sec(&mut app, SecurityMessage::CopyKey).units() > 0);
        app
    }

    #[test]
    fn logout_close_and_disconnect_check_and_clear_the_clipboard_at_once() {
        let mut app = copied_app();
        // With recovery set up, logout needs no warning first.
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        );
        assert!(app.update(Message::LogoutRequested).units() > 0);
        assert!(app.security.copied.is_none());
        let mut app = copied_app();
        assert!(app.update(Message::CloseRequested).units() > 0);
        assert!(app.security.copied.is_none());
        let mut app = copied_app();
        assert!(app.update(Message::CoreClosed).units() > 0);
        assert_eq!(app.screen, Screen::Disconnected);
        assert!(app.security.setup.is_none() && app.security.copied.is_none());
        let mut app = copied_app();
        assert!(app.update(core(CoreEvent::LoggedOut)).units() > 0);
        let mut app = copied_app();
        let task = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }));
        assert!(task.units() > 0, "a pending copy is flushed, not lost");
        // Nothing copied: nothing to check.
        let (mut app, _rx) = recovery_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(app.update(Message::LogoutRequested).units(), 0);
    }

    #[test]
    fn close_waits_for_the_clipboard_clear_before_exiting() {
        let key = RecoveryKey::from("secret-key-x");
        let mut app = copied_app();
        let _ = app.update(Message::CloseRequested);
        // The core closes first: the flush has not been answered yet.
        let _ = app.update(Message::CoreClosed);
        assert!(!app.exit_requested, "exit must wait for the flush");
        let flushed = SecurityMessage::ClipboardFlushed(Some(key.clone()), key.clone());
        assert!(sec(&mut app, flushed).units() > 0, "the clear is written");
        assert!(app.exit_requested);
        // The flush answered first: the core closing then exits at once.
        let mut app = copied_app();
        let _ = app.update(Message::CloseRequested);
        let flushed = SecurityMessage::ClipboardFlushed(Some(key.clone()), key);
        let _ = sec(&mut app, flushed);
        assert!(!app.exit_requested, "the core has not closed yet");
        let _ = app.update(Message::CoreClosed);
        assert!(app.exit_requested);
        // The deadline stays the hard limit.
        let mut app = copied_app();
        let _ = app.update(Message::CloseRequested);
        let _ = app.update(Message::ShutdownDeadline);
        assert!(app.exit_requested);
    }

    #[test]
    fn back_and_a_closed_restore_screen_empty_the_restore_field() {
        let (mut app, _rx) = recovery_app();
        let _ = sec(&mut app, SecurityMessage::OpenRestore);
        let _ = sec(
            &mut app,
            SecurityMessage::RestoreInput(RecoveryKey::from("secret-input-x")),
        );
        let _ = sec(&mut app, SecurityMessage::Back);
        assert_eq!(app.security.restore_input.expose(), "");
    }

    #[test]
    fn the_flush_clears_only_a_clipboard_that_still_holds_the_key() {
        let key = RecoveryKey::from("secret-key-x");
        let (mut app, _rx) = recovery_app();
        let same = SecurityMessage::ClipboardFlushed(Some(key.clone()), key.clone());
        assert!(sec(&mut app, same).units() > 0);
        let other = SecurityMessage::ClipboardFlushed(Some(RecoveryKey::from("x")), key.clone());
        assert_eq!(sec(&mut app, other).units(), 0);
        let none = SecurityMessage::ClipboardFlushed(None, key);
        assert_eq!(sec(&mut app, none).units(), 0);
    }

    #[test]
    fn security_messages_never_print_their_secrets() {
        let dbg = format!(
            "{:?} {:?} {:?}",
            SecurityMessage::RestoreInput(RecoveryKey::from("secret-input-x")),
            SecurityMessage::ReauthInput(Password::from("secret-pw-x")),
            SecurityMessage::ClipboardFlushed(
                Some(RecoveryKey::from("secret-key-x")),
                RecoveryKey::from("secret-key-y")
            ),
        );
        assert!(!dbg.contains("secret-"), "{dbg}");
    }

    // ---- Task 11: verification dialog ----

    use crate::verification::VerificationMessage as VM;
    use twicecat_core::api::{
        CancelReason, QrImage, SasEmoji, VerificationId, VerificationPeer, VerificationPhase as VP,
    };

    fn vid() -> VerificationId {
        VerificationId::from("flow-1")
    }

    fn vpeer(own: bool) -> VerificationPeer {
        VerificationPeer {
            user_id: UserId::from("@bob:x"),
            device_id: Some(DeviceId::from("DEV")),
            device_name: Some("Phone".to_owned()),
            own_account: own,
        }
    }

    fn vmsg(app: &mut App, message: VM) -> Task<Message> {
        app.update(Message::Verification(message))
    }

    fn vphase(app: &mut App, phase: VP) {
        let _ = app.update(core(CoreEvent::Verification { id: vid(), phase }));
    }

    /// Ends the wait of the card now showing (what its timer does), so the
    /// buttons that agree work. Panics when no wait runs.
    fn varm(app: &mut App) {
        let seq = app
            .verification
            .as_ref()
            .and_then(|m| m.arming_seq())
            .expect("the card is waiting");
        let _ = vmsg(app, VM::Armed(seq));
        assert!(app.verification.as_ref().expect("card").armed());
    }

    fn vqr() -> QrImage {
        QrImage {
            size: 21,
            dark: vec![true; 21 * 21],
        }
    }

    fn vemojis() -> Box<[SasEmoji; 7]> {
        Box::new(std::array::from_fn(|_| SasEmoji {
            symbol: "\u{1F436}".to_owned(),
            description: "Dog".to_owned(),
        }))
    }

    /// An app with a flow with `own` peer open, commands drained.
    fn verifying(own: bool) -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        let _ = app.update(core(CoreEvent::VerificationRequested {
            id: vid(),
            peer: vpeer(own),
        }));
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    fn only(rx: &mut mpsc::Receiver<Command>) -> Command {
        let mut commands = drain(rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        commands.remove(0)
    }

    #[test]
    fn a_request_opens_the_dialog_and_a_stale_id_changes_nothing() {
        let (mut app, _rx) = verifying(true);
        let flow = app.verification.as_ref().expect("dialog open");
        assert_eq!(flow.name, "Phone");
        let _ = app.update(core(CoreEvent::Verification {
            id: VerificationId::from("other"),
            phase: VP::Done,
        }));
        assert_eq!(app.verification.as_ref().unwrap().phase.kind(), "requested");
        vphase(&mut app, VP::Requested { incoming: true });
        assert_eq!(app.verification.as_ref().unwrap().phase.kind(), "requested");
    }

    #[test]
    fn a_phase_without_a_request_opens_nothing() {
        let (mut app, _rx) = main_app();
        vphase(&mut app, VP::Ready);
        assert!(app.verification.is_none());
    }

    #[test]
    fn a_second_request_does_not_replace_a_running_flow() {
        let (mut app, _rx) = verifying(true);
        let _ = app.update(core(CoreEvent::VerificationRequested {
            id: VerificationId::from("flow-2"),
            peer: vpeer(false),
        }));
        assert_eq!(app.verification.as_ref().unwrap().id, vid());
    }

    #[test]
    fn a_request_behind_a_finished_card_shows_after_close() {
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::Done);
        let flow2 = VerificationId::from("flow-2");
        let _ = app.update(core(CoreEvent::VerificationRequested {
            id: flow2.clone(),
            peer: vpeer(false),
        }));
        // The finished card stays, so a click meant for Close cannot Accept.
        assert_eq!(app.verification.as_ref().unwrap().id, vid());
        let _ = vmsg(&mut app, VM::Accept);
        assert!(drain(&mut rx).is_empty());
        // The queued flow keeps following its events.
        let _ = app.update(core(CoreEvent::Verification {
            id: flow2.clone(),
            phase: VP::Requested { incoming: true },
        }));
        let _ = vmsg(&mut app, VM::Close);
        let shown = app.verification.as_ref().expect("queued request shown");
        assert_eq!(shown.id, flow2);
        assert_eq!(shown.phase.kind(), "requested");
        // Promoted from the queue, its Accept waits again.
        let _ = vmsg(&mut app, VM::Accept);
        assert!(drain(&mut rx).is_empty(), "a click meant for Close");
        varm(&mut app);
        let _ = vmsg(&mut app, VM::Accept);
        assert!(matches!(only(&mut rx), Command::AcceptVerification { id } if id == flow2));
    }

    #[test]
    fn a_queued_request_that_ended_before_close_is_dropped() {
        let (mut app, _rx) = verifying(true);
        vphase(&mut app, VP::Done);
        let flow2 = VerificationId::from("flow-2");
        let _ = app.update(core(CoreEvent::VerificationRequested {
            id: flow2.clone(),
            peer: vpeer(false),
        }));
        let _ = app.update(core(CoreEvent::Verification {
            id: flow2,
            phase: VP::Cancelled {
                reason: twicecat_core::api::CancelReason::ByThem,
            },
        }));
        let _ = vmsg(&mut app, VM::Close);
        assert!(app.verification.is_none());
    }

    #[test]
    fn accept_and_decline_send_one_command_each() {
        let (mut app, mut rx) = verifying(false);
        vphase(&mut app, VP::Requested { incoming: true });
        varm(&mut app);
        let _ = vmsg(&mut app, VM::Accept);
        let _ = vmsg(&mut app, VM::Accept);
        assert!(matches!(only(&mut rx), Command::AcceptVerification { id } if id == vid()));
        let (mut app, mut rx) = verifying(false);
        vphase(&mut app, VP::Requested { incoming: true });
        let _ = vmsg(&mut app, VM::Cancel);
        let _ = vmsg(&mut app, VM::Cancel);
        assert!(matches!(only(&mut rx), Command::CancelVerification { id } if id == vid()));
    }

    #[test]
    fn accept_is_only_for_an_incoming_request() {
        let (mut app, mut rx) = verifying(false);
        let _ = vmsg(&mut app, VM::Accept);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_qr_buttons_send_start_sas_confirm_and_cancel() {
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::ShowQr(vqr()));
        let _ = vmsg(&mut app, VM::CompareEmoji);
        let _ = vmsg(&mut app, VM::CompareEmoji);
        assert!(matches!(only(&mut rx), Command::StartSas { id } if id == vid()));
        vphase(&mut app, VP::QrScanned);
        varm(&mut app);
        let _ = vmsg(&mut app, VM::QrConfirmed);
        let _ = vmsg(&mut app, VM::QrConfirmed);
        assert!(matches!(only(&mut rx), Command::ConfirmQrScanned { id } if id == vid()));
        let _ = vmsg(&mut app, VM::Cancel);
        assert!(matches!(only(&mut rx), Command::CancelVerification { id } if id == vid()));
    }

    #[test]
    fn the_emoji_buttons_send_confirm_and_mismatch() {
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::Emojis(vemojis()));
        varm(&mut app);
        let _ = vmsg(&mut app, VM::Match);
        let _ = vmsg(&mut app, VM::Match);
        assert!(matches!(only(&mut rx), Command::ConfirmSas { id } if id == vid()));
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::Decimals([1, 2, 3]));
        let _ = vmsg(&mut app, VM::NoMatch);
        let _ = vmsg(&mut app, VM::NoMatch);
        assert!(matches!(only(&mut rx), Command::SasMismatch { id } if id == vid()));
    }

    #[test]
    fn the_agreeing_buttons_wait_and_a_stale_timer_or_a_new_phase_never_arms_early() {
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::Emojis(vemojis()));
        let first = app
            .verification
            .as_ref()
            .unwrap()
            .arming_seq()
            .expect("waits");
        let _ = vmsg(&mut app, VM::Match);
        assert!(drain(&mut rx).is_empty(), "Match waits for the card");
        // The refusing buttons never wait.
        let (mut other, mut orx) = verifying(true);
        vphase(&mut other, VP::Emojis(vemojis()));
        let _ = vmsg(&mut other, VM::NoMatch);
        assert!(matches!(only(&mut orx), Command::SasMismatch { .. }));
        // A new set of emoji starts a new wait; the old timer does nothing.
        vphase(&mut app, VP::Decimals([1, 2, 3]));
        let second = app
            .verification
            .as_ref()
            .unwrap()
            .arming_seq()
            .expect("waits again");
        assert_ne!(first, second);
        let _ = vmsg(&mut app, VM::Armed(first));
        let _ = vmsg(&mut app, VM::Match);
        assert!(drain(&mut rx).is_empty(), "a stale timer armed it");
        let _ = vmsg(&mut app, VM::Armed(second));
        let _ = vmsg(&mut app, VM::Match);
        assert!(matches!(only(&mut rx), Command::ConfirmSas { .. }));
    }

    #[test]
    fn a_button_of_another_phase_sends_nothing() {
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::Waiting);
        for m in [VM::CompareEmoji, VM::QrConfirmed, VM::Match, VM::NoMatch] {
            let _ = vmsg(&mut app, m);
        }
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn close_works_only_when_the_flow_is_over() {
        let (mut app, mut rx) = verifying(true);
        let _ = vmsg(&mut app, VM::Close);
        assert!(app.verification.is_some());
        vphase(
            &mut app,
            VP::Cancelled {
                reason: CancelReason::ByThem,
            },
        );
        let _ = vmsg(&mut app, VM::Close);
        assert!(app.verification.is_none());
        assert!(drain(&mut rx).is_empty(), "closing sends nothing");
    }

    #[test]
    fn the_qr_image_survives_unrelated_updates_and_goes_with_the_flow() {
        let (mut app, _rx) = verifying(true);
        vphase(&mut app, VP::ShowQr(vqr()));
        let first = app
            .verification
            .as_ref()
            .unwrap()
            .qr_handle()
            .map(|h| h.id());
        assert!(first.is_some());
        let _ = app.update(Message::WindowFocus(false));
        let _ = app.update(core(CoreEvent::Security(status(
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        ))));
        assert_eq!(
            app.verification
                .as_ref()
                .unwrap()
                .qr_handle()
                .map(|h| h.id()),
            first
        );
        vphase(&mut app, VP::Emojis(vemojis()));
        assert!(app.verification.as_ref().unwrap().qr_handle().is_none());
    }

    #[test]
    fn debug_of_the_app_with_a_qr_flow_shows_no_module_data() {
        let (mut app, _rx) = verifying(true);
        vphase(&mut app, VP::ShowQr(vqr()));
        let text = format!("{app:?}");
        assert!(text.contains("verification"), "{text}");
        assert!(!text.contains("dark") && !text.contains("Rgba"), "{text}");
    }

    #[test]
    fn a_failure_in_a_running_flow_keeps_the_dialog_and_allows_a_retry() {
        let (mut app, mut rx) = verifying(true);
        vphase(&mut app, VP::ShowQr(vqr()));
        let _ = vmsg(&mut app, VM::CompareEmoji);
        let _ = drain(&mut rx);
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: Kind::Verification,
            error: CoreError::Internal("qr".into()),
        }));
        assert!(app.verification.is_some());
        assert!(app.security.notice.is_none(), "no spurious notice");
        let _ = vmsg(&mut app, VM::CompareEmoji);
        assert!(matches!(only(&mut rx), Command::StartSas { .. }));
    }

    #[test]
    fn a_failed_request_without_a_flow_shows_a_notice_and_frees_the_button() {
        let (mut app, mut rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::UseOtherDevice);
        let _ = drain(&mut rx);
        assert!(app.security.asked_other_device);
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: Kind::Verification,
            error: CoreError::Busy,
        }));
        assert!(app.verification.is_none());
        assert!(app.security.notice.is_some());
        assert!(!app.security.asked_other_device);
    }

    #[test]
    fn a_request_clears_the_request_sent_state_of_the_identity_screen() {
        let (mut app, _rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::UseOtherDevice);
        let _ = app.update(core(CoreEvent::VerificationRequested {
            id: vid(),
            peer: vpeer(true),
        }));
        assert!(!app.security.asked_other_device);
    }

    #[test]
    fn logout_drops_the_flow() {
        let (mut app, _rx) = verifying(true);
        vphase(&mut app, VP::ShowQr(vqr()));
        let _ = app.back_to_login("signed out");
        assert!(app.verification.is_none());
    }

    // ---- Task 12: the trust of a DM peer in the header --------------------

    fn dm_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = app_with(32);
        logged_in(&mut app);
        let mut dm = RoomSummary::new(RoomId::from("!dm:x"), "Bob".to_owned());
        dm.is_dm = true;
        dm.dm_peer = Some(UserId::from("@bob:x"));
        let room = RoomSummary::new(RoomId::from("!room:x"), "Lounge".to_owned());
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
            values: vec![dm, room],
        }])));
        let _ = app.update(Message::SelectRoom(RoomId::from("!dm:x")));
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    fn from_sender(sender: &str) -> TimelineItem {
        TimelineItem::new(
            "m".to_owned(),
            TimelineItemKind::Message {
                sender: UserId::from(sender),
                body: String::new(),
                is_own: false,
                timestamp_ms: 0,
                sent: twicecat_core::api::SendStatus::Sent,
                details: Box::default(),
            },
        )
    }

    fn trust(app: &mut App, user: &str, trust: UserTrust) {
        let _ = app.update(core(CoreEvent::UserTrust {
            user_id: UserId::from(user),
            trust,
        }));
    }

    #[test]
    fn the_header_shows_the_trust_of_the_selected_dm_peer() {
        let (mut app, _rx) = dm_app();
        assert_eq!(app.dm_peer_trust(), None, "nothing reported yet");
        trust(&mut app, "@bob:x", UserTrust::Unverified);
        assert_eq!(
            app.dm_peer_trust(),
            Some((&UserId::from("@bob:x"), UserTrust::Unverified))
        );
        trust(&mut app, "@bob:x", UserTrust::Verified);
        assert_eq!(
            app.dm_peer_trust(),
            Some((&UserId::from("@bob:x"), UserTrust::Verified))
        );
    }

    #[test]
    fn a_trust_for_another_user_is_ignored_by_the_header() {
        let (mut app, _rx) = dm_app();
        trust(&mut app, "@bob:x", UserTrust::Unverified);
        // A late report of an aborted watcher for the previous room's peer.
        trust(&mut app, "@carol:x", UserTrust::Verified);
        assert_eq!(
            app.dm_peer_trust(),
            Some((&UserId::from("@bob:x"), UserTrust::Unverified))
        );
        let (mut app, _rx) = dm_app();
        trust(&mut app, "@carol:x", UserTrust::Verified);
        assert_eq!(app.dm_peer_trust(), None);
    }

    #[test]
    fn a_dm_whose_peer_has_not_spoken_still_shows_the_badge() {
        let (mut app, _rx) = dm_app();
        assert!(app.timeline.items.is_empty(), "nobody has spoken");
        trust(&mut app, "@bob:x", UserTrust::Verified);
        assert_eq!(
            app.dm_peer_trust(),
            Some((&UserId::from("@bob:x"), UserTrust::Verified))
        );
    }

    #[test]
    fn the_peer_is_the_one_the_core_names_not_a_sender() {
        let (mut app, _rx) = dm_app();
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: RoomId::from("!dm:x"),
            diffs: vec![ListDiff::Reset {
                values: vec![from_sender("@mallory:x")],
            }],
        }));
        trust(&mut app, "@mallory:x", UserTrust::Verified);
        assert_eq!(app.dm_peer_trust(), None);
        assert_eq!(app.dm_peer().map(|(p, _)| p.as_str()), Some("@bob:x"));
    }

    #[test]
    fn the_peer_follows_the_summary_when_it_is_rebuilt() {
        let (mut app, _rx) = dm_app();
        let mut changed = RoomSummary::new(RoomId::from("!dm:x"), "Bob".to_owned());
        changed.is_dm = true;
        changed.dm_peer = Some(UserId::from("@bea:x"));
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Set {
            index: 0,
            value: changed,
        }])));
        assert_eq!(app.dm_peer().map(|(p, _)| p.as_str()), Some("@bea:x"));
    }

    #[test]
    fn a_room_that_is_not_a_dm_has_no_peer_trust() {
        let (mut app, _rx) = dm_app();
        trust(&mut app, "@bob:x", UserTrust::Unverified);
        let _ = app.update(Message::SelectRoom(RoomId::from("!room:x")));
        trust(&mut app, "@bob:x", UserTrust::Unverified);
        assert_eq!(app.dm_peer_trust(), None);
    }

    #[test]
    fn the_kept_trusts_are_capped_and_cleared_on_selecting_a_room() {
        let (mut app, _rx) = dm_app();
        for n in 0..(MAX_TRUST_KEPT * 3) {
            trust(&mut app, &format!("@u{n}:x"), UserTrust::Verified);
        }
        assert_eq!(app.user_trust.len(), MAX_TRUST_KEPT);
        let _ = app.update(Message::SelectRoom(RoomId::from("!room:x")));
        assert!(app.user_trust.is_empty());
    }

    #[test]
    fn verify_sends_a_request_for_the_peer() {
        let (mut app, mut rx) = dm_app();
        trust(&mut app, "@bob:x", UserTrust::Unverified);
        let _ = app.update(Message::VerifyPeerRequested(UserId::from("@bob:x")));
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(
            matches!(&commands[0], Command::RequestUserVerification { user_id } if user_id.as_str() == "@bob:x"),
            "{commands:?}"
        );
    }

    #[test]
    fn verify_without_a_peer_sends_nothing() {
        let (mut app, mut rx) = dm_app();
        let _ = app.update(Message::SelectRoom(RoomId::from("!room:x")));
        let _ = drain(&mut rx);
        let _ = app.update(Message::VerifyPeerRequested(UserId::from("@bob:x")));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn verify_for_a_user_who_is_not_the_peer_sends_nothing() {
        let (mut app, mut rx) = dm_app();
        let _ = app.update(Message::VerifyPeerRequested(UserId::from("@carol:x")));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn going_back_to_login_forgets_the_trusts() {
        let (mut app, _rx) = dm_app();
        trust(&mut app, "@bob:x", UserTrust::Verified);
        assert!(!app.user_trust.is_empty());
        let _ = app.back_to_login("signed out");
        assert!(app.user_trust.is_empty());
        assert_eq!(app.dm_peer_trust(), None);
    }

    // ---- Task 13: sessions screen and re-authentication ----

    use crate::sessions::SessionsMessage;
    use twicecat_core::api::{DeviceSummary, DeviceTrust};

    fn ses(app: &mut App, message: SessionsMessage) -> Task<Message> {
        app.update(Message::Sessions(message))
    }

    fn summary(id: &str, name: Option<&str>, current: bool, trust: DeviceTrust) -> DeviceSummary {
        DeviceSummary {
            device_id: DeviceId::from(id),
            display_name: name.map(str::to_owned),
            last_seen_ms: Some(1_000),
            current,
            trust,
        }
    }

    /// Three devices: this one, a verified laptop and an unverified phone.
    fn device_list() -> Vec<DeviceSummary> {
        vec![
            summary("THIS", Some("Twicecat"), true, DeviceTrust::Verified),
            summary("LAPTOP", Some("Laptop"), false, DeviceTrust::Verified),
            summary("PHONE", Some("Phone"), false, DeviceTrust::Unverified),
        ]
    }

    /// An app on the Sessions screen with the three devices listed.
    fn sessions_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        let _ = ses(&mut app, SessionsMessage::Open);
        let _ = app.update(core(CoreEvent::Devices(device_list())));
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    #[test]
    fn opening_sessions_asks_the_core_for_the_list() {
        let (mut app, mut rx) = main_app();
        let _ = ses(&mut app, SessionsMessage::Open);
        assert!(app.sessions.open && app.sessions.loading);
        assert!(matches!(rx.try_recv(), Ok(Command::ListDevices)));
        let _ = app.update(core(CoreEvent::Devices(device_list())));
        assert!(!app.sessions.loading);
        assert_eq!(app.sessions.devices.as_ref().map(Vec::len), Some(3));
        // The core does not push other clients' changes: Refresh asks again.
        let _ = ses(&mut app, SessionsMessage::Refresh);
        assert!(matches!(rx.try_recv(), Ok(Command::ListDevices)));
        let _ = ses(&mut app, SessionsMessage::Close);
        assert!(!app.sessions.open);
    }

    #[test]
    fn the_current_device_is_listed_first_whatever_the_order() {
        let (mut app, _rx) = main_app();
        let mut list = device_list();
        list.rotate_left(1);
        let _ = app.update(core(CoreEvent::Devices(list)));
        let first = &app.sessions.devices.as_ref().expect("a list")[0];
        assert!(first.current);
    }

    #[test]
    fn renaming_sends_the_trimmed_name_on_enter() {
        let (mut app, mut rx) = sessions_app();
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let edit = app.sessions.rename.as_ref().expect("an open field");
        assert_eq!(edit.input, "Phone");
        let _ = ses(
            &mut app,
            SessionsMessage::RenameInput("  Work phone ".into()),
        );
        let _ = ses(&mut app, SessionsMessage::RenameSubmitted);
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::RenameDevice { device_id, name }]
                    if *device_id == DeviceId::from("PHONE") && name == "Work phone"
            ),
            "{commands:?}"
        );
        assert!(app.sessions.rename.is_none());
    }

    #[test]
    fn an_empty_or_unchanged_name_sends_nothing() {
        let (mut app, mut rx) = sessions_app();
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::RenameSubmitted);
        assert!(drain(&mut rx).is_empty(), "unchanged");
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::RenameInput("   ".into()));
        let _ = ses(&mut app, SessionsMessage::RenameSubmitted);
        assert!(drain(&mut rx).is_empty(), "blank");
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("NOPE")),
        );
        assert!(app.sessions.rename.is_none(), "not in the list");
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::RenameCancelled);
        assert!(app.sessions.rename.is_none());
    }

    #[test]
    fn the_rename_field_is_capped() {
        let (mut app, _rx) = sessions_app();
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::RenameInput("x".repeat(500)));
        let edit = app.sessions.rename.as_ref().expect("an open field");
        assert_eq!(edit.input.chars().count(), crate::sessions::MAX_NAME_CHARS);
    }

    #[test]
    fn verify_is_sent_for_an_unverified_other_device_only() {
        let (mut app, mut rx) = sessions_app();
        for id in ["THIS", "LAPTOP", "NOPE"] {
            let _ = ses(&mut app, SessionsMessage::Verify(DeviceId::from(id)));
            assert!(drain(&mut rx).is_empty(), "{id}");
        }
        let _ = ses(&mut app, SessionsMessage::Verify(DeviceId::from("PHONE")));
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::RequestDeviceVerification { device_id }] if *device_id == DeviceId::from("PHONE")
            ),
            "{commands:?}"
        );
    }

    #[test]
    fn signing_out_one_device_asks_first() {
        let (mut app, mut rx) = sessions_app();
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("LAPTOP")),
        );
        assert!(drain(&mut rx).is_empty(), "nothing before the confirmation");
        let _ = ses(&mut app, SessionsMessage::SignOutCancelled);
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert!(drain(&mut rx).is_empty(), "cancelled");
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("LAPTOP")),
        );
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::SignOutDevices { device_ids, password: None }]
                    if *device_ids == [DeviceId::from("LAPTOP")]
            ),
            "{commands:?}"
        );
        assert_eq!(app.sessions.pending, Some(vec![DeviceId::from("LAPTOP")]));
    }

    #[test]
    fn this_device_cannot_be_signed_out_from_the_list() {
        let (mut app, mut rx) = sessions_app();
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("THIS")),
        );
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert!(drain(&mut rx).is_empty());
        assert!(app.sessions.pending.is_none());
    }

    #[test]
    fn signing_out_all_sends_every_other_device_once() {
        let (mut app, mut rx) = main_app();
        let mut list = device_list();
        // A repeated entry (the core does not dedup) must not repeat in the call.
        list.push(summary(
            "PHONE",
            Some("Phone"),
            false,
            DeviceTrust::Unverified,
        ));
        let _ = app.update(core(CoreEvent::Devices(list)));
        let _ = ses(&mut app, SessionsMessage::SignOutAllAsk);
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::SignOutDevices { device_ids, password: None }]
                    if *device_ids == [DeviceId::from("LAPTOP"), DeviceId::from("PHONE")]
            ),
            "{commands:?}"
        );
    }

    #[test]
    fn nothing_is_sent_to_sign_out_all_when_there_is_no_other_device() {
        let (mut app, mut rx) = main_app();
        let only = vec![summary("THIS", None, true, DeviceTrust::Verified)];
        let _ = app.update(core(CoreEvent::Devices(only)));
        let _ = ses(&mut app, SessionsMessage::SignOutAllAsk);
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert!(drain(&mut rx).is_empty());
    }

    /// Signs the phone out and answers with `ReauthRequired`.
    fn reauth_asked() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = sessions_app();
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        let _ = drain(&mut rx);
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::DeleteDevices,
        }));
        (app, rx)
    }

    #[test]
    fn the_reauth_round_trip_repeats_the_command_with_the_password() {
        let (mut app, mut rx) = reauth_asked();
        assert!(app.sessions.reauth.is_some());
        let _ = ses(&mut app, SessionsMessage::ReauthSubmitted);
        assert!(drain(&mut rx).is_empty(), "an empty password is not sent");
        let _ = ses(
            &mut app,
            SessionsMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let dbg = format!("{app:?}");
        assert!(!dbg.contains("secret-pw-x"), "{dbg}");
        let _ = ses(&mut app, SessionsMessage::ReauthSubmitted);
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::SignOutDevices { device_ids, password: Some(password) }]
                    if *device_ids == [DeviceId::from("PHONE")] && password.expose() == "secret-pw-x"
            ),
            "{commands:?}"
        );
        // The app no longer holds the password.
        let prompt = app.sessions.reauth.as_ref().expect("the prompt stays");
        assert_eq!(prompt.password.expose(), "");
        assert!(prompt.waiting);
        let dbg = format!("{app:?} {:?}", app.sessions);
        assert!(!dbg.contains("secret-pw-x"), "{dbg}");
        // Success: the next list no longer has the phone.
        let rest: Vec<_> = device_list()
            .into_iter()
            .filter(|d| d.device_id != DeviceId::from("PHONE"))
            .collect();
        let _ = app.update(core(CoreEvent::Devices(rest)));
        assert!(app.sessions.reauth.is_none() && app.sessions.pending.is_none());
    }

    #[test]
    fn a_wrong_password_keeps_the_prompt_and_asks_again() {
        let (mut app, mut rx) = reauth_asked();
        let _ = ses(
            &mut app,
            SessionsMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let _ = ses(&mut app, SessionsMessage::ReauthSubmitted);
        let _ = drain(&mut rx);
        // A list that still shows the phone must not close the prompt.
        let _ = app.update(core(CoreEvent::Devices(device_list())));
        assert!(app.sessions.reauth.is_some());
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::SignOutDevices,
            error: CoreError::ReauthFailed,
        }));
        let prompt = app.sessions.reauth.as_ref().expect("the prompt stays");
        assert!(prompt.failed && !prompt.waiting);
        assert_eq!(prompt.password.expose(), "");
        assert_eq!(app.sessions.pending, Some(vec![DeviceId::from("PHONE")]));
        // Typing again and submitting sends the command again.
        let _ = ses(
            &mut app,
            SessionsMessage::ReauthInput(Password::from("secret-pw-y")),
        );
        let _ = ses(&mut app, SessionsMessage::ReauthSubmitted);
        let commands = drain(&mut rx);
        assert!(
            matches!(
                commands.as_slice(),
                [Command::SignOutDevices { password: Some(p), .. }] if p.expose() == "secret-pw-y"
            ),
            "{commands:?}"
        );
    }

    #[test]
    fn cancelling_the_prompt_zeroizes_the_field_and_forgets_the_sign_out() {
        let (mut app, mut rx) = reauth_asked();
        let _ = ses(
            &mut app,
            SessionsMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let _ = ses(&mut app, SessionsMessage::ReauthCancelled);
        assert!(app.sessions.reauth.is_none() && app.sessions.pending.is_none());
        let _ = ses(&mut app, SessionsMessage::ReauthSubmitted);
        assert!(drain(&mut rx).is_empty());
        let dbg = format!("{app:?}");
        assert!(!dbg.contains("secret-pw-x"), "{dbg}");
    }

    #[test]
    fn a_reauth_request_nobody_asked_for_is_ignored() {
        let (mut app, _rx) = sessions_app();
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::DeleteDevices,
        }));
        assert!(app.sessions.reauth.is_none());
    }

    #[test]
    fn logout_and_close_drop_the_sessions_password() {
        for message in [Message::LogoutRequested, Message::CloseRequested] {
            let (mut app, _rx) = reauth_asked();
            let _ = ses(
                &mut app,
                SessionsMessage::ReauthInput(Password::from("secret-pw-x")),
            );
            let _ = app.update(message);
            assert!(app.sessions.reauth.is_none());
        }
    }

    #[test]
    fn another_failure_ends_the_sign_out_with_a_notice() {
        let (mut app, _rx) = reauth_asked();
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::SignOutDevices,
            error: CoreError::Network("the devices could not be loaded".into()),
        }));
        assert!(app.sessions.reauth.is_none() && app.sessions.pending.is_none());
        let notice = app.sessions.notice.clone().expect("a notice");
        assert!(!notice.contains('\n') && !notice.is_empty());
    }

    #[test]
    fn no_sign_out_starts_while_an_identity_reset_runs() {
        let (mut app, mut rx) = sessions_app();
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        // One sign-out is asked before the reset begins, one is not.
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert_eq!(drain(&mut rx).len(), 1, "only the reset command");
        assert_eq!(app.security.busy, Some(Busy::Resetting));

        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert!(app.sessions.pending.is_none() && app.sessions.confirm.is_none());
        let notice = app.sessions.notice.clone().expect("a notice");
        assert!(!notice.contains('\n') && !notice.is_empty());
        app.sessions.notice = None;
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("LAPTOP")),
        );
        assert!(app.sessions.confirm.is_none() && app.sessions.notice.is_some());
        app.sessions.notice = None;
        let _ = ses(&mut app, SessionsMessage::SignOutAllAsk);
        assert!(app.sessions.confirm.is_none() && app.sessions.notice.is_some());
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert!(drain(&mut rx).is_empty(), "no sign-out was sent");
        assert_eq!(app.security.busy, Some(Busy::Resetting));
    }

    #[test]
    fn no_identity_reset_starts_while_a_sign_out_is_pending() {
        let (mut app, mut rx) = sessions_app();
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        let _ = drain(&mut rx);
        // The warning was already up when the sign-out began.
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert_eq!(drain(&mut rx).len(), 1, "only the sign-out");
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert!(drain(&mut rx).is_empty(), "no reset was sent");
        assert!(app.security.busy.is_none());
        assert!(app.security.notice.is_some());
        assert!(app.sessions.pending.is_some(), "the sign-out keeps going");

        // A fresh request is refused too, without the warning.
        app.security.overlay = None;
        app.security.notice = None;
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        assert!(app.security.overlay.is_none() && app.security.notice.is_some());
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert!(drain(&mut rx).is_empty());
        assert!(app.sessions.pending.is_some());
    }

    fn devices_failed(app: &mut App, error: CoreError) {
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::Devices,
            error,
        }));
    }

    #[test]
    fn another_devices_failure_keeps_the_sign_out_pending() {
        let others = [
            CoreError::Internal("the user's trust could not be watched".into()),
            CoreError::Internal("the device could not be renamed".into()),
            CoreError::Network("the devices could not be loaded".into()),
            CoreError::Internal("that is not a valid user ID".into()),
            CoreError::NotAllowed,
            CoreError::NotLoggedIn,
            CoreError::ReauthFailed,
        ];
        for error in others {
            let (mut app, mut rx) = sessions_app();
            set_status(
                &mut app,
                DeviceVerification::Unverified,
                RecoveryStatus::Enabled,
            );
            let _ = ses(
                &mut app,
                SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
            );
            let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
            let _ = drain(&mut rx);
            devices_failed(&mut app, error.clone());
            assert!(app.sessions.pending.is_some(), "{error:?} ended it");
            let _ = sec(&mut app, SecurityMessage::ResetRequested);
            let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
            assert!(drain(&mut rx).is_empty(), "{error:?}: a reset was sent");
            assert!(app.security.busy.is_none());
            // The later account page still goes to the approval bar.
            let _ = app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(
                "https://account.example.org/devices",
            ))));
            assert!(app.sessions.approval.is_some(), "{error:?}");
        }
    }

    fn sign_out_failed(app: &mut App, error: CoreError) {
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::SignOutDevices,
            error,
        }));
    }

    #[test]
    fn a_sign_out_failure_of_any_error_ends_it_with_a_one_line_notice() {
        let own = [
            CoreError::ReauthFailed,
            CoreError::NotAllowed,
            CoreError::NotLoggedIn,
            CoreError::Busy,
            CoreError::Network("the devices could not be loaded".into()),
            CoreError::Internal("anything at all".into()),
        ];
        for error in own {
            let (mut app, mut rx) = sessions_app();
            let _ = ses(
                &mut app,
                SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
            );
            let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
            let _ = drain(&mut rx);
            sign_out_failed(&mut app, error.clone());
            assert!(app.sessions.pending.is_none(), "{error:?} kept it");
            let notice = app.sessions.notice.clone().expect("a notice");
            assert!(!notice.contains('\n') && !notice.is_empty(), "{error:?}");
        }
    }

    #[test]
    fn a_network_failure_after_the_delete_ends_the_sign_out_and_a_reset_is_allowed() {
        let (mut app, mut rx) = signed_out_and_closed();
        sign_out_failed(
            &mut app,
            CoreError::Network("the devices could not be loaded".into()),
        );
        assert!(app.sessions.pending.is_none());
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert!(!drain(&mut rx).is_empty(), "the reset was sent");
    }

    #[test]
    fn the_same_failure_of_the_devices_kind_keeps_a_closed_sign_out_pending() {
        let (mut app, mut rx) = signed_out_and_closed();
        devices_failed(
            &mut app,
            CoreError::Network("the devices could not be loaded".into()),
        );
        assert!(app.sessions.pending.is_some());
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert!(drain(&mut rx).is_empty(), "no reset was sent");
    }

    #[test]
    fn reopening_the_screen_lists_again_and_the_answer_ends_a_sign_out() {
        let (mut app, mut rx) = signed_out_and_closed();
        let _ = ses(&mut app, SessionsMessage::Open);
        assert!(matches!(drain(&mut rx).as_slice(), [Command::ListDevices]));
        let _ = ses(&mut app, SessionsMessage::Refresh);
        assert!(matches!(drain(&mut rx).as_slice(), [Command::ListDevices]));
        assert!(app.sessions.pending.is_some());
        let _ = app.update(core(CoreEvent::Devices(vec![])));
        assert!(app.sessions.pending.is_none());
    }

    #[test]
    fn the_reset_password_submit_is_refused_while_a_sign_out_is_pending() {
        let (mut app, mut rx) = sessions_app();
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        // The reset's password prompt arrives while the sign-out runs.
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::ResetIdentity,
        }));
        let _ = drain(&mut rx);
        let _ = sec(
            &mut app,
            SecurityMessage::ReauthInput(Password::from("secret-pw-x")),
        );
        let _ = sec(&mut app, SecurityMessage::ReauthSubmitted);
        assert!(drain(&mut rx).is_empty(), "no reset was sent");
        assert!(app.security.busy.is_none());
        assert!(app.security.notice.is_some());
        assert!(app.sessions.pending.is_some());
    }

    #[test]
    fn no_sign_out_starts_while_the_reset_password_prompt_is_open() {
        let (mut app, mut rx) = sessions_app();
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::ResetIdentity,
        }));
        assert!(app.security.busy.is_none());
        let _ = drain(&mut rx);
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        assert!(app.sessions.confirm.is_none() && app.sessions.notice.is_some());
        let _ = ses(&mut app, SessionsMessage::SignOutAllAsk);
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        assert!(drain(&mut rx).is_empty(), "no sign-out was sent");
        assert!(app.sessions.pending.is_none());
    }

    /// Signs the phone out on an OAuth session: the core answers with a URL.
    fn oauth_url(app: &mut App, url: &str) -> Task<Message> {
        let _ = ses(app, SessionsMessage::SignOutAsk(DeviceId::from("PHONE")));
        let _ = ses(app, SessionsMessage::SignOutConfirmed);
        app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(url))))
    }

    #[test]
    fn an_account_page_waits_for_the_go_ahead_and_shows_its_host() {
        let (mut app, _rx) = sessions_app();
        let task = oauth_url(
            &mut app,
            "https://Account.Example.org:8443/devices?state=secret-state-x",
        );
        assert_eq!(task.units(), 0, "nothing opens before the user agrees");
        let approval = app.sessions.approval.as_ref().expect("a bar");
        assert_eq!(approval.host, "account.example.org");
        assert!(app.sessions.pending.is_none());
        let dbg = format!("{app:?}");
        assert!(!dbg.contains("secret-state-x"), "{dbg}");
        let task = ses(&mut app, SessionsMessage::ApprovalConfirmed);
        assert!(task.units() > 0, "opened inside a task");
        assert!(app.sessions.approval.is_none());
        // A second confirmation has nothing to open.
        assert_eq!(ses(&mut app, SessionsMessage::ApprovalConfirmed).units(), 0);
    }

    #[test]
    fn dismissing_the_account_page_bar_opens_nothing() {
        let (mut app, _rx) = sessions_app();
        let _ = oauth_url(&mut app, "https://account.example.org/devices");
        assert_eq!(ses(&mut app, SessionsMessage::ApprovalDismissed).units(), 0);
        assert!(app.sessions.approval.is_none());
    }

    #[test]
    fn an_unsafe_account_page_address_is_refused() {
        for url in [
            "http://account.example.org/devices",
            "https://user:pw@account.example.org/devices",
            "javascript:alert(1)",
            "https://account.example.org/a b",
            "file:///etc/passwd",
        ] {
            let (mut app, _rx) = sessions_app();
            let task = oauth_url(&mut app, url);
            assert_eq!(task.units(), 0, "{url}");
            assert!(app.sessions.approval.is_none(), "{url}");
            assert!(app.sessions.notice.is_some(), "{url}");
            assert_eq!(ses(&mut app, SessionsMessage::ApprovalConfirmed).units(), 0);
        }
    }

    /// Signs the phone out, closes the screen at once and resets afterwards.
    fn signed_out_and_closed() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = sessions_app();
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        let _ = ses(&mut app, SessionsMessage::Close);
        let _ = drain(&mut rx);
        (app, rx)
    }

    #[test]
    fn closing_the_screen_keeps_a_running_sign_out_from_overlapping_a_reset() {
        let (mut app, mut rx) = signed_out_and_closed();
        assert!(app.sessions.pending.is_some());
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert!(drain(&mut rx).is_empty(), "no reset was sent");
        assert!(app.security.busy.is_none());
    }

    #[test]
    fn an_account_page_after_close_goes_to_the_bar_not_the_browser() {
        let (mut app, _rx) = signed_out_and_closed();
        let busy = app.security.busy;
        let task = app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(
            "https://account.example.org/devices",
        ))));
        assert_eq!(task.units(), 0, "nothing opens without a click");
        assert_eq!(app.security.busy, busy);
        assert!(app.sessions.approval.is_some());
        assert!(app.sessions.pending.is_none());
    }

    #[test]
    fn a_sign_out_closed_early_ends_on_the_cores_answer_and_a_reset_is_allowed() {
        let (mut app, mut rx) = signed_out_and_closed();
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::SignOutDevices,
            error: CoreError::Network("the devices could not be loaded".into()),
        }));
        assert!(app.sessions.pending.is_none());
        let _ = sec(&mut app, SecurityMessage::ResetRequested);
        let _ = sec(&mut app, SecurityMessage::ResetConfirmed);
        assert!(!drain(&mut rx).is_empty(), "the reset was sent");

        let (mut app, _rx) = signed_out_and_closed();
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::DeleteDevices,
        }));
        assert!(app.sessions.pending.is_none() && app.sessions.reauth.is_none());

        let (mut app, _rx) = signed_out_and_closed();
        let _ = app.update(core(CoreEvent::Devices(vec![])));
        assert!(app.sessions.pending.is_none());
    }

    #[test]
    fn an_account_page_nobody_asked_for_is_ignored() {
        let (mut app, _rx) = sessions_app();
        let task = app.update(core(CoreEvent::OpenAccountManagement(AuthUrl::from(
            "https://account.example.org/devices",
        ))));
        assert_eq!(task.units(), 0);
        assert!(app.sessions.approval.is_none());
    }

    #[test]
    fn the_logout_warning_offers_recovery_or_signing_out_anyway() {
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        while rx.try_recv().is_ok() {}
        let _ = app.update(Message::LogoutRequested);
        assert!(app.sessions.logout_warning);
        assert!(
            drain(&mut rx).is_empty(),
            "nothing is sent before the answer"
        );
        // Cancel closes it.
        let _ = ses(&mut app, SessionsMessage::LogoutCancelled);
        assert!(!app.sessions.logout_warning && drain(&mut rx).is_empty());
        // Set up recovery starts it and does not log out.
        let _ = app.update(Message::LogoutRequested);
        let _ = ses(&mut app, SessionsMessage::LogoutSetupRecovery);
        assert!(!app.sessions.logout_warning);
        let commands = drain(&mut rx);
        assert!(
            matches!(commands.as_slice(), [Command::EnableRecovery]),
            "{commands:?}"
        );
        // Sign out anyway logs out.
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        while rx.try_recv().is_ok() {}
        let _ = app.update(Message::LogoutRequested);
        let _ = ses(&mut app, SessionsMessage::LogoutAnyway);
        assert!(matches!(drain(&mut rx).as_slice(), [Command::Logout]));
        assert!(!app.sessions.logout_warning);
    }

    #[test]
    fn the_answers_of_the_warning_only_count_while_it_is_shown() {
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        while rx.try_recv().is_ok() {}
        let _ = ses(&mut app, SessionsMessage::LogoutAnyway);
        let _ = ses(&mut app, SessionsMessage::LogoutSetupRecovery);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn logout_goes_straight_through_with_recovery_or_another_session() {
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        );
        while rx.try_recv().is_ok() {}
        let _ = app.update(Message::LogoutRequested);
        assert!(matches!(drain(&mut rx).as_slice(), [Command::Logout]));
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        let _ = app.update(core(CoreEvent::Devices(device_list())));
        while rx.try_recv().is_ok() {}
        let _ = app.update(Message::LogoutRequested);
        assert!(matches!(drain(&mut rx).as_slice(), [Command::Logout]));
        assert!(!app.sessions.logout_warning);
    }

    #[test]
    fn logged_out_resets_the_sessions_state() {
        let (mut app, _rx) = reauth_asked();
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert!(!app.sessions.open && app.sessions.devices.is_none());
        assert!(app.sessions.reauth.is_none() && app.sessions.pending.is_none());
    }

    #[test]
    fn device_names_from_the_server_never_reach_the_app_debug_output_unsanitized() {
        let (mut app, _rx) = main_app();
        let evil = summary("D", Some("a\u{202E}b"), false, DeviceTrust::Unverified);
        let _ = app.update(core(CoreEvent::Devices(vec![evil])));
        // The title is what the view draws.
        let list = app.sessions.devices.as_ref().expect("a list");
        assert!(!crate::sessions::device_title(&list[0]).contains('\u{202E}'));
    }

    #[test]
    fn every_sessions_state_builds_a_view() {
        let (mut app, _rx) = sessions_app();
        let _ = app.view();
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let _ = app.view();
        let _ = ses(&mut app, SessionsMessage::SignOutAllAsk);
        let _ = app.view();
        let _ = ses(
            &mut app,
            SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
        );
        let _ = app.view();
        let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::DeleteDevices,
        }));
        let _ = app.view();
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::SignOutDevices,
            error: CoreError::ReauthFailed,
        }));
        let _ = app.view();
        let _ = ses(&mut app, SessionsMessage::ReauthCancelled);
        let _ = oauth_url(&mut app, "https://account.example.org/devices");
        let _ = app.view();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Disabled,
        );
        let _ = app.update(core(CoreEvent::Devices(vec![summary(
            "THIS",
            None,
            true,
            DeviceTrust::Verified,
        )])));
        let _ = app.update(Message::LogoutRequested);
        let _ = app.view();
        let _ = ses(&mut app, SessionsMessage::Close);
        let _ = app.view();
    }

    // ---- Task 14: redaction sweep ----

    /// Asserts none of `sentinels` appears in `text`; the sentinel is named,
    /// the text (which may carry the leak) is not printed.
    fn assert_no_sentinel(context: &str, text: &str, sentinels: &[&str]) {
        for sentinel in sentinels {
            assert!(
                !text.contains(sentinel),
                "{context}: `{sentinel}` reached a Debug string"
            );
        }
    }

    const KEY_S: &str = "secret-key-sweep";
    const INPUT_S: &str = "secret-input-sweep";
    const COPIED_S: &str = "secret-copied-sweep";
    const PW_S: &str = "secret-pw-sweep";
    const EMOJI_S: &str = "secret-emoji-sweep";
    const NAME_S: &str = "secret-name-sweep";
    const LOGIN_PW_S: &str = "secret-login-pw-sweep";
    const URL_S: &str = "secret-url-state-sweep";
    const TEXT_S: &str = "secret-text-sweep";
    const PASTE_S: &str = "secret-paste-sweep";
    const ALL_S: [&str; 10] = [
        KEY_S, INPUT_S, COPIED_S, PW_S, EMOJI_S, NAME_S, LOGIN_PW_S, URL_S, TEXT_S, PASTE_S,
    ];

    /// The messages that carry a secret or typed text, built the way the app
    /// and the core build them: the recovery key and its inputs, the
    /// re-authentication passwords, the login password, the two OAuth URLs
    /// (their `state` is a secret), the QR code, and the text typed or pasted
    /// into the composer, the picker search and a reaction key. Not covered:
    /// messages that carry only IDs, flags or an index.
    fn secret_messages() -> Vec<Message> {
        vec![
            core(CoreEvent::RecoveryKeyReady(RecoveryKey::from(KEY_S))),
            Message::Security(SecurityMessage::RestoreInput(RecoveryKey::from(INPUT_S))),
            Message::Security(SecurityMessage::ClipboardRead(Some(RecoveryKey::from(
                COPIED_S,
            )))),
            Message::Security(SecurityMessage::ClipboardFlushed(
                Some(RecoveryKey::from(COPIED_S)),
                RecoveryKey::from(KEY_S),
            )),
            Message::Security(SecurityMessage::ReauthInput(Password::from(PW_S))),
            Message::Sessions(SessionsMessage::ReauthInput(Password::from(PW_S))),
            core(CoreEvent::Verification {
                id: vid(),
                phase: twicecat_core::api::VerificationPhase::ShowQr(vqr()),
            }),
            Message::PasswordChanged(Password::from(LOGIN_PW_S)),
            core(CoreEvent::OAuthUrl(AuthUrl::from(format!(
                "https://auth.example.org/a?state={URL_S}"
            )))),
            core(CoreEvent::OpenAccountManagement(AuthUrl::from(format!(
                "https://auth.example.org/account?state={URL_S}"
            )))),
            Message::Composer(EditorAction(Action::Edit(Edit::Paste(Arc::new(
                PASTE_S.to_owned(),
            ))))),
            Message::Composer(EditorAction(Action::Edit(Edit::Insert('x')))),
            Message::PickerQuery(MessageText::from(TEXT_S)),
            Message::PickerPicked(MessageText::from(TEXT_S)),
            Message::ReactionToggled {
                item: "item-1".to_owned(),
                key: MessageText::from(TEXT_S),
            },
        ]
    }

    #[test]
    fn the_redaction_sweep_finds_no_secret_in_any_state_or_message() {
        use twicecat_core::api::VerificationPhase;

        // The show-once key screen, with a copy pending.
        let (mut app, _rx) = recovery_app();
        key_ready(&mut app, KEY_S);
        app.security.copied = Some(RecoveryKey::from(COPIED_S));
        assert!(app.security.setup.is_some(), "the key screen is open");
        assert_no_sentinel("key screen", &format!("{app:?}"), &ALL_S);

        // A restore input with typed text.
        let (mut app, _rx) = identity_app();
        let _ = sec(&mut app, SecurityMessage::OpenRestore);
        let _ = sec(
            &mut app,
            SecurityMessage::RestoreInput(RecoveryKey::from(INPUT_S)),
        );
        assert_eq!(app.security.restore_input.expose(), INPUT_S);
        assert_no_sentinel("restore input", &format!("{app:?}"), &ALL_S);

        // The reset re-authentication modal with a typed password.
        let (mut app, _rx) = recovery_app();
        let _ = app.update(core(CoreEvent::ReauthRequired {
            action: ReauthAction::ResetIdentity,
        }));
        let _ = sec(&mut app, SecurityMessage::ReauthInput(Password::from(PW_S)));
        assert!(app.security.reauth.is_some(), "the prompt is open");
        assert_no_sentinel("reset prompt", &format!("{app:?}"), &ALL_S);

        // The Sessions password prompt (the other re-authentication modal).
        let (mut app, _rx) = reauth_asked();
        let _ = ses(&mut app, SessionsMessage::ReauthInput(Password::from(PW_S)));
        assert!(app.sessions.reauth.is_some(), "the prompt is open");
        assert_no_sentinel("sessions prompt", &format!("{app:?}"), &ALL_S);

        // A verification in Emojis, and one in ShowQr.
        let (mut app, _rx) = verifying(true);
        let set = Box::new(std::array::from_fn(|_| SasEmoji {
            symbol: "\u{1F436}".to_owned(),
            description: EMOJI_S.to_owned(),
        }));
        let _ = app.update(core(CoreEvent::Verification {
            id: vid(),
            phase: VerificationPhase::Emojis(set),
        }));
        assert_eq!(
            app.verification.as_ref().map(|v| v.phase.kind()),
            Some("emojis")
        );
        assert_no_sentinel("emojis", &format!("{app:?}"), &ALL_S);

        let (mut app, _rx) = verifying(true);
        let _ = app.update(core(CoreEvent::Verification {
            id: vid(),
            phase: VerificationPhase::ShowQr(vqr()),
        }));
        assert_eq!(
            app.verification.as_ref().map(|v| v.phase.kind()),
            Some("show_qr")
        );
        // The test grid is all dark, so any dump of the modules repeats `true`.
        let dbg = format!("{app:?}");
        assert!(
            !dbg.contains("true, true") && !dbg.contains("dark"),
            "QR modules printed"
        );

        // The Sessions screen, with a rename in progress.
        let (mut app, _rx) = main_app();
        let _ = ses(&mut app, SessionsMessage::Open);
        let _ = app.update(core(CoreEvent::Devices(vec![
            summary("THIS", Some(NAME_S), true, DeviceTrust::Verified),
            summary("PHONE", Some(NAME_S), false, DeviceTrust::Unverified),
        ])));
        let _ = ses(
            &mut app,
            SessionsMessage::RenameStart(DeviceId::from("PHONE")),
        );
        let _ = ses(&mut app, SessionsMessage::RenameInput(NAME_S.to_owned()));
        assert!(app.sessions.open && app.sessions.rename.is_some());
        assert_no_sentinel("sessions", &format!("{app:?}"), &ALL_S);

        // The login form with a typed password, then waiting for the browser
        // with the OAuth URL (and its `state`) held for **Copy link**.
        let (mut app, _rx) = login_app();
        let _ = app.update(Message::PasswordChanged(Password::from(LOGIN_PW_S)));
        assert_no_sentinel("login form", &format!("{app:?}"), &ALL_S);
        let (app, _rx, _task) = waiting_app(&format!("https://auth.example.org/a?state={URL_S}"));
        assert!(app.browser_link.is_some(), "the link is held");
        assert_no_sentinel("waiting for the browser", &format!("{app:?}"), &ALL_S);

        // The composer with pasted text, and the picker with a search typed.
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::Composer(EditorAction(Action::Edit(Edit::Paste(
            Arc::new(PASTE_S.to_owned()),
        )))));
        let _ = app.update(Message::PickerQuery(MessageText::from(TEXT_S)));
        assert_no_sentinel("composer and picker", &format!("{app:?}"), &ALL_S);

        // The messages that carry secrets.
        for message in secret_messages() {
            let dbg = format!("{message:?}");
            assert_no_sentinel("message", &dbg, &ALL_S);
            assert!(!dbg.contains("true, true"), "QR modules printed");
        }
    }

    /// Lays `app.view()` out headlessly in a 1000x700 window and returns the
    /// size of the root.
    fn laid_out_size(app: &App) -> iced::Size {
        use iced::advanced::layout::Limits;
        use iced::advanced::renderer::Headless;
        use iced::advanced::widget::Tree;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let renderer = runtime
            .block_on(iced::Renderer::new(
                iced::Font::DEFAULT,
                iced::Pixels(16.0),
                Some("tiny-skia"),
            ))
            .expect("the headless tiny-skia renderer is not available");
        let mut element = app.view();
        let mut tree = Tree::new(element.as_widget());
        element
            .as_widget_mut()
            .layout(
                &mut tree,
                &renderer,
                &Limits::new(iced::Size::ZERO, iced::Size::new(1000.0, 700.0)),
            )
            .size()
    }

    /// The main screen collapsed to zero width (and the security card over
    /// it with it) because its column did not fill the window: the window
    /// went blank after a fresh login.
    #[test]
    fn the_main_view_and_the_security_cards_fill_the_window() {
        let window = iced::Size::new(1000.0, 700.0);
        let (mut app, _rx) = main_app();
        assert_eq!(laid_out_size(&app), window, "the main view");
        set_status(
            &mut app,
            DeviceVerification::Unverified,
            RecoveryStatus::Enabled,
        );
        assert_eq!(laid_out_size(&app), window, "the identity screen");
        let _ = sec(&mut app, SecurityMessage::SkipIdentity);
        assert_eq!(laid_out_size(&app), window, "the unverified banner");
        let (mut app, _rx) = recovery_app();
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        assert_eq!(laid_out_size(&app), window, "setting up recovery");
    }

    /// The Voice & audio card is a modal over the main view: the window
    /// keeps its size with every section shown.
    #[test]
    fn the_voice_and_audio_card_keeps_the_window_filled() {
        use twicecat_core::api::{AudioSettings, NoiseSuppression};
        let (mut app, _rx) = main_app();
        let mut settings = AudioSettings::default();
        settings.processing.noise_suppression = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 40.0,
            gpu: false,
        };
        settings.remote_cleaning.enabled = true;
        let _ = app.update(core(CoreEvent::AudioSettings(settings)));
        let _ = app.update(Message::Audio(crate::audio_settings::AudioMessage::Open));
        let _ = app.update(Message::Audio(
            crate::audio_settings::AudioMessage::Diagnostics,
        ));
        assert_eq!(laid_out_size(&app), iced::Size::new(1000.0, 700.0));
    }

    // ---- Task 15: change the recovery key ----

    /// A verified app with recovery on and the Sessions page open, with the
    /// commands drained.
    fn change_key_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        set_status(
            &mut app,
            DeviceVerification::Verified,
            RecoveryStatus::Enabled,
        );
        let _ = ses(&mut app, SessionsMessage::Open);
        while rx.try_recv().is_ok() {}
        (app, rx)
    }

    #[test]
    fn change_recovery_key_asks_first_and_cancel_sends_nothing() {
        let (mut app, mut rx) = change_key_app();
        let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
        assert_eq!(
            app.security.visible(),
            Visible::Overlay(Overlay::ChangeKeyConfirm)
        );
        let _ = sec(&mut app, SecurityMessage::ChangeKeyCancelled);
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert!(app.sessions.open, "back on the Sessions page");
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn change_key_sends_one_command_and_nothing_while_busy() {
        let (mut app, mut rx) = change_key_app();
        let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
        let _ = sec(&mut app, SecurityMessage::ChangeKeyConfirmed);
        let _ = sec(&mut app, SecurityMessage::ChangeKeyConfirmed);
        let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        assert!(matches!(commands[0], Command::ChangeRecoveryKey));
        assert_eq!(app.security.busy, Some(Busy::ChangingKey));
        assert_eq!(
            app.security.visible(),
            Visible::Overlay(Overlay::ChangeKeyConfirm),
            "the card stays, showing the wait"
        );
        // Cancel does nothing once the command is out.
        let _ = sec(&mut app, SecurityMessage::ChangeKeyCancelled);
        assert_eq!(app.security.busy, Some(Busy::ChangingKey));
    }

    #[test]
    fn change_key_is_refused_unless_recovery_is_on() {
        for recovery in [
            RecoveryStatus::Disabled,
            RecoveryStatus::Incomplete,
            RecoveryStatus::Unknown,
        ] {
            let (mut app, mut rx) = change_key_app();
            set_status(&mut app, DeviceVerification::Verified, recovery);
            let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
            let _ = sec(&mut app, SecurityMessage::ChangeKeyConfirmed);
            assert_eq!(app.security.visible(), Visible::Nothing, "{recovery:?}");
            assert!(drain(&mut rx).is_empty(), "{recovery:?}");
        }
    }

    #[test]
    fn the_new_key_opens_the_show_once_screen_as_a_new_key() {
        let (mut app, _rx) = change_key_app();
        let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
        let _ = sec(&mut app, SecurityMessage::ChangeKeyConfirmed);
        key_ready(&mut app, "secret-key-x");
        assert_eq!(app.security.visible(), Visible::Key);
        assert_eq!(app.security.busy, None);
        let setup = app.security.setup.as_ref().expect("the key screen");
        assert_eq!(crate::security::key_title(setup), "Your new recovery key");
        let _ = sec(&mut app, SecurityMessage::Done);
        assert_eq!(app.security.visible(), Visible::Key, "Done needs the box");
        let _ = sec(&mut app, SecurityMessage::RevealToggled(true));
        let _ = sec(&mut app, SecurityMessage::SavedToggled(true));
        let _ = sec(&mut app, SecurityMessage::Done);
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert!(app.sessions.open, "back on the Sessions page");
    }

    #[test]
    fn a_key_from_set_up_keeps_the_first_title() {
        let (mut app, _rx) = recovery_app();
        let _ = sec(&mut app, SecurityMessage::SetupRecovery);
        key_ready(&mut app, "secret-key-x");
        let setup = app.security.setup.as_ref().expect("the key screen");
        assert_eq!(crate::security::key_title(setup), "Your recovery key");
    }

    #[test]
    fn a_failed_change_shows_the_reason_on_the_sessions_page() {
        let (mut app, _rx) = change_key_app();
        let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
        let _ = sec(&mut app, SecurityMessage::ChangeKeyConfirmed);
        let _ = app.update(recovery_failed(CoreError::Internal(
            "recovery is not complete on this device".into(),
        )));
        assert_eq!(app.security.busy, None);
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert_eq!(app.security.overlay, None);
        let notice = app.sessions.notice.clone().expect("a Sessions notice");
        assert!(notice.contains("not complete"), "{notice}");
    }

    #[test]
    fn the_change_key_card_fills_the_window_and_hides_no_secret() {
        let window = iced::Size::new(1000.0, 700.0);
        let (mut app, _rx) = change_key_app();
        let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
        assert_eq!(laid_out_size(&app), window, "the warning");
        let _ = sec(&mut app, SecurityMessage::ChangeKeyConfirmed);
        assert_eq!(laid_out_size(&app), window, "the wait");
        key_ready(&mut app, KEY_S);
        assert_eq!(laid_out_size(&app), window, "the new key");
        assert_no_sentinel("new key screen", &format!("{app:?}"), &ALL_S);
    }

    // ---- Phase 5 Task 9: media in the timeline ----

    use std::path::PathBuf;

    use twicecat_core::api::{MediaBytes, MediaContent, MediaKind, MediaSize};

    use crate::dialogs::RecordingDialogs;
    use crate::media::{self, Decoded, SavePath, Thumb};

    const FILE_SENTINEL: &str = "sentinel-file-name-x";

    fn media_item(id: &str, kind: MediaKind, filename: &str, w: u32, h: u32) -> TimelineItem {
        let mut content = MediaContent::new(kind, filename.to_owned());
        content.width = Some(w);
        content.height = Some(h);
        content.size = Some(2048);
        TimelineItem::new(
            id.to_owned(),
            TimelineItemKind::Message {
                sender: UserId::from("@them:x"),
                body: filename.to_owned(),
                is_own: false,
                timestamp_ms: 1,
                sent: SendStatus::Sent,
                details: Box::new(MessageDetails {
                    media: Some(Box::new(content)),
                    ..MessageDetails::default()
                }),
            },
        )
    }

    fn room_a() -> RoomId {
        RoomId::from("!a:x")
    }

    fn key_of(id: &str) -> media::Key {
        (room_a(), id.to_owned())
    }

    /// `main_app` with `count` images `img0` ... in room `a`, and the fake
    /// dialogs installed.
    fn media_app(count: usize) -> (App, mpsc::Receiver<Command>, RecordingDialogs) {
        let (mut app, mut rx) = main_app();
        let dialogs = RecordingDialogs::default();
        app.set_dialogs(Box::new(dialogs.clone()));
        let values = (0..count)
            .map(|n| media_item(&format!("img{n}"), MediaKind::Image, "pic.png", 640, 480))
            .collect();
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append { values }],
        }));
        let _ = drain(&mut rx);
        (app, rx, dialogs)
    }

    fn fetches(commands: &[Command], wanted: MediaSize) -> Vec<String> {
        commands
            .iter()
            .filter_map(|c| match c {
                Command::FetchMedia { item_id, size, .. } if *size == wanted => {
                    Some(item_id.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn png(width: u32, height: u32) -> MediaBytes {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png)
            .expect("encode");
        MediaBytes::from(out.into_inner())
    }

    fn media_event(id: &str, size: MediaSize, result: Result<MediaBytes, CoreError>) -> Message {
        core(CoreEvent::Media {
            room_id: room_a(),
            item_id: id.to_owned(),
            size,
            result,
        })
    }

    fn decoded(width: u32, height: u32) -> Decoded {
        let (handle, w, h) =
            media::decode_media(png(width, height).as_bytes(), media::decode_limits())
                .expect("decodes");
        Decoded {
            handle,
            width: w,
            height: h,
        }
    }

    fn arrive(app: &mut App, id: &str, d: Option<Decoded>) {
        let _ = app.update(Message::MediaDecoded {
            room_id: room_a(),
            item_id: id.to_owned(),
            size: MediaSize::Thumbnail,
            decoded: d,
        });
    }

    #[test]
    fn only_rows_in_the_window_plus_one_screen_ask_for_a_thumbnail() {
        let (mut app, mut rx) = main_app();
        let values = (0..60)
            .map(|n| media_item(&format!("img{n}"), MediaKind::Image, "pic.png", 640, 480))
            .collect();
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append { values }],
        }));
        let asked = fetches(&drain(&mut rx), MediaSize::Thumbnail);
        assert!(asked.contains(&"img59".to_owned()), "{asked:?}");
        assert!(!asked.contains(&"img0".to_owned()), "{asked:?}");
        assert!(asked.len() < 20, "{} requests", asked.len());
        let unique: std::collections::HashSet<_> = asked.iter().collect();
        assert_eq!(unique.len(), asked.len(), "each at most once");
    }

    #[test]
    fn a_thumbnail_is_requested_once_while_it_is_pending_or_cached() {
        let (mut app, mut rx, _) = media_app(3);
        // The diff already asked once. Anything that runs the effects again
        // (a measured row, a scroll) must not ask again.
        let _ = app.update(Message::RowMeasured {
            id: "img2".into(),
            height: 230.0,
        });
        let _ = app.on_timeline_scrolled(0.0, 600.0, 800.0);
        assert!(fetches(&drain(&mut rx), MediaSize::Thumbnail).is_empty());
        arrive(&mut app, "img2", Some(decoded(4, 3)));
        let _ = app.on_timeline_scrolled(0.0, 600.0, 800.0);
        assert!(fetches(&drain(&mut rx), MediaSize::Thumbnail).is_empty());
        assert!(matches!(
            app.media.thumb(&key_of("img2")),
            Some(Thumb::Ready(_))
        ));
    }

    #[test]
    fn scrolling_up_asks_for_the_rows_that_come_into_range() {
        let (mut app, mut rx) = main_app();
        let values = (0..60)
            .map(|n| media_item(&format!("img{n}"), MediaKind::Image, "pic.png", 640, 480))
            .collect();
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append { values }],
        }));
        let _ = drain(&mut rx);
        let _ = app.on_timeline_scrolled(5000.0, 600.0, 800.0);
        let asked = fetches(&drain(&mut rx), MediaSize::Thumbnail);
        assert!(!asked.is_empty(), "new rows came into range");
        assert!(!asked.contains(&"img59".to_owned()), "{asked:?}");
    }

    #[test]
    fn a_non_image_and_a_text_message_never_ask_for_a_thumbnail() {
        let (mut app, mut rx) = main_app();
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append {
                values: vec![
                    media_item("f", MediaKind::File, "a.zip", 0, 0),
                    media_item("v", MediaKind::Video, "a.mp4", 10, 10),
                    media_item("a", MediaKind::Audio, "a.ogg", 0, 0),
                    message_item("t", "hello", false, ALL_ACTIONS),
                ],
            }],
        }));
        assert!(fetches(&drain(&mut rx), MediaSize::Thumbnail).is_empty());
    }

    #[test]
    fn an_arriving_thumbnail_changes_no_row_height() {
        let (mut app, _rx, _) = media_app(4);
        for n in 0..4 {
            let _ = app.update(Message::RowMeasured {
                id: format!("img{n}"),
                height: 233.0,
            });
        }
        let before = (app.timeline.prefix.clone(), app.timeline.measured.clone());
        let _ = app.update(media_event("img3", MediaSize::Thumbnail, Ok(png(8, 6))));
        arrive(&mut app, "img3", Some(decoded(8, 6)));
        arrive(&mut app, "img2", None);
        assert_eq!(app.timeline.prefix, before.0);
        assert_eq!(app.timeline.measured, before.1);
        // The estimate of an unmeasured row does not read the cache either.
        let item = app.timeline.items[3].clone();
        let est =
            timeline::estimate_height(&item, false, 800.0, crate::ui::tokens::Density::Comfortable);
        arrive(&mut app, "img3", Some(decoded(8, 6)));
        assert_eq!(
            timeline::estimate_height(&item, false, 800.0, crate::ui::tokens::Density::Comfortable),
            est
        );
    }

    #[test]
    fn the_bytes_are_decoded_in_a_task_not_in_update() {
        let (mut app, _rx, _) = media_app(1);
        let _ = app.update(media_event("img0", MediaSize::Thumbnail, Ok(png(8, 6))));
        assert!(
            app.media.thumb(&key_of("img0")).is_none(),
            "nothing is cached until the decode task answers"
        );
        arrive(&mut app, "img0", Some(decoded(8, 6)));
        assert!(app.media.thumb(&key_of("img0")).is_some());
    }

    #[test]
    fn a_failed_or_undecodable_thumbnail_is_unavailable_and_not_retried() {
        let (mut app, mut rx, _) = media_app(2);
        let _ = app.update(media_event(
            "img0",
            MediaSize::Thumbnail,
            Err(CoreError::Internal("no".into())),
        ));
        arrive(&mut app, "img1", None);
        assert!(matches!(
            app.media.thumb(&key_of("img0")),
            Some(Thumb::Unavailable)
        ));
        assert!(matches!(
            app.media.thumb(&key_of("img1")),
            Some(Thumb::Unavailable)
        ));
        let _ = app.on_timeline_scrolled(0.0, 600.0, 800.0);
        assert!(fetches(&drain(&mut rx), MediaSize::Thumbnail).is_empty());
    }

    #[test]
    fn an_answer_nobody_asked_for_is_dropped() {
        let (mut app, _rx, _) = media_app(1);
        arrive(&mut app, "stranger", Some(decoded(2, 2)));
        assert!(app.media.thumb(&key_of("stranger")).is_none());
        let task = app.update(media_event("stranger", MediaSize::Thumbnail, Ok(png(2, 2))));
        drop(task);
        assert!(app.media.thumb(&key_of("stranger")).is_none());
        // A full-size answer with no viewer open is dropped too.
        let _ = app.update(media_event("img0", MediaSize::Full, Ok(png(2, 2))));
        assert!(app.media.viewer.is_none());
    }

    #[test]
    fn clicking_an_image_opens_the_viewer_and_asks_for_the_full_file() {
        let (mut app, mut rx, _) = media_app(1);
        let _ = app.update(Message::MediaClicked("img0".into()));
        assert!(app.media.viewer.is_some());
        let commands = drain(&mut rx);
        assert_eq!(fetches(&commands, MediaSize::Full), vec!["img0".to_owned()]);
        // The full image arrives, is decoded, and is shown.
        let _ = app.update(media_event("img0", MediaSize::Full, Ok(png(9, 7))));
        let _ = app.update(Message::MediaDecoded {
            room_id: room_a(),
            item_id: "img0".into(),
            size: MediaSize::Full,
            decoded: Some(decoded(9, 7)),
        });
        let viewer = app.media.viewer.as_ref().expect("viewer");
        assert!(viewer.decoded().is_some());
        // Close drops the full image; a late answer does not bring it back.
        let _ = app.update(Message::ViewerClosed);
        assert!(app.media.viewer.is_none());
        let _ = app.update(Message::MediaDecoded {
            room_id: room_a(),
            item_id: "img0".into(),
            size: MediaSize::Full,
            decoded: Some(decoded(9, 7)),
        });
        assert!(app.media.viewer.is_none());
    }

    #[test]
    fn only_an_image_opens_the_viewer() {
        let (mut app, mut rx, _) = media_app(0);
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append {
                values: vec![
                    media_item("f", MediaKind::File, "a.zip", 0, 0),
                    message_item("t", "hello", false, ALL_ACTIONS),
                ],
            }],
        }));
        for id in ["f", "t", "missing"] {
            let _ = app.update(Message::MediaClicked(id.into()));
            assert!(app.media.viewer.is_none(), "{id}");
        }
        assert!(fetches(&drain(&mut rx), MediaSize::Full).is_empty());
    }

    #[test]
    fn a_failed_full_fetch_shows_the_failure_in_the_viewer() {
        let (mut app, _rx, _) = media_app(1);
        let _ = app.update(Message::MediaClicked("img0".into()));
        let _ = app.update(media_event(
            "img0",
            MediaSize::Full,
            Err(CoreError::Internal("too big".into())),
        ));
        let viewer = app.media.viewer.as_ref().expect("viewer");
        assert!(viewer.failed());
        assert!(viewer.decoded().is_none());
    }

    #[test]
    fn leaving_the_room_closes_the_viewer() {
        let (mut app, _rx, _) = media_app(1);
        let _ = app.update(Message::MediaClicked("img0".into()));
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert!(app.media.viewer.is_none());
    }

    fn file_app() -> (App, mpsc::Receiver<Command>, RecordingDialogs) {
        let (mut app, mut rx, dialogs) = media_app(0);
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append {
                values: vec![
                    media_item("doc", MediaKind::File, "../../report.pdf", 0, 0),
                    media_item("prog", MediaKind::File, "setup.exe", 0, 0),
                    media_item("bare", MediaKind::File, "README", 0, 0),
                ],
            }],
        }));
        let _ = drain(&mut rx);
        (app, rx, dialogs)
    }

    /// An absolute path on every platform: a bare `/home/..` has no drive
    /// letter, so it is relative on Windows.
    fn abs(path: &str) -> String {
        if cfg!(windows) {
            format!("C:{path}")
        } else {
            path.to_owned()
        }
    }

    fn picked(id: &str, path: Option<&str>) -> Message {
        Message::SavePicked {
            room_id: room_a(),
            item_id: id.to_owned(),
            path: path.map(|p| SavePath::from(PathBuf::from(p))),
        }
    }

    #[test]
    fn save_proposes_a_sanitised_name_and_sends_the_path_the_user_picked() {
        let (mut app, mut rx, dialogs) = file_app();
        let _ = app.update(Message::SaveRequested("doc".into()));
        assert_eq!(
            *dialogs.asked.lock().expect("mutex"),
            vec!["report.pdf".to_owned()]
        );
        assert!(
            drain(&mut rx).is_empty(),
            "nothing is saved before the pick"
        );
        let _ = app.update(picked("doc", Some(&abs("/home/u/Documents/mine.pdf"))));
        let commands = drain(&mut rx);
        assert_eq!(commands.len(), 1, "{commands:?}");
        match &commands[0] {
            Command::SaveMedia {
                room_id,
                item_id,
                path,
            } => {
                assert_eq!(room_id, &room_a());
                assert_eq!(item_id, "doc");
                assert_eq!(path, &PathBuf::from(abs("/home/u/Documents/mine.pdf")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn save_adds_the_sent_time_to_a_generic_name() {
        let (mut app, mut rx, dialogs) = media_app(0);
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append {
                values: vec![media_item("pasted", MediaKind::Image, "image.png", 4, 4)],
            }],
        }));
        let _ = drain(&mut rx);
        let _ = app.update(Message::SaveRequested("pasted".into()));
        // `media_item` sends at 1 ms after the epoch, shown in local time.
        let sent = chrono::TimeZone::timestamp_millis_opt(&chrono::Local, 1).unwrap();
        assert_eq!(
            *dialogs.asked.lock().expect("mutex"),
            vec![format!("image-{}.png", sent.format("%Y%m%d-%H%M%S"))]
        );
    }

    #[test]
    fn a_cancelled_or_unusable_pick_saves_nothing() {
        let (mut app, mut rx, _) = file_app();
        for path in [
            None,
            Some("relative/file.pdf"),
            Some("file.pdf"),
            Some("/"),
            Some(""),
            Some("/home/u/.."),
        ] {
            let _ = app.update(picked("doc", path));
            assert!(drain(&mut rx).is_empty(), "{path:?}");
        }
    }

    #[test]
    fn a_pick_for_an_item_that_is_not_media_or_in_another_room_saves_nothing() {
        let (mut app, mut rx, dialogs) = file_app();
        let _ = app.update(Message::SaveRequested("missing".into()));
        assert!(dialogs.asked.lock().expect("mutex").is_empty());
        let _ = app.update(picked("missing", Some("/tmp/x.bin")));
        let _ = app.update(Message::SavePicked {
            room_id: RoomId::from("!b:x"),
            item_id: "doc".into(),
            path: Some(SavePath::from(PathBuf::from("/tmp/x.bin"))),
        });
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn open_is_offered_for_any_file_once_it_is_saved() {
        let (mut app, mut rx, _) = file_app();
        let doc = key_of("doc");
        assert!(!app.media.open_allowed(&doc));
        let _ = app.update(picked("doc", Some(&abs("/home/u/mine.pdf"))));
        assert!(!app.media.open_allowed(&doc), "not saved yet");
        let _ = app.update(core(CoreEvent::MediaSaved {
            room_id: room_a(),
            item_id: "doc".into(),
        }));
        assert!(app.media.open_allowed(&doc));
        // Programs and names without an extension too: the bar asks first.
        for (id, path) in [("prog", "/home/u/setup.exe"), ("bare", "/home/u/README")] {
            let _ = app.update(picked(id, Some(&abs(path))));
            let _ = app.update(core(CoreEvent::MediaSaved {
                room_id: room_a(),
                item_id: id.into(),
            }));
            assert!(app.media.open_allowed(&key_of(id)), "{id}");
        }
        let _ = drain(&mut rx);
    }

    fn saved_and_asked(app: &mut App, id: &str, path: &str) -> Option<(String, Option<String>)> {
        let _ = app.update(picked(id, Some(&abs(path))));
        let _ = app.update(core(CoreEvent::MediaSaved {
            room_id: room_a(),
            item_id: id.into(),
        }));
        let _ = app.update(Message::OpenRequested(id.into()));
        let bar = app.media.open_bar.as_ref()?;
        Some((bar.question(), bar.warning().map(str::to_owned)))
    }

    #[test]
    fn open_asks_first_and_warns_before_a_program() {
        let (mut app, _rx, _) = file_app();
        let (question, warning) =
            saved_and_asked(&mut app, "doc", "/home/u/mine.pdf").expect("bar");
        assert_eq!(question, "Open mine.pdf?");
        assert_eq!(warning, None, "a document gets the plain question");
        let _ = app.update(Message::OpenDismissed);
        assert!(app.media.open_bar.is_none());
        // The sender named it a program.
        let (question, warning) =
            saved_and_asked(&mut app, "prog", "/home/u/setup.exe").expect("bar");
        assert_eq!(question, "Open setup.exe?");
        assert_eq!(
            warning.as_deref(),
            Some(
                "This file can run a program on your computer. Open it only if you trust the sender."
            )
        );
        // A document saved under a program's name warns too.
        let (_, warning) = saved_and_asked(&mut app, "doc", "/home/u/innocent.sh").expect("bar");
        assert!(warning.is_some(), "the saved name decides as well");
        // Never saved: no bar.
        let _ = app.update(Message::OpenDismissed);
        let _ = app.update(Message::OpenRequested("bare".into()));
        assert!(app.media.open_bar.is_none());
    }

    #[test]
    fn the_open_question_keeps_the_end_of_a_long_file_name() {
        let (mut app, _rx, _) = file_app();
        let long = format!("/home/u/{}.pdf.exe", "a".repeat(200));
        let (question, warning) = saved_and_asked(&mut app, "doc", &long).expect("bar");
        assert!(
            question.ends_with(".pdf.exe?"),
            "the extension is what tells a file from a document: {question}"
        );
        assert!(question.contains('\u{2026}'), "cut in the middle");
        assert!(warning.is_some());
    }

    #[test]
    fn a_failed_save_shows_a_notice_and_forgets_the_pending_path() {
        let (mut app, _rx, _) = file_app();
        let _ = app.update(picked("doc", Some(&abs("/home/u/mine.pdf"))));
        let _ = app.update(core(CoreEvent::ActionFailed {
            room_id: room_a(),
            action: ActionKind::Media,
            error: CoreError::Internal("disk full".into()),
        }));
        assert!(app.actions.notice.is_some());
        let _ = app.update(core(CoreEvent::MediaSaved {
            room_id: room_a(),
            item_id: "doc".into(),
        }));
        assert!(
            !app.media.open_allowed(&key_of("doc")),
            "a late success for a save that was reported failed opens nothing"
        );
    }

    #[test]
    fn debug_of_the_app_and_its_media_messages_shows_no_bytes_or_names() {
        let (mut app, mut rx, _) = media_app(0);
        let _ = app.update(core(CoreEvent::TimelineDiff {
            room_id: room_a(),
            diffs: vec![ListDiff::Append {
                values: vec![
                    media_item("pic", MediaKind::Image, FILE_SENTINEL, 640, 480),
                    media_item("doc", MediaKind::File, FILE_SENTINEL, 0, 0),
                ],
            }],
        }));
        let _ = drain(&mut rx);
        arrive(&mut app, "pic", Some(decoded(8, 6)));
        let _ = app.update(Message::MediaClicked("pic".into()));
        let _ = app.update(media_event("pic", MediaSize::Full, Ok(png(9, 7))));
        let _ = app.update(Message::MediaDecoded {
            room_id: room_a(),
            item_id: "pic".into(),
            size: MediaSize::Full,
            decoded: Some(decoded(9, 7)),
        });
        let _ = app.update(picked("doc", Some("/home/u/sentinel-file-name-x")));
        let _ = app.update(core(CoreEvent::MediaSaved {
            room_id: room_a(),
            item_id: "doc".into(),
        }));
        let _ = app.update(Message::OpenRequested("doc".into()));
        assert!(app.media.viewer.is_some());
        let dbg = format!("{app:?}");
        assert!(!dbg.contains(FILE_SENTINEL), "{dbg}");
        assert!(!dbg.contains("/home/u"), "{dbg}");
        assert!(dbg.contains("media"), "the app Debug still reports media");
        let message = Message::MediaDecoded {
            room_id: room_a(),
            item_id: "x".into(),
            size: MediaSize::Full,
            decoded: Some(decoded(9, 7)),
        };
        let dbg = format!(
            "{message:?} {:?}",
            picked("doc", Some("/home/u/sentinel-file-name-x"))
        );
        assert!(
            !dbg.contains(FILE_SENTINEL) && !dbg.contains("/home/u"),
            "{dbg}"
        );
        assert!(!dbg.contains("pixels"), "{dbg}");
    }

    /// The tags of the first-child chain of a laid-out tree.
    fn tag_path(app: &App) -> Vec<iced::advanced::widget::tree::Tag> {
        use iced::advanced::widget::Tree;
        let element = app.view();
        let tree = Tree::new(element.as_widget());
        let mut tags = vec![tree.tag];
        let mut node = &tree;
        while let Some(child) = node.children.first() {
            tags.push(child.tag);
            node = child;
        }
        tags
    }

    #[test]
    fn opening_the_viewer_keeps_the_widget_tree_under_it() {
        let (mut app, _rx, _) = media_app(2);
        let closed = tag_path(&app);
        let _ = app.update(Message::MediaClicked("img0".into()));
        assert_eq!(tag_path(&app), closed, "the base keeps its place");
        let _ = app.update(Message::ViewerClosed);
        assert_eq!(tag_path(&app), closed);
    }

    #[test]
    fn logging_out_forgets_the_media() {
        let (mut app, _rx, _) = media_app(2);
        arrive(&mut app, "img1", Some(decoded(8, 6)));
        let _ = app.update(Message::MediaClicked("img0".into()));
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert!(app.media.viewer.is_none());
        assert!(app.media.thumb(&key_of("img1")).is_none());
    }

    // ---- Task 10: uploads ----

    mod uploads {
        use std::path::PathBuf;
        use std::sync::Arc;

        use twicecat_core::api::{
            ActionKind, AttachmentData, MediaContent, MediaKind, MessageText, UploadProgress,
        };

        use super::*;
        use crate::dialogs::{ClipboardContent, RecordingClipboard, RecordingDialogs};
        use crate::upload::{self, EntryState, Outcome, Refusal, TooLarge, UploadPaths};

        fn rig() -> (
            App,
            mpsc::Receiver<Command>,
            RecordingDialogs,
            RecordingClipboard,
        ) {
            let (mut app, rx) = main_app();
            let dialogs = RecordingDialogs::default();
            let clipboard = RecordingClipboard::default();
            app.set_dialogs(Box::new(dialogs.clone()));
            app.set_clipboard(Box::new(clipboard.clone()));
            (app, rx, dialogs, clipboard)
        }

        fn paths(names: &[&str]) -> UploadPaths {
            UploadPaths(
                names
                    .iter()
                    .map(|n| PathBuf::from(format!("/x/{n}")))
                    .collect(),
            )
        }

        fn outcome_for(name: &str, bytes: Vec<u8>, limit: Option<u64>) -> Outcome {
            upload::prepare_bytes(name.to_owned(), bytes, limit)
        }

        fn prepared(entry: u64, name: &str, bytes: Vec<u8>) -> Message {
            Message::UploadPrepared {
                entry,
                outcome: Box::new(outcome_for(name, bytes, None)),
            }
        }

        fn attachments(commands: &[Command]) -> Vec<&twicecat_core::api::OutgoingAttachment> {
            commands
                .iter()
                .filter_map(|c| match c {
                    Command::SendAttachment(a) => Some(a),
                    _ => None,
                })
                .collect()
        }

        #[test]
        fn attach_opens_the_picker_only_with_a_room() {
            let (mut app, _rx, dialogs, _) = rig();
            let _ = app.update(Message::AttachRequested);
            assert_eq!(*dialogs.picked.lock().expect("mutex"), 1);
            let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
            app.deselect();
            let _ = app.update(Message::AttachRequested);
            assert_eq!(
                *dialogs.picked.lock().expect("mutex"),
                1,
                "no room, no dialog"
            );
        }

        #[test]
        fn chosen_files_open_one_confirmation_in_order() {
            let (mut app, _rx, _, _) = rig();
            let task = app.update(Message::FilesChosen(paths(&["b.txt", "a.txt"])));
            assert!(task.units() >= 2, "one read per file, in tasks");
            let dialog = app.uploads.dialog.as_ref().expect("a confirmation");
            assert_eq!(dialog.room_id, room_a());
            let names: Vec<_> = dialog.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, ["b.txt", "a.txt"]);
            assert!(dialog.loading());
            assert!(!dialog.can_send());
        }

        #[test]
        fn an_empty_answer_opens_nothing() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(Message::FilesChosen(UploadPaths(Vec::new())));
            assert!(app.uploads.dialog.is_none());
        }

        #[test]
        fn at_most_twenty_files_are_taken() {
            let (mut app, _rx, _, _) = rig();
            let many: Vec<String> = (0..30).map(|n| format!("f{n}.txt")).collect();
            let many: Vec<&str> = many.iter().map(String::as_str).collect();
            let _ = app.update(Message::FilesChosen(paths(&many)));
            let dialog = app.uploads.dialog.as_ref().expect("dialog");
            assert_eq!(dialog.entries.len(), upload::MAX_FILES);
            let _ = app.update(Message::FilesChosen(paths(&["more.txt"])));
            let dialog = app.uploads.dialog.as_ref().expect("dialog");
            assert_eq!(dialog.entries.len(), upload::MAX_FILES, "the cap holds");
        }

        #[test]
        fn names_are_base_names_without_separators_or_controls() {
            let (mut app, _rx, _, _) = rig();
            let evil = UploadPaths(vec![PathBuf::from("/tmp/dir/sub/na\u{202E}me\n.txt")]);
            let _ = app.update(Message::FilesChosen(evil));
            let dialog = app.uploads.dialog.as_ref().expect("dialog");
            assert_eq!(dialog.entries[0].name, "name.txt");
        }

        #[test]
        fn prepared_files_fill_their_lines_and_unlock_send() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(Message::FilesChosen(paths(&["a.txt", "b.txt"])));
            let ids: Vec<u64> = app
                .uploads
                .dialog
                .as_ref()
                .expect("dialog")
                .entries
                .iter()
                .map(|e| e.id)
                .collect();
            let _ = app.update(prepared(ids[1], "b.txt", b"bee".to_vec()));
            assert!(
                !app.uploads.dialog.as_ref().expect("dialog").can_send(),
                "a.txt still loads"
            );
            let _ = app.update(prepared(ids[0], "a.txt", b"ay".to_vec()));
            let dialog = app.uploads.dialog.as_ref().expect("dialog");
            assert!(dialog.can_send());
            assert_eq!(dialog.sendable(), 2);
        }

        #[test]
        fn a_late_answer_for_a_closed_confirmation_is_ignored() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(Message::FilesChosen(paths(&["a.txt"])));
            let id = app.uploads.dialog.as_ref().expect("dialog").entries[0].id;
            let _ = app.update(Message::UploadCancelled);
            let _ = app.update(prepared(id, "a.txt", b"x".to_vec()));
            assert!(app.uploads.dialog.is_none());
        }

        #[test]
        fn over_limit_files_are_refused_with_the_limit_shown() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(core(CoreEvent::MediaLimits {
                max_upload_bytes: Some(10),
            }));
            assert_eq!(app.uploads.max_upload_bytes, Some(10));
            let refused = outcome_for("big.bin", vec![0; 11], Some(10));
            assert!(matches!(
                refused,
                Outcome::Refused(Refusal::TooLarge(TooLarge { limit: 10 }))
            ));
            let _ = app.update(Message::FilesChosen(paths(&["big.bin"])));
            let id = app.uploads.dialog.as_ref().expect("dialog").entries[0].id;
            let _ = app.update(Message::UploadPrepared {
                entry: id,
                outcome: Box::new(refused),
            });
            let dialog = app.uploads.dialog.as_ref().expect("dialog");
            assert!(matches!(
                dialog.entries[0].state,
                EntryState::Refused(Refusal::TooLarge(_))
            ));
            assert!(!dialog.can_send());
            assert!(
                Refusal::TooLarge(TooLarge { limit: 10_000_000 })
                    .text()
                    .contains("10.0 MB")
            );
        }

        #[test]
        fn hovering_shows_the_drop_layer_and_leaving_hides_it() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(Message::FileHovered);
            assert!(app.uploads.hovering);
            let _ = app.update(Message::FilesHoveredLeft);
            assert!(!app.uploads.hovering);
        }

        #[test]
        fn dropped_files_gather_into_one_confirmation_after_the_pause() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(Message::FileHovered);
            let _ = app.update(Message::FileDropped(paths(&["one.txt"])));
            let first_seq = app.uploads.drop_seq;
            let _ = app.update(Message::FileDropped(paths(&["two.txt"])));
            assert!(!app.uploads.hovering, "a drop ends the hover");
            assert!(
                app.uploads.dialog.is_none(),
                "nothing opens before the pause"
            );
            // The timer of the first drop is stale.
            let _ = app.update(Message::DropsDue(first_seq));
            assert!(app.uploads.dialog.is_none());
            let _ = app.update(Message::DropsDue(app.uploads.drop_seq));
            let dialog = app.uploads.dialog.as_ref().expect("a confirmation");
            let names: Vec<_> = dialog.entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, ["one.txt", "two.txt"]);
            assert!(app.uploads.dropped.is_empty());
        }

        #[test]
        fn drops_without_a_room_are_ignored() {
            let (mut app, _rx, _, _) = rig();
            app.deselect();
            let _ = app.update(Message::FileHovered);
            assert!(!app.uploads.hovering);
            let _ = app.update(Message::FileDropped(paths(&["a.txt"])));
            assert!(app.uploads.dropped.is_empty());
        }

        #[test]
        fn paste_reads_the_clipboard_in_a_task() {
            let (mut app, _rx, _, clipboard) = rig();
            let _ = app.update(Message::PasteRequested);
            assert_eq!(*clipboard.reads.lock().expect("mutex"), 1);
        }

        #[test]
        fn pasted_text_goes_into_the_composer() {
            let (mut app, _rx, _, _) = rig();
            type_in(&mut app, "ab");
            let _ = app.update(Message::Pasted(ClipboardContent::Text(MessageText::from(
                "XY",
            ))));
            assert_eq!(app.composer.text(), "abXY");
            assert!(app.uploads.dialog.is_none());
        }

        #[test]
        fn a_pasted_picture_opens_the_confirmation_with_a_png() {
            let (mut app, _rx, _, _) = rig();
            let task = app.update(Message::Pasted(ClipboardContent::Image {
                width: 2,
                height: 2,
                rgba: Arc::new(vec![255; 16]),
            }));
            assert!(task.units() >= 1, "the PNG is made in a task");
            let dialog = app.uploads.dialog.as_ref().expect("a confirmation");
            assert_eq!(dialog.entries.len(), 1);
            assert!(dialog.loading());
            assert_eq!(composer_text(&app), "");
        }

        #[test]
        fn pixels_become_a_png_file() {
            let rgba = vec![200u8; 4 * 3 * 4];
            let Outcome::Ready(p) = upload::prepare_pixels("p.png".into(), 4, 3, &rgba, None)
            else {
                panic!("ready");
            };
            assert_eq!(p.mime, "image/png");
            assert_eq!(p.kind, MediaKind::Image);
            assert_eq!((p.width, p.height), (Some(4), Some(3)));
            assert!(
                matches!(&p.source, upload::Source::Bytes(b) if b.starts_with(&[0x89, b'P', b'N', b'G']))
            );
            // A size that does not match the pixels is refused.
            assert!(matches!(
                upload::prepare_pixels("p.png".into(), 4, 3, &rgba[..10], None),
                Outcome::Refused(Refusal::BadPicture)
            ));
        }

        #[test]
        fn an_unreadable_clipboard_falls_back_to_the_window_systems_text_paste() {
            let (mut app, _rx, _, _) = rig();
            let task = app.update(Message::Pasted(ClipboardContent::Unavailable));
            assert!(task.units() >= 1, "iced::clipboard::read");
            assert!(app.uploads.dialog.is_none());
            type_in(&mut app, "a");
            let _ = app.update(Message::PasteFallback(Some(MessageText::from("bc"))));
            assert_eq!(app.composer.text(), "abc");
            let _ = app.update(Message::PasteFallback(None));
            assert_eq!(app.composer.text(), "abc");
        }

        fn composer_text(app: &App) -> String {
            app.composer.text()
        }

        /// Confirms two ready files with a reply and a thread root set.
        fn two_ready(app: &mut App) {
            let _ = app.update(Message::FilesChosen(paths(&["a.txt", "b.png"])));
            let ids: Vec<u64> = app
                .uploads
                .dialog
                .as_ref()
                .expect("dialog")
                .entries
                .iter()
                .map(|e| e.id)
                .collect();
            let _ = app.update(prepared(ids[0], "a.txt", b"ay".to_vec()));
            let _ = app.update(prepared(ids[1], "b.txt", b"bee".to_vec()));
        }

        #[test]
        fn send_queues_one_attachment_per_file_in_order() {
            let (mut app, mut rx, _, _) = rig();
            two_ready(&mut app);
            let _ = drain(&mut rx);
            let _ = app.update(Message::UploadConfirmed);
            let commands = drain(&mut rx);
            let sent = attachments(&commands);
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[0].filename, "a.txt");
            assert_eq!(sent[1].filename, "b.txt");
            assert!(sent.iter().all(|a| a.room_id == room_a()));
            assert!(
                sent.iter()
                    .all(|a| a.reply_to.is_none() && a.thread_root.is_none())
            );
            assert_eq!(sent[0].mimetype, "text/plain");
            assert!(matches!(&sent[0].data, AttachmentData::Bytes(b) if b.as_bytes() == b"ay"));
            assert!(app.uploads.dialog.is_none());
        }

        #[test]
        fn path_files_go_to_the_core_as_paths_with_the_base_name_only() {
            let (mut app, mut rx, _, _) = rig();
            let _ = app.update(Message::FilesChosen(paths(&["a.txt"])));
            let id = app.uploads.dialog.as_ref().expect("dialog").entries[0].id;
            let outcome = Outcome::Ready(Box::new(upload::Prepared {
                name: "a.txt".into(),
                size: 3,
                source: upload::Source::Path(PathBuf::from("/x/a.txt")),
                mime: "text/plain",
                kind: MediaKind::File,
                width: None,
                height: None,
                thumbnail: None,
                preview: None,
            }));
            let _ = app.update(Message::UploadPrepared {
                entry: id,
                outcome: Box::new(outcome),
            });
            let _ = drain(&mut rx);
            let _ = app.update(Message::UploadConfirmed);
            let commands = drain(&mut rx);
            let sent = attachments(&commands);
            assert_eq!(sent.len(), 1);
            assert_eq!(sent[0].filename, "a.txt");
            assert!(
                matches!(&sent[0].data, AttachmentData::Path(p) if p == &PathBuf::from("/x/a.txt"))
            );
        }

        #[test]
        fn send_carries_the_reply_target_and_the_thread_root() {
            let (mut app, mut rx, _, _) = rig();
            let mut target = media_item("m1", MediaKind::File, "f.bin", 1, 1);
            target.event_id = Some(EventId::from("$target:x"));
            let _ = app.update(core(CoreEvent::TimelineDiff {
                room_id: room_a(),
                diffs: vec![ListDiff::Append {
                    values: vec![target],
                }],
            }));
            app.composer.mode = composer::Mode::Reply("m1".into());
            app.uploads.thread_root = Some(EventId::from("$root:x"));
            two_ready(&mut app);
            let _ = drain(&mut rx);
            let _ = app.update(Message::UploadConfirmed);
            let commands = drain(&mut rx);
            let sent = attachments(&commands);
            assert_eq!(sent.len(), 2);
            for a in &sent {
                assert_eq!(a.reply_to.as_ref().map(EventId::as_str), Some("$target:x"));
                assert_eq!(a.thread_root.as_ref().map(EventId::as_str), Some("$root:x"));
            }
            assert_eq!(
                app.composer.mode,
                composer::Mode::Normal,
                "the reply is used up"
            );
        }

        #[test]
        fn send_skips_refused_files_and_waits_for_loading_ones() {
            let (mut app, mut rx, _, _) = rig();
            let _ = app.update(Message::FilesChosen(paths(&["a.txt", "b.txt"])));
            let ids: Vec<u64> = app
                .uploads
                .dialog
                .as_ref()
                .expect("dialog")
                .entries
                .iter()
                .map(|e| e.id)
                .collect();
            let _ = app.update(prepared(ids[0], "a.txt", b"x".to_vec()));
            let _ = drain(&mut rx);
            let _ = app.update(Message::UploadConfirmed);
            assert!(attachments(&drain(&mut rx)).is_empty(), "b.txt still loads");
            assert!(app.uploads.dialog.is_some());
            let _ = app.update(Message::UploadPrepared {
                entry: ids[1],
                outcome: Box::new(Outcome::Refused(Refusal::Unreadable)),
            });
            let _ = app.update(Message::UploadConfirmed);
            let commands = drain(&mut rx);
            assert_eq!(attachments(&commands).len(), 1);
        }

        #[test]
        fn cancel_and_changing_rooms_close_the_confirmation() {
            let (mut app, mut rx, _, _) = rig();
            two_ready(&mut app);
            let _ = app.update(Message::UploadCancelled);
            assert!(app.uploads.dialog.is_none());
            assert!(attachments(&drain(&mut rx)).is_empty());
            two_ready(&mut app);
            let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
            assert!(app.uploads.dialog.is_none());
        }

        #[test]
        fn logging_out_forgets_uploads() {
            let (mut app, _rx, _, _) = rig();
            two_ready(&mut app);
            app.uploads.max_upload_bytes = Some(5);
            let _ = app.update(core(CoreEvent::LoggedOut));
            assert!(app.uploads.dialog.is_none());
            assert_eq!(app.uploads.max_upload_bytes, None);
        }

        #[test]
        fn messages_and_state_never_print_paths_names_or_pixels() {
            let (mut app, _rx, _, _) = rig();
            let secret = UploadPaths(vec![PathBuf::from("/home/u/secret-sentinel.txt")]);
            let shown = format!("{:?}", Message::FilesChosen(secret.clone()));
            assert!(!shown.contains("secret-sentinel"), "{shown}");
            let shown = format!("{:?}", Message::FileDropped(secret.clone()));
            assert!(!shown.contains("secret-sentinel"), "{shown}");
            let _ = app.update(Message::FilesChosen(secret));
            let id = app.uploads.dialog.as_ref().expect("dialog").entries[0].id;
            let _ = app.update(prepared(
                id,
                "secret-sentinel.txt",
                b"body-sentinel".to_vec(),
            ));
            let shown = format!("{app:?}");
            assert!(!shown.contains("secret-sentinel"), "{shown}");
            assert!(!shown.contains("body-sentinel"), "{shown}");
            let message = prepared(id, "secret-sentinel.txt", b"body-sentinel".to_vec());
            let shown = format!("{message:?}");
            assert!(
                !shown.contains("secret-sentinel") && !shown.contains("body-sentinel"),
                "{shown}"
            );
            let clip = format!(
                "{:?}",
                Message::Pasted(ClipboardContent::Text(MessageText::from("clip-sentinel")))
            );
            assert!(!clip.contains("clip-sentinel"), "{clip}");
        }

        // ---- progress and cancel ----

        fn upload_echo(
            id: &str,
            sent: SendStatus,
            progress: Option<UploadProgress>,
        ) -> TimelineItem {
            let mut content = MediaContent::new(MediaKind::File, "f.bin".to_owned());
            content.upload = progress;
            TimelineItem::new(
                id.to_owned(),
                TimelineItemKind::Message {
                    sender: UserId::from("@a:x"),
                    body: "f.bin".to_owned(),
                    is_own: true,
                    timestamp_ms: 1,
                    sent,
                    details: Box::new(MessageDetails {
                        media: Some(Box::new(content)),
                        ..MessageDetails::default()
                    }),
                },
            )
        }

        fn show(app: &mut App, item: TimelineItem) {
            let _ = app.update(core(CoreEvent::TimelineDiff {
                room_id: room_a(),
                diffs: vec![ListDiff::Append { values: vec![item] }],
            }));
        }

        #[test]
        fn cancel_discards_an_upload_that_is_in_progress() {
            let (mut app, mut rx, _, _) = rig();
            show(
                &mut app,
                upload_echo(
                    "up",
                    SendStatus::Sending,
                    Some(UploadProgress { sent: 1, total: 4 }),
                ),
            );
            let _ = drain(&mut rx);
            let _ = app.update(Message::DiscardRequested("up".into()));
            assert!(matches!(
                drain(&mut rx).as_slice(),
                [Command::DiscardSend { item_id, .. }] if item_id == "up"
            ));
        }

        #[test]
        fn a_plain_message_that_is_sending_cannot_be_discarded() {
            let (mut app, mut rx, _, _) = rig();
            show(&mut app, upload_echo("up", SendStatus::Sending, None));
            let _ = drain(&mut rx);
            let _ = app.update(Message::DiscardRequested("up".into()));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn failures_of_cancel_and_upload_show_a_notice_in_the_open_room() {
            let (mut app, _rx, _, _) = rig();
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: room_a(),
                action: ActionKind::Discard,
                error: CoreError::Internal("x".into()),
            }));
            let notice = app.actions.notice.as_ref().expect("a notice").text.clone();
            assert!(notice.contains("cancel"), "{notice}");
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: room_a(),
                action: ActionKind::Upload,
                error: CoreError::NotAllowed,
            }));
            let notice = app.actions.notice.as_ref().expect("a notice").text.clone();
            assert!(notice.contains("not sent"), "{notice}");
            // Another room's failure is not shown here.
            app.actions.notice = None;
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: RoomId::from("!b:x"),
                action: ActionKind::Upload,
                error: CoreError::NotAllowed,
            }));
            assert!(app.actions.notice.is_none());
        }

        #[test]
        fn opening_the_confirmation_keeps_the_widget_tree_under_it() {
            let (mut app, _rx, _, _) = rig();
            let closed = tag_path(&app);
            let _ = app.update(Message::FileHovered);
            assert_eq!(tag_path(&app), closed);
            let _ = app.update(Message::FilesHoveredLeft);
            two_ready(&mut app);
            assert_eq!(tag_path(&app), closed, "the base keeps its place");
            let _ = app.update(Message::UploadCancelled);
            assert_eq!(tag_path(&app), closed);
        }
    }

    mod notification_flow {
        use twicecat_core::api::{
            EventId, NotificationKind, NotificationView, RoomNotificationModeView,
        };

        use super::*;
        use crate::notifications::{ModeChoice, RecordingNotifier, Request};
        use crate::tray::{FakeTray, FakeTrayFactory, TrayAction};

        struct Rig {
            app: App,
            rx: mpsc::Receiver<Command>,
            shown: Arc<std::sync::Mutex<Vec<Request>>>,
            tray: FakeTray,
        }

        fn rig_with(tray_fails: bool) -> Rig {
            let (mut app, rx) = main_app();
            let notifier = RecordingNotifier::default();
            let shown = notifier.shown.clone();
            app.set_notifier(Box::new(notifier));
            let factory = FakeTrayFactory {
                fail: tray_fails,
                ..FakeTrayFactory::default()
            };
            let tray = factory.tray.clone();
            app.set_tray_factory(Box::new(factory));
            Rig {
                app,
                rx,
                shown,
                tray,
            }
        }

        fn rig() -> Rig {
            rig_with(false)
        }

        fn unfocus(app: &mut App) {
            let _ = app.update(Message::WindowFocus(false));
        }

        fn view(room: &str, kind: NotificationKind, is_dm: bool) -> NotificationView {
            NotificationView {
                room_id: RoomId::from(format!("!{room}:x")),
                event_id: EventId::from("$e"),
                room_name: room.to_owned(),
                sender: UserId::from("@bob:x"),
                sender_name: Some("Bob".into()),
                is_dm,
                kind,
                preview: Some(MessageText::from("secret preview text")),
            }
        }

        fn notify(app: &mut App, v: NotificationView) -> Task<Message> {
            app.update(core(CoreEvent::Notification(v)))
        }

        fn requests(rig: &Rig) -> Vec<Request> {
            rig.shown.lock().expect("test mutex").clone()
        }

        #[test]
        fn an_unfocused_window_shows_a_dm_and_a_focused_one_does_not() {
            let mut rig = rig();
            let _ = notify(&mut rig.app, view("b", NotificationKind::Message, true));
            assert!(requests(&rig).is_empty(), "the window has focus");
            unfocus(&mut rig.app);
            let _ = notify(&mut rig.app, view("b", NotificationKind::Message, true));
            let shown = requests(&rig);
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].room_id, RoomId::from("!b:x"));
            assert_eq!(shown[0].toast.title, "Bob");
            assert_eq!(shown[0].toast.body, "secret preview text");
        }

        #[test]
        fn the_selected_room_notifies_only_while_unfocused() {
            let mut rig = rig();
            assert_eq!(rig.app.selected, Some(RoomId::from("!a:x")));
            let _ = notify(&mut rig.app, view("a", NotificationKind::Message, false));
            assert!(requests(&rig).is_empty());
            unfocus(&mut rig.app);
            let _ = notify(&mut rig.app, view("a", NotificationKind::Message, false));
            assert_eq!(requests(&rig).len(), 1);
        }

        #[test]
        fn desktop_notifications_off_shows_nothing() {
            let mut rig = rig();
            rig.app.ui_settings.desktop_notifications = false;
            unfocus(&mut rig.app);
            let task = notify(&mut rig.app, view("b", NotificationKind::Mention, false));
            assert!(requests(&rig).is_empty());
            assert_eq!(task.units(), 0, "no attention request either");
        }

        #[test]
        fn settings_stored_on_disk_reach_the_notification_path() {
            let mut rig = rig();
            rig.app.set_ui_settings(UiSettings {
                show_message_content: false,
                ..UiSettings::default()
            });
            unfocus(&mut rig.app);
            let _ = notify(&mut rig.app, view("b", NotificationKind::Message, false));
            let shown = requests(&rig);
            assert_eq!(shown.len(), 1);
            assert_eq!(shown[0].toast.body, "New message");
            rig.app.set_ui_settings(UiSettings {
                desktop_notifications: false,
                ..UiSettings::default()
            });
            let _ = notify(&mut rig.app, view("c", NotificationKind::Message, false));
            assert_eq!(requests(&rig).len(), 1, "notifications off shows nothing");
        }

        #[test]
        fn with_content_off_the_preview_never_reaches_the_notifier() {
            let mut rig = rig();
            rig.app.ui_settings.show_message_content = false;
            unfocus(&mut rig.app);
            for (kind, is_dm) in [
                (NotificationKind::Message, false),
                (NotificationKind::Message, true),
                (NotificationKind::Mention, false),
                (NotificationKind::Encrypted, false),
                (NotificationKind::Invite, false),
            ] {
                let _ = notify(
                    &mut rig.app,
                    view(&format!("r{is_dm}{kind:?}"), kind, is_dm),
                );
            }
            let shown = requests(&rig);
            assert_eq!(shown.len(), 5);
            let bodies: Vec<&str> = shown.iter().map(|r| r.toast.body.as_str()).collect();
            assert_eq!(
                bodies,
                [
                    "New message",
                    "New message",
                    "Mentioned you",
                    "New message",
                    "Invited you"
                ]
            );
            for request in &shown {
                let text = format!("{} {}", request.toast.title, request.toast.body);
                assert!(!text.contains("secret"), "{text}");
                assert!(!text.contains("Bob"), "{text}");
            }
        }

        #[test]
        fn a_burst_in_one_room_folds_into_the_shown_notification() {
            let mut rig = rig();
            unfocus(&mut rig.app);
            let _ = notify(&mut rig.app, view("b", NotificationKind::Message, false));
            let _ = rig.app.update(Message::NotificationShown {
                room_id: RoomId::from("!b:x"),
                id: 7,
            });
            let _ = notify(&mut rig.app, view("b", NotificationKind::Message, false));
            let _ = notify(&mut rig.app, view("b", NotificationKind::Message, false));
            let shown = requests(&rig);
            assert_eq!(shown.len(), 3);
            assert_eq!(shown[0].replace, None);
            assert_eq!(shown[1].replace, Some(7));
            assert_eq!(shown[1].toast.body, "2 new messages");
            assert_eq!(shown[2].toast.body, "3 new messages");
            // Another room is not folded in.
            let _ = notify(&mut rig.app, view("a", NotificationKind::Message, false));
            assert_eq!(requests(&rig)[3].replace, None);
        }

        #[test]
        fn a_mention_asks_for_attention_and_a_plain_message_does_not() {
            let mut rig = rig();
            unfocus(&mut rig.app);
            let plain = notify(&mut rig.app, view("b", NotificationKind::Message, false));
            assert_eq!(plain.units(), 0);
            let mention = notify(&mut rig.app, view("a", NotificationKind::Mention, false));
            assert!(mention.units() > 0);
        }

        #[test]
        fn clicking_a_notification_selects_the_room_and_focuses_the_window() {
            let mut rig = rig();
            let task = rig
                .app
                .update(Message::OpenRoomFromNotification(RoomId::from("!b:x")));
            assert!(task.units() > 0);
            assert_eq!(rig.app.selected, Some(RoomId::from("!b:x")));
            let commands = drain(&mut rig.rx);
            assert!(
                commands
                    .iter()
                    .any(|c| matches!(c, Command::SelectRoom(id) if *id == RoomId::from("!b:x")))
            );
            // The same room again: only the focus.
            let task = rig
                .app
                .update(Message::OpenRoomFromNotification(RoomId::from("!b:x")));
            assert!(task.units() > 0);
            assert!(drain(&mut rig.rx).is_empty());
            // A room that is not listed is not selected.
            let _ = rig
                .app
                .update(Message::OpenRoomFromNotification(RoomId::from("!zz:x")));
            assert_eq!(rig.app.selected, Some(RoomId::from("!b:x")));
        }

        fn mode_commands(rig: &mut Rig) -> Vec<Command> {
            drain(&mut rig.rx)
                .into_iter()
                .filter(|c| matches!(c, Command::SetRoomNotificationMode { .. }))
                .collect()
        }

        #[test]
        fn each_menu_choice_sends_its_command_once() {
            let mut rig = rig();
            let room = RoomId::from("!a:x");
            for (choice, mode) in [
                (
                    ModeChoice::AllMessages,
                    Some(RoomNotificationModeView::AllMessages),
                ),
                (
                    ModeChoice::MentionsAndKeywords,
                    Some(RoomNotificationModeView::MentionsAndKeywords),
                ),
                (ModeChoice::Mute, Some(RoomNotificationModeView::Mute)),
                (ModeChoice::Default, None),
            ] {
                let _ = rig.app.update(Message::RoomModeChosen(choice));
                let commands = mode_commands(&mut rig);
                assert_eq!(commands.len(), 1, "{choice:?}");
                assert!(
                    matches!(&commands[0], Command::SetRoomNotificationMode { room_id, mode: m }
                        if *room_id == room && *m == mode),
                    "{choice:?}: {commands:?}"
                );
                // The menu shows the choice at once, before the list catches up.
                assert_eq!(rig.app.room_mode_choice(&room), choice);
                // Choosing it again changes nothing.
                let _ = rig.app.update(Message::RoomModeChosen(choice));
                assert!(mode_commands(&mut rig).is_empty(), "{choice:?} twice");
            }
        }

        #[test]
        fn choosing_a_mode_without_a_room_sends_nothing() {
            let mut rig = rig();
            let _ = rig
                .app
                .update(core(CoreEvent::RoomListChanged(vec![ListDiff::Clear])));
            let _ = drain(&mut rig.rx);
            let _ = rig.app.update(Message::RoomModeChosen(ModeChoice::Mute));
            assert!(mode_commands(&mut rig).is_empty());
        }

        #[test]
        fn a_failed_change_goes_back_to_what_the_list_says() {
            let mut rig = rig();
            let room = RoomId::from("!a:x");
            let _ = rig.app.update(Message::RoomModeChosen(ModeChoice::Mute));
            assert_eq!(rig.app.room_mode_choice(&room), ModeChoice::Mute);
            let _ = rig.app.update(core(CoreEvent::ActionFailed {
                room_id: room.clone(),
                action: ActionKind::Notifications,
                error: CoreError::NotAllowed,
            }));
            assert_eq!(rig.app.room_mode_choice(&room), ModeChoice::Default);
        }

        #[test]
        fn the_list_taking_over_drops_the_local_choice() {
            let mut rig = rig();
            let room = RoomId::from("!a:x");
            let _ = rig.app.update(Message::RoomModeChosen(ModeChoice::Mute));
            let mut changed = RoomSummary::new(room.clone(), "a".into());
            changed.notification_mode = Some(RoomNotificationModeView::Mute);
            let _ = rig
                .app
                .update(core(CoreEvent::RoomListChanged(vec![ListDiff::Set {
                    index: 0,
                    value: changed,
                }])));
            assert!(rig.app.room_modes.is_empty());
            assert_eq!(rig.app.room_mode_choice(&room), ModeChoice::Mute);
        }

        #[test]
        fn the_tray_appears_on_the_first_window_and_only_once() {
            let mut rig = rig();
            assert!(rig.tray.created.lock().expect("test mutex").is_empty());
            let id = iced::window::Id::unique();
            let _ = rig.app.update(Message::WindowOpened(id));
            let _ = rig.app.update(Message::WindowOpened(id));
            assert_eq!(rig.tray.created.lock().expect("test mutex").len(), 1);
        }

        #[test]
        fn a_tray_failure_leaves_the_app_working() {
            let mut rig = rig_with(true);
            let _ = rig
                .app
                .update(Message::WindowOpened(iced::window::Id::unique()));
            let _ = rig.app.update(core(CoreEvent::UnreadTotals {
                notifications: 3,
                highlights: 0,
            }));
            assert!(rig.tray.badges.lock().expect("test mutex").is_empty());
            assert_eq!(rig.app.screen, Screen::Main);
        }

        #[test]
        fn unread_totals_update_the_tray_only_when_the_look_changes() {
            let mut rig = rig();
            let _ = rig
                .app
                .update(Message::WindowOpened(iced::window::Id::unique()));
            let totals = |n| {
                core(CoreEvent::UnreadTotals {
                    notifications: n,
                    highlights: 0,
                })
            };
            let _ = rig.app.update(totals(3));
            let _ = rig.app.update(totals(3));
            assert_eq!(*rig.tray.badges.lock().expect("test mutex"), [true]);
            assert_eq!(rig.tray.tooltips.lock().expect("test mutex").len(), 1);
            let _ = rig.app.update(totals(0));
            assert_eq!(*rig.tray.badges.lock().expect("test mutex"), [true, false]);
        }

        #[test]
        fn totals_that_arrive_before_the_tray_are_used_at_creation() {
            let mut rig = rig();
            let _ = rig.app.update(core(CoreEvent::UnreadTotals {
                notifications: 5,
                highlights: 1,
            }));
            let _ = rig
                .app
                .update(Message::WindowOpened(iced::window::Id::unique()));
            let created = rig.tray.created.lock().expect("test mutex");
            assert_eq!(created.len(), 1);
            assert!(created[0].badge);
        }

        #[test]
        fn the_badge_setting_off_keeps_the_tray_plain() {
            let mut rig = rig();
            rig.app.ui_settings.tray_badge = false;
            let _ = rig
                .app
                .update(Message::WindowOpened(iced::window::Id::unique()));
            let _ = rig.app.update(core(CoreEvent::UnreadTotals {
                notifications: 5,
                highlights: 0,
            }));
            assert!(rig.tray.badges.lock().expect("test mutex").is_empty());
        }

        #[test]
        fn tray_show_focuses_the_window_and_quit_takes_the_close_path() {
            let mut rig = rig();
            let show = rig.app.update(Message::Tray(TrayAction::Show));
            assert!(show.units() > 0);
            assert_ne!(rig.app.screen, Screen::ShuttingDown);
            let _ = drain(&mut rig.rx);
            let _ = rig.app.update(Message::Tray(TrayAction::Quit));
            assert_eq!(rig.app.screen, Screen::ShuttingDown);
            assert!(
                drain(&mut rig.rx)
                    .iter()
                    .any(|c| matches!(c, Command::Shutdown))
            );
        }

        #[test]
        fn the_notification_settings_event_is_kept() {
            let mut rig = rig();
            let settings = twicecat_core::api::NotificationSettingsView {
                group_default: RoomNotificationModeView::AllMessages,
                dm_default: RoomNotificationModeView::Mute,
                keywords: vec!["kw".into()],
            };
            let _ = rig
                .app
                .update(core(CoreEvent::NotificationSettings(settings.clone())));
            assert_eq!(rig.app.notification_settings.as_ref(), Some(&settings));
        }
    }

    // ---- Phase 5 Task 12: room dialogs, settings, members and pills ----

    mod rooms_ui {
        use std::path::PathBuf;

        use twicecat_core::api::{
            Block, Inline, MediaBytes, MembershipView, MentionTarget, MessageDetails,
            PowerLevelView, RichText, Role, RoomDetailsView, RoomMemberView, RoomPermissions,
            SendStatus, TimelineItemKind,
        };

        use super::*;
        use crate::dialogs::RecordingDialogs;
        use crate::links::RoomTarget;
        use crate::rooms::{AvatarRefusal, AvatarUpload, Dialog, MEMBERS_PAGE, RoomsMessage as R};

        fn rig() -> (App, mpsc::Receiver<Command>, RecordingDialogs) {
            let (mut app, rx) = main_app();
            let dialogs = RecordingDialogs::default();
            app.set_dialogs(Box::new(dialogs.clone()));
            (app, rx, dialogs)
        }

        fn rooms(app: &mut App, message: R) {
            let _ = app.update(Message::Rooms(message));
        }

        fn t(text: &str) -> MessageText {
            MessageText::from(text)
        }

        fn id(room: &str) -> RoomId {
            RoomId::from(room)
        }

        fn all(can: bool) -> RoomPermissions {
            RoomPermissions {
                can_set_name: can,
                can_set_topic: can,
                can_set_avatar: can,
                can_enable_encryption: can,
                can_invite: can,
            }
        }

        fn details(room: &str, encrypted: bool, permissions: RoomPermissions) -> RoomDetailsView {
            RoomDetailsView {
                room_id: id(room),
                name: "Lounge".into(),
                topic: Some("Chat".into()),
                avatar: None,
                is_encrypted: encrypted,
                alias: None,
                permissions,
            }
        }

        fn member(user: &str, name: Option<&str>, role: Role) -> RoomMemberView {
            RoomMemberView {
                user_id: UserId::from(user),
                display_name: name.map(str::to_owned),
                avatar: None,
                power: PowerLevelView::Level(0),
                role,
                membership: MembershipView::Joined,
            }
        }

        fn open_settings(app: &mut App, rx: &mut mpsc::Receiver<Command>, perms: RoomPermissions) {
            rooms(app, R::OpenSettings);
            let _ = drain(rx);
            let _ = app.update(core(CoreEvent::RoomDetails(details("!a:x", false, perms))));
        }

        #[test]
        fn new_room_opens_with_encryption_on_and_a_bad_form_sends_nothing() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            let Some(Dialog::Create(dialog)) = &app.rooms_ui.dialog else {
                panic!("the create dialog is open");
            };
            assert!(dialog.form.encrypted);
            rooms(&mut app, R::CreateSubmit);
            assert!(drain(&mut rx).is_empty(), "an empty name sends nothing");
            let Some(Dialog::Create(dialog)) = &app.rooms_ui.dialog else {
                panic!("still open");
            };
            assert!(dialog.error.is_some());
            assert!(!dialog.busy);
        }

        #[test]
        fn create_sends_one_command_even_when_pressed_twice() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::CreateName(t(" Lounge ")));
            rooms(&mut app, R::CreateTopic(t("Chat")));
            rooms(&mut app, R::CreateInvitees(t("@bob:x")));
            rooms(&mut app, R::CreateSubmit);
            rooms(&mut app, R::CreateSubmit);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            let Command::CreateRoom(request) = &sent[0] else {
                panic!("{sent:?}");
            };
            assert_eq!(request.name, "Lounge");
            assert!(request.encrypted);
            assert_eq!(request.invitees, [UserId::from("@bob:x")]);
        }

        #[test]
        fn an_unencrypted_public_room_is_asked_for_as_typed() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::CreateName(t("Open")));
            rooms(&mut app, R::CreateEncrypted(false));
            rooms(&mut app, R::CreatePublic(true));
            rooms(&mut app, R::CreateAlias(t("open-room")));
            rooms(&mut app, R::CreateSubmit);
            let sent = drain(&mut rx);
            let Command::CreateRoom(request) = &sent[0] else {
                panic!("{sent:?}");
            };
            assert!(!request.encrypted);
            assert_eq!(
                request.visibility,
                twicecat_core::api::RoomVisibility::Public {
                    alias: "open-room".into()
                }
            );
        }

        #[test]
        fn a_created_room_is_selected_when_it_appears_in_the_list() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::CreateName(t("Lounge")));
            rooms(&mut app, R::CreateSubmit);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::RoomCreated {
                room_id: id("!new:x"),
            }));
            assert!(app.rooms_ui.dialog.is_none(), "the dialog closes");
            assert!(drain(&mut rx).is_empty(), "the room is not listed yet");
            assert_eq!(app.selected, Some(id("!a:x")));
            let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
                value: RoomSummary::new(id("!new:x"), "Lounge".into()),
            }])));
            let sent = drain(&mut rx);
            assert!(
                sent.iter()
                    .any(|c| matches!(c, Command::SelectRoom(r) if *r == id("!new:x"))),
                "{sent:?}"
            );
            assert_eq!(app.selected, Some(id("!new:x")));
            assert_eq!(app.rooms_ui.select_when_listed, None);
            // A later list change does not select it again.
            let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Clear])));
            assert!(
                !drain(&mut rx)
                    .iter()
                    .any(|c| matches!(c, Command::SelectRoom(_)))
            );
        }

        #[test]
        fn a_room_that_is_already_listed_is_selected_at_once() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(core(CoreEvent::RoomCreated {
                room_id: id("!b:x"),
            }));
            assert_eq!(app.selected, Some(id("!b:x")));
            assert!(
                drain(&mut rx)
                    .iter()
                    .any(|c| matches!(c, Command::SelectRoom(r) if *r == id("!b:x")))
            );
        }

        #[test]
        fn a_failed_create_shows_a_fixed_line_and_allows_another_try() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::CreateName(t("Lounge")));
            rooms(&mut app, R::CreateSubmit);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: id(""),
                action: ActionKind::Rooms,
                error: CoreError::Internal("sentinel-secret-text".into()),
            }));
            let Some(Dialog::Create(dialog)) = &app.rooms_ui.dialog else {
                panic!("the dialog stays open");
            };
            assert!(!dialog.busy);
            let line = dialog.error.expect("an error line");
            assert!(!line.contains("sentinel"));
            rooms(&mut app, R::CreateSubmit);
            assert_eq!(drain(&mut rx).len(), 1);
        }

        #[test]
        fn a_dm_needs_a_user_id_and_is_sent_once() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenDm);
            rooms(&mut app, R::DmInput(t("bob")));
            rooms(&mut app, R::DmSubmit);
            assert!(drain(&mut rx).is_empty());
            rooms(&mut app, R::DmInput(t(" @bob:x ")));
            rooms(&mut app, R::DmSubmit);
            rooms(&mut app, R::DmSubmit);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(
                matches!(&sent[0], Command::CreateDm { user_id } if user_id.as_str() == "@bob:x")
            );
        }

        #[test]
        fn join_takes_an_alias_an_id_or_a_link_and_sends_once() {
            for (typed, target, via) in [
                ("#lounge:x", "#lounge:x", vec![]),
                ("!abc:x", "!abc:x", vec![]),
                (
                    "https://matrix.to/#/!abc:x?via=h.example",
                    "!abc:x",
                    vec!["h.example".to_owned()],
                ),
            ] {
                let (mut app, mut rx, _) = rig();
                rooms(&mut app, R::OpenJoin);
                rooms(&mut app, R::JoinInput(t(typed)));
                rooms(&mut app, R::JoinSubmit);
                rooms(&mut app, R::JoinSubmit);
                let sent = drain(&mut rx);
                assert_eq!(sent.len(), 1, "{typed}: {sent:?}");
                match &sent[0] {
                    Command::JoinRoom { target: t, via: v } => {
                        assert_eq!(t, target);
                        assert_eq!(*v, via);
                    }
                    other => panic!("{other:?}"),
                }
            }
        }

        #[test]
        fn join_refuses_junk_without_sending() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenJoin);
            for bad in ["", "lounge", "@bob:x", "https://evil.example/#/!a:x"] {
                rooms(&mut app, R::JoinInput(t(bad)));
                rooms(&mut app, R::JoinSubmit);
            }
            assert!(drain(&mut rx).is_empty());
            let Some(Dialog::Join(dialog)) = &app.rooms_ui.dialog else {
                panic!("still open");
            };
            assert!(dialog.error.is_some() && !dialog.busy);
        }

        #[test]
        fn a_joined_room_is_selected_and_the_dialog_closes() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenJoin);
            rooms(&mut app, R::JoinInput(t("!j:x")));
            rooms(&mut app, R::JoinSubmit);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::RoomJoined {
                room_id: id("!j:x"),
            }));
            assert!(app.rooms_ui.dialog.is_none());
            let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
                value: RoomSummary::new(id("!j:x"), "J".into()),
            }])));
            assert_eq!(app.selected, Some(id("!j:x")));
        }

        fn with_invite(app: &mut App) {
            let mut invite = RoomSummary::new(id("!inv:x"), "Invited".into());
            invite.is_invite = true;
            let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
                value: invite,
            }])));
        }

        #[test]
        fn accept_and_decline_send_their_command_once_each() {
            let (mut app, mut rx, _) = rig();
            with_invite(&mut app);
            rooms(&mut app, R::Accept(id("!inv:x")));
            rooms(&mut app, R::Accept(id("!inv:x")));
            rooms(&mut app, R::Decline(id("!inv:x")));
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "one answer per invitation: {sent:?}");
            assert!(
                matches!(&sent[0], Command::AcceptInvite { room_id } if *room_id == id("!inv:x"))
            );

            let (mut app, mut rx, _) = rig();
            with_invite(&mut app);
            rooms(&mut app, R::Decline(id("!inv:x")));
            rooms(&mut app, R::Decline(id("!inv:x")));
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(
                matches!(&sent[0], Command::DeclineInvite { room_id } if *room_id == id("!inv:x"))
            );
        }

        #[test]
        fn only_invitations_can_be_accepted_or_declined() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::Accept(id("!b:x")));
            rooms(&mut app, R::Decline(id("!b:x")));
            rooms(&mut app, R::Accept(id("!unknown:x")));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn a_failed_answer_can_be_tried_again() {
            let (mut app, mut rx, _) = rig();
            with_invite(&mut app);
            rooms(&mut app, R::Accept(id("!inv:x")));
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: id("!inv:x"),
                action: ActionKind::Rooms,
                error: CoreError::Network("x".into()),
            }));
            assert!(app.actions.notice.is_some(), "the failure is shown");
            rooms(&mut app, R::Accept(id("!inv:x")));
            assert_eq!(drain(&mut rx).len(), 1);
        }

        #[test]
        fn settings_ask_for_details_and_members() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenSettings);
            let sent = drain(&mut rx);
            assert!(sent.iter().any(
                |c| matches!(c, Command::OpenRoomDetails { room_id } if *room_id == id("!a:x"))
            ));
            assert!(sent.iter().any(
                |c| matches!(c, Command::ListRoomMembers { room_id } if *room_id == id("!a:x"))
            ));
            let panel = app.rooms_ui.settings.as_ref().expect("open");
            assert_eq!(panel.room, id("!a:x"));
        }

        #[test]
        fn settings_without_a_selected_room_do_not_open() {
            let (mut app, mut rx) = login_app();
            logged_in(&mut app);
            drain(&mut rx);
            rooms(&mut app, R::OpenSettings);
            rooms(&mut app, R::OpenMembers);
            assert!(app.rooms_ui.settings.is_none() && app.rooms_ui.members_panel.is_none());
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn details_fill_the_fields_until_the_user_edits_them() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            let panel = app.rooms_ui.settings.as_ref().expect("open");
            assert_eq!(
                (panel.name.as_str(), panel.topic.as_str()),
                ("Lounge", "Chat")
            );
            rooms(&mut app, R::NameInput(t("Typed")));
            let mut newer = details("!a:x", false, all(true));
            newer.name = "From server".into();
            newer.topic = Some("Server topic".into());
            let _ = app.update(core(CoreEvent::RoomDetails(newer)));
            let panel = app.rooms_ui.settings.as_ref().expect("open");
            assert_eq!(panel.name, "Typed", "an edited field is kept");
            assert_eq!(panel.topic, "Server topic");
        }

        #[test]
        fn details_of_another_room_are_ignored() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenSettings);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::RoomDetails(details(
                "!b:x",
                false,
                all(true),
            ))));
            assert!(app.rooms_ui.details.is_none());
        }

        #[test]
        fn name_and_topic_are_saved_only_with_permission() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(false));
            rooms(&mut app, R::NameInput(t("New")));
            rooms(&mut app, R::TopicInput(t("Topic")));
            rooms(&mut app, R::SaveName);
            rooms(&mut app, R::SaveTopic);
            assert!(drain(&mut rx).is_empty(), "no permission, no command");

            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::NameInput(t("  New name ")));
            rooms(&mut app, R::TopicInput(t("  ")));
            rooms(&mut app, R::SaveName);
            rooms(&mut app, R::SaveTopic);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 2, "{sent:?}");
            assert!(matches!(&sent[0], Command::SetRoomName { name, .. } if name == "New name"));
            assert!(matches!(
                &sent[1],
                Command::SetRoomTopic { topic: None, .. }
            ));
        }

        #[test]
        fn a_bad_name_is_not_sent() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::NameInput(t("   ")));
            rooms(&mut app, R::SaveName);
            rooms(&mut app, R::NameInput(t("a\u{202E}b")));
            rooms(&mut app, R::SaveName);
            assert!(drain(&mut rx).is_empty());
            assert!(
                app.rooms_ui
                    .settings
                    .as_ref()
                    .expect("open")
                    .error
                    .is_some()
            );
        }

        #[test]
        fn the_picture_goes_through_the_picker_and_is_sent_once() {
            let (mut app, mut rx, dialogs) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::ChooseAvatar);
            assert_eq!(*dialogs.images.lock().expect("mutex"), 1);
            rooms(&mut app, R::AvatarPicked(None));
            assert!(!app.rooms_ui.settings.as_ref().expect("open").avatar_busy);
            // A path starts the read; the scaled picture arrives as a message.
            rooms(
                &mut app,
                R::AvatarPicked(Some(PathBuf::from("/nonexistent/p.png").into())),
            );
            assert!(app.rooms_ui.settings.as_ref().expect("open").avatar_busy);
            let upload = AvatarUpload {
                data: vec![1, 2, 3],
                mimetype: "image/png",
                width: 2,
                height: 2,
            };
            let ready = R::AvatarReady {
                room: id("!a:x"),
                result: Ok(Box::new(upload)),
            };
            rooms(&mut app, ready.clone());
            rooms(&mut app, ready);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            match &sent[0] {
                Command::SetRoomAvatar {
                    room_id,
                    data,
                    mimetype,
                } => {
                    assert_eq!(*room_id, id("!a:x"));
                    assert_eq!(data, &MediaBytes::from(vec![1, 2, 3]));
                    assert_eq!(mimetype, "image/png");
                }
                other => panic!("{other:?}"),
            }
        }

        #[test]
        fn a_refused_picture_shows_a_line_and_sends_nothing() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(
                &mut app,
                R::AvatarPicked(Some(PathBuf::from("/nonexistent/p.png").into())),
            );
            rooms(
                &mut app,
                R::AvatarReady {
                    room: id("!a:x"),
                    result: Err(AvatarRefusal::NotAPicture),
                },
            );
            let panel = app.rooms_ui.settings.as_ref().expect("open");
            assert!(!panel.avatar_busy);
            assert_eq!(panel.error, Some(AvatarRefusal::NotAPicture.text()));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn a_picture_for_another_room_or_a_closed_panel_is_dropped() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            let upload = || AvatarUpload {
                data: vec![1],
                mimetype: "image/png",
                width: 1,
                height: 1,
            };
            rooms(
                &mut app,
                R::AvatarReady {
                    room: id("!b:x"),
                    result: Ok(Box::new(upload())),
                },
            );
            rooms(&mut app, R::CloseSettings);
            rooms(
                &mut app,
                R::AvatarReady {
                    room: id("!a:x"),
                    result: Ok(Box::new(upload())),
                },
            );
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn enabling_encryption_needs_the_warning_card_and_permission() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::EncryptConfirm);
            assert!(drain(&mut rx).is_empty(), "no confirm without the card");
            rooms(&mut app, R::EncryptAsk);
            assert!(app.rooms_ui.settings.as_ref().expect("open").encrypt_ask);
            rooms(&mut app, R::EncryptCancel);
            rooms(&mut app, R::EncryptConfirm);
            assert!(drain(&mut rx).is_empty(), "cancel withdraws the card");
            rooms(&mut app, R::EncryptAsk);
            rooms(&mut app, R::EncryptConfirm);
            rooms(&mut app, R::EncryptConfirm);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(
                matches!(&sent[0], Command::EnableRoomEncryption { room_id } if *room_id == id("!a:x"))
            );

            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(false));
            rooms(&mut app, R::EncryptAsk);
            rooms(&mut app, R::EncryptConfirm);
            assert!(drain(&mut rx).is_empty(), "no permission");

            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenSettings);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::RoomDetails(details(
                "!a:x",
                true,
                all(true),
            ))));
            rooms(&mut app, R::EncryptAsk);
            rooms(&mut app, R::EncryptConfirm);
            assert!(drain(&mut rx).is_empty(), "already encrypted");
        }

        #[test]
        fn leaving_asks_first_then_sends_once_and_closes_the_room() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::LeaveConfirm);
            assert!(drain(&mut rx).is_empty(), "no confirm without the question");
            rooms(&mut app, R::LeaveAsk);
            rooms(&mut app, R::LeaveCancel);
            rooms(&mut app, R::LeaveConfirm);
            assert!(drain(&mut rx).is_empty());
            rooms(&mut app, R::LeaveAsk);
            rooms(&mut app, R::LeaveConfirm);
            rooms(&mut app, R::LeaveConfirm);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(&sent[0], Command::LeaveRoom { room_id } if *room_id == id("!a:x")));
            assert!(app.rooms_ui.settings.is_none());
            assert_eq!(app.selected, None, "the timeline is closed");
            // The list still shows the room until the next sync says more.
            assert!(room_list::contains(&app.rooms, &id("!a:x")));
        }

        #[test]
        fn a_refused_leave_is_reported() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::LeaveAsk);
            rooms(&mut app, R::LeaveConfirm);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: id("!a:x"),
                action: ActionKind::Rooms,
                error: CoreError::NotAllowed,
            }));
            assert!(app.actions.notice.is_some());
        }

        #[test]
        fn the_leave_warning_follows_what_is_known() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            assert!(app.leave_warning_now().unknown, "members not listed yet");
            let _ = app.update(core(CoreEvent::RoomDetails(details(
                "!a:x",
                true,
                all(true),
            ))));
            let _ = app.update(core(CoreEvent::RoomMembers {
                room_id: id("!a:x"),
                members: vec![
                    member("@a:x", None, Role::Admin),
                    member("@b:x", None, Role::User),
                ],
                truncated: false,
            }));
            let w = app.leave_warning_now();
            assert!(w.encrypted && w.last_admin && !w.unknown, "{w:?}");
        }

        #[test]
        fn members_open_with_details_and_a_list_sorted_by_role_then_name() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenMembers);
            let sent = drain(&mut rx);
            assert!(
                sent.iter()
                    .any(|c| matches!(c, Command::ListRoomMembers { .. }))
            );
            assert!(
                sent.iter()
                    .any(|c| matches!(c, Command::OpenRoomDetails { .. }))
            );
            let _ = app.update(core(CoreEvent::RoomMembers {
                room_id: id("!a:x"),
                members: vec![
                    member("@z:x", Some("Zed"), Role::User),
                    member("@m:x", Some("Mod"), Role::Moderator),
                    member("@c:x", Some("Boss"), Role::Creator),
                    member("@a:x", Some("alice"), Role::User),
                ],
                truncated: true,
            }));
            let known = app.rooms_ui.known.as_ref().expect("listed");
            let ids: Vec<&str> = known.members.iter().map(|m| m.user_id.as_str()).collect();
            assert_eq!(ids, ["@c:x", "@m:x", "@a:x", "@z:x"]);
            assert!(known.truncated);
        }

        #[test]
        fn the_member_search_is_view_state_only_capped_and_cleared_when_the_panel_opens() {
            let (mut app, mut rx, _) = rig();
            // No panel, no search.
            rooms(&mut app, R::MemberSearch("zed".to_owned().into()));
            assert!(app.rooms_ui.member_search.is_empty());
            rooms(&mut app, R::OpenMembers);
            let _ = drain(&mut rx);
            rooms(&mut app, R::MemberSearch("zed".to_owned().into()));
            assert_eq!(app.rooms_ui.member_search, "zed");
            assert!(drain(&mut rx).is_empty(), "the search sends nothing");
            rooms(&mut app, R::MemberSearch("x".repeat(500).into()));
            assert_eq!(
                app.rooms_ui.member_search.chars().count(),
                crate::rooms::MAX_MEMBER_SEARCH_CHARS
            );
            // A narrower search starts at the first page again.
            rooms(&mut app, R::ShowMoreMembers);
            rooms(&mut app, R::MemberSearch("a".to_owned().into()));
            let panel = app.rooms_ui.members_panel.as_ref().expect("open");
            assert_eq!(panel.shown, crate::rooms::MEMBERS_PAGE);
            // Closing and opening again starts with an empty field.
            rooms(&mut app, R::CloseMembers);
            rooms(&mut app, R::OpenMembers);
            assert!(app.rooms_ui.member_search.is_empty());
        }

        #[test]
        fn a_member_search_fetches_the_avatars_of_the_rows_it_brings_into_view() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenMembers);
            let _ = drain(&mut rx);
            // More members than a page: the last five are not drawn at first.
            let page = crate::rooms::MEMBERS_PAGE;
            let members = (0..page + 5)
                .map(|i| {
                    // Sorted by name: the late ones are the last five.
                    let name = if i < page {
                        format!("a{i:03}")
                    } else {
                        format!("zzz{i}")
                    };
                    let mut m = member(&format!("@m{i}:x"), Some(&name), Role::User);
                    if i >= page {
                        m.avatar = Some(AvatarId::from(format!("mxc://x/late{i}")));
                    }
                    m
                })
                .collect();
            let _ = app.update(core(CoreEvent::RoomMembers {
                room_id: id("!a:x"),
                members,
                truncated: false,
            }));
            let first = drain(&mut rx);
            assert!(
                !first.iter().any(|c| matches!(c, Command::FetchAvatars(_))),
                "rows that are not on screen are not fetched: {first:?}"
            );
            // Searching for them draws them: their pictures are asked for.
            rooms(&mut app, R::MemberSearch("zzz".to_owned().into()));
            let sent = drain(&mut rx);
            assert!(
                matches!(
                    sent.as_slice(),
                    [Command::FetchAvatars(ids)] if ids.len() == 5
                ),
                "{sent:?}"
            );
            // The same row again asks for nothing.
            rooms(&mut app, R::MemberSearch("zzz1".to_owned().into()));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn a_member_list_for_another_room_is_ignored() {
            let (mut app, _rx, _) = rig();
            let _ = app.update(core(CoreEvent::RoomMembers {
                room_id: id("!b:x"),
                members: vec![member("@z:x", Some("Zed"), Role::User)],
                truncated: false,
            }));
            assert!(app.rooms_ui.known.is_none());
        }

        #[test]
        fn invites_are_validated_in_the_app_and_need_permission() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenMembers);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::RoomDetails(details(
                "!a:x",
                false,
                all(true),
            ))));
            rooms(&mut app, R::InviteInput(t("bob")));
            rooms(&mut app, R::InviteSubmit);
            assert!(drain(&mut rx).is_empty(), "not a user ID");
            rooms(&mut app, R::InviteInput(t(" @bob:x ")));
            rooms(&mut app, R::InviteSubmit);
            let sent = drain(&mut rx);
            assert!(
                matches!(&sent[0], Command::InviteUser { room_id, user_id }
                    if *room_id == id("!a:x") && user_id.as_str() == "@bob:x"),
                "{sent:?}"
            );
            assert_eq!(
                sent.iter()
                    .filter(|c| matches!(c, Command::InviteUser { .. }))
                    .count(),
                1
            );
            assert_eq!(
                app.rooms_ui.members_panel.as_ref().expect("open").invite,
                "",
                "the field is emptied"
            );

            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenMembers);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::RoomDetails(details(
                "!a:x",
                false,
                all(false),
            ))));
            rooms(&mut app, R::InviteInput(t("@bob:x")));
            rooms(&mut app, R::InviteSubmit);
            assert!(drain(&mut rx).is_empty(), "no permission");
        }

        #[test]
        fn the_member_list_grows_page_by_page() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenMembers);
            let _ = drain(&mut rx);
            let many: Vec<RoomMemberView> = (0..MEMBERS_PAGE + 50)
                .map(|i| member(&format!("@u{i:04}:x"), None, Role::User))
                .collect();
            let _ = app.update(core(CoreEvent::RoomMembers {
                room_id: id("!a:x"),
                members: many,
                truncated: false,
            }));
            let panel = app.rooms_ui.members_panel.as_ref().expect("open");
            assert_eq!(panel.shown, MEMBERS_PAGE);
            rooms(&mut app, R::ShowMoreMembers);
            let panel = app.rooms_ui.members_panel.as_ref().expect("open");
            assert!(panel.shown > MEMBERS_PAGE);
        }

        fn user_pill_item(label: &str, user: &str) -> TimelineItem {
            let details = MessageDetails {
                content: RichText {
                    blocks: vec![Block::Paragraph(vec![Inline::Mention {
                        target: MentionTarget::User(UserId::from(user)),
                        label: label.to_owned(),
                    }])],
                },
                ..MessageDetails::default()
            };
            TimelineItem::new(
                format!("m-{user}"),
                TimelineItemKind::Message {
                    sender: UserId::from("@s:x"),
                    body: label.to_owned(),
                    is_own: false,
                    timestamp_ms: 1,
                    sent: SendStatus::Sent,
                    details: Box::new(details),
                },
            )
        }

        #[test]
        fn the_members_are_asked_for_once_when_a_user_pill_shows() {
            let (mut app, mut rx, _) = rig();
            let plain = TimelineItem::new("x".into(), TimelineItemKind::ReadMarker);
            let _ = app.update(core(CoreEvent::TimelineDiff {
                room_id: id("!a:x"),
                diffs: vec![ListDiff::Append {
                    values: vec![plain],
                }],
            }));
            assert!(
                !drain(&mut rx)
                    .iter()
                    .any(|c| matches!(c, Command::ListRoomMembers { .. })),
                "no pill, no request"
            );
            let diff = |label: &str| CoreEvent::TimelineDiff {
                room_id: id("!a:x"),
                diffs: vec![ListDiff::PushBack {
                    value: user_pill_item(label, "@bob:x"),
                }],
            };
            let _ = app.update(core(diff("Not Bob")));
            let _ = app.update(core(diff("Again")));
            let sent = drain(&mut rx);
            let asked = sent
                .iter()
                .filter(
                    |c| matches!(c, Command::ListRoomMembers { room_id } if *room_id == id("!a:x")),
                )
                .count();
            assert_eq!(asked, 1, "{sent:?}");
        }

        #[test]
        fn a_user_pill_opens_the_card_for_its_target_and_message_starts_a_dm() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(Message::LinkClicked(LinkTarget::User(UserId::from(
                "@bob:x",
            ))));
            let card = app.rooms_ui.card.clone().expect("the card");
            assert_eq!(card.user, UserId::from("@bob:x"));
            assert!(drain(&mut rx).is_empty(), "opening the card sends nothing");
            rooms(&mut app, R::CardMessage);
            rooms(&mut app, R::CardMessage);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(
                matches!(&sent[0], Command::CreateDm { user_id } if user_id.as_str() == "@bob:x")
            );
            let _ = app.update(core(CoreEvent::RoomCreated {
                room_id: id("!dm:x"),
            }));
            assert!(app.rooms_ui.card.is_none(), "the card closes");
        }

        #[test]
        fn the_card_of_your_own_pill_does_not_open() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(Message::LinkClicked(LinkTarget::User(UserId::from("@a:x"))));
            assert!(app.rooms_ui.card.is_none());
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn a_failed_message_from_the_card_can_be_pressed_again() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(Message::LinkClicked(LinkTarget::User(UserId::from(
                "@bob:x",
            ))));
            rooms(&mut app, R::CardMessage);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: id(""),
                action: ActionKind::Rooms,
                error: CoreError::Busy,
            }));
            assert!(app.rooms_ui.card.is_some());
            rooms(&mut app, R::CardMessage);
            assert_eq!(drain(&mut rx).len(), 1);
        }

        #[test]
        fn a_listed_room_pill_selects_the_room() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(Message::LinkClicked(LinkTarget::Room("!b:x".into())));
            assert_eq!(app.selected, Some(id("!b:x")));
            assert!(app.rooms_ui.join_bar.is_none());
            let _ = drain(&mut rx);
        }

        #[test]
        fn an_unjoined_room_pill_asks_before_joining_and_join_sends_once() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(Message::LinkClicked(LinkTarget::Room("#new:x".into())));
            assert!(drain(&mut rx).is_empty(), "clicking joins nothing");
            let bar = app.rooms_ui.join_bar.clone().expect("the bar");
            assert_eq!(bar.id, "#new:x");
            rooms(&mut app, R::JoinBarJoin);
            rooms(&mut app, R::JoinBarJoin);
            let sent = drain(&mut rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(&sent[0], Command::JoinRoom { target, via }
                if target == "#new:x" && via.is_empty()));
            let _ = app.update(core(CoreEvent::RoomJoined {
                room_id: id("!new:x"),
            }));
            assert!(app.rooms_ui.join_bar.is_none());
        }

        #[test]
        fn cancel_closes_the_join_bar_and_junk_pill_targets_open_nothing() {
            let (mut app, mut rx, _) = rig();
            let _ = app.update(Message::LinkClicked(LinkTarget::Room("#new:x".into())));
            rooms(&mut app, R::JoinBarCancel);
            assert!(app.rooms_ui.join_bar.is_none());
            rooms(&mut app, R::JoinBarJoin);
            let _ = app.update(Message::LinkClicked(LinkTarget::Room("junk".into())));
            assert!(app.rooms_ui.join_bar.is_none());
            assert!(drain(&mut rx).is_empty());
            let target: Option<RoomTarget> = crate::links::parse_room_target("junk");
            assert!(target.is_none());
        }

        #[test]
        fn leaving_the_room_view_closes_everything_of_that_room() {
            let (mut app, mut rx, _) = rig();
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::OpenMembers);
            let _ = app.update(Message::LinkClicked(LinkTarget::User(UserId::from(
                "@bob:x",
            ))));
            let _ = app.update(Message::LinkClicked(LinkTarget::Room("#new:x".into())));
            let _ = app.update(Message::SelectRoom(id("!b:x")));
            assert!(app.rooms_ui.settings.is_none());
            assert!(app.rooms_ui.members_panel.is_none());
            assert!(app.rooms_ui.card.is_none());
            assert!(app.rooms_ui.join_bar.is_none());
            assert!(app.rooms_ui.known.is_none());
            let _ = drain(&mut rx);
        }

        #[test]
        fn logging_out_forgets_the_room_screens() {
            let (mut app, _rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::OpenSettings);
            let _ = app.update(core(CoreEvent::LoggedOut));
            assert!(app.rooms_ui.dialog.is_none());
            assert!(app.rooms_ui.settings.is_none());
        }

        #[test]
        fn escape_closes_the_topmost_screen_first() {
            let (mut app, _rx, _) = rig();
            rooms(&mut app, R::OpenSettings);
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::Escape);
            assert!(app.rooms_ui.dialog.is_none());
            assert!(app.rooms_ui.settings.is_some());
            rooms(&mut app, R::Escape);
            assert!(app.rooms_ui.settings.is_none());
        }

        #[test]
        fn debug_output_shows_no_names_topics_or_ids_typed() {
            let (mut app, mut rx, _) = rig();
            rooms(&mut app, R::OpenCreate);
            rooms(&mut app, R::CreateName(t("sentinel-name")));
            rooms(&mut app, R::CreateTopic(t("sentinel-topic")));
            rooms(&mut app, R::CreateAlias(t("sentinel-alias")));
            rooms(&mut app, R::CreateInvitees(t("@sentinel-invitee:x")));
            let shown = format!("{:?}", app.rooms_ui);
            assert!(!shown.contains("sentinel"), "{shown}");
            rooms(&mut app, R::CloseDialog);
            rooms(&mut app, R::OpenDm);
            rooms(&mut app, R::DmInput(t("@sentinel-dm:x")));
            rooms(&mut app, R::CloseDialog);
            rooms(&mut app, R::OpenJoin);
            rooms(&mut app, R::JoinInput(t("#sentinel-join:x")));
            let shown = format!("{:?}", app.rooms_ui);
            assert!(!shown.contains("sentinel"), "{shown}");
            rooms(&mut app, R::CloseDialog);
            open_settings(&mut app, &mut rx, all(true));
            rooms(&mut app, R::NameInput(t("sentinel-room-name")));
            rooms(&mut app, R::TopicInput(t("sentinel-room-topic")));
            let mut with_sentinels = details("!a:x", false, all(true));
            with_sentinels.name = "sentinel-details-name".into();
            with_sentinels.topic = Some("sentinel-details-topic".into());
            let _ = app.update(core(CoreEvent::RoomDetails(with_sentinels)));
            rooms(&mut app, R::OpenMembers);
            rooms(&mut app, R::InviteInput(t("@sentinel-inv:x")));
            let _ = app.update(core(CoreEvent::RoomMembers {
                room_id: id("!a:x"),
                members: vec![member(
                    "@sentinel-member:x",
                    Some("sentinel-person"),
                    Role::User,
                )],
                truncated: false,
            }));
            let _ = app.update(Message::LinkClicked(LinkTarget::Room(
                "#sentinel-pill:x".into(),
            )));
            let shown = format!("{:?} {:?}", app.rooms_ui, app);
            assert!(
                !shown.contains("sentinel-room")
                    && !shown.contains("sentinel-details")
                    && !shown.contains("sentinel-person")
                    && !shown.contains("sentinel-inv")
                    && !shown.contains("sentinel-pill"),
                "{shown}"
            );
            for message in [
                R::NameInput(t("sentinel-msg")),
                R::CreateInvitees(t("sentinel-msg")),
                R::InviteInput(t("sentinel-msg")),
                R::AvatarPicked(Some(PathBuf::from("/sentinel-msg/p.png").into())),
            ] {
                assert!(!format!("{message:?}").contains("sentinel"));
            }
            let _ = drain(&mut rx);
        }
    }
    // ---- Task 13: spaces and threads ----

    mod spaces_and_threads {
        use iced::advanced::widget::{Tree, tree::Tag};
        use twicecat_core::api::{
            ActionKind, MediaKind, PaginationState, RoomListQuery, RoomListTab, SpaceView,
        };

        use super::*;
        use crate::dialogs::RecordingDialogs;
        use crate::spaces::SpacesMessage;
        use crate::threads::ThreadsMessage;

        fn room() -> RoomId {
            RoomId::from("!a:x")
        }

        fn root(n: &str) -> EventId {
            EventId::from(format!("${n}:x"))
        }

        fn space(id: &str, level: u32) -> SpaceView {
            SpaceView {
                id: RoomId::from(id),
                name: id.to_owned(),
                avatar: None,
                level,
                children: 0,
            }
        }

        fn queries(commands: &[Command]) -> Vec<&RoomListQuery> {
            commands
                .iter()
                .filter_map(|c| match c {
                    Command::SetRoomListQuery(q) => Some(q),
                    _ => None,
                })
                .collect()
        }

        fn thread_item(root: &EventId, n: u32, body: &str) -> TimelineItem {
            message_item(
                &format!("thread:{}:{n}", root.as_str()),
                body,
                false,
                ALL_ACTIONS,
            )
        }

        fn thread_diff(root: &EventId, items: Vec<TimelineItem>) -> Message {
            core(CoreEvent::ThreadDiff {
                room_id: room(),
                root: root.clone(),
                diffs: vec![ListDiff::Append { values: items }],
            })
        }

        /// `main_app` with a space list and room `a` selected.
        fn spaces_app() -> (App, mpsc::Receiver<Command>) {
            let (mut app, mut rx) = main_app();
            let _ = app.update(core(CoreEvent::Spaces(vec![
                space("!s1:x", 0),
                space("!s1c:x", 1),
                space("!s2:x", 0),
            ])));
            let _ = drain(&mut rx);
            (app, rx)
        }

        /// `main_app` with the thread `$r:x` open.
        fn thread_app() -> (App, mpsc::Receiver<Command>) {
            let (mut app, mut rx) = main_app();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            let _ = drain(&mut rx);
            (app, rx)
        }

        fn open_panel(app: &App) -> &crate::threads::ThreadPanel {
            app.threads.panel.as_ref().expect("an open thread")
        }

        // ---- spaces ----

        #[test]
        fn selecting_a_space_filters_the_list_and_keeps_tab_and_search() {
            let (mut app, mut rx) = spaces_app();
            let _ = app.update(Message::TabSelected(RoomListTab::People));
            let _ = app.update(Message::SearchChanged("  lo ".into()));
            let _ = drain(&mut rx);
            let _ = app.update(Message::Spaces(SpacesMessage::Select(Some(RoomId::from(
                "!s1:x",
            )))));
            let sent = drain(&mut rx);
            let [q] = queries(&sent)[..] else {
                panic!("one query expected: {sent:?}")
            };
            assert_eq!(q.space, Some(RoomId::from("!s1:x")));
            assert_eq!(q.tab, RoomListTab::People);
            assert_eq!(q.search, "lo");
            // Back to Home: the tab and the search stay.
            let _ = app.update(Message::Spaces(SpacesMessage::Select(None)));
            let sent = drain(&mut rx);
            let [q] = queries(&sent)[..] else {
                panic!("one query expected: {sent:?}")
            };
            assert_eq!(q.space, None);
            assert_eq!((q.tab, q.search.as_str()), (RoomListTab::People, "lo"));
        }

        #[test]
        fn selecting_the_same_space_again_or_an_unknown_one_sends_nothing() {
            let (mut app, mut rx) = spaces_app();
            let s1 = Some(RoomId::from("!s1:x"));
            let _ = app.update(Message::Spaces(SpacesMessage::Select(s1.clone())));
            let _ = drain(&mut rx);
            let _ = app.update(Message::Spaces(SpacesMessage::Select(s1)));
            let _ = app.update(Message::Spaces(SpacesMessage::Select(Some(RoomId::from(
                "!nope:x",
            )))));
            assert!(drain(&mut rx).is_empty());
            assert_eq!(app.query.space, Some(RoomId::from("!s1:x")));
        }

        #[test]
        fn a_space_that_vanishes_resets_the_filter_to_home() {
            let (mut app, mut rx) = spaces_app();
            let _ = app.update(Message::TabSelected(RoomListTab::Rooms));
            let _ = app.update(Message::Spaces(SpacesMessage::Select(Some(RoomId::from(
                "!s2:x",
            )))));
            let _ = drain(&mut rx);
            // Still there: nothing happens.
            let _ = app.update(core(CoreEvent::Spaces(vec![space("!s2:x", 0)])));
            assert!(queries(&drain(&mut rx)).is_empty());
            // Gone: Home, with the tab kept.
            let _ = app.update(core(CoreEvent::Spaces(vec![space("!s1:x", 0)])));
            let sent = drain(&mut rx);
            let [q] = queries(&sent)[..] else {
                panic!("one query expected: {sent:?}")
            };
            assert_eq!((q.space.clone(), q.tab), (None, RoomListTab::Rooms));
            assert_eq!(app.query.space, None);
        }

        #[test]
        fn space_avatars_are_requested_once() {
            let (mut app, mut rx) = main_app();
            let mut s = space("!s1:x", 0);
            s.avatar = Some(twicecat_core::api::AvatarId::from("mxc://x/a"));
            let _ = app.update(core(CoreEvent::Spaces(vec![s.clone()])));
            let first = drain(&mut rx);
            assert!(matches!(first.as_slice(), [Command::FetchAvatars(ids)] if ids.len() == 1));
            let _ = app.update(core(CoreEvent::Spaces(vec![s])));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn logging_out_forgets_the_spaces_and_the_thread() {
            let (mut app, _rx) = spaces_app();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            let _ = app.update(core(CoreEvent::LoggedOut));
            assert!(app.spaces.is_empty());
            assert!(app.threads.panel.is_none());
        }

        // ---- the thread panel ----

        #[test]
        fn opening_a_thread_sends_open_thread_once() {
            let (mut app, mut rx) = main_app();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            let sent = drain(&mut rx);
            assert!(
                matches!(
                    sent.as_slice(),
                    [Command::OpenThread { room_id, root: r }] if *room_id == room() && r.as_str() == "$r:x"
                ),
                "{sent:?}"
            );
            assert!(open_panel(&app).is(&room(), &root("r")));
            // Clicking the summary again does not reopen it.
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn without_a_room_nothing_opens() {
            let (mut app, mut rx) = login_app();
            logged_in(&mut app);
            let _ = drain(&mut rx);
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            assert!(drain(&mut rx).is_empty() && app.threads.panel.is_none());
        }

        #[test]
        fn thread_diffs_go_to_the_panel_and_not_to_the_room_timeline() {
            let (mut app, _rx) = thread_app();
            let r = root("r");
            let before = app.timeline.items.len();
            let _ = app.update(thread_diff(
                &r,
                vec![thread_item(&r, 0, "root"), thread_item(&r, 1, "reply")],
            ));
            assert_eq!(open_panel(&app).timeline.items.len(), 2);
            assert_eq!(app.timeline.items.len(), before);
            // A second batch applies in order.
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 2, "more")]));
            assert_eq!(open_panel(&app).timeline.items.len(), 3);
        }

        #[test]
        fn an_image_in_the_thread_panel_asks_for_its_thumbnail() {
            let (mut app, mut rx) = thread_app();
            let r = root("r");
            let image = media_item(
                &format!("thread:{}:0", r.as_str()),
                MediaKind::Image,
                "pic.png",
                640,
                480,
            );
            let _ = app.update(thread_diff(&r, vec![image]));
            let asked = fetches(&drain(&mut rx), MediaSize::Thumbnail);
            assert_eq!(asked, vec!["thread:$r:x:0".to_owned()]);
        }

        #[test]
        fn events_of_another_thread_or_room_are_ignored() {
            let (mut app, mut rx) = thread_app();
            let other = root("other");
            let _ = app.update(thread_diff(&other, vec![thread_item(&other, 0, "x")]));
            let _ = app.update(core(CoreEvent::ThreadDiff {
                room_id: RoomId::from("!b:x"),
                root: root("r"),
                diffs: vec![ListDiff::Append {
                    values: vec![thread_item(&root("r"), 0, "x")],
                }],
            }));
            let _ = app.update(core(CoreEvent::ThreadPagination {
                room_id: room(),
                root: other,
                state: PaginationState::ReachedStart,
            }));
            assert!(open_panel(&app).timeline.items.is_empty());
            assert_eq!(open_panel(&app).timeline.pagination, PaginationState::Idle);
            assert!(drain(&mut rx).is_empty());
            // Nothing open: nothing happens either.
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            let _ = drain(&mut rx);
            let r = root("r");
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 0, "late")]));
            assert!(app.threads.panel.is_none());
        }

        #[test]
        fn pagination_state_applies_to_the_open_thread() {
            let (mut app, _rx) = thread_app();
            let _ = app.update(core(CoreEvent::ThreadPagination {
                room_id: room(),
                root: root("r"),
                state: PaginationState::ReachedStart,
            }));
            assert_eq!(
                open_panel(&app).timeline.pagination,
                PaginationState::ReachedStart
            );
        }

        #[test]
        fn a_new_root_starts_with_a_fresh_model_and_one_open_thread() {
            let (mut app, mut rx) = thread_app();
            let r = root("r");
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 0, "old")]));
            let _ = app.update(Message::Threads(ThreadsMessage::Draft(MessageText::from(
                "half typed",
            ))));
            // A short thread asks for older events by itself.
            assert!(
                drain(&mut rx)
                    .iter()
                    .any(|c| matches!(c, Command::PaginateThread { .. }))
            );
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("s"))));
            let sent = drain(&mut rx);
            assert!(
                matches!(sent.as_slice(), [Command::OpenThread { root: n, .. }] if n.as_str() == "$s:x"),
                "{sent:?}"
            );
            let panel = open_panel(&app);
            assert!(panel.is(&room(), &root("s")));
            assert!(panel.timeline.items.is_empty() && panel.draft.is_empty());
            // A straggler of the old thread cannot get in.
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 1, "late")]));
            assert!(open_panel(&app).timeline.items.is_empty());
        }

        #[test]
        fn closing_sends_close_thread_and_drops_the_model() {
            let (mut app, mut rx) = thread_app();
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            assert!(matches!(drain(&mut rx).as_slice(), [Command::CloseThread]));
            assert!(app.threads.panel.is_none());
            // A second close has nothing to close.
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn selecting_another_room_closes_the_panel() {
            let (mut app, mut rx) = thread_app();
            let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
            let sent = drain(&mut rx);
            let close = sent
                .iter()
                .position(|c| matches!(c, Command::CloseThread))
                .expect("CloseThread");
            let select = sent
                .iter()
                .position(|c| matches!(c, Command::SelectRoom(_)))
                .expect("SelectRoom");
            assert!(close < select, "the thread closes before the room changes");
            assert!(app.threads.panel.is_none());
        }

        #[test]
        fn the_panel_composer_sends_into_the_thread() {
            let (mut app, mut rx) = thread_app();
            let _ = app.update(Message::Threads(ThreadsMessage::Draft(MessageText::from(
                "in the thread",
            ))));
            let _ = app.update(Message::Threads(ThreadsMessage::Submit));
            let sent = drain(&mut rx);
            let [Command::Send(m)] = sent.as_slice() else {
                panic!("{sent:?}")
            };
            assert_eq!(m.text.expose(), "in the thread");
            assert_eq!(m.thread_root.as_ref().map(EventId::as_str), Some("$r:x"));
            assert_eq!(m.room_id, room());
            assert!(open_panel(&app).draft.is_empty());
            // Blank text sends nothing.
            let _ = app.update(Message::Threads(ThreadsMessage::Submit));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn actions_on_panel_items_carry_their_thread_ids() {
            let (mut app, mut rx) = thread_app();
            let r = root("r");
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 4, "reply")]));
            let id = "thread:$r:x:4".to_owned();
            let _ = drain(&mut rx);
            let _ = app.update(Message::ReactionToggled {
                item: id.clone(),
                key: MessageText::from("\u{1F44D}"),
            });
            let _ = app.update(Message::DeleteRequested(id.clone()));
            let _ = app.update(Message::DeleteConfirmed);
            let sent = drain(&mut rx);
            assert!(
                matches!(&sent[0], Command::ToggleReaction { room_id, item_id, .. } if *room_id == room() && *item_id == id),
                "{sent:?}"
            );
            assert!(
                matches!(&sent[1], Command::Redact { room_id, item_id, .. } if *room_id == room() && *item_id == id),
                "{sent:?}"
            );
        }

        #[test]
        fn reply_and_edit_stay_with_the_room_composer() {
            let (mut app, _rx) = thread_app();
            let r = root("r");
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 0, "reply")]));
            let _ = app.update(Message::ReplyRequested("thread:$r:x:0".into()));
            let _ = app.update(Message::EditRequested("thread:$r:x:0".into()));
            assert!(matches!(app.composer.mode, composer::Mode::Normal));
        }

        #[test]
        fn thread_media_opens_with_its_thread_id() {
            let (mut app, mut rx) = thread_app();
            let r = root("r");
            let mut image = media_item("thread:$r:x:0", MediaKind::Image, "pic.png", 64, 48);
            // The same counter in the room's own timeline is another key.
            let mut main = media_item("0", MediaKind::Image, "pic.png", 64, 48);
            std::mem::swap(&mut image, &mut main);
            let _ = app.update(core(CoreEvent::TimelineDiff {
                room_id: room(),
                diffs: vec![ListDiff::Append {
                    values: vec![image],
                }],
            }));
            let _ = app.update(thread_diff(&r, vec![main]));
            let _ = drain(&mut rx);
            let _ = app.update(Message::MediaClicked("thread:$r:x:0".into()));
            let viewer = app.media.viewer.as_ref().expect("viewer");
            assert_eq!(viewer.key, (room(), "thread:$r:x:0".to_owned()));
            assert!(matches!(
                drain(&mut rx).as_slice(),
                [Command::FetchMedia { item_id, .. }] if item_id == "thread:$r:x:0"
            ));
        }

        #[test]
        fn a_failed_thread_action_says_so_and_keeps_the_panel() {
            let (mut app, _rx) = thread_app();
            let _ = app.update(core(CoreEvent::ActionFailed {
                room_id: room(),
                action: ActionKind::Thread,
                error: CoreError::Internal("x".into()),
            }));
            assert!(app.actions.notice.is_some());
            assert!(app.threads.panel.is_some());
        }

        // ---- uploads from the panel ----

        #[test]
        fn attach_in_the_panel_targets_the_thread_and_the_room_attach_does_not() {
            let (mut app, mut rx) = thread_app();
            app.set_dialogs(Box::new(RecordingDialogs::default()));
            let _ = app.update(Message::Threads(ThreadsMessage::Attach));
            assert_eq!(
                app.uploads.thread_root.as_ref().map(EventId::as_str),
                Some("$r:x")
            );
            // The room's own Attach (no confirmation open) goes to the room.
            let _ = app.update(Message::AttachRequested);
            assert_eq!(app.uploads.thread_root, None);
            let _ = drain(&mut rx);
        }

        #[test]
        fn closing_the_panel_drops_its_upload_confirmation() {
            let (mut app, _rx) = thread_app();
            app.set_dialogs(Box::new(RecordingDialogs::default()));
            let _ = app.update(Message::Threads(ThreadsMessage::Attach));
            app.uploads.dialog = Some(crate::upload::UploadDialog {
                room_id: room(),
                entries: Vec::new(),
            });
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            assert!(app.uploads.dialog.is_none() && app.uploads.thread_root.is_none());
        }

        // ---- the room pane is not disturbed ----

        #[test]
        fn opening_and_closing_the_panel_keeps_the_room_scroll_and_draft() {
            let (mut app, mut rx) = main_app();
            type_in(&mut app, "half a message");
            let _ = app.on_timeline_scrolled(120.0, 600.0, 800.0);
            let _ = drain(&mut rx);
            let before = (
                app.timeline.offset_from_bottom,
                app.timeline.anchor.clone(),
                app.timeline.last_scroll_request,
                app.timeline.items.len(),
                app.composer.text(),
            );
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            let r = root("r");
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 0, "x")]));
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            let after = (
                app.timeline.offset_from_bottom,
                app.timeline.anchor.clone(),
                app.timeline.last_scroll_request,
                app.timeline.items.len(),
                app.composer.text(),
            );
            assert_eq!(before, after);
            assert!(
                !drain(&mut rx).iter().any(|c| matches!(
                    c,
                    Command::SaveDraft { .. } | Command::PaginateBackwards { .. }
                )),
                "the panel sends no room commands"
            );
        }

        /// Every widget tag of a tree, depth first.
        fn tags(tree: &Tree, out: &mut Vec<Tag>) {
            out.push(tree.tag);
            for child in &tree.children {
                tags(child, out);
            }
        }

        #[test]
        fn the_panes_row_keeps_five_slots_and_the_room_pane_its_tree() {
            let (mut app, _rx) = main_app();
            let slots = |app: &App| {
                let element = crate::screens::main_view::main_row(app);
                let tree = Tree::new(element.as_widget());
                // The room pane is the column `[header, timeline, bars,
                // editor]`. The header changes
                // shape with the pane's width (it stacks when narrow), so
                // what must keep its place is everything under it.
                let mut room_tags = Vec::new();
                for part in &tree.children[2].children[1..] {
                    tags(part, &mut room_tags);
                }
                (tree.children.len(), room_tags)
            };
            let (n_closed, room_closed) = slots(&app);
            assert_eq!(n_closed, 5);
            let _ = app.update(Message::Threads(ThreadsMessage::Open(root("r"))));
            let (n_open, room_open) = slots(&app);
            assert_eq!(n_open, 5);
            assert_eq!(room_open, room_closed, "the room pane keeps its widgets");
            let _ = app.update(core(CoreEvent::Spaces(vec![space("!s1:x", 0)])));
            let (n_spaces, room_spaces) = slots(&app);
            assert_eq!(n_spaces, 5);
            assert_eq!(room_spaces, room_closed);
            // In a call in the open room, the participants panel comes and
            // goes in its own slot.
            let _ = app.update(core(CoreEvent::CallState(
                twicecat_core::api::CallState::Joining {
                    room_id: RoomId::from("!a:x"),
                },
            )));
            let (_, room_in_call) = slots(&app);
            let _ = app.update(Message::Call(crate::calls::CallMessage::TogglePanel));
            assert!(app.calls.panel_shown(app.selected.as_ref()));
            let (n_panel, room_panel) = slots(&app);
            assert_eq!(n_panel, 5);
            assert_eq!(
                room_panel, room_in_call,
                "the panel leaves the room pane alone"
            );
        }

        /// The widths of the panes row and of its five slots, laid out in a
        /// 1000 x 700 window.
        fn pane_widths(app: &App) -> (f32, Vec<f32>) {
            use iced::advanced::layout::Limits;
            use iced::advanced::renderer::Headless;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime");
            let renderer = runtime
                .block_on(iced::Renderer::new(
                    iced::Font::DEFAULT,
                    iced::Pixels(16.0),
                    Some("tiny-skia"),
                ))
                .expect("the headless tiny-skia renderer is not available");
            let mut element = crate::screens::main_view::main_row(app);
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &Limits::new(iced::Size::ZERO, iced::Size::new(1000.0, 700.0)),
            );
            let slots = node.children().iter().map(|c| c.size().width).collect();
            (node.size().width, slots)
        }

        /// The panes row was built with `Row::from_vec`, which does not look
        /// at its children, and only told to fill the height: it shrank to its
        /// content and the room pane lost the free width.
        #[test]
        fn the_panes_fill_the_window_with_and_without_a_thread() {
            let (mut app, _rx) = spaces_app();
            let (row, slots) = pane_widths(&app);
            assert_eq!(row, 1000.0, "the row fills the window: {slots:?}");
            let room_alone = slots[2];
            assert_eq!(slots[3], 0.0, "no thread, no panel");
            let r = root("r");
            let _ = app.update(Message::Threads(ThreadsMessage::Open(r.clone())));
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 0, "x")]));
            let (row, slots) = pane_widths(&app);
            assert_eq!(row, 1000.0, "the row fills the window: {slots:?}");
            assert!(slots[3] > 0.0, "the panel has its width: {slots:?}");
            assert_eq!(
                slots[2],
                room_alone - slots[3],
                "the panel takes its width from the room pane only: {slots:?}"
            );
        }

        #[test]
        fn the_participants_panel_takes_its_width_from_the_room_pane() {
            use twicecat_core::api::{CallState, CleaningStatus, ListDiff, Participant};
            let (mut app, _rx) = spaces_app();
            let room = app.selected.clone().expect("a room is open");
            let (_, alone) = pane_widths(&app);
            let _ = app.update(core(CoreEvent::CallState(CallState::Connected {
                room_id: room.clone(),
                encrypted: true,
            })));
            let person = |n: &str, local: bool| Participant {
                id: twicecat_core::api::ParticipantId::from(n),
                user_id: UserId::from(format!("@{n}:x")),
                device_id: DeviceId::from("D"),
                display_name: Some(n.repeat(40)),
                is_local: local,
                muted: !local,
                speaking: local,
                volume_percent: 100,
                cleaning: CleaningStatus::Cleaned,
            };
            let _ = app.update(core(CoreEvent::CallParticipants {
                room_id: room,
                diffs: vec![ListDiff::Reset {
                    values: vec![person("a", true), person("b", false)],
                }],
            }));
            let _ = app.update(core(CoreEvent::Error(CoreError::Call(
                "a long reason ".repeat(20),
            ))));
            let _ = app.update(Message::Call(crate::calls::CallMessage::TogglePanel));
            let (row, slots) = pane_widths(&app);
            assert_eq!(row, 1000.0, "the row fills the window: {slots:?}");
            assert_eq!(slots[4], crate::calls::PANEL_WIDTH, "{slots:?}");
            assert_eq!(
                slots[1], alone[1],
                "the call bar keeps the list's width: {slots:?}"
            );
            assert_eq!(slots[2], alone[2] - slots[4], "{slots:?}");
            assert_eq!(laid_out_size(&app), iced::Size::new(1000.0, 700.0));
        }

        #[test]
        fn the_whole_window_builds_with_spaces_and_a_thread() {
            let (mut app, _rx) = spaces_app();
            let r = root("r");
            let _ = app.update(Message::Threads(ThreadsMessage::Open(r.clone())));
            let _ = app.update(thread_diff(&r, vec![thread_item(&r, 0, "x")]));
            let _ = app.view();
        }
    }

    // ---- Phase 5 Task 15: the Settings screen ----

    mod settings_ui {
        use std::path::PathBuf;

        use twicecat_core::api::{
            AvatarId, EventId, MediaBytes, NotificationKind, NotificationSettingsView,
            OwnProfileView, RoomNotificationModeView as Mode, ThemeChoice,
        };

        use super::*;
        use crate::dialogs::RecordingDialogs;
        use crate::notifications::RecordingNotifier;
        use crate::rooms::{AvatarRefusal, AvatarUpload};
        use crate::settings::{
            RecordingSettings, SettingsError, SettingsMessage as S, check_keyword,
        };
        use crate::tray::{FakeTray, FakeTrayFactory};

        struct Rig {
            app: App,
            rx: mpsc::Receiver<Command>,
            dialogs: RecordingDialogs,
            saved: Arc<std::sync::Mutex<Vec<UiSettings>>>,
            shown: Arc<std::sync::Mutex<Vec<crate::notifications::Request>>>,
            tray: FakeTray,
        }

        fn rig() -> Rig {
            let (mut app, rx) = main_app();
            let dialogs = RecordingDialogs::default();
            app.set_dialogs(Box::new(dialogs.clone()));
            let store = RecordingSettings::default();
            let saved = store.saved.clone();
            app.set_settings_store(Box::new(store));
            let notifier = RecordingNotifier::default();
            let shown = notifier.shown.clone();
            app.set_notifier(Box::new(notifier));
            let factory = FakeTrayFactory::default();
            let tray = factory.tray.clone();
            app.set_tray_factory(Box::new(factory));
            Rig {
                app,
                rx,
                dialogs,
                saved,
                shown,
                tray,
            }
        }

        fn open(rig: &mut Rig) {
            let _ = rig.app.update(Message::Settings(S::Open));
            assert!(rig.app.settings_ui.open);
        }

        fn st(rig: &mut Rig, message: S) {
            let _ = rig.app.update(Message::Settings(message));
        }

        fn profile(name: Option<&str>, avatar: bool) -> CoreEvent {
            CoreEvent::OwnProfile(OwnProfileView {
                display_name: name.map(str::to_owned),
                avatar: avatar.then(|| AvatarId::from("mxc://x/a")),
            })
        }

        fn settings_event(group: Mode, dm: Mode, keywords: &[&str]) -> CoreEvent {
            CoreEvent::NotificationSettings(NotificationSettingsView {
                group_default: group,
                dm_default: dm,
                keywords: keywords.iter().map(|k| (*k).to_owned()).collect(),
            })
        }

        fn text(s: &str) -> MessageText {
            MessageText::from(s)
        }

        fn writes(rig: &Rig) -> Vec<UiSettings> {
            rig.saved.lock().expect("test mutex").clone()
        }

        #[test]
        fn settings_open_from_the_button_and_close_again() {
            let mut rig = rig();
            assert!(!rig.app.settings_open());
            open(&mut rig);
            st(&mut rig, S::Close);
            assert!(!rig.app.settings_open());
            assert!(drain(&mut rig.rx).is_empty(), "opening sends nothing");
        }

        #[test]
        fn settings_open_on_profile_and_the_page_list_changes_the_page() {
            use crate::settings::SettingsPage;
            let mut rig = rig();
            open(&mut rig);
            assert_eq!(rig.app.settings_ui.page, SettingsPage::Profile);
            for page in SettingsPage::ALL {
                st(&mut rig, S::Page(page));
                assert_eq!(rig.app.settings_ui.page, page);
            }
            // Closing and opening again (Ctrl+, too) starts at Profile.
            st(&mut rig, S::Page(SettingsPage::About));
            st(&mut rig, S::Close);
            open(&mut rig);
            assert_eq!(rig.app.settings_ui.page, SettingsPage::Profile);
            assert!(drain(&mut rig.rx).is_empty(), "a page change sends nothing");
        }

        #[test]
        fn a_page_change_while_settings_is_shut_does_nothing() {
            use crate::settings::SettingsPage;
            let mut rig = rig();
            st(&mut rig, S::Page(SettingsPage::About));
            assert_eq!(rig.app.settings_ui.page, SettingsPage::Profile);
        }

        #[test]
        fn a_page_change_keeps_the_unsaved_name() {
            use crate::settings::SettingsPage;
            let mut rig = rig();
            open(&mut rig);
            st(&mut rig, S::NameInput(text("Alicia")));
            st(&mut rig, S::Page(SettingsPage::Notifications));
            st(&mut rig, S::Page(SettingsPage::Profile));
            assert_eq!(rig.app.settings_ui.name.expose(), "Alicia");
            assert!(rig.app.settings_ui.name_edited);
        }

        #[test]
        fn the_pages_and_the_messages_they_send_cover_every_message() {
            use crate::settings::SettingsPage;
            // The pages own distinct messages.
            let mut owned = std::collections::BTreeSet::new();
            for page in SettingsPage::ALL {
                for name in page.sends() {
                    assert!(owned.insert(*name), "{name} is on two pages");
                }
            }
            // Every message is a page's, the card's, or comes from a task or
            // a key (never from a control of a page).
            for name in S::ALL_NAMES {
                let known = owned.contains(name)
                    || S::CARD_WIDE.contains(&name)
                    || S::NOT_FROM_A_CONTROL.contains(&name);
                assert!(known, "{name} belongs to no page");
            }
            for name in owned {
                assert!(S::ALL_NAMES.contains(&name), "{name} is not a message");
            }
        }

        #[test]
        fn settings_do_not_open_on_the_login_screen_or_over_another_modal() {
            let (mut app, _rx) = login_app();
            let _ = app.update(Message::Settings(S::Open));
            assert!(!app.settings_open());

            let mut rig = rig();
            let _ = rig.app.update(Message::Sessions(SessionsMessage::Open));
            let _ = rig.app.update(Message::Settings(S::Open));
            assert!(!rig.app.settings_open(), "Sessions is open");
        }

        #[test]
        fn the_profile_fills_the_name_field_until_the_user_edits_it() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("Alice"), false)));
            assert_eq!(rig.app.settings_ui.name.expose(), "Alice");
            st(&mut rig, S::NameInput(text("Alicia")));
            let _ = rig.app.update(core(profile(Some("Alice B"), false)));
            assert_eq!(rig.app.settings_ui.name.expose(), "Alicia");
        }

        #[test]
        fn saving_the_name_sends_one_command() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("Alice"), false)));
            st(&mut rig, S::NameInput(text("  Alicia  ")));
            st(&mut rig, S::SaveName);
            st(&mut rig, S::SaveName);
            let sent = drain(&mut rig.rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(&sent[0], Command::SetDisplayName { name } if name == "Alicia"));
        }

        #[test]
        fn an_unchanged_name_sends_nothing_and_a_bad_one_is_refused() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("Alice"), false)));
            st(&mut rig, S::NameInput(text("Alice")));
            st(&mut rig, S::SaveName);
            assert!(drain(&mut rig.rx).is_empty());
            for (bad, error) in [
                ("   ", SettingsError::NameEmpty),
                (&"a".repeat(256), SettingsError::NameTooLong),
                ("a\u{7}b", SettingsError::NameUnsafe),
                ("a\u{202E}b", SettingsError::NameUnsafe),
            ] {
                st(&mut rig, S::NameInput(text(bad)));
                st(&mut rig, S::SaveName);
                assert_eq!(rig.app.settings_ui.error, Some(error), "{bad:?}");
            }
            assert!(drain(&mut rig.rx).is_empty());
        }

        #[test]
        fn the_picture_goes_through_the_picker_and_is_sent_once() {
            let mut rig = rig();
            st(&mut rig, S::ChooseAvatar);
            st(&mut rig, S::ChooseAvatar);
            assert_eq!(*rig.dialogs.images.lock().expect("mutex"), 1);
            st(&mut rig, S::AvatarPicked(None));
            assert!(!rig.app.settings_ui.avatar_busy);
            st(&mut rig, S::ChooseAvatar);
            st(
                &mut rig,
                S::AvatarPicked(Some(PathBuf::from("/nonexistent/p.png").into())),
            );
            let ready = S::AvatarReady(Ok(Box::new(AvatarUpload {
                data: vec![1, 2, 3],
                mimetype: "image/png",
                width: 2,
                height: 2,
            })));
            st(&mut rig, ready.clone());
            st(&mut rig, ready);
            let sent = drain(&mut rig.rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            match &sent[0] {
                Command::SetAvatar { data, mimetype } => {
                    assert_eq!(data, &MediaBytes::from(vec![1, 2, 3]));
                    assert_eq!(mimetype, "image/png");
                }
                other => panic!("{other:?}"),
            }
        }

        #[test]
        fn a_refused_picture_shows_a_line_and_sends_nothing() {
            let mut rig = rig();
            st(&mut rig, S::ChooseAvatar);
            st(&mut rig, S::AvatarReady(Err(AvatarRefusal::NotAPicture)));
            assert_eq!(
                rig.app.settings_ui.error,
                Some(SettingsError::Picture(AvatarRefusal::NotAPicture))
            );
            assert!(!rig.app.settings_ui.avatar_busy);
            assert!(drain(&mut rig.rx).is_empty());
        }

        #[test]
        fn remove_picture_needs_a_picture_and_sends_once() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("A"), false)));
            st(&mut rig, S::RemoveAvatar);
            assert!(drain(&mut rig.rx).is_empty(), "there is no picture");
            let _ = rig.app.update(core(profile(Some("A"), true)));
            st(&mut rig, S::RemoveAvatar);
            st(&mut rig, S::RemoveAvatar);
            let sent = drain(&mut rig.rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(sent[0], Command::RemoveAvatar));
        }

        #[test]
        fn a_failed_profile_change_shows_a_line_and_frees_the_buttons() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("A"), true)));
            st(&mut rig, S::RemoveAvatar);
            assert!(rig.app.settings_ui.profile_pending);
            let _ = rig.app.update(core(CoreEvent::ActionFailed {
                room_id: RoomId::from(""),
                action: ActionKind::Profile,
                error: CoreError::Internal("x".into()),
            }));
            assert_eq!(
                rig.app.settings_ui.error,
                Some(SettingsError::ProfileFailed)
            );
            assert!(!rig.app.settings_ui.profile_pending);
        }

        #[test]
        fn a_default_mode_sends_once_and_the_same_mode_sends_nothing() {
            let mut rig = rig();
            let _ = rig.app.update(core(settings_event(
                Mode::AllMessages,
                Mode::AllMessages,
                &[],
            )));
            let pick = S::DefaultMode {
                one_to_one: false,
                mode: Mode::Mute,
            };
            st(&mut rig, pick.clone());
            st(&mut rig, pick);
            st(
                &mut rig,
                S::DefaultMode {
                    one_to_one: true,
                    mode: Mode::AllMessages,
                },
            );
            let sent = drain(&mut rig.rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(
                sent[0],
                Command::SetDefaultNotificationMode {
                    one_to_one: false,
                    mode: Mode::Mute
                }
            ));
        }

        #[test]
        fn a_late_echo_of_the_old_mode_does_not_undo_the_pick() {
            let mut rig = rig();
            let shown = |rig: &Rig| {
                rig.app
                    .settings_ui
                    .shown_default(rig.app.notification_settings.as_ref(), false)
            };
            let event = |mode| core(settings_event(mode, Mode::AllMessages, &[]));
            let _ = rig.app.update(event(Mode::AllMessages));
            st(
                &mut rig,
                S::DefaultMode {
                    one_to_one: false,
                    mode: Mode::MentionsAndKeywords,
                },
            );
            // The first events after the change still carry the old value.
            let _ = rig.app.update(event(Mode::AllMessages));
            assert_eq!(shown(&rig), Some(Mode::MentionsAndKeywords));
            let _ = rig.app.update(event(Mode::AllMessages));
            assert_eq!(shown(&rig), Some(Mode::MentionsAndKeywords));
            // The event that agrees settles it.
            let _ = rig.app.update(event(Mode::MentionsAndKeywords));
            assert!(rig.app.settings_ui.pending_group.is_none());
            assert_eq!(shown(&rig), Some(Mode::MentionsAndKeywords));
        }

        #[test]
        fn a_change_the_core_never_confirms_is_not_shown_for_ever() {
            let mut rig = rig();
            let event = || core(settings_event(Mode::AllMessages, Mode::AllMessages, &[]));
            let _ = rig.app.update(event());
            st(
                &mut rig,
                S::DefaultMode {
                    one_to_one: false,
                    mode: Mode::Mute,
                },
            );
            for _ in 0..crate::settings::MAX_STALE_EVENTS {
                let _ = rig.app.update(event());
            }
            assert_eq!(
                rig.app
                    .settings_ui
                    .shown_default(rig.app.notification_settings.as_ref(), false),
                Some(Mode::AllMessages)
            );
        }

        #[test]
        fn a_failed_default_mode_shows_what_the_core_reports_again() {
            let mut rig = rig();
            let _ = rig.app.update(core(settings_event(
                Mode::AllMessages,
                Mode::AllMessages,
                &[],
            )));
            st(
                &mut rig,
                S::DefaultMode {
                    one_to_one: true,
                    mode: Mode::Mute,
                },
            );
            let _ = rig.app.update(core(CoreEvent::ActionFailed {
                room_id: RoomId::from(""),
                action: ActionKind::Notifications,
                error: CoreError::Internal("x".into()),
            }));
            assert_eq!(
                rig.app
                    .settings_ui
                    .shown_default(rig.app.notification_settings.as_ref(), true),
                Some(Mode::AllMessages)
            );
            assert_eq!(
                rig.app.settings_ui.error,
                Some(SettingsError::NotificationFailed)
            );
        }

        #[test]
        fn a_keyword_is_validated_sent_once_and_the_field_cleared() {
            let mut rig = rig();
            st(&mut rig, S::KeywordInput(text("  lunch  ")));
            st(&mut rig, S::AddKeyword);
            st(&mut rig, S::AddKeyword);
            let sent = drain(&mut rig.rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(&sent[0], Command::AddKeyword { keyword } if keyword == "lunch"));
            assert_eq!(rig.app.settings_ui.keyword_input.expose(), "");
        }

        #[test]
        fn a_keyword_over_64_characters_is_refused() {
            let mut rig = rig();
            st(&mut rig, S::KeywordInput(text(&"k".repeat(65))));
            st(&mut rig, S::AddKeyword);
            assert!(drain(&mut rig.rx).is_empty());
            assert_eq!(
                rig.app.settings_ui.error,
                Some(SettingsError::KeywordTooLong)
            );
            // The field keeps the text so it can be shortened.
            assert_eq!(rig.app.settings_ui.keyword_input.expose().len(), 65);
            st(&mut rig, S::KeywordInput(text(&"k".repeat(64))));
            st(&mut rig, S::AddKeyword);
            assert_eq!(drain(&mut rig.rx).len(), 1, "64 characters are fine");
        }

        #[test]
        fn keyword_checks_cover_blank_control_limit_and_duplicates() {
            assert_eq!(check_keyword("  ", 0, false), Ok(None));
            assert_eq!(
                check_keyword("a\nb", 0, false),
                Err(SettingsError::KeywordUnsafe)
            );
            assert_eq!(
                check_keyword("é".repeat(64).as_str(), 0, false).map(|k| k.is_some()),
                Ok(true)
            );
            assert_eq!(
                check_keyword("x", 50, false),
                Err(SettingsError::TooManyKeywords)
            );
            assert_eq!(check_keyword("x", 50, true), Ok(Some("x".to_owned())));
        }

        #[test]
        fn a_known_keyword_is_not_sent_again_and_remove_sends_only_known_ones() {
            let mut rig = rig();
            let _ = rig.app.update(core(settings_event(
                Mode::AllMessages,
                Mode::AllMessages,
                &["lunch"],
            )));
            st(&mut rig, S::KeywordInput(text("lunch")));
            st(&mut rig, S::AddKeyword);
            st(&mut rig, S::RemoveKeyword(text("dinner")));
            assert!(drain(&mut rig.rx).is_empty());
            st(&mut rig, S::RemoveKeyword(text("lunch")));
            let sent = drain(&mut rig.rx);
            assert_eq!(sent.len(), 1, "{sent:?}");
            assert!(matches!(&sent[0], Command::RemoveKeyword { keyword } if keyword == "lunch"));
        }

        #[test]
        fn each_local_toggle_writes_the_settings_once() {
            let mut rig = rig();
            st(&mut rig, S::DesktopNotifications(false));
            st(&mut rig, S::DesktopNotifications(false));
            st(&mut rig, S::ShowMessageContent(false));
            st(&mut rig, S::TrayBadge(false));
            st(&mut rig, S::Theme(ThemeChoice::Dark));
            st(&mut rig, S::Theme(ThemeChoice::Dark));
            let saved = writes(&rig);
            assert_eq!(saved.len(), 4, "{saved:?}");
            assert_eq!(
                saved[3],
                UiSettings {
                    theme: ThemeChoice::Dark,
                    desktop_notifications: false,
                    show_message_content: false,
                    tray_badge: false,
                    ..UiSettings::default()
                }
            );
            assert!(
                drain(&mut rig.rx).is_empty(),
                "local settings are not commands"
            );
        }

        #[test]
        fn a_failed_write_shows_a_line() {
            let mut rig = rig();
            st(&mut rig, S::Saved(true));
            assert!(rig.app.settings_ui.error.is_none());
            st(&mut rig, S::Saved(false));
            assert_eq!(rig.app.settings_ui.error, Some(SettingsError::SaveFailed));
        }

        fn notification(kind: NotificationKind) -> NotificationView {
            NotificationView {
                room_id: RoomId::from("!b:x"),
                event_id: EventId::from("$e"),
                room_name: "b".into(),
                sender: UserId::from("@bob:x"),
                sender_name: Some("Bob".into()),
                is_dm: false,
                kind,
                preview: Some(MessageText::from("secret preview text")),
            }
        }

        #[test]
        fn turning_desktop_notifications_off_stops_notifier_calls_at_once() {
            let mut rig = rig();
            let _ = rig.app.update(Message::WindowFocus(false));
            let _ = rig.app.update(core(CoreEvent::Notification(notification(
                NotificationKind::Message,
            ))));
            assert_eq!(rig.shown.lock().expect("mutex").len(), 1);
            st(&mut rig, S::DesktopNotifications(false));
            let _ = rig.app.update(core(CoreEvent::Notification(notification(
                NotificationKind::Mention,
            ))));
            assert_eq!(rig.shown.lock().expect("mutex").len(), 1);
            st(&mut rig, S::DesktopNotifications(true));
            let _ = rig.app.update(core(CoreEvent::Notification(notification(
                NotificationKind::Message,
            ))));
            assert_eq!(rig.shown.lock().expect("mutex").len(), 2);
        }

        #[test]
        fn turning_content_off_drops_the_preview_and_the_sender_at_once() {
            let mut rig = rig();
            let _ = rig.app.update(Message::WindowFocus(false));
            st(&mut rig, S::ShowMessageContent(false));
            let _ = rig.app.update(core(CoreEvent::Notification(notification(
                NotificationKind::Message,
            ))));
            let shown = rig.shown.lock().expect("mutex").clone();
            assert_eq!(shown.len(), 1);
            let all = format!("{} {}", shown[0].toast.title, shown[0].toast.body);
            assert!(!all.contains("secret") && !all.contains("Bob"), "{all}");
            assert_eq!(shown[0].toast.body, "New message");
        }

        #[test]
        fn the_tray_badge_toggle_redraws_the_icon_without_a_new_count() {
            let mut rig = rig();
            let _ = rig
                .app
                .update(Message::WindowOpened(iced::window::Id::unique()));
            let _ = rig.app.update(core(CoreEvent::UnreadTotals {
                notifications: 5,
                highlights: 0,
            }));
            assert_eq!(*rig.tray.badges.lock().expect("mutex"), [true]);
            st(&mut rig, S::TrayBadge(false));
            assert_eq!(*rig.tray.badges.lock().expect("mutex"), [true, false]);
            st(&mut rig, S::TrayBadge(true));
            assert_eq!(*rig.tray.badges.lock().expect("mutex"), [true, false, true]);
        }

        #[test]
        fn the_theme_choice_overrides_the_system_mode_live() {
            let mut rig = rig();
            let _ = rig.app.update(Message::ThemeMode(theme::Mode::Dark));
            let dark = |app: &App| app.theme().expect("a theme").extended_palette().is_dark;
            assert!(dark(&rig.app));
            st(&mut rig, S::Theme(ThemeChoice::Light));
            assert!(!dark(&rig.app));
            st(&mut rig, S::Theme(ThemeChoice::System));
            assert!(dark(&rig.app));
        }

        #[test]
        fn the_colour_set_applies_at_once_is_saved_and_can_go_back_to_the_default() {
            use twicecat_core::api::PaletteChoice;
            let mut rig = rig();
            let name = |app: &App| app.theme().expect("a theme").to_string();
            let default_name = name(&rig.app);
            st(&mut rig, S::Palette(Some(PaletteChoice::Frost)));
            st(&mut rig, S::Palette(Some(PaletteChoice::Frost)));
            assert!(name(&rig.app).starts_with("Frost"), "{}", name(&rig.app));
            assert_eq!(writes(&rig).len(), 1, "an unchanged pick writes nothing");
            assert_eq!(rig.app.ui_settings.palette, Some(PaletteChoice::Frost));
            st(&mut rig, S::Palette(None));
            assert_eq!(name(&rig.app), default_name);
            let saved = writes(&rig);
            assert_eq!(saved.len(), 2);
            assert_eq!(saved[1].palette, None);
        }

        #[test]
        fn the_appearance_section_builds_for_every_set_and_mode() {
            use twicecat_core::api::PaletteChoice;
            let mut rig = rig();
            open(&mut rig);
            for choice in [
                None,
                Some(PaletteChoice::Twicecat),
                Some(PaletteChoice::Frost),
                Some(PaletteChoice::Ember),
                Some(PaletteChoice::HighContrast),
            ] {
                for theme in [ThemeChoice::Light, ThemeChoice::Dark] {
                    st(&mut rig, S::Palette(choice));
                    st(&mut rig, S::Theme(theme));
                    let _ = rig.app.view();
                }
            }
        }

        #[test]
        fn density_and_room_list_are_saved_once_per_change() {
            use twicecat_core::api::{DensityChoice, RoomListChoice};
            let mut rig = rig();
            st(&mut rig, S::Density(DensityChoice::Compact));
            st(&mut rig, S::Density(DensityChoice::Compact));
            st(&mut rig, S::RoomList(RoomListChoice::Rail));
            st(&mut rig, S::RoomList(RoomListChoice::Rail));
            let saved = writes(&rig);
            assert_eq!(saved.len(), 2, "{saved:?}");
            assert_eq!(saved[1].density, DensityChoice::Compact);
            assert_eq!(saved[1].room_list, RoomListChoice::Rail);
        }

        fn press(rig: &mut Rig, shortcut: crate::keys::Shortcut) {
            let _ = rig.app.update(Message::Shortcut {
                shortcut,
                captured: false,
            });
        }

        #[test]
        fn ctrl_b_toggles_the_rail_and_saves_each_choice() {
            use crate::keys::Shortcut;
            use twicecat_core::api::RoomListChoice;
            let mut rig = rig();
            assert_eq!(rig.app.ui_settings.room_list, RoomListChoice::Expanded);
            press(&mut rig, Shortcut::ToggleRoomList);
            assert_eq!(rig.app.ui_settings.room_list, RoomListChoice::Rail);
            press(&mut rig, Shortcut::ToggleRoomList);
            assert_eq!(rig.app.ui_settings.room_list, RoomListChoice::Expanded);
            let saved = writes(&rig);
            assert_eq!(saved.len(), 2, "{saved:?}");
            assert_eq!(saved[0].room_list, RoomListChoice::Rail);
            assert_eq!(saved[1].room_list, RoomListChoice::Expanded);
            // Not on the sign-in screen, where there is no list.
            rig.app.screen = Screen::Login;
            press(&mut rig, Shortcut::ToggleRoomList);
            assert_eq!(rig.app.ui_settings.room_list, RoomListChoice::Expanded);
        }

        #[test]
        fn the_room_keys_work_the_same_in_the_rail() {
            use crate::keys::Shortcut;
            use twicecat_core::api::RoomListChoice;
            let mut rig = rig();
            rig.app.rooms = ["!a:example.org", "!b:example.org", "!c:example.org"]
                .into_iter()
                .map(|id| RoomSummary::new(RoomId::from(id), id.to_owned()))
                .collect();
            rig.app.rooms[2].unread_messages = 1;
            for list in [RoomListChoice::Expanded, RoomListChoice::Rail] {
                rig.app.ui_settings.room_list = list;
                rig.app.selected = Some(RoomId::from("!a:example.org"));
                press(&mut rig, Shortcut::NextRoom);
                assert_eq!(rig.app.selected, Some(RoomId::from("!b:example.org")));
                press(&mut rig, Shortcut::PreviousRoom);
                assert_eq!(rig.app.selected, Some(RoomId::from("!a:example.org")));
                press(&mut rig, Shortcut::NextUnread);
                assert_eq!(rig.app.selected, Some(RoomId::from("!c:example.org")));
                press(&mut rig, Shortcut::PreviousUnread);
                // Only !c is unread: nothing before it to jump to.
                assert_eq!(rig.app.selected, Some(RoomId::from("!c:example.org")));
            }
        }

        #[test]
        fn a_row_height_change_keeps_the_place_in_the_list() {
            use twicecat_core::api::{DensityChoice, RoomListChoice};
            let mut rig = rig();
            rig.app.list_offset_y = 100.0 * 64.0;
            st(&mut rig, S::Density(DensityChoice::Compact));
            assert_eq!(rig.app.list_offset_y, 100.0 * 44.0, "row 100 stays on top");
            st(&mut rig, S::RoomList(RoomListChoice::Rail));
            assert_eq!(rig.app.list_offset_y, 100.0 * 56.0);
            st(&mut rig, S::TrayBadge(false));
            assert_eq!(
                rig.app.list_offset_y,
                100.0 * 56.0,
                "other settings keep it"
            );
        }

        #[test]
        fn the_saved_density_reaches_the_timeline_at_start() {
            use twicecat_core::api::DensityChoice;
            let mut rig = rig();
            let settings = UiSettings {
                density: DensityChoice::Compact,
                ..UiSettings::default()
            };
            rig.app.set_ui_settings(settings);
            assert_eq!(
                rig.app.timeline.density(),
                crate::ui::tokens::Density::Compact
            );
        }

        #[test]
        fn a_density_switch_drops_measured_heights_but_keeps_the_place() {
            use twicecat_core::api::DensityChoice;
            let mut rig = rig();
            rig.app.timeline.measured.insert("a".to_owned(), 40.0);
            rig.app.timeline.offset_from_bottom = 120.0;
            st(&mut rig, S::Density(DensityChoice::Compact));
            assert!(rig.app.timeline.measured.is_empty());
            assert_eq!(rig.app.timeline.offset_from_bottom, 120.0);
            assert_eq!(
                rig.app.timeline.density(),
                crate::ui::tokens::Density::Compact
            );
            // Another setting keeps what was measured.
            rig.app.timeline.measured.insert("a".to_owned(), 40.0);
            st(&mut rig, S::TrayBadge(false));
            assert_eq!(rig.app.timeline.measured.len(), 1);
        }

        #[test]
        fn zoom_steps_by_ten_within_80_and_200_and_resets() {
            use crate::settings::Zoom;
            let mut rig = rig();
            let scale = |rig: &Rig| rig.app.scale_factor();
            assert_eq!(scale(&rig), 1.0);
            st(&mut rig, S::Zoom(Zoom::In));
            assert_eq!(rig.app.ui_settings.text_scale_percent, 110);
            assert!((scale(&rig) - 1.1).abs() < 1e-6);
            for _ in 0..20 {
                st(&mut rig, S::Zoom(Zoom::In));
            }
            assert_eq!(rig.app.ui_settings.text_scale_percent, 200);
            let writes_at_max = writes(&rig).len();
            st(&mut rig, S::Zoom(Zoom::In));
            assert_eq!(writes(&rig).len(), writes_at_max, "no write at the limit");
            for _ in 0..20 {
                st(&mut rig, S::Zoom(Zoom::Out));
            }
            assert_eq!(rig.app.ui_settings.text_scale_percent, 80);
            assert!((scale(&rig) - 0.8).abs() < 1e-6);
            st(&mut rig, S::Zoom(Zoom::Reset));
            assert_eq!(rig.app.ui_settings.text_scale_percent, 100);
            assert_eq!(scale(&rig), 1.0);
        }

        #[test]
        fn the_zoom_keys_work_but_not_while_a_push_to_talk_key_is_awaited() {
            use crate::keys::Shortcut;
            let mut rig = rig();
            let key = |rig: &mut Rig, shortcut| {
                let _ = rig.app.update(Message::Shortcut {
                    shortcut,
                    captured: false,
                });
            };
            key(&mut rig, Shortcut::ZoomIn);
            assert_eq!(rig.app.ui_settings.text_scale_percent, 110);
            key(&mut rig, Shortcut::ZoomOut);
            key(&mut rig, Shortcut::ZoomOut);
            assert_eq!(rig.app.ui_settings.text_scale_percent, 90);
            key(&mut rig, Shortcut::ZoomReset);
            assert_eq!(rig.app.ui_settings.text_scale_percent, 100);
            rig.app.audio_ui.capturing = true;
            key(&mut rig, Shortcut::ZoomIn);
            key(&mut rig, Shortcut::ZoomOut);
            assert_eq!(rig.app.ui_settings.text_scale_percent, 100);
        }

        #[test]
        fn the_sessions_link_closes_settings_and_opens_the_sessions_page() {
            let mut rig = rig();
            open(&mut rig);
            st(&mut rig, S::OpenSessions);
            assert!(!rig.app.settings_open());
            assert!(rig.app.sessions.open);
            assert!(
                drain(&mut rig.rx)
                    .iter()
                    .any(|c| matches!(c, Command::ListDevices))
            );
        }

        #[test]
        fn opening_settings_keeps_the_widget_tree_under_it() {
            let mut rig = rig();
            let closed = tag_path(&rig.app);
            open(&mut rig);
            assert_eq!(tag_path(&rig.app), closed, "the base keeps its place");
            st(&mut rig, S::Close);
            assert_eq!(tag_path(&rig.app), closed);
        }

        #[test]
        fn the_screen_builds_with_every_section() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("Alice"), true)));
            let _ = rig.app.update(core(settings_event(
                Mode::Mute,
                Mode::AllMessages,
                &["lunch"],
            )));
            open(&mut rig);
            let _ = rig.app.view();
        }

        #[test]
        fn names_and_keywords_stay_out_of_debug_output() {
            let mut rig = rig();
            let _ = rig
                .app
                .update(core(profile(Some("sentinel-name-x"), false)));
            let _ = rig.app.update(core(settings_event(
                Mode::AllMessages,
                Mode::AllMessages,
                &["sentinel-kw-x"],
            )));
            st(&mut rig, S::KeywordInput(text("sentinel-typed-x")));
            let dbg = format!(
                "{:?} {:?} {:?} {:?}",
                rig.app,
                rig.app.settings_ui,
                S::NameInput(text("sentinel-name-x")),
                S::RemoveKeyword(text("sentinel-kw-x")),
            );
            assert!(!dbg.contains("sentinel"), "{dbg}");
        }

        #[test]
        fn logging_out_closes_settings_and_forgets_the_profile() {
            let mut rig = rig();
            let _ = rig.app.update(core(profile(Some("Alice"), true)));
            open(&mut rig);
            let _ = rig.app.update(core(CoreEvent::LoggedOut));
            assert!(!rig.app.settings_open());
            assert!(rig.app.settings_ui.profile.is_none());
            assert!(rig.app.notification_settings.is_none());
        }
    }

    mod keyboard {
        use twicecat_core::api::{ListDiff, RoomListQuery, RoomListTab};

        use super::*;
        use crate::dialogs::RecordingDialogs;
        use crate::keys::Shortcut;
        use crate::rooms::RoomsMessage as R;
        use crate::settings::SettingsMessage as S;
        use crate::switcher::SwitcherMessage as W;
        use crate::threads::ThreadsMessage;

        fn key(app: &mut App, shortcut: Shortcut) -> Task<Message> {
            app.update(Message::Shortcut {
                shortcut,
                captured: false,
            })
        }

        fn sw(app: &mut App, message: W) {
            let _ = app.update(Message::Switcher(message));
        }

        fn id(n: &str) -> RoomId {
            RoomId::from(format!("!{n}:x"))
        }

        fn list(app: &mut App, rooms: Vec<RoomSummary>) {
            let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
                values: rooms,
            }])));
        }

        fn named(n: &str) -> RoomSummary {
            RoomSummary::new(id(n), n.to_owned())
        }

        fn unread(n: &str) -> RoomSummary {
            let mut room = named(n);
            room.unread_messages = 2;
            room
        }

        /// Rooms `alpha`, `beta`, `gamma`, `delta`, with `alpha` open.
        fn app4() -> (App, mpsc::Receiver<Command>) {
            let (mut app, mut rx) = main_app();
            list(
                &mut app,
                vec![
                    named("alpha"),
                    named("beta"),
                    named("gamma"),
                    named("delta"),
                ],
            );
            let _ = app.update(Message::SelectRoom(id("alpha")));
            let _ = drain(&mut rx);
            (app, rx)
        }

        fn queries(commands: &[Command]) -> Vec<&RoomListQuery> {
            commands
                .iter()
                .filter_map(|c| match c {
                    Command::SetRoomListQuery(q) => Some(q),
                    _ => None,
                })
                .collect()
        }

        // ---- the switcher ----

        #[test]
        fn ctrl_k_opens_the_switcher_with_the_field_focused() {
            let (mut app, _rx) = app4();
            let task = key(&mut app, Shortcut::Switcher);
            assert!(app.switcher.open);
            assert_eq!(task.units(), 1, "one operation: focus the field");
        }

        #[test]
        fn the_switcher_does_not_open_over_another_overlay_or_off_the_main_screen() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Settings(S::Open));
            let _ = key(&mut app, Shortcut::Switcher);
            assert!(!app.switcher.open);
            let (mut login, _rx) = login_app();
            let _ = key(&mut login, Shortcut::Switcher);
            assert!(!login.switcher.open);
        }

        #[test]
        fn typing_ranks_rooms_and_the_arrows_move_the_highlight() {
            let (mut app, _rx) = app4();
            let _ = key(&mut app, Shortcut::Switcher);
            // Blank query: every room, in list order.
            let all = app.switcher.results(&app.rooms);
            assert_eq!(all, [id("alpha"), id("beta"), id("gamma"), id("delta")]);
            sw(&mut app, W::Input("DE".into()));
            assert_eq!(app.switcher.results(&app.rooms), [id("delta")]);
            sw(&mut app, W::Input("".into()));
            assert_eq!(app.switcher.selected, 0);
            let _ = key(&mut app, Shortcut::SwitcherNext);
            let _ = key(&mut app, Shortcut::SwitcherNext);
            assert_eq!(app.switcher.selected, 2);
            let _ = key(&mut app, Shortcut::SwitcherPrevious);
            assert_eq!(app.switcher.selected, 1);
            // Wraps at both ends.
            let _ = key(&mut app, Shortcut::SwitcherPrevious);
            let _ = key(&mut app, Shortcut::SwitcherPrevious);
            assert_eq!(app.switcher.selected, 3);
            let _ = key(&mut app, Shortcut::SwitcherNext);
            assert_eq!(app.switcher.selected, 0);
        }

        #[test]
        fn the_arrows_are_the_composers_while_the_switcher_is_closed() {
            let (mut app, _rx) = app4();
            let task = key(&mut app, Shortcut::SwitcherNext);
            assert_eq!(task.units(), 0);
            assert_eq!(app.switcher.selected, 0);
        }

        #[test]
        fn enter_opens_the_highlighted_room_and_closes_the_switcher() {
            let (mut app, mut rx) = app4();
            let _ = key(&mut app, Shortcut::Switcher);
            sw(&mut app, W::Input("ga".into()));
            let _ = drain(&mut rx);
            let task = app.update(Message::Switcher(W::Submit));
            assert!(!app.switcher.open);
            assert_eq!(app.selected, Some(id("gamma")));
            assert!(task.units() >= 1, "the composer gets the focus back");
            assert!(drain(&mut rx).iter().any(|c| matches!(
                c,
                Command::SelectRoom(r) if *r == id("gamma")
            )));
        }

        #[test]
        fn enter_with_no_match_keeps_the_switcher_open() {
            let (mut app, _rx) = app4();
            let _ = key(&mut app, Shortcut::Switcher);
            sw(&mut app, W::Input("nothing like it".into()));
            sw(&mut app, W::Submit);
            assert!(app.switcher.open);
            assert_eq!(app.selected, Some(id("alpha")));
        }

        #[test]
        fn clicking_a_row_selects_it_too() {
            let (mut app, _rx) = app4();
            let _ = key(&mut app, Shortcut::Switcher);
            sw(&mut app, W::Pick(id("delta")));
            assert!(!app.switcher.open);
            assert_eq!(app.selected, Some(id("delta")));
        }

        #[test]
        fn esc_closes_the_switcher_and_focuses_the_composer() {
            let (mut app, _rx) = app4();
            let _ = key(&mut app, Shortcut::Switcher);
            let task = key(&mut app, Shortcut::Escape);
            assert!(!app.switcher.open);
            assert_eq!(task.units(), 1, "focus the composer");
            assert_eq!(app.selected, Some(id("alpha")));
        }

        #[test]
        fn few_local_matches_also_search_in_the_core_and_closing_restores_the_list() {
            let (mut app, mut rx) = app4();
            let before = app.query.clone();
            let _ = key(&mut app, Shortcut::Switcher);
            // Four local matches: no core search.
            sw(&mut app, W::Input("a".into()));
            assert!(queries(&drain(&mut rx)).is_empty());
            // One local match: the core searches too.
            sw(&mut app, W::Input("delta".into()));
            let commands = drain(&mut rx);
            assert_eq!(
                queries(&commands),
                [&RoomListQuery {
                    tab: RoomListTab::All,
                    search: "delta".into(),
                    space: None,
                }]
            );
            // The core's answer replaces the sidebar list; the rooms from
            // before stay among the switcher's candidates.
            list(&mut app, vec![named("delta"), named("delta two")]);
            assert_eq!(
                app.switcher.results(&app.rooms),
                [id("delta"), id("delta two")]
            );
            sw(&mut app, W::Input("gam".into()));
            let _ = drain(&mut rx);
            assert_eq!(app.switcher.results(&app.rooms), [id("gamma")]);
            // Choosing a room only the opening list knew selects it, and the
            // list query is put back.
            sw(&mut app, W::Submit);
            assert_eq!(app.selected, Some(id("gamma")));
            let commands = drain(&mut rx);
            assert_eq!(queries(&commands).last().copied(), Some(&before));
            assert!(!app.switcher.open);
        }

        #[test]
        fn a_long_query_is_cut_and_a_closed_switcher_ignores_messages() {
            let (mut app, _rx) = app4();
            sw(&mut app, W::Input("x".into()));
            assert!(app.switcher.query.is_empty());
            let _ = key(&mut app, Shortcut::Switcher);
            sw(&mut app, W::Input("y".repeat(500)));
            assert_eq!(
                app.switcher.query.chars().count(),
                crate::switcher::MAX_QUERY_CHARS
            );
        }

        #[test]
        fn opening_the_switcher_keeps_the_widget_tree_under_it() {
            let (mut app, _rx) = app4();
            let closed = tag_path(&app);
            let _ = key(&mut app, Shortcut::Switcher);
            sw(&mut app, W::Input("al".into()));
            assert_eq!(tag_path(&app), closed, "the base keeps its place");
            let _ = key(&mut app, Shortcut::Escape);
            assert_eq!(tag_path(&app), closed);
        }

        // ---- stepping through rooms ----

        #[test]
        fn alt_down_and_up_step_through_the_list_and_wrap() {
            let (mut app, _rx) = app4();
            let _ = key(&mut app, Shortcut::NextRoom);
            assert_eq!(app.selected, Some(id("beta")));
            let _ = key(&mut app, Shortcut::PreviousRoom);
            let _ = key(&mut app, Shortcut::PreviousRoom);
            assert_eq!(app.selected, Some(id("delta")), "wraps to the end");
            let _ = key(&mut app, Shortcut::NextRoom);
            assert_eq!(app.selected, Some(id("alpha")), "and back");
        }

        #[test]
        fn ctrl_shift_arrows_find_the_next_room_with_unread_messages() {
            let (mut app, _rx) = main_app();
            list(
                &mut app,
                vec![named("a"), named("b"), unread("c"), named("d"), unread("e")],
            );
            let _ = app.update(Message::SelectRoom(id("a")));
            let _ = key(&mut app, Shortcut::NextUnread);
            assert_eq!(app.selected, Some(id("c")));
            let _ = key(&mut app, Shortcut::NextUnread);
            assert_eq!(app.selected, Some(id("e")));
            let _ = key(&mut app, Shortcut::NextUnread);
            assert_eq!(app.selected, Some(id("c")), "wraps");
            let _ = key(&mut app, Shortcut::PreviousUnread);
            assert_eq!(app.selected, Some(id("e")));
        }

        #[test]
        fn nothing_unread_means_no_move_and_spaces_and_invites_are_skipped() {
            let (mut app, mut rx) = main_app();
            let mut space = named("s");
            space.is_space = true;
            let mut invite = named("i");
            invite.is_invite = true;
            list(&mut app, vec![named("a"), space, invite, named("b")]);
            let _ = app.update(Message::SelectRoom(id("a")));
            let _ = drain(&mut rx);
            let _ = key(&mut app, Shortcut::NextUnread);
            assert_eq!(app.selected, Some(id("a")));
            assert!(drain(&mut rx).is_empty());
            let _ = key(&mut app, Shortcut::NextRoom);
            assert_eq!(app.selected, Some(id("b")));
        }

        #[test]
        fn room_shortcuts_wait_while_an_overlay_is_open() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Settings(S::Open));
            let _ = key(&mut app, Shortcut::NextRoom);
            assert_eq!(app.selected, Some(id("alpha")));
        }

        #[test]
        fn a_key_a_widget_used_is_not_taken_twice() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::NextRoom,
                captured: true,
            });
            assert_eq!(app.selected, Some(id("alpha")));
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Switcher,
                captured: true,
            });
            assert!(!app.switcher.open);
        }

        #[test]
        fn ctrl_comma_opens_settings_and_ctrl_u_asks_for_files() {
            let (mut app, _rx) = app4();
            let dialogs = RecordingDialogs::default();
            app.set_dialogs(Box::new(dialogs.clone()));
            let _ = key(&mut app, Shortcut::Attach);
            assert_eq!(*dialogs.picked.lock().expect("test mutex"), 1);
            let _ = key(&mut app, Shortcut::Settings);
            assert!(app.settings_ui.open);
        }

        // ---- focus order ----

        #[test]
        fn tab_and_shift_tab_move_the_focus_unless_a_widget_used_the_key() {
            let (mut app, _rx) = app4();
            assert_eq!(key(&mut app, Shortcut::FocusNext).units(), 1);
            assert_eq!(key(&mut app, Shortcut::FocusPrevious).units(), 1);
            let used = app.update(Message::Shortcut {
                shortcut: Shortcut::FocusNext,
                captured: true,
            });
            assert_eq!(used.units(), 0, "the editor's Tab is not stolen");
        }

        // ---- Esc closes every overlay ----

        #[test]
        fn esc_closes_the_upload_confirmation() {
            let (mut app, _rx) = app4();
            app.uploads.dialog = Some(crate::upload::UploadDialog {
                room_id: id("alpha"),
                entries: Vec::new(),
            });
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.uploads.dialog.is_none());
        }

        #[test]
        fn esc_closes_the_viewer() {
            let (mut app, _rx, _) = media_app(1);
            let _ = app.update(Message::MediaClicked("img0".into()));
            assert!(app.media.viewer.is_some());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.media.viewer.is_none());
        }

        #[test]
        fn esc_closes_the_upload_dialog_before_the_viewer_when_both_are_open() {
            // view(): the upload dialog is drawn over the viewer.
            let (mut app, _rx, _) = media_app(1);
            let _ = app.update(Message::MediaClicked("img0".into()));
            app.uploads.dialog = Some(crate::upload::UploadDialog {
                room_id: RoomId::from("!a:x"),
                entries: Vec::new(),
            });
            assert!(app.media.viewer.is_some() && app.uploads.dialog.is_some());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.uploads.dialog.is_none(), "the dialog on top closes");
            assert!(app.media.viewer.is_some(), "the viewer under it stays");
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.media.viewer.is_none());
        }

        #[test]
        fn esc_closes_settings() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Settings(S::Open));
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.settings_ui.open);
        }

        #[test]
        fn esc_closes_a_room_dialog() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Rooms(R::OpenCreate));
            assert!(app.rooms_ui.overlay_open());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.rooms_ui.overlay_open());
        }

        // ---- the composer under an overlay ----

        /// One way to put a modal layer over the room, with the app it needs.
        type Opener = (
            &'static str,
            fn() -> (App, mpsc::Receiver<Command>),
            fn(&mut App),
            fn(&mut App),
        );

        fn viewer_app() -> (App, mpsc::Receiver<Command>) {
            let (app, rx, _) = media_app(1);
            (app, rx)
        }

        const OPENERS: [Opener; 11] = [
            (
                "create dialog",
                app4,
                |app| {
                    let _ = app.update(Message::Rooms(R::OpenCreate));
                },
                |app| {
                    let _ = app.update(Message::Rooms(R::Escape));
                },
            ),
            (
                "switcher",
                app4,
                |app| {
                    let _ = key(app, Shortcut::Switcher);
                },
                |app| {
                    let _ = app.update(Message::Switcher(W::Close));
                },
            ),
            (
                "settings",
                app4,
                |app| {
                    let _ = app.update(Message::Settings(S::Open));
                },
                |app| {
                    let _ = app.update(Message::Settings(S::Close));
                },
            ),
            (
                "audio card",
                app4,
                |app| {
                    let _ = app.update(Message::Audio(crate::audio_settings::AudioMessage::Open));
                },
                |app| {
                    let _ = app.update(Message::Audio(crate::audio_settings::AudioMessage::Close));
                },
            ),
            (
                "sessions card",
                app4,
                |app| {
                    let _ = app.update(Message::Sessions(crate::sessions::SessionsMessage::Open));
                },
                |app| {
                    let _ = app.update(Message::Sessions(crate::sessions::SessionsMessage::Close));
                },
            ),
            (
                "logout warning",
                app4,
                |app| {
                    app.sessions.logout_warning = true;
                },
                |app| {
                    app.sessions.logout_warning = false;
                },
            ),
            (
                "upload dialog",
                app4,
                |app| {
                    app.uploads.dialog = Some(crate::upload::UploadDialog {
                        room_id: id("alpha"),
                        entries: Vec::new(),
                    });
                },
                |app| {
                    let _ = app.update(Message::UploadCancelled);
                },
            ),
            (
                "viewer",
                viewer_app,
                |app| {
                    let _ = app.update(Message::MediaClicked("img0".into()));
                },
                |app| {
                    let _ = app.update(Message::ViewerClosed);
                },
            ),
            (
                "security screen",
                app4,
                |app| {
                    app.security.overlay = Some(Overlay::ResetConfirm);
                },
                |app| {
                    app.security.overlay = None;
                },
            ),
            (
                "verification card",
                app4,
                |app| {
                    let _ = app.update(core(CoreEvent::VerificationRequested {
                        id: vid(),
                        peer: vpeer(true),
                    }));
                },
                |app| {
                    app.verification = None;
                },
            ),
            (
                "ring card",
                app4,
                |app| {
                    let _ = app.update(core(CoreEvent::IncomingCall {
                        room_id: id("beta"),
                        caller: UserId::from("@bea:x"),
                        intent: twicecat_core::api::CallIntent::Audio,
                        ring: true,
                    }));
                },
                |app| {
                    let _ = app.update(Message::Call(CallMessage::HideRing));
                },
            ),
        ];

        #[test]
        fn enter_sends_nothing_under_any_overlay_and_sends_again_once_it_is_closed() {
            for (name, build, open, close) in OPENERS {
                let (mut app, mut rx) = build();
                type_in(&mut app, "hello");
                open(&mut app);
                assert!(app.any_overlay_open(), "{name}: the overlay is open");
                let _ = drain(&mut rx);
                for key in [
                    ComposerKey::Send,
                    ComposerKey::Pick,
                    ComposerKey::Next,
                    ComposerKey::Previous,
                    ComposerKey::Escape,
                ] {
                    let _ = app.update(Message::ComposerKey(key));
                }
                assert!(
                    send_commands(&drain(&mut rx)).is_empty(),
                    "{name}: Enter sent the draft under the overlay"
                );
                assert_eq!(app.composer.text(), "hello", "{name}: the draft is kept");
                close(&mut app);
                assert!(!app.any_overlay_open(), "{name}: it is closed");
                let _ = drain(&mut rx);
                let _ = app.update(Message::ComposerKey(ComposerKey::Send));
                assert!(
                    matches!(
                        send_commands(&drain(&mut rx)).as_slice(),
                        [Command::Send(m)] if m.text.expose() == "hello"
                    ),
                    "{name}: sending works again after the overlay"
                );
            }
        }

        #[test]
        fn esc_closes_the_thread_panel() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            assert!(app.threads.panel.is_some());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.threads.panel.is_none());
        }

        // ---- Esc and the Sessions card ----

        use crate::sessions::SessionsMessage as SM;

        fn ses_msg(app: &mut App, message: SM) {
            let _ = app.update(Message::Sessions(message));
        }

        /// `app4` with the Sessions card open and its devices listed.
        fn sessions_over_app4() -> (App, mpsc::Receiver<Command>) {
            let (mut app, mut rx) = app4();
            ses_msg(&mut app, SM::Open);
            let _ = app.update(core(CoreEvent::Devices(device_list())));
            let _ = drain(&mut rx);
            (app, rx)
        }

        /// The card with a password prompt open (not waiting).
        fn prompt_over_app4() -> (App, mpsc::Receiver<Command>) {
            let (mut app, mut rx) = sessions_over_app4();
            ses_msg(&mut app, SM::SignOutAsk(DeviceId::from("PHONE")));
            ses_msg(&mut app, SM::SignOutConfirmed);
            let _ = drain(&mut rx);
            let _ = app.update(core(CoreEvent::ReauthRequired {
                action: ReauthAction::DeleteDevices,
            }));
            assert!(app.sessions.reauth.is_some());
            (app, rx)
        }

        /// Reply mode, a thread panel and a call panel, all open behind.
        fn layers_behind(app: &mut App, rx: &mut mpsc::Receiver<Command>) {
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            app.composer.mode = composer::Mode::Reply("x".into());
            app.calls.panel_open = true;
            let _ = drain(rx);
        }

        fn behind_untouched(app: &App) {
            assert!(matches!(app.composer.mode, composer::Mode::Reply(_)));
            assert!(app.threads.panel.is_some());
            assert!(app.calls.panel_open);
        }

        #[test]
        fn esc_closes_the_sessions_card() {
            let (mut app, mut rx) = sessions_over_app4();
            layers_behind(&mut app, &mut rx);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.open);
            behind_untouched(&app);
        }

        #[test]
        fn esc_cancels_the_sign_out_question_and_keeps_the_card() {
            let (mut app, mut rx) = sessions_over_app4();
            layers_behind(&mut app, &mut rx);
            ses_msg(&mut app, SM::SignOutAsk(DeviceId::from("PHONE")));
            assert!(app.sessions.confirm.is_some());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.sessions.confirm.is_none() && app.sessions.open);
            assert!(app.sessions.pending.is_none());
            assert!(drain(&mut rx).is_empty(), "Esc never confirms");
            behind_untouched(&app);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.open, "the next press closes the card");
        }

        #[test]
        fn esc_cancels_the_password_prompt_and_forgets_the_password() {
            let (mut app, mut rx) = prompt_over_app4();
            layers_behind(&mut app, &mut rx);
            ses_msg(&mut app, SM::ReauthInput(Password::from("secret-pw-e")));
            // A press the password field used itself arrives as captured.
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Escape,
                captured: true,
            });
            assert!(app.sessions.reauth.is_none() && app.sessions.pending.is_none());
            assert!(app.sessions.open);
            assert!(drain(&mut rx).is_empty(), "Esc never submits");
            assert!(!format!("{app:?}").contains("secret-pw-e"));
            behind_untouched(&app);
        }

        #[test]
        fn esc_while_the_password_is_in_flight_is_swallowed() {
            let (mut app, mut rx) = prompt_over_app4();
            layers_behind(&mut app, &mut rx);
            ses_msg(&mut app, SM::ReauthInput(Password::from("secret-pw-f")));
            ses_msg(&mut app, SM::ReauthSubmitted);
            assert!(app.sessions.reauth.as_ref().is_some_and(|p| p.waiting));
            let _ = drain(&mut rx);
            let task = key(&mut app, Shortcut::Escape);
            assert_eq!(task.units(), 0);
            assert!(app.sessions.reauth.as_ref().is_some_and(|p| p.waiting));
            assert!(app.sessions.pending.is_some() && app.sessions.open);
            assert!(drain(&mut rx).is_empty());
            behind_untouched(&app);
        }

        #[test]
        fn esc_cancels_the_logout_warning_and_never_logs_out() {
            let (mut app, mut rx) = sessions_over_app4();
            layers_behind(&mut app, &mut rx);
            app.sessions.logout_warning = true;
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.logout_warning);
            assert!(app.sessions.open, "the card under it stays");
            assert!(drain(&mut rx).is_empty(), "Esc never logs out");
            behind_untouched(&app);
        }

        #[test]
        fn the_logout_warning_comes_before_the_prompt_and_the_question() {
            let (mut app, _rx) = prompt_over_app4();
            app.sessions.logout_warning = true;
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.logout_warning && app.sessions.reauth.is_some());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.sessions.reauth.is_none() && app.sessions.open);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.open);
        }

        #[test]
        fn esc_acts_on_the_layer_drawn_on_top_a_room_dialog_over_the_sessions_card() {
            // view(): room dialogs are drawn over main_view's Sessions
            // layer, so Esc closes the dialog first.
            let (mut app, mut rx) = sessions_over_app4();
            let _ = app.update(Message::Rooms(R::OpenCreate));
            assert!(app.rooms_ui.overlay_open() && app.sessions.open);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.rooms_ui.overlay_open(), "the dialog on top closes");
            assert!(app.sessions.open, "the card under it stays");
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.open);
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn esc_acts_on_the_layer_drawn_on_top_a_room_dialog_over_a_security_screen() {
            // view(): room dialogs are drawn over main_view's security layer.
            let (mut app, _rx) = app4();
            app.security.overlay = Some(Overlay::ResetConfirm);
            let _ = app.update(Message::Rooms(R::OpenCreate));
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.rooms_ui.overlay_open());
            assert_eq!(
                app.security.overlay,
                Some(Overlay::ResetConfirm),
                "the security screen under the dialog stays"
            );
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.security.overlay.is_none());
        }

        #[test]
        fn esc_in_the_rename_field_cancels_the_rename_and_keeps_the_card() {
            let (mut app, mut rx) = sessions_over_app4();
            ses_msg(&mut app, SM::RenameStart(DeviceId::from("PHONE")));
            assert!(app.sessions.rename.is_some());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.sessions.rename.is_none(), "the rename is cancelled");
            assert!(app.sessions.open, "the card stays");
            assert!(drain(&mut rx).is_empty(), "nothing is renamed");
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.open, "the next press closes the card");
        }

        #[test]
        fn esc_with_nothing_open_does_nothing() {
            let (mut app, mut rx) = app4();
            assert_eq!(key(&mut app, Shortcut::Escape).units(), 0);
            assert!(drain(&mut rx).is_empty());
        }

        // ---- Esc over the verification card and the security screens ----

        /// `app4` with a verification flow (own session) in `phase`.
        fn verifying4(phase: VP) -> (App, mpsc::Receiver<Command>) {
            let (mut app, mut rx) = app4();
            let _ = app.update(core(CoreEvent::VerificationRequested {
                id: vid(),
                peer: vpeer(true),
            }));
            vphase(&mut app, phase);
            let _ = drain(&mut rx);
            (app, rx)
        }

        #[test]
        fn esc_on_the_emoji_card_cancels_and_never_confirms_a_match() {
            let (mut app, mut rx) = verifying4(VP::Emojis(vemojis()));
            varm(&mut app);
            let _ = key(&mut app, Shortcut::Escape);
            let _ = key(&mut app, Shortcut::Escape);
            let sent = drain(&mut rx);
            assert!(
                matches!(sent.as_slice(), [Command::CancelVerification { id }] if *id == vid()),
                "{sent:?}"
            );
            // The core ends the flow; the next Esc closes the ended card.
            vphase(
                &mut app,
                VP::Cancelled {
                    reason: CancelReason::ByUs,
                },
            );
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.verification.is_none());
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn esc_on_the_other_cards_cancels_and_never_accepts_or_confirms() {
            for phase in [
                VP::Requested { incoming: true },
                VP::QrScanned,
                VP::Decimals([1, 2, 3]),
                VP::Waiting,
            ] {
                let (mut app, mut rx) = verifying4(phase.clone());
                if app.verification.as_ref().is_some_and(|m| !m.armed()) {
                    varm(&mut app);
                }
                let _ = key(&mut app, Shortcut::Escape);
                let sent = drain(&mut rx);
                assert!(
                    matches!(sent.as_slice(), [Command::CancelVerification { .. }]),
                    "{phase:?}: {sent:?}"
                );
            }
        }

        #[test]
        fn esc_on_the_verification_card_closes_nothing_under_it() {
            let (mut app, mut rx) = verifying4(VP::Emojis(vemojis()));
            ses_msg(&mut app, SM::Open);
            let _ = app.update(Message::Rooms(R::OpenCreate));
            let _ = drain(&mut rx);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.sessions.open, "the Sessions card stays");
            assert!(app.rooms_ui.overlay_open(), "the room dialog stays");
            assert!(matches!(
                drain(&mut rx).as_slice(),
                [Command::CancelVerification { .. }]
            ));
        }

        #[test]
        fn esc_over_a_security_screen_does_not_close_the_sessions_card_under_it() {
            let (mut app, mut rx) = sessions_over_app4();
            let _ = sec(&mut app, SecurityMessage::OpenRestore);
            let _ = drain(&mut rx);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.sessions.open, "the card was not the one drawn on top");
            assert_eq!(app.security.overlay, None, "the screen backed out");
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.sessions.open, "now the card closes");
        }

        #[test]
        fn esc_backs_out_of_the_questions_and_never_confirms_them() {
            // Reset: Esc is Back; no reset command.
            let (mut app, mut rx) = app4();
            let _ = app.update(core(CoreEvent::Security(status(
                DeviceVerification::Unverified,
                RecoveryStatus::Enabled,
            ))));
            let _ = sec(&mut app, SecurityMessage::OpenConfirmIdentity);
            let _ = sec(&mut app, SecurityMessage::ResetRequested);
            assert_eq!(app.security.overlay, Some(Overlay::ResetConfirm));
            let _ = drain(&mut rx);
            let _ = key(&mut app, Shortcut::Escape);
            assert_eq!(app.security.overlay, Some(Overlay::ConfirmIdentity));
            assert!(drain(&mut rx).is_empty());
            // The identity screen: Esc skips for now.
            let _ = key(&mut app, Shortcut::Escape);
            assert_eq!(app.security.overlay, None);
            assert!(app.security.identity_screen_skipped);
            // Change key: Esc cancels; no command.
            let (mut app, mut rx) = app4();
            let _ = app.update(core(CoreEvent::Security(status(
                DeviceVerification::Verified,
                RecoveryStatus::Enabled,
            ))));
            let _ = sec(&mut app, SecurityMessage::ChangeKeyRequested);
            assert_eq!(app.security.overlay, Some(Overlay::ChangeKeyConfirm));
            let _ = drain(&mut rx);
            let _ = key(&mut app, Shortcut::Escape);
            assert_eq!(app.security.overlay, None);
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn esc_is_swallowed_by_the_key_screen_and_by_work_that_runs() {
            let (mut app, mut rx) = sessions_over_app4();
            app.security.setup = Some(crate::security::RecoverySetup::new(
                RecoveryKey::from("dummy"),
                false,
            ));
            let _ = drain(&mut rx);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.security.setup.is_some(), "the key is not dropped");
            assert!(app.sessions.open, "nothing under it saw the key");
            app.security.setup = None;
            // Setting recovery up.
            app.security.busy = Some(crate::security::Busy::SettingUp);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.sessions.open);
            // A running reset keeps its screen.
            app.security.busy = Some(crate::security::Busy::Resetting);
            app.security.overlay = Some(Overlay::ResetConfirm);
            let _ = key(&mut app, Shortcut::Escape);
            assert_eq!(app.security.overlay, Some(Overlay::ResetConfirm));
            assert!(drain(&mut rx).is_empty());
            // A password on its way.
            app.security.busy = None;
            app.security.overlay = None;
            let mut prompt = crate::security::ReauthPrompt::new(ReauthAction::ResetIdentity);
            prompt.waiting = true;
            app.security.reauth = Some(prompt);
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.security.reauth.is_some());
            assert!(app.sessions.open);
        }

        #[test]
        fn esc_closes_one_layer_per_press_topmost_first() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            let _ = app.update(Message::Settings(S::Open));
            let _ = key(&mut app, Shortcut::Escape);
            assert!(!app.settings_ui.open);
            assert!(app.threads.panel.is_some(), "the panel waits its turn");
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.threads.panel.is_none());
        }

        #[test]
        fn esc_cancels_a_reply_before_it_closes_the_panel() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            app.composer.mode = composer::Mode::Reply("x".into());
            let _ = key(&mut app, Shortcut::Escape);
            assert!(matches!(app.composer.mode, composer::Mode::Normal));
            assert!(app.threads.panel.is_some());
        }

        #[test]
        fn the_composers_own_esc_is_not_taken_twice_with_the_apps() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            app.composer.mode = composer::Mode::Reply("x".into());
            // One key press, two messages: the composer's binding, and the
            // window listener, which saw the editor take the key.
            let _ = app.update(Message::ComposerKey(ComposerKey::Escape));
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Escape,
                captured: true,
            });
            assert!(matches!(app.composer.mode, composer::Mode::Normal));
            assert!(app.threads.panel.is_some(), "one layer per press");
            let _ = key(&mut app, Shortcut::Escape);
            assert!(app.threads.panel.is_none());
        }

        #[test]
        fn a_text_fields_esc_closes_the_panel_behind_it() {
            let (mut app, _rx) = app4();
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            // The composer has nothing to cancel; the key was taken by a
            // field that dropped its focus.
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Escape,
                captured: true,
            });
            assert!(app.threads.panel.is_none());
        }
    }

    // ---- UI/UX rework Task 8: narrow windows ----

    mod narrow_window {
        use twicecat_core::api::{CallState, EventId, RoomListChoice};

        use super::*;
        use crate::keys::Shortcut;
        use crate::layout::{PaneLayout, SidePanel};
        use crate::threads::ThreadsMessage;

        fn resize(app: &mut App, width: f32) -> Task<Message> {
            app.update(Message::WindowResized(width))
        }

        fn open_thread(app: &mut App) {
            let _ = app.update(Message::Threads(ThreadsMessage::Open(EventId::from(
                "$r:x",
            ))));
            assert!(app.threads.panel.is_some());
        }

        fn open_call_panel(app: &mut App) {
            if app.calls.call.is_none() {
                let _ = app.update(core(CoreEvent::CallState(CallState::Connected {
                    room_id: RoomId::from("!a:x"),
                    encrypted: true,
                })));
            }
            if !app.calls.panel_open {
                let _ = app.update(Message::Call(crate::calls::CallMessage::TogglePanel));
            }
            assert!(app.calls.panel_shown(app.selected.as_ref()));
        }

        fn key(app: &mut App, shortcut: Shortcut) {
            let _ = app.update(Message::Shortcut {
                shortcut,
                captured: false,
            });
        }

        #[test]
        fn a_resize_records_the_width_and_runs_no_task() {
            let (mut app, mut rx) = main_app();
            assert_eq!(app.window_width, crate::layout::DEFAULT_WINDOW_WIDTH);
            for width in [1500.0, 1000.0, 700.0, 1200.0] {
                assert_eq!(resize(&mut app, width).units(), 0, "{width}");
                assert_eq!(app.window_width, width);
            }
            assert!(drain(&mut rx).is_empty(), "no command either");
        }

        fn layout(app: &App) -> PaneLayout {
            app.pane_layout()
        }

        #[test]
        fn crossing_both_thresholds_with_no_panel_changes_only_the_sidebar() {
            let (mut app, _rx) = main_app();
            let _ = resize(&mut app, 1280.0);
            assert!(layout(&app).sidebar);
            assert_eq!(layout(&app).list, RoomListChoice::Expanded);
            let _ = resize(&mut app, 900.0);
            assert!(layout(&app).sidebar);
            assert_eq!(layout(&app).list, RoomListChoice::Expanded);
            let _ = resize(&mut app, 700.0);
            assert!(!layout(&app).sidebar);
            assert_eq!(layout(&app).list, RoomListChoice::Rail);
            assert!(!layout(&app).panel_fills, "nothing to fill with");
            let _ = resize(&mut app, 1280.0);
            assert!(layout(&app).sidebar);
            assert_eq!(layout(&app).list, RoomListChoice::Expanded);
        }

        #[test]
        fn crossing_both_thresholds_with_the_thread_open() {
            let (mut app, _rx) = main_app();
            open_thread(&mut app);
            let _ = resize(&mut app, 1280.0);
            let wide = layout(&app);
            assert!(wide.thread && wide.sidebar && !wide.panel_fills);
            assert_eq!(wide.list, RoomListChoice::Expanded);
            let _ = resize(&mut app, 1000.0);
            assert!(layout(&app).thread);
            assert_eq!(layout(&app).list, RoomListChoice::Rail);
            let _ = resize(&mut app, 700.0);
            let narrow = layout(&app);
            assert!(narrow.thread && !narrow.sidebar && narrow.panel_fills);
            assert_eq!(narrow.list, RoomListChoice::Rail);
        }

        #[test]
        fn crossing_both_thresholds_with_the_call_panel_open() {
            let (mut app, _rx) = main_app();
            open_call_panel(&mut app);
            let _ = resize(&mut app, 1280.0);
            assert!(layout(&app).call && !layout(&app).panel_fills);
            let _ = resize(&mut app, 1000.0);
            assert!(layout(&app).call);
            assert_eq!(layout(&app).list, RoomListChoice::Rail);
            let _ = resize(&mut app, 700.0);
            assert!(layout(&app).call && layout(&app).panel_fills);
            assert!(!layout(&app).sidebar);
        }

        #[test]
        fn with_both_open_the_last_opened_shows_and_the_other_waits() {
            let (mut app, _rx) = main_app();
            open_thread(&mut app);
            open_call_panel(&mut app);
            let _ = resize(&mut app, 1280.0);
            assert!(
                layout(&app).thread && layout(&app).call,
                "both are side panes"
            );
            for width in [1000.0, 700.0] {
                let _ = resize(&mut app, width);
                assert!(layout(&app).call && !layout(&app).thread, "{width}");
            }
            // The thread is opened again after the panel: it is the newest.
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            open_thread(&mut app);
            for width in [1000.0, 700.0] {
                let _ = resize(&mut app, width);
                assert!(layout(&app).thread && !layout(&app).call, "{width}");
            }
            assert_eq!(app.newest_panel, SidePanel::Thread);
        }

        #[test]
        fn the_saved_choice_never_changes_with_the_width_and_nothing_is_saved() {
            let (mut app, mut rx) = main_app();
            open_thread(&mut app);
            let _ = drain(&mut rx);
            let saved = app.ui_settings;
            for width in [1280.0, 1000.0, 700.0, 1280.0] {
                let _ = resize(&mut app, width);
            }
            assert_eq!(app.ui_settings, saved);
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn the_rows_follow_the_list_as_shown() {
            let (mut app, _rx) = main_app();
            assert_eq!(app.room_row_height(), 64.0);
            app.list_offset_y = 100.0 * 64.0;
            // A resize starts no task, so the offset stays what the
            // scrollable really has; only the height of a row follows.
            assert_eq!(resize(&mut app, 700.0).units(), 0);
            assert_eq!(app.room_row_height(), 56.0, "the forced rail's rows");
            assert_eq!(app.list_offset_y, 100.0 * 64.0);
            assert_eq!(resize(&mut app, 1280.0).units(), 0);
            assert_eq!(app.room_row_height(), 64.0);
        }

        #[test]
        fn opening_a_panel_in_a_medium_window_keeps_the_place_in_the_list() {
            let (mut app, _rx) = main_app();
            let _ = resize(&mut app, 1000.0);
            app.list_offset_y = 100.0 * 64.0;
            open_thread(&mut app);
            assert_eq!(app.room_row_height(), 56.0);
            assert_eq!(app.list_offset_y, 100.0 * 56.0, "row 100 stays on top");
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            assert!((app.list_offset_y - 100.0 * 64.0).abs() < 0.1);
        }

        #[test]
        fn a_room_pane_behind_a_panel_takes_no_measurements() {
            // With no width the pane's rows would be measured one glyph per
            // line; the measurements are for the pane the user sees.
            let (mut app, _rx, _) = media_app(3);
            let measure = |app: &mut App, height: f32| {
                let _ = app.update(Message::RowMeasured {
                    id: "img2".into(),
                    height,
                });
            };
            let _ = resize(&mut app, 700.0);
            open_thread(&mut app);
            assert!(layout(&app).panel_fills);
            measure(&mut app, 230.0);
            assert!(app.timeline.measured.is_empty(), "hidden: not recorded");
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            measure(&mut app, 230.0);
            assert_eq!(app.timeline.measured.get("img2"), Some(&230.0));
        }

        #[test]
        fn esc_closes_the_shown_panel_first_and_returns_to_the_room_pane() {
            let (mut app, _rx) = main_app();
            open_thread(&mut app);
            open_call_panel(&mut app);
            let _ = resize(&mut app, 700.0);
            assert!(layout(&app).call && layout(&app).panel_fills);
            key(&mut app, Shortcut::Escape);
            assert!(!app.calls.panel_shown(app.selected.as_ref()));
            assert!(app.threads.panel.is_some(), "the waiting thread stays open");
            let now = layout(&app);
            assert!(now.thread && now.panel_fills, "the thread takes its turn");
            key(&mut app, Shortcut::Escape);
            assert!(app.threads.panel.is_none());
            assert!(!layout(&app).panel_fills, "the room pane is back");
        }

        #[test]
        fn esc_closes_the_thread_first_when_it_is_the_one_shown() {
            let (mut app, _rx) = main_app();
            open_call_panel(&mut app);
            open_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            key(&mut app, Shortcut::Escape);
            assert!(app.threads.panel.is_none());
            assert!(app.calls.panel_shown(app.selected.as_ref()));
        }

        // ---- A pane the user cannot see does nothing ----

        const THREAD_ITEM: &str = "thread:$r:x:4";

        fn reactable_thread(app: &mut App) {
            open_thread(app);
            let root = EventId::from("$r:x");
            let _ = app.update(core(CoreEvent::ThreadDiff {
                room_id: RoomId::from("!a:x"),
                root,
                diffs: vec![ListDiff::Append {
                    values: vec![message_item(THREAD_ITEM, "reply", false, ALL_ACTIONS)],
                }],
            }));
        }

        fn reactions(commands: &[Command]) -> usize {
            commands
                .iter()
                .filter(|c| matches!(c, Command::ToggleReaction { .. }))
                .count()
        }

        #[test]
        fn a_thread_picker_is_drawn_by_the_panel_even_when_it_fills_the_window() {
            let (mut app, mut rx) = main_app();
            reactable_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            assert!(layout(&app).panel_fills && layout(&app).thread);
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerOpened(THREAD_ITEM.into()));
            let picker = app.actions.picker.as_ref().expect("shown in the panel");
            assert!(picker.in_thread, "the thread panel draws it");
            let _ = app.update(Message::PickerSubmitted);
            assert_eq!(reactions(&drain(&mut rx)), 1);
        }

        #[test]
        fn no_picker_opens_for_a_thread_item_the_call_panel_has_hidden() {
            let (mut app, mut rx) = main_app();
            reactable_thread(&mut app);
            open_call_panel(&mut app);
            let _ = resize(&mut app, 700.0);
            assert!(layout(&app).call && !layout(&app).thread);
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerOpened(THREAD_ITEM.into()));
            let _ = app.update(Message::PickerSubmitted);
            let sent = drain(&mut rx);
            assert_eq!(reactions(&sent), 0, "an unseen reaction: {sent:?}");
            assert!(app.actions.picker.is_none());
        }

        #[test]
        fn no_picker_opens_for_a_room_item_while_a_panel_hides_the_room_pane() {
            let (mut app, mut rx) = main_app();
            append(&mut app, vec![message_item("m", "hi", false, ALL_ACTIONS)]);
            open_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerOpened("m".into()));
            let _ = app.update(Message::PickerSubmitted);
            assert_eq!(reactions(&drain(&mut rx)), 0);
            assert!(app.actions.picker.is_none());
        }

        #[test]
        fn a_thread_picker_works_where_the_panel_is_beside_the_room() {
            let (mut app, mut rx) = main_app();
            reactable_thread(&mut app);
            let _ = resize(&mut app, 1280.0);
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerOpened(THREAD_ITEM.into()));
            assert!(app.actions.picker.is_some());
            let _ = app.update(Message::PickerSubmitted);
            assert_eq!(reactions(&drain(&mut rx)), 1);
        }

        #[test]
        fn shrinking_the_window_closes_a_picker_that_would_be_hidden() {
            // A room item's picker, then the thread panel fills the window.
            let (mut app, mut rx) = main_app();
            append(&mut app, vec![message_item("m", "hi", false, ALL_ACTIONS)]);
            open_thread(&mut app);
            let _ = resize(&mut app, 1280.0);
            let _ = app.update(Message::PickerOpened("m".into()));
            assert!(app.actions.picker.is_some());
            let _ = resize(&mut app, 700.0);
            assert!(app.actions.picker.is_none(), "closed with the resize");
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerQuery(MessageText::from("smile")));
            let _ = app.update(Message::PickerSubmitted);
            assert_eq!(reactions(&drain(&mut rx)), 0);

            // A thread item's picker, then the call panel takes the turn.
            let (mut app, mut rx) = main_app();
            reactable_thread(&mut app);
            open_call_panel(&mut app);
            let _ = resize(&mut app, 1280.0);
            let _ = app.update(Message::PickerOpened(THREAD_ITEM.into()));
            assert!(app.actions.picker.is_some());
            let _ = resize(&mut app, 700.0);
            assert!(!layout(&app).thread);
            assert!(app.actions.picker.is_none(), "closed with the resize");
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerSubmitted);
            assert_eq!(reactions(&drain(&mut rx)), 0);

            // A room item's picker in a narrow window, then a thread opens.
            let (mut app, mut rx) = main_app();
            append(&mut app, vec![message_item("m", "hi", false, ALL_ACTIONS)]);
            let _ = resize(&mut app, 700.0);
            let _ = app.update(Message::PickerOpened("m".into()));
            assert!(app.actions.picker.is_some(), "the room pane is shown");
            open_thread(&mut app);
            assert!(app.actions.picker.is_none(), "the panel now hides it");
            let _ = drain(&mut rx);
            let _ = app.update(Message::PickerSubmitted);
            assert_eq!(reactions(&drain(&mut rx)), 0);
        }

        #[test]
        fn a_hidden_room_pane_sends_no_receipt_until_it_is_shown_again() {
            let (mut app, mut rx) = main_app();
            open_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            let _ = drain(&mut rx);
            append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
            let _ = app.update(Message::WindowFocus(false));
            let _ = app.update(Message::WindowFocus(true));
            let _ = app.update(Message::ReadDue(RoomId::from("!a:x")));
            let sent = drain(&mut rx);
            assert_eq!(marks(&sent), Vec::<bool>::new(), "{sent:?}");
            assert!(!app.timeline.bottom_seen, "nothing was seen");
            let _ = resize(&mut app, 1280.0);
            assert_eq!(marks(&drain(&mut rx)), [false], "once, when shown");
            let _ = resize(&mut app, 700.0);
            let _ = resize(&mut app, 1280.0);
            assert_eq!(marks(&drain(&mut rx)), Vec::<bool>::new(), "same item");
        }

        #[test]
        fn closing_the_panel_shows_the_room_pane_and_sends_the_receipt() {
            let (mut app, mut rx) = main_app();
            open_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
            let _ = drain(&mut rx);
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            assert_eq!(marks(&drain(&mut rx)), [false]);
        }

        #[test]
        fn leaving_a_room_that_was_hidden_moves_no_marker() {
            let (mut app, mut rx) = main_app();
            append(&mut app, vec![message_item("a", "hi", false, ALL_ACTIONS)]);
            open_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            let _ = app.update(Message::ReadDue(RoomId::from("!a:x")));
            let _ = drain(&mut rx);
            let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
            let sent = drain(&mut rx);
            assert!(
                !sent.iter().any(|c| matches!(
                    c,
                    Command::MoveReadMarker { .. } | Command::MarkRead { .. }
                )),
                "{sent:?}"
            );
        }

        #[test]
        fn ctrl_u_and_drops_are_refused_while_a_panel_hides_the_room() {
            use crate::dialogs::RecordingDialogs;
            use crate::upload::UploadPaths;
            let (mut app, _rx) = main_app();
            let dialogs = RecordingDialogs::default();
            app.set_dialogs(Box::new(dialogs.clone()));
            open_thread(&mut app);
            let _ = resize(&mut app, 700.0);
            key(&mut app, Shortcut::Attach);
            assert_eq!(*dialogs.picked.lock().expect("mutex"), 0, "no file dialog");
            let _ = app.update(Message::FileHovered);
            assert!(!app.uploads.hovering);
            let dropped = UploadPaths(vec![std::path::PathBuf::from("/x/a.txt")]);
            let _ = app.update(Message::FileDropped(dropped.clone()));
            assert!(app.uploads.dropped.is_empty());
            assert!(app.uploads.dialog.is_none());
            // The thread panel's own Attach is on screen and stays.
            let _ = app.update(Message::Threads(ThreadsMessage::Attach));
            assert_eq!(*dialogs.picked.lock().expect("mutex"), 1);
            // With the room pane back, both work.
            let _ = app.update(Message::Threads(ThreadsMessage::Close));
            key(&mut app, Shortcut::Attach);
            assert_eq!(*dialogs.picked.lock().expect("mutex"), 2);
            let _ = app.update(Message::FileDropped(dropped));
            assert!(!app.uploads.dropped.is_empty());
        }
    }

    // ---- Task 17: Phase 5 redaction sweep ----

    /// One sentinel per kind of Phase 5 text. Each is built into the state
    /// or message that carries it, and none may come back out in a `Debug`
    /// string of the app or of any message.
    mod redaction_sweep {
        use std::path::PathBuf;

        use twicecat_core::api::{
            AvatarId, EventId, MediaKind, MembershipView, NotificationKind,
            NotificationSettingsView, NotificationView, OwnProfileView, PowerLevelView, Role,
            RoomDetailsView, RoomMemberView, RoomNotificationModeView as Mode, RoomPermissions,
        };

        use super::*;
        use crate::dialogs::{ClipboardContent, RecordingDialogs};
        use crate::notifications::RecordingNotifier;
        use crate::rooms::RoomsMessage as R;
        use crate::settings::SettingsMessage as S;
        use crate::switcher::SwitcherMessage as W;
        use crate::threads::ThreadsMessage as T;
        use crate::upload::UploadPaths;

        const SENTINELS: [&str; 12] = [
            "sweep-file-name",
            "sweep-save-path",
            "sweep-upload-name",
            "sweep-upload-body",
            "sweep-preview",
            "sweep-room-name",
            "sweep-sender-name",
            "sweep-topic",
            "sweep-member",
            "sweep-draft",
            "sweep-keyword",
            "sweep-query",
        ];

        fn text(s: &str) -> MessageText {
            MessageText::from(s)
        }

        /// Fails if a sentinel is in the app's `Debug` or in the `Debug` of
        /// any of `messages`.
        fn sweep(context: &str, app: &App, messages: &[Message]) {
            assert_no_sentinel(context, &format!("{app:?}"), &SENTINELS);
            for message in messages {
                assert_no_sentinel(context, &format!("{message:?}"), &SENTINELS);
            }
        }

        #[test]
        fn the_media_viewer_and_cache_hold_no_names_or_paths_in_debug() {
            let (mut app, mut rx, _) = media_app(0);
            let item = media_item("pic", MediaKind::Image, "sweep-file-name.png", 640, 480);
            let _ = app.update(core(CoreEvent::TimelineDiff {
                room_id: room_a(),
                diffs: vec![ListDiff::Append { values: vec![item] }],
            }));
            let _ = drain(&mut rx);
            arrive(&mut app, "pic", Some(decoded(8, 6)));
            let _ = app.update(Message::MediaClicked("pic".into()));
            let _ = app.update(media_event("pic", MediaSize::Full, Ok(png(9, 7))));
            let decoded_full = Message::MediaDecoded {
                room_id: room_a(),
                item_id: "pic".into(),
                size: MediaSize::Full,
                decoded: Some(decoded(9, 7)),
            };
            let _ = app.update(decoded_full.clone());
            let chosen = Message::SavePicked {
                room_id: room_a(),
                item_id: "pic".into(),
                path: Some(media::SavePath::from(PathBuf::from(
                    "/home/u/sweep-save-path",
                ))),
            };
            let _ = app.update(chosen.clone());
            assert!(app.media.viewer.is_some(), "the viewer is open");
            sweep(
                "media viewer and cache",
                &app,
                &[
                    decoded_full,
                    chosen,
                    media_event("pic", MediaSize::Full, Ok(png(2, 2))),
                ],
            );
        }

        #[test]
        fn the_upload_confirmation_with_a_pasted_image_holds_no_names_or_bytes_in_debug() {
            let (mut app, _rx) = main_app();
            let dialogs = RecordingDialogs::default();
            app.set_dialogs(Box::new(dialogs));
            let pasted = Message::Pasted(ClipboardContent::Image {
                width: 2,
                height: 2,
                rgba: Arc::new(vec![171; 16]),
            });
            let _ = app.update(pasted.clone());
            // A drop waits for its debounce with the path in the model.
            let dropped = Message::FileDropped(UploadPaths(vec![PathBuf::from(
                "/home/u/sweep-upload-name-dropped.txt",
            )]));
            let _ = app.update(dropped.clone());
            assert!(!app.uploads.dropped.is_empty(), "the drop is waiting");
            sweep("dropped file in its debounce", &app, &[dropped]);
            app.uploads.dropped.clear();
            let chosen = Message::FilesChosen(UploadPaths(vec![PathBuf::from(
                "/home/u/sweep-upload-name.txt",
            )]));
            let _ = app.update(chosen.clone());
            let dialog = app.uploads.dialog.as_ref().expect("a confirmation");
            assert_eq!(dialog.entries.len(), 2, "the picture and the file");
            let ids: Vec<u64> = dialog.entries.iter().map(|e| e.id).collect();
            let prepared = Message::UploadPrepared {
                entry: ids[1],
                outcome: Box::new(crate::upload::prepare_bytes(
                    "sweep-upload-name.txt".to_owned(),
                    b"sweep-upload-body".to_vec(),
                    None,
                )),
            };
            let _ = app.update(prepared.clone());
            sweep("upload confirmation", &app, &[pasted, chosen, prepared]);
            assert!(!format!("{app:?}").contains("171, 171"), "no pixels");
        }

        #[test]
        fn a_pending_notification_holds_no_text_in_debug() {
            let (mut app, _rx) = main_app();
            let notifier = RecordingNotifier::default();
            let shown = notifier.shown.clone();
            app.set_notifier(Box::new(notifier));
            let _ = app.update(Message::WindowFocus(false));
            let view = NotificationView {
                room_id: RoomId::from("!b:x"),
                event_id: EventId::from("$e"),
                room_name: "sweep-room-name".into(),
                sender: UserId::from("@bob:x"),
                sender_name: Some("sweep-sender-name".into()),
                is_dm: false,
                kind: NotificationKind::Message,
                preview: Some(text("sweep-preview")),
            };
            let event = core(CoreEvent::Notification(view));
            let _ = app.update(event.clone());
            let requests = shown.lock().expect("test mutex").clone();
            assert_eq!(requests.len(), 1, "the notification reached the desktop");
            assert_no_sentinel("notification request", &format!("{requests:?}"), &SENTINELS);
            sweep("pending notification", &app, &[event]);
        }

        #[test]
        fn room_settings_with_a_topic_and_the_member_list_hold_no_text_in_debug() {
            let (mut app, mut rx) = main_app();
            let rooms = |app: &mut App, m: R| {
                let _ = app.update(Message::Rooms(m));
            };
            rooms(&mut app, R::OpenSettings);
            let _ = drain(&mut rx);
            let details = core(CoreEvent::RoomDetails(RoomDetailsView {
                room_id: room_a(),
                name: "sweep-room-name".into(),
                topic: Some("sweep-topic".into()),
                avatar: None,
                is_encrypted: false,
                alias: None,
                permissions: RoomPermissions {
                    can_set_name: true,
                    can_set_topic: true,
                    can_set_avatar: true,
                    can_enable_encryption: true,
                    can_invite: true,
                },
            }));
            let _ = app.update(details.clone());
            let typed = Message::Rooms(R::TopicInput(text("sweep-topic")));
            let _ = app.update(typed.clone());
            rooms(&mut app, R::OpenMembers);
            let members = core(CoreEvent::RoomMembers {
                room_id: room_a(),
                members: vec![RoomMemberView {
                    user_id: UserId::from("@bob:x"),
                    display_name: Some("sweep-member".into()),
                    avatar: None,
                    power: PowerLevelView::Level(0),
                    role: Role::User,
                    membership: MembershipView::Joined,
                }],
                truncated: false,
            });
            let _ = app.update(members.clone());
            sweep(
                "room settings and members",
                &app,
                &[details, typed, members],
            );
        }

        #[test]
        fn the_thread_panel_with_a_draft_holds_no_text_in_debug() {
            let (mut app, _rx) = main_app();
            let root = EventId::from("$r:x");
            let _ = app.update(Message::Threads(T::Open(root)));
            let draft = Message::Threads(T::Draft(text("sweep-draft")));
            let _ = app.update(draft.clone());
            let panel = app.threads.panel.as_ref().expect("an open thread");
            assert_eq!(panel.draft, "sweep-draft", "the draft is kept");
            sweep("thread panel", &app, &[draft]);
        }

        #[test]
        fn the_settings_screen_with_keywords_holds_no_text_in_debug() {
            let (mut app, _rx) = main_app();
            let _ = app.update(Message::Settings(S::Open));
            let _ = app.update(core(CoreEvent::OwnProfile(OwnProfileView {
                display_name: Some("sweep-member".into()),
                avatar: Some(AvatarId::from("mxc://x/a")),
            })));
            let settings = core(CoreEvent::NotificationSettings(NotificationSettingsView {
                group_default: Mode::AllMessages,
                dm_default: Mode::AllMessages,
                keywords: vec!["sweep-keyword".into()],
            }));
            let _ = app.update(settings.clone());
            let typed = Message::Settings(S::KeywordInput(text("sweep-keyword")));
            let _ = app.update(typed.clone());
            let removal = Message::Settings(S::RemoveKeyword(text("sweep-keyword")));
            assert!(app.settings_ui.open, "the screen is open");
            sweep("settings screen", &app, &[settings, typed, removal]);
        }

        #[test]
        fn the_switcher_with_a_query_holds_no_text_in_debug() {
            let (mut app, _rx) = main_app();
            let _ = app.open_switcher();
            let typed = Message::Switcher(W::Input("sweep-query".into()));
            let _ = app.update(typed.clone());
            assert_eq!(app.switcher.query, "sweep-query", "the query is kept");
            sweep("switcher", &app, &[typed]);
        }

        #[test]
        fn the_app_debug_reports_the_settings_screen_and_the_switcher() {
            let (app, _rx) = main_app();
            let shown = format!("{app:?}");
            assert!(shown.contains("settings_ui"), "{shown}");
            assert!(shown.contains("switcher"), "{shown}");
        }
    }
}
