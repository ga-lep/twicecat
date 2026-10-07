//! App state and the pure `update` function.
//!
//! `update` only mutates state, queues commands without blocking, and returns
//! [`Task`]s. It never awaits, never touches the network and never sees an SDK
//! type.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use iced::theme;
use iced::widget::{operation, scrollable};
use iced::{Element, Subscription, Task, Theme, clipboard, system, window};
use tessera_core::api::{
    ActionKind, Command, CoreError, CoreEvent, Draft, EventId, ListDiff, MessageDetails,
    MessageText, OutgoingMessage, Password, ReauthAction, RoomId, RoomListQuery, RoomListTab,
    RoomSummary, SyncState, TimelineItem, TimelineItemKind, TypingUser, UserId, UserTrust,
};
use tokio::sync::mpsc;

use crate::actions::{self, ActionButtons, ActionState, NOTICE_DURATION, Notice, Picker};
use crate::avatars::Avatars;
use crate::bridge::{self, Bridge, EventsSlot};
use crate::composer::{self, Composer, ComposerKey, EditorAction, SearchStep};
use crate::emoji;
use crate::links::{self, LinkBar};
use crate::rich_text::{LinkTarget, MAX_REVEALED, Revealed};
use crate::room_list;
use crate::screens::login::{self, BrowserUrl, LoginForm};
use crate::screens::{self, loading, main_view};
use crate::security::{Busy, SecurityMessage, SecurityModel};
use crate::sessions::{SessionsMessage, SessionsModel, logout_needs_warning};
use crate::text_safety;
use crate::timeline::{self, Effects, TimelineView};
use crate::verification::{VerificationMessage, VerificationModel};

