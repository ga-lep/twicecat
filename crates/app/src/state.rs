//! App state and the pure `update` function.
//!
//! `update` only mutates state, queues commands without blocking, and returns
//! [`Task`]s. It never awaits, never touches the network and never sees an SDK
//! type.

use std::sync::Mutex;
use std::time::Duration;

use iced::theme;
use iced::widget::text;
use iced::{Element, Subscription, Task, Theme, system, window};
use tessera_core::api::{
    Command, CoreError, CoreEvent, ListDiff, Password, RoomId, RoomSummary, SyncState,
    TimelineItem, UserId,
};
use tokio::sync::mpsc;

use crate::bridge::{self, Bridge, EventsSlot};
use crate::room_list;

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
    /// The user submitted the login form.
    LoginSubmitted {
        /// Server name or URL; empty means "from the user ID".
        server: String,
        /// Localpart or full MXID.
        username: String,
        /// The password; its `Debug` is redacted.
        password: Password,
    },
    /// The user picked a room in the list.
    SelectRoom(RoomId),
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
    /// The room whose timeline is open.
    pub selected: Option<RoomId>,
    /// The selected room's timeline, kept in step with the core's diffs.
    pub timeline: Vec<TimelineItem>,
    /// The signed-in account.
    pub user_id: Option<UserId>,
    /// The last sync state.
    pub sync_state: SyncState,
    /// Why the last login failed.
    pub login_error: Option<CoreError>,
    /// The system theme mode.
    pub theme_mode: theme::Mode,
    /// Set when the app decided to exit (the task itself is opaque to tests).
    pub exit_requested: bool,
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
            selected: None,
            timeline: Vec::new(),
            user_id: None,
            sync_state: SyncState::Idle,
            login_error: None,
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
            Message::LoginSubmitted {
                server,
                username,
                password,
            } => {
                let server = Some(server.trim().to_owned()).filter(|s| !s.is_empty());
                self.login_error = None;
                self.send(Command::LoginPassword {
                    username,
                    password,
                    server,
                })
            }
            Message::SelectRoom(id) => {
                if !room_list::contains(&self.rooms, &id) {
                    return Task::none();
                }
                self.selected = Some(id.clone());
                self.timeline.clear();
                self.send(Command::SelectRoom(id))
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
        match event {
            CoreEvent::NoSession => self.screen = Screen::Login,
            CoreEvent::LoggedIn { user_id, .. } => {
                self.user_id = Some(user_id);
                self.login_error = None;
                self.screen = Screen::Loading;
            }
            CoreEvent::LoginFailed(error) => {
                self.login_error = Some(error);
                self.screen = Screen::Login;
            }
            CoreEvent::LoggedOut | CoreEvent::SessionExpired => {
                self.user_id = None;
                self.rooms.clear();
                self.total_rooms = None;
                self.deselect();
                self.sync_state = SyncState::Idle;
                self.screen = Screen::Login;
            }
            CoreEvent::SyncState(state) => self.sync_state = state,
            CoreEvent::RoomListChanged(diffs) => {
                room_list::apply_room_diffs(&mut self.rooms, &diffs);
                let selection_gone = self
                    .selected
                    .as_ref()
                    .is_some_and(|id| !room_list::contains(&self.rooms, id));
                if selection_gone {
                    self.deselect();
                }
                if self.screen == Screen::Loading {
                    self.screen = Screen::Main;
                }
            }
            CoreEvent::RoomListLoaded { total_rooms } => {
                self.total_rooms = total_rooms;
                if self.screen == Screen::Loading {
                    self.screen = Screen::Main;
                }
            }
            CoreEvent::TimelineDiff { room_id, diffs } => {
                self.apply_timeline(&room_id, &diffs);
            }
            // Later steps: avatars, OAuth, errors.
            CoreEvent::OAuthUrl(_)
            | CoreEvent::Avatar { .. }
            | CoreEvent::SendFailed { .. }
            | CoreEvent::Error(_) => {}
        }
        Task::none()
    }

    fn deselect(&mut self) {
        self.selected = None;
        self.timeline.clear();
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
        let label = match self.screen {
            Screen::Starting => "Starting\u{2026}",
            Screen::Login => "Log in",
            Screen::Loading => "Loading\u{2026}",
            Screen::Main => "Rooms",
            Screen::ShuttingDown => "Closing\u{2026}",
            Screen::Disconnected => "The core stopped",
        };
        text(label).into()
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
    fn login_submit_sends_a_password_login_without_leaking_it() {
        let (mut app, mut rx) = app_with(8);
        let _ = rx.try_recv();
        let message = Message::LoginSubmitted {
            server: String::new(),
            username: "alice".into(),
            password: Password::from("hunter2-x"),
        };
        assert!(!format!("{message:?}").contains("hunter2-x"));
        let _ = app.update(message);
        match rx.try_recv() {
            Ok(Command::LoginPassword {
                username,
                password,
                server,
            }) => {
                assert_eq!(username, "alice");
                assert_eq!(password.expose(), "hunter2-x");
                assert_eq!(server, None);
            }
            other => panic!("unexpected {other:?}"),
        }
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
}
