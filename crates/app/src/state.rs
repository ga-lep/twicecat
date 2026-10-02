//! App state and the pure `update` function.
//!
//! `update` only mutates state, queues commands without blocking, and returns
//! [`Task`]s. It never awaits, never touches the network and never sees an SDK
//! type.

use std::sync::Mutex;
use std::time::Duration;

use iced::theme;
use iced::{Element, Subscription, Task, Theme, clipboard, system, window};
use tessera_core::api::{
    Command, CoreError, CoreEvent, ListDiff, MessageText, Password, RoomId, RoomListQuery,
    RoomListTab, RoomSummary, SyncState, TimelineItem, UserId,
};
use tokio::sync::mpsc;

use crate::avatars::Avatars;
use crate::bridge::{self, Bridge, EventsSlot};
use crate::room_list;
use crate::screens::login::{self, BrowserUrl, LoginForm};
use crate::screens::{self, loading, main_view};
use crate::text_safety;

/// How long to wait for the core to close after `Shutdown` (plan Q18).
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(10);

/// Where commands go. The [`Bridge`] in production; a plain channel sender in
/// tests, so `update` can be tested without a runtime.
pub trait CommandSink: Send {
    /// Queues a command without blocking. `None` when queued; `Some` task when
    /// the channel was full (the task queues it later).
    fn send(&self, command: Command) -> Option<Task<Message>>;

    /// A task that yields [`Message::ShutdownDeadline`] after `after`.
    fn deadline(&self, after: Duration) -> Task<Message>;
}

impl CommandSink for Bridge {
    fn send(&self, command: Command) -> Option<Task<Message>> {
        Bridge::send(self, command)
    }

    fn deadline(&self, after: Duration) -> Task<Message> {
        Bridge::deadline(self, after)
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
    /// The composer text changed; its `Debug` is redacted.
    ComposerChanged(MessageText),
    /// The user pressed Enter in the composer.
    ComposerSubmitted,
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
    /// The user asked to close the window.
    CloseRequested,
    /// The core did not close in time after `Shutdown`.
    ShutdownDeadline,
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
    pub timeline: Vec<TimelineItem>,
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
    /// The composer text of the selected room.
    pub composer: MessageText,
    /// A non-secret line about the last failed send.
    pub send_error: Option<String>,
    /// The system theme mode.
    pub theme_mode: theme::Mode,
    /// Set when the app decided to exit (the task itself is opaque to tests).
    pub exit_requested: bool,
}

// Manual impl: the sink and the event slot are not `Debug`, and the composer
// and password fields stay redacted by their own types (plan Q19).
impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("screen", &self.screen)
            .field("rooms", &self.rooms.len())
            .field("timeline", &self.timeline.len())
            .field("selected", &self.selected)
            .field("login", &self.login)
            .field("login_in_flight", &self.login_in_flight)
            .field("composer", &self.composer)
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
            timeline: Vec::new(),
            user_id: None,
            sync_state: SyncState::Idle,
            login: LoginForm::new(),
            login_in_flight: false,
            pending_cancel: false,
            notice: None,
            browser_link: None,
            browser_opened: None,
            composer: MessageText::from(String::new()),
            send_error: None,
            theme_mode: theme::Mode::None,
            exit_requested: false,
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

    fn send(&self, command: Command) -> Task<Message> {
        self.sink.send(command).unwrap_or_else(Task::none)
    }