/// How many users' trust the app keeps: the selected peer, and a stale one or
/// two from watchers the core replaced.
pub const MAX_TRUST_KEPT: usize = 8;

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
    /// Set when the app decided to exit (the task itself is opaque to tests).
    pub exit_requested: bool,
    /// Clipboard reads sent by `drop_secrets` whose answer is still due.
    pub(crate) flushes_pending: u32,
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
            .field("login", &self.login)
            .field("login_in_flight", &self.login_in_flight)
            .field("composer", &self.composer)
            .field("actions", &self.actions)
            .field("security", &self.security)
            .field("verification", &self.verification)
            .field("sessions", &self.sessions)
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
            load_more_at: None,
            avatars: Avatars::default(),
            selected: None,
            timeline: TimelineView::default(),
            user_trust: Vec::new(),
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
            exit_requested: false,
            flushes_pending: 0,
            core_closed: false,
        };
        let restore = app.send(Command::RestoreSession);
        let boot = Task::batch([restore, system::theme().map(Message::ThemeMode)]);
        (app, boot)
    }

    /// A boot function for `iced::application`; Iced calls it once.
    pub fn booter(startup: Startup) -> impl Fn() -> (App, Task<Message>) {
        let startup = Mutex::new(Some(startup));
        move || {
            let startup = startup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take()
                .expect("iced boots the application exactly once");
            App::new(startup)
        }
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
        );
        let task = self.update_inner(message);
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
            Message::SuggestionPicked(index) => {
                self.pick_suggestion_at(Some(index), Instant::now())
            }
            Message::MembersDue => self.members_due_at(Instant::now()),
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
            Message::WindowFocus(focused) => {
                self.window_focused = focused;
                self.read_receipt(Instant::now())
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
                if self.buttons_for(&id).is_some_and(|b| b.react) {
                    self.actions.picker = Some(Picker {
                        target: id,
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
                self.selected = Some(id.clone());
                self.timeline.reset();
                self.user_trust.clear();
                self.typing.clear();
                self.actions.reset();
                self.clear_draft();
                self.reset_link_state();
                Task::batch([
                    stop_typing,
                    marker,
                    save,
                    self.send(Command::SelectRoom(id)),
                ])
            }
            Message::TabSelected(tab) => {
                let query = RoomListQuery {
                    tab,
                    search: self.query.search.clone(),
                };
                self.set_query(query)
            }
            Message::SearchChanged(search) => {
                let query = RoomListQuery {
                    tab: self.query.tab,
                    search: search.trim().to_owned(),
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
                task = self.drop_secrets();
                self.security = SecurityModel::default();
                self.security.fresh_login = !restored;
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
                task = self.after_list_change();
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
        let username = self.login.username.trim().to_owned();
        if username.is_empty() || self.login.password.expose().is_empty() {
            self.notice = Some("Enter a user name and password.".to_owned());
            return Task::none();
        }
        let server = Some(self.login.server.trim().to_owned()).filter(|s| !s.is_empty());
        let password = std::mem::replace(&mut self.login.password, Password::from(String::new()));
        self.notice = None;
        self.login_in_flight = true;
        self.send(Command::LoginPassword {
            username,
            password,
            server,
        })
    }

    /// Asks the core for an OAuth login at the server named in the form.
    fn request_oauth(&mut self) -> Task<Message> {
        if self.screen != Screen::Login || self.login_in_flight {
            return Task::none();
        }
        let server = self.login.server.trim().to_owned();
        if server.is_empty() {
            self.notice = Some("Enter a server name to sign in with the browser.".to_owned());
            return Task::none();
        }
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
    fn set_query(&mut self, query: RoomListQuery) -> Task<Message> {
        if query == self.query {
            return Task::none();
        }
        self.query = query.clone();
        self.load_more_at = None;
        self.send(Command::SetRoomListQuery(query))
    }

    /// What follows every change of the list or of the viewport: paging and
    /// avatar requests.
    fn after_list_change(&mut self) -> Task<Message> {
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
            room_list::ROW_HEIGHT,
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
            room_list::ROW_HEIGHT,
            len,
            room_list::OVERSCAN_ROWS,
        );
        if !room_list::should_load_more(range.end, len, self.total_rooms) {
            return Task::none();
        }
        self.load_more_at = Some(len);
        self.send(Command::LoadMoreRooms)
    }

    /// Applies an editor action to the composer of the open room and sends
    /// what that asks for: typing notices and member searches.
    fn composer_action_at(
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
                let was_edit = matches!(self.composer.mode, composer::Mode::Edit(_));
                self.composer.escape();
                self.after_edit_left(was_edit)
            }
        }
    }

    /// The message `id` of the open timeline: its plain body, whether it is
    /// the user's own, and its details. `None` for other kinds of item.
    fn find_message(&self, id: &str) -> Option<(&str, bool, &MessageDetails)> {
        self.timeline
            .items
            .iter()
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
        if self.selected.is_none() || !self.buttons_for(&id).is_some_and(|b| b.reply) {
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
        if self.selected.is_none() || !self.buttons_for(&id).is_some_and(|b| b.edit) {
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
            Some(picker) => self.toggle_reaction(&picker.target, key),
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
    fn show_notice_text(&mut self, text: String) -> Task<Message> {
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
        self.list_offset_y = 0.0;
        self.deselect();
        self.sync_state = SyncState::Idle;
        let flush = self.drop_secrets();
        self.security = SecurityModel::default();
        self.verification = None;
        self.sessions = SessionsModel::default();
        self.notice = Some(notice.to_owned());
        self.end_login_attempt();
        self.screen = Screen::Login;
        flush
    }

    fn deselect(&mut self) {
        self.selected = None;
        self.timeline.reset();
        self.user_trust.clear();
        self.typing.clear();
        self.actions.reset();
        self.clear_draft();
        self.reset_link_state();
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
                let id = RoomId::from(room);
                if room_list::contains(&self.rooms, &id) {
                    self.update(Message::SelectRoom(id))
                } else {
                    Task::none()
                }
            }
            // Profiles are Phase 5.
            LinkTarget::User(_) => Task::none(),
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
        self.timeline_effects(effects)
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
        tasks.push(self.read_receipt(Instant::now()));
        Task::batch(tasks)
    }

    /// Sends the read receipt when one is due, or starts the timer that
    /// asks again (plan Q12). `now` is a parameter for the tests.
    fn read_receipt(&mut self, now: Instant) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
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
            Some(room_id) if self.timeline.bottom_seen => self.send(Command::MoveReadMarker {
                room_id: room_id.clone(),
            }),
            _ => Task::none(),
        }
    }

    /// **Retry** or **Discard** for item `id`, only while it is a failed
    /// local echo.
    fn echo_command(&mut self, item_id: String, discard: bool) -> Task<Message> {
        let failed = self
            .find_message(&item_id)
            .is_some_and(|(_, _, details)| details.send_failure.is_some());
        let Some(room_id) = self.selected.clone().filter(|_| failed) else {
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
            Screen::Starting => screens::centered_text("Starting\u{2026}"),
            Screen::Login => login::view(self),
            Screen::WaitingForBrowser => login::waiting_view(self),
            Screen::Loading => {
                let base = screens::security::layer(
                    loading::view(self),
                    screens::security::top_layer(self),
                );
                screens::verification::layer(base, self)
            }
            Screen::Main => screens::verification::layer(main_view::view(self), self),
            Screen::ShuttingDown => screens::centered_text("Closing\u{2026}"),
            Screen::Disconnected => screens::centered_text("The core stopped"),
        }
    }

    /// The core's events and window close requests.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            bridge::events_subscription(self.events.clone()),
            window::close_requests().map(|_| Message::CloseRequested),
            iced::event::listen_with(|event, _status, _id| match event {
                iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocus(true)),
                iced::Event::Window(window::Event::Unfocused) => Some(Message::WindowFocus(false)),
                _ => None,
            }),
            system::theme_changes().map(Message::ThemeMode),
        ])
    }

    /// The theme for the current system mode (plan Q13).
    pub fn theme(&self) -> Option<Theme> {
        Some(crate::theme::theme_for(self.theme_mode))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iced::widget::text_editor::{Action, Edit};
    use tessera_core::api::{DeviceId, ListDiff, RoomId};

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
        assert_eq!(name(&app), crate::theme::LIGHT_NAME);
        let _ = app.update(Message::ThemeMode(theme::Mode::Dark));
        assert_eq!(name(&app), crate::theme::DARK_NAME);
        assert!(app.theme().expect("themed").extended_palette().is_dark);
        let _ = app.update(Message::ThemeMode(theme::Mode::Light));
        assert_eq!(name(&app), crate::theme::LIGHT_NAME);
        let _ = app.update(Message::ThemeMode(theme::Mode::None));
        assert_eq!(name(&app), crate::theme::LIGHT_NAME);
    }

    // ---- Task 9: screens ----

    use tessera_core::api::{AuthUrl, AvatarId, AvatarImage, CoreError};

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
            !code.contains("tessera_app::open") && !code.contains("that_detached"),
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
                query: tessera_core::api::MessageText::from(query),
                members: vec![tessera_core::api::Member {
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
            query: tessera_core::api::MessageText::from("a"),
            members: vec![tessera_core::api::Member {
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
    use tessera_core::api::{
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

    fn saved_drafts(commands: &[Command]) -> Vec<(String, Option<tessera_core::api::Draft>)> {
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

    fn draft_text(draft: &Option<tessera_core::api::Draft>) -> Option<String> {
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
        let draft = tessera_core::api::Draft {
            text: MessageText::from("@Bob: back"),
            mentions: vec![tessera_core::api::MentionRef {
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
            draft: tessera_core::api::Draft {
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
                assert_eq!(m.format, tessera_core::api::TextFormat::Markdown);
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
            [Command::Send(m)] if m.format == tessera_core::api::TextFormat::Html
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
            state: tessera_core::api::PaginationState::Idle,
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
            state: tessera_core::api::PaginationState::ReachedStart,
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

    fn typist(id: &str, name: Option<&str>) -> tessera_core::api::TypingUser {
        tessera_core::api::TypingUser {
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
            details.send_failure = Some(tessera_core::api::SendFailure { recoverable });
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
    use tessera_core::api::{
        ActionKind as Kind, CrossSigning, DeviceVerification, ReauthAction, RecoveryKey,
        RecoveryStatus, SecurityStatus,
    };

    fn sec(app: &mut App, message: SecurityMessage) -> Task<Message> {
        app.update(Message::Security(message))
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
        assert_eq!(app.security.visible(), Visible::Nothing);
        assert_eq!(app.security.busy, None);
        assert_eq!(app.security.notice, None);
        assert_eq!(app.security.banner(), Some(Banner::RecoveryDisabled));
    }

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
        assert_eq!(app.security.busy, None);
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
    use tessera_core::api::{
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
                reason: tessera_core::api::CancelReason::ByThem,
            },
        }));
        let _ = vmsg(&mut app, VM::Close);
        assert!(app.verification.is_none());
    }

    #[test]
    fn accept_and_decline_send_one_command_each() {
        let (mut app, mut rx) = verifying(false);
        vphase(&mut app, VP::Requested { incoming: true });
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
                sent: tessera_core::api::SendStatus::Sent,
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
    use tessera_core::api::{DeviceSummary, DeviceTrust};

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
            summary("THIS", Some("Tessera"), true, DeviceTrust::Verified),
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
            action: ActionKind::Devices,
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
            action: ActionKind::Devices,
            error: CoreError::Internal("the devices could not be signed out".into()),
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

    #[test]
    fn a_sign_outs_own_failures_end_it() {
        let own = [
            CoreError::ReauthFailed,
            CoreError::NotAllowed,
            CoreError::NotLoggedIn,
            CoreError::Internal("the devices could not be signed out".into()),
            CoreError::Internal("manage your devices on your account page".into()),
        ];
        for error in own {
            let (mut app, mut rx) = sessions_app();
            let _ = ses(
                &mut app,
                SessionsMessage::SignOutAsk(DeviceId::from("PHONE")),
            );
            let _ = ses(&mut app, SessionsMessage::SignOutConfirmed);
            let _ = drain(&mut rx);
            devices_failed(&mut app, error.clone());
            assert!(app.sessions.pending.is_none(), "{error:?} kept it");
        }
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
            action: ActionKind::Devices,
            error: CoreError::Internal("the devices could not be signed out".into()),
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
            action: ActionKind::Devices,
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
}