    /// Applies a message.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Core(event) => self.on_core(*event),
            Message::CoreClosed => {
                if self.screen == Screen::ShuttingDown {
                    self.exit()
                } else {
                    self.screen = Screen::Disconnected;
                    Task::none()
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
            Message::ComposerChanged(text) => {
                if self.selected.is_some() {
                    self.composer = text;
                }
                Task::none()
            }
            Message::ComposerSubmitted => self.submit_composer(),
            Message::LogoutRequested => self.send(Command::Logout),
            Message::SelectRoom(id) => {
                if !room_list::contains(&self.rooms, &id) {
                    return Task::none();
                }
                self.selected = Some(id.clone());
                self.timeline.clear();
                self.clear_draft();
                self.send(Command::SelectRoom(id))
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
                self.screen = Screen::ShuttingDown;
                Task::batch([
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
            CoreEvent::LoggedIn { user_id, .. } => {
                // Any cancel reply would have come before this login.
                self.pending_cancel = false;
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
            CoreEvent::LoggedOut => self.back_to_login("You were logged out."),
            CoreEvent::SessionExpired => {
                self.back_to_login("Your session expired. Please log in again.");
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
                self.apply_timeline(&room_id, &diffs);
            }
            CoreEvent::SendFailed { room_id, error } => {
                if self.selected.as_ref() == Some(&room_id) {
                    self.send_error = Some(safe_line(&error.to_string()));
                }
            }
            CoreEvent::Error(error) => task = self.on_core_error(&error),
            CoreEvent::Avatar { id, image } => self.avatars.answer(&id, image),
            // Shown by the Phase 3 timeline and composer; ignored until then.
            CoreEvent::Pagination { .. }
            | CoreEvent::Typing { .. }
            | CoreEvent::Members { .. }
            | CoreEvent::ActionFailed { .. } => {}
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

    /// Sends the composer text to the selected room and clears the field.
    fn submit_composer(&mut self) -> Task<Message> {
        if self.selected.is_none() || self.composer.expose().trim().is_empty() {
            return Task::none();
        }
        let text = std::mem::replace(&mut self.composer, MessageText::from(String::new()));
        self.send_error = None;
        self.send(Command::SendMessage { text })
    }

    /// Forgets everything about a login attempt: the flag, the browser link
    /// and the password.
    fn end_login_attempt(&mut self) {
        self.login_in_flight = false;
        self.browser_link = None;
        self.browser_opened = None;
        self.login.password = Password::from(String::new());
    }

    fn clear_draft(&mut self) {
        self.composer = MessageText::from(String::new());
        self.send_error = None;
    }

    fn back_to_login(&mut self, notice: &str) {
        self.pending_cancel = false;
        self.user_id = None;
        self.rooms.clear();
        self.total_rooms = None;
        self.load_more_at = None;
        self.avatars.clear();
        self.list_offset_y = 0.0;
        self.deselect();
        self.sync_state = SyncState::Idle;
        self.notice = Some(notice.to_owned());
        self.end_login_attempt();
        self.screen = Screen::Login;
    }

    fn deselect(&mut self) {
        self.selected = None;
        self.timeline.clear();
        self.clear_draft();
    }

    /// Applies timeline diffs of the selected room; other rooms are ignored.
    fn apply_timeline(&mut self, room_id: &RoomId, diffs: &[ListDiff<TimelineItem>]) {
        if self.selected.as_ref() != Some(room_id) {
            return;
        }
        for diff in diffs {
            diff.apply(&mut self.timeline);
        }
    }

    /// The window content.
    pub fn view(&self) -> Element<'_, Message> {
        match self.screen {
            Screen::Starting => screens::centered_text("Starting\u{2026}"),
            Screen::Login => login::view(self),
            Screen::WaitingForBrowser => login::waiting_view(self),
            Screen::Loading => loading::view(self),
            Screen::Main => main_view::view(self),
            Screen::ShuttingDown => screens::centered_text("Closing\u{2026}"),
            Screen::Disconnected => screens::centered_text("The core stopped"),
        }
    }

    /// The core's events and window close requests.
    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            bridge::events_subscription(self.events.clone()),
            window::close_requests().map(|_| Message::CloseRequested),
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
    use std::time::Instant;

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

        let message = Message::ComposerChanged(MessageText::from("secret-body-x"));
        assert!(!format!("{message:?}").contains("secret-body-x"));
        let _ = app.update(core(CoreEvent::LoggedOut));
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a"])));
        let _ = app.update(Message::SelectRoom(RoomId::from("!a:x")));
        let _ = app.update(message);
        assert_eq!(app.composer.expose(), "secret-body-x");
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

    #[test]
    fn composer_enter_sends_the_message_and_clears_the_field() {
        let (mut app, mut rx) = main_app();
        let _ = app.update(Message::ComposerChanged(MessageText::from("hello")));
        let _ = app.update(Message::ComposerSubmitted);
        match rx.try_recv() {
            Ok(Command::SendMessage { text }) => assert_eq!(text.expose(), "hello"),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(app.composer.expose(), "");
        let _ = app.update(Message::ComposerSubmitted);
        assert!(rx.try_recv().is_err(), "an empty composer sends nothing");
        let _ = app.update(Message::ComposerChanged(MessageText::from("  \t ")));
        let _ = app.update(Message::ComposerSubmitted);
        assert!(rx.try_recv().is_err(), "blank text sends nothing");
    }

    #[test]
    fn composer_is_disabled_without_a_selected_room() {
        let (mut app, mut rx) = login_app();
        logged_in(&mut app);
        let _ = app.update(core(room_event(&["a"])));
        let _ = app.update(Message::ComposerChanged(MessageText::from("hello")));
        assert_eq!(app.composer.expose(), "", "input ignored");
        let _ = app.update(Message::ComposerSubmitted);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn send_failed_shows_a_non_secret_error_for_the_selected_room() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::ComposerChanged(MessageText::from("secret-body-x")));
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
        let _ = app.update(Message::ComposerChanged(MessageText::from("draft")));
        let _ = app.update(Message::SelectRoom(RoomId::from("!b:x")));
        assert_eq!(app.composer.expose(), "");
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
        let _ = app.update(Message::ComposerChanged(MessageText::from("draft")));
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert_eq!(app.composer.expose(), "");
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
        let _ = app.update(Message::ComposerChanged(MessageText::from("unsent")));
        let _ = app.update(Message::SearchChanged("zzz".into()));
        let _ = app.update(core(room_event(&["z"])));
        assert_eq!(app.selected, Some(RoomId::from("!a:x")));
        assert_eq!(app.composer.expose(), "unsent");
        // Emptied list, too.
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Clear])));
        assert_eq!(app.selected, Some(RoomId::from("!a:x")));
        assert_eq!(app.composer.expose(), "unsent");
    }

    #[test]
    fn switching_tabs_keeps_the_open_room_and_its_draft() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::ComposerChanged(MessageText::from("unsent")));
        let _ = app.update(Message::TabSelected(RoomListTab::People));
        let _ = app.update(core(room_event(&["z"])));
        assert_eq!(app.selected, Some(RoomId::from("!a:x")));
        assert_eq!(app.composer.expose(), "unsent");
    }

    #[test]
    fn the_open_room_is_deselected_when_the_default_query_loses_it() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::ComposerChanged(MessageText::from("unsent")));
        assert_eq!(app.query, RoomListQuery::default());
        let _ = app.update(core(room_event(&["b"])));
        assert_eq!(app.selected, None);
        assert_eq!(app.composer.expose(), "");

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
}
