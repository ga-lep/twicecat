//! Push-to-talk in the app (plan docs/plans/phase6-push-to-talk.md, #15).
//!
//! The service (`twicecat-ptt`) runs while the input mode is push-to-talk
//! or toggle-to-talk and a call, the mic test or the Voice & audio card
//! needs it (plan Q1), so the key is not grabbed the rest of the time. It is
//! started, restarted and dropped from `update`, which Iced runs on the
//! main thread: global-hotkey needs that on macOS and Windows.
//!
//! Its events go to one forwarding thread, not through `update` (roadmap:
//! "Don't route the toggle through Iced's `update`"). The thread turns
//! `Transmit` into `Command::PushToTalk` (a press flips the state in
//! toggle-to-talk) and sends it to the core, waiting for room so a release
//! is never lost; then it tells the window, for the screen only.
//!
//! The key pressed in Twicecat's own window is a source too (gamescope and
//! compositors without the shortcuts portal have no other keyboard source).
//! Key presses are never logged, and bindings only by name.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};

use iced::Subscription;
use iced::futures::channel::mpsc as ui_mpsc;
use iced::futures::{Stream, StreamExt as _, stream};
use iced::keyboard::Modifiers;
use iced::keyboard::key::{Code, Physical};
use tokio::sync::mpsc;
use twicecat_core::api::{AudioSettings, Command, InputMode};
use twicecat_ptt::{
    GamepadButton, Held, KeyBinding, KeyboardBackend, PttConfig, PttEvent, PttService, Sink, Source,
};

use crate::state::Message;

/// A running service, as the app sees it.
pub trait PttHandle {
    /// The bound key was pressed or released in the window.
    fn window_key(&self, pressed: bool);
}

impl PttHandle for PttService {
    fn window_key(&self, pressed: bool) {
        PttService::window_key(self, pressed);
    }
}

/// Starts the service. Called from `update`, so on the main thread. Tests
/// use a fake.
pub trait PttStarter {
    /// Starts the sources of `config`; they report through `sink`.
    fn start(&self, config: PttConfig, sink: Sink) -> Box<dyn PttHandle>;
}

/// The real service; the portal runs on the core's runtime.
#[derive(Debug, Clone)]
pub struct ServiceStarter(pub tokio::runtime::Handle);

impl PttStarter for ServiceStarter {
    fn start(&self, config: PttConfig, sink: Sink) -> Box<dyn PttHandle> {
        Box::new(PttService::start(config, &self.0, sink))
    }
}

/// A saved binding the service cannot use. Its text is not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadBinding {
    /// The key.
    Key,
    /// The gamepad button.
    Button,
}

/// What the service should listen to, from the audio settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The sources.
    pub config: PttConfig,
    /// Toggle-to-talk: a press flips.
    pub toggle: bool,
    /// Saved bindings left out because they do not parse.
    pub bad: Vec<BadBinding>,
}

/// The setup for `settings`, or `None` when the input mode is voice
/// activity. A binding that does not parse is left out and named in `bad`.
pub fn setup_for(settings: &AudioSettings, keyboard: KeyboardBackend) -> Option<Setup> {
    let InputMode::PushToTalk { toggle, .. } = settings.input_mode else {
        return None;
    };
    let mut bad = Vec::new();
    let bindings = &settings.push_to_talk;
    let key = bindings.key.as_deref().and_then(|text| {
        let parsed = text.parse::<KeyBinding>().ok();
        if parsed.is_none() {
            bad.push(BadBinding::Key);
        }
        parsed
    });
    let gamepad = bindings.gamepad_button.as_deref().and_then(|text| {
        let parsed = text.parse::<GamepadButton>().ok();
        if parsed.is_none() {
            bad.push(BadBinding::Button);
        }
        parsed
    });
    Some(Setup {
        config: PttConfig {
            key,
            gamepad,
            keyboard,
        },
        toggle,
        bad,
    })
}

/// Holding or toggling: what the core is told for each change of "held".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Talk {
    toggle: bool,
    on: bool,
}

impl Talk {
    /// Not transmitting, holding or toggling.
    pub fn new(toggle: bool) -> Self {
        Self { toggle, on: false }
    }

    /// The sources' merged state changed to `held`; returns the new
    /// transmit state when it changes. Toggling flips on each press and
    /// ignores releases.
    pub fn held(&mut self, held: bool) -> Option<bool> {
        let next = match (self.toggle, held) {
            (false, held) => held,
            (true, true) => !self.on,
            (true, false) => return None,
        };
        (next != self.on).then(|| {
            self.on = next;
            next
        })
    }

    /// Lets go: `Some(false)` when it was transmitting.
    pub fn stop(&mut self) -> Option<bool> {
        self.on.then(|| {
            self.on = false;
            false
        })
    }
}

/// What the forwarding thread is told.
#[derive(Debug)]
enum Forward {
    /// A service of this generation starts; its events count from now.
    Start { generation: u64, toggle: bool },
    /// An event from a service's sink.
    Event { generation: u64, event: PttEvent },
    /// Let go: the audio session changed (`retire: false`), or the service
    /// stopped (`retire: true`), after which none of its events count, even
    /// one a portal task still delivers.
    Stop { retire: bool },
}

/// What the forwarding thread tells the window, for the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PttUpdate {
    /// Transmitting or not, as sent to the core.
    Talking { generation: u64, on: bool },
    /// A source listens; `trigger` is what to press (on Wayland, the key the
    /// portal confirmed).
    Ready {
        generation: u64,
        source: Source,
        trigger: String,
    },
    /// A source could not start; the others still work.
    Unavailable {
        generation: u64,
        source: Source,
        reason: String,
    },
}

/// The forwarding loop: runs until every sender is gone.
fn forward(
    events: std_mpsc::Receiver<Forward>,
    commands: mpsc::Sender<Command>,
    ui: ui_mpsc::UnboundedSender<PttUpdate>,
) {
    let mut current = 0;
    let mut talk = Talk::default();
    let tell_core = |on: bool| {
        // Waits for room: a dropped release would leave the microphone
        // open. An error means the core is gone.
        let _ = commands.blocking_send(Command::PushToTalk { pressed: on });
    };
    for message in events {
        match message {
            Forward::Start { generation, toggle } => {
                current = generation;
                talk = Talk::new(toggle);
            }
            Forward::Stop { retire } => {
                if retire {
                    // Generations start at 1: nothing matches 0.
                    current = 0;
                }
                if let Some(on) = talk.stop() {
                    tell_core(on);
                    let _ = ui.unbounded_send(PttUpdate::Talking {
                        generation: current,
                        on,
                    });
                }
            }
            // A stopped service's late events.
            Forward::Event { generation, .. } if generation != current => {}
            Forward::Event { generation, event } => {
                let update = match event {
                    PttEvent::Transmit(held) => match talk.held(held) {
                        Some(on) => {
                            tell_core(on);
                            PttUpdate::Talking { generation, on }
                        }
                        None => continue,
                    },
                    PttEvent::Ready { source, trigger } => PttUpdate::Ready {
                        generation,
                        source,
                        trigger,
                    },
                    PttEvent::Unavailable { source, reason } => PttUpdate::Unavailable {
                        generation,
                        source,
                        reason,
                    },
                };
                let _ = ui.unbounded_send(update);
            }
        }
    }
}

/// Holds the window's end of the forwarding thread's updates until the
/// subscription takes it. All clones share it and hash alike.
#[derive(Clone, Debug, Default)]
pub struct UpdatesSlot(Arc<Mutex<Option<ui_mpsc::UnboundedReceiver<PttUpdate>>>>);

impl UpdatesSlot {
    fn put(&self, receiver: ui_mpsc::UnboundedReceiver<PttUpdate>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(receiver);
    }

    fn take(&self) -> Option<ui_mpsc::UnboundedReceiver<PttUpdate>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Hash for UpdatesSlot {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "twicecat-ptt-updates".hash(state);
    }
}

fn updates_stream(slot: &UpdatesSlot) -> impl Stream<Item = PttUpdate> + use<> {
    match slot.take() {
        Some(receiver) => receiver.left_stream(),
        None => stream::empty().right_stream(),
    }
}

/// A key pressed or released in the window, while the service runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowKey {
    /// The physical key.
    pub code: Code,
    /// The modifiers held.
    pub modifiers: Modifiers,
    /// Pressed (`true`) or released.
    pub pressed: bool,
}

/// A push-to-talk message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PttMessage {
    /// From the forwarding thread.
    Update(PttUpdate),
    /// A key in the window.
    WindowKey(WindowKey),
}

/// The window key message for a keyboard event: presses (not repeats) and
/// releases of identified keys. Also when a widget used the key: a focused
/// composer still talks.
fn window_key_message(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    let (physical_key, modifiers, pressed) = match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            physical_key,
            modifiers,
            repeat: false,
            ..
        }) => (physical_key, modifiers, true),
        iced::Event::Keyboard(iced::keyboard::Event::KeyReleased {
            physical_key,
            modifiers,
            ..
        }) => (physical_key, modifiers, false),
        _ => return None,
    };
    let Physical::Code(code) = physical_key else {
        return None;
    };
    Some(Message::Ptt(PttMessage::WindowKey(WindowKey {
        code,
        modifiers,
        pressed,
    })))
}

/// The modifiers of a window key, as `twicecat-ptt` names them.
pub fn held(modifiers: Modifiers) -> Held {
    Held {
        ctrl: modifiers.control(),
        alt: modifiers.alt(),
        shift: modifiers.shift(),
        logo: modifiers.logo(),
    }
}

/// The W3C name of a physical key (`KeyT`), as `twicecat-ptt` reads it.
pub fn code_name(code: Code) -> String {
    format!("{code:?}")
}

/// What a source reports, for the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceState {
    /// Listening; press this.
    Ready(String),
    /// Not available, and why.
    Unavailable(String),
}

struct Running {
    generation: u64,
    setup: Setup,
    handle: Box<dyn PttHandle>,
}

/// The forwarding thread's inputs.
struct Forwarder {
    events: std_mpsc::Sender<Forward>,
}

/// The forwarding thread's channel to the window. Made with the model, so
/// the receiver is in its slot when Iced first builds the subscriptions
/// (once, at boot: a stream that ended then is never started again).
type UpdatesSender = ui_mpsc::UnboundedSender<PttUpdate>;

/// Starts the forwarding thread.
fn spawn_forwarder(commands: mpsc::Sender<Command>, ui: UpdatesSender) -> Forwarder {
    let (events, received) = std_mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("ptt-forward".into())
        .spawn(move || forward(received, commands, ui));
    if spawned.is_err() {
        eprintln!("twicecat: push-to-talk is not available (no thread)");
    }
    Forwarder { events }
}

/// Push-to-talk on `App`.
pub struct PttModel {
    starter: Option<Box<dyn PttStarter>>,
    keyboard: KeyboardBackend,
    forwarder: Option<Forwarder>,
    updates: UpdatesSlot,
    /// Given to the forwarding thread when it starts.
    updates_sender: Option<UpdatesSender>,
    running: Option<Running>,
    generation: u64,
    /// The audio session last seen (in a call, mic test running), so a new
    /// one starts with the microphone let go.
    session: (bool, bool),
    /// The window source is held.
    window_held: bool,
    /// What each source of the running service reported.
    pub sources: Vec<(Source, SourceState)>,
    /// Transmitting, as last sent to the core.
    pub talking: bool,
}

impl std::fmt::Debug for PttModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No bindings: they are the user's keys.
        f.debug_struct("PttModel")
            .field("keyboard", &self.keyboard)
            .field("running", &self.running.is_some())
            .field("generation", &self.generation)
            .field("sources", &self.sources.len())
            .field("talking", &self.talking)
            .finish_non_exhaustive()
    }
}

impl Default for PttModel {
    fn default() -> Self {
        Self::new(KeyboardBackend::WindowOnly)
    }
}

impl PttModel {
    /// A model with no starter (inert until one is set) for a session with
    /// `keyboard`.
    pub fn new(keyboard: KeyboardBackend) -> Self {
        let (sender, receiver) = ui_mpsc::unbounded();
        let updates = UpdatesSlot::default();
        updates.put(receiver);
        Self {
            starter: None,
            keyboard,
            forwarder: None,
            updates,
            updates_sender: Some(sender),
            running: None,
            generation: 0,
            session: (false, false),
            window_held: false,
            sources: Vec::new(),
            talking: false,
        }
    }

    /// Installs the starter (the real service at boot, a fake in tests)
    /// and the session's keyboard mechanism.
    pub fn set_starter(&mut self, starter: Box<dyn PttStarter>, keyboard: KeyboardBackend) {
        self.stop();
        self.starter = Some(starter);
        self.keyboard = keyboard;
    }

    /// Whether a service runs.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The running service's setup.
    pub fn setup(&self) -> Option<&Setup> {
        self.running.as_ref().map(|r| &r.setup)
    }

    /// The session's keyboard mechanism.
    pub fn keyboard(&self) -> KeyboardBackend {
        self.keyboard
    }

    /// Starts, restarts or stops the service so that it runs `wanted`.
    /// `session` is (in a call, mic test running); when it changes the
    /// microphone is let go, as the core's new audio session starts closed.
    /// `commands` is the core's channel, for the forwarding thread.
    pub fn sync(
        &mut self,
        wanted: Option<Setup>,
        session: (bool, bool),
        commands: impl FnOnce() -> Option<mpsc::Sender<Command>>,
    ) {
        if session != self.session {
            self.session = session;
            self.send(Forward::Stop { retire: false });
        }
        if self.running.as_ref().map(|r| &r.setup) == wanted.as_ref() {
            return;
        }
        self.stop();
        let Some(setup) = wanted else {
            return;
        };
        if self.starter.is_none() {
            return;
        }
        if self.forwarder.is_none() {
            let Some(commands) = commands() else {
                return;
            };
            let Some(ui) = self.updates_sender.take() else {
                return;
            };
            self.forwarder = Some(spawn_forwarder(commands, ui));
        }
        let Some(forwarder) = &self.forwarder else {
            return;
        };
        let Some(starter) = &self.starter else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let _ = forwarder.events.send(Forward::Start {
            generation,
            toggle: setup.toggle,
        });
        let events = forwarder.events.clone();
        // Only a channel send: the service calls this under its lock.
        let sink: Sink = Arc::new(move |event| {
            let _ = events.send(Forward::Event { generation, event });
        });
        let handle = starter.start(setup.config.clone(), sink);
        self.running = Some(Running {
            generation,
            setup,
            handle,
        });
    }

    fn send(&self, message: Forward) {
        if let Some(forwarder) = &self.forwarder {
            let _ = forwarder.events.send(message);
        }
    }

    /// Drops the service (on the calling thread, the main one) and lets go.
    pub fn stop(&mut self) {
        self.running = None;
        self.window_held = false;
        self.sources.clear();
        self.send(Forward::Stop { retire: true });
    }

    /// A key in the window: the bound key with its modifiers presses, its
    /// release (whatever the modifiers) lets go.
    pub fn window_key(&mut self, key: WindowKey) {
        let Some(running) = &self.running else {
            return;
        };
        let Some(binding) = running.setup.config.key else {
            return;
        };
        if !binding.is_key(&code_name(key.code)) {
            return;
        }
        if key.pressed && !binding.has_modifiers(held(key.modifiers)) {
            return;
        }
        if self.window_held != key.pressed {
            self.window_held = key.pressed;
            running.handle.window_key(key.pressed);
        }
    }

    /// The window lost focus: its key is let go (its release goes
    /// elsewhere).
    pub fn window_unfocused(&mut self) {
        if self.window_held {
            self.window_held = false;
            if let Some(running) = &self.running {
                running.handle.window_key(false);
            }
        }
    }

    /// An update from the forwarding thread; a stopped service's are
    /// dropped.
    pub fn update(&mut self, update: PttUpdate) {
        let current = self.running.as_ref().map(|r| r.generation);
        match update {
            PttUpdate::Talking { on, .. } => self.talking = on,
            PttUpdate::Ready {
                generation,
                source,
                trigger,
            } if Some(generation) == current => {
                self.set_source(source, SourceState::Ready(trigger));
            }
            PttUpdate::Unavailable {
                generation,
                source,
                reason,
            } if Some(generation) == current => {
                self.set_source(source, SourceState::Unavailable(reason));
            }
            PttUpdate::Ready { .. } | PttUpdate::Unavailable { .. } => {}
        }
    }

    fn set_source(&mut self, source: Source, state: SourceState) {
        match self.sources.iter_mut().find(|(s, _)| *s == source) {
            Some((_, now)) => *now = state,
            None => self.sources.push((source, state)),
        }
    }

    /// The subscriptions: the forwarding thread's updates always, and the
    /// window's keys while a service runs.
    pub fn subscription(&self) -> Subscription<Message> {
        let updates = Subscription::run_with(self.updates.clone(), updates_stream)
            .map(|update| Message::Ptt(PttMessage::Update(update)));
        if self.running.is_some() {
            Subscription::batch([updates, iced::event::listen_with(window_key_message)])
        } else {
            updates
        }
    }
}

#[cfg(test)]
mod tests {
    //! Push-to-talk through `App::update`, with a fake service (plan Task 2).

    use std::time::{Duration, Instant};

    use twicecat_core::api::{
        CallEndReason, CallState, CoreEvent, DeviceId, ListDiff, PushToTalkBindings, RoomId,
        RoomSummary, UserId,
    };

    use super::*;
    use crate::audio_settings::AudioMessage;
    use crate::bridge::EventsSlot;
    use crate::state::{App, Screen, Startup};

    /// What the fake service saw.
    #[derive(Default)]
    struct Seen {
        configs: Vec<PttConfig>,
        sinks: Vec<Sink>,
        window: Vec<bool>,
        dropped: usize,
    }

    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<Seen>>);

    struct FakeHandle {
        seen: Arc<Mutex<Seen>>,
        main_thread: std::thread::ThreadId,
    }

    impl PttHandle for FakeHandle {
        fn window_key(&self, pressed: bool) {
            self.seen.lock().unwrap().window.push(pressed);
        }
    }

    impl Drop for FakeHandle {
        fn drop(&mut self) {
            assert_eq!(std::thread::current().id(), self.main_thread);
            self.seen.lock().unwrap().dropped += 1;
        }
    }

    impl PttStarter for Fake {
        fn start(&self, config: PttConfig, sink: Sink) -> Box<dyn PttHandle> {
            let mut seen = self.0.lock().unwrap();
            seen.configs.push(config);
            seen.sinks.push(sink);
            Box::new(FakeHandle {
                seen: self.0.clone(),
                main_thread: std::thread::current().id(),
            })
        }
    }

    impl Fake {
        fn started(&self) -> usize {
            self.0.lock().unwrap().configs.len()
        }

        fn dropped(&self) -> usize {
            self.0.lock().unwrap().dropped
        }

        fn running(&self) -> usize {
            self.started() - self.dropped()
        }

        /// The latest service's sink reports `event`.
        fn report(&self, event: PttEvent) {
            let sink = self.0.lock().unwrap().sinks.last().unwrap().clone();
            sink(event);
        }

        fn window(&self) -> Vec<bool> {
            std::mem::take(&mut self.0.lock().unwrap().window)
        }
    }

    fn core(event: CoreEvent) -> Message {
        Message::Core(Box::new(event))
    }

    fn ptt(release_delay_ms: u32, toggle: bool) -> AudioSettings {
        AudioSettings {
            input_mode: InputMode::PushToTalk {
                release_delay_ms,
                toggle,
            },
            push_to_talk: PushToTalkBindings {
                key: Some("ctrl+shift+KeyT".into()),
                gamepad_button: Some("south".into()),
            },
            ..AudioSettings::default()
        }
    }

    struct Rig {
        app: App,
        fake: Fake,
        commands: mpsc::Receiver<Command>,
    }

    impl Rig {
        /// The main screen with `settings` echoed, a fake service on an
        /// X11-like session, nothing open.
        fn new(settings: AudioSettings) -> Self {
            let (tx, commands) = mpsc::channel(64);
            let (_events_tx, events_rx) = mpsc::channel(1);
            let (mut app, _boot) = App::new(Startup {
                sink: Box::new(tx),
                events: EventsSlot::new(events_rx),
            });
            let fake = Fake::default();
            app.set_ptt_starter(Box::new(fake.clone()), KeyboardBackend::Hotkey);
            let _ = app.update(core(CoreEvent::NoSession));
            let _ = app.update(core(CoreEvent::LoggedIn {
                user_id: UserId::from("@a:x"),
                device_id: DeviceId::from("D"),
                restored: false,
            }));
            let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
                values: vec![RoomSummary::new(RoomId::from("!a:x"), "a".into())],
            }])));
            assert_eq!(app.screen, Screen::Main);
            let _ = app.update(core(CoreEvent::AudioSettings(settings)));
            let mut rig = Self {
                app,
                fake,
                commands,
            };
            rig.drain();
            rig
        }

        fn update(&mut self, message: Message) {
            let _ = self.app.update(message);
        }

        fn join(&mut self) {
            self.update(core(CoreEvent::CallState(CallState::Joining {
                room_id: RoomId::from("!a:x"),
            })));
        }

        fn leave(&mut self) {
            self.update(core(CoreEvent::CallState(CallState::Left {
                room_id: RoomId::from("!a:x"),
                reason: CallEndReason::Hangup,
            })));
        }

        fn drain(&mut self) -> Vec<Command> {
            std::iter::from_fn(|| self.commands.try_recv().ok()).collect()
        }

        /// The next push-to-talk command, which the forwarding thread
        /// sends on its own: `update` is never called here.
        fn next_ptt(&mut self) -> bool {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match self.commands.try_recv() {
                    Ok(Command::PushToTalk { pressed }) => return pressed,
                    Ok(_) => {}
                    Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(e) => panic!("no push-to-talk command: {e}"),
                }
            }
        }

        /// Nothing more comes from the forwarding thread for a moment.
        fn quiet(&mut self) {
            std::thread::sleep(Duration::from_millis(50));
            let late: Vec<_> = self
                .drain()
                .into_iter()
                .filter(|c| matches!(c, Command::PushToTalk { .. }))
                .collect();
            assert!(late.is_empty(), "{late:?}");
        }

        fn key(&mut self, code: Code, modifiers: Modifiers, pressed: bool) {
            self.update(Message::Ptt(PttMessage::WindowKey(WindowKey {
                code,
                modifiers,
                pressed,
            })));
        }
    }

    const CTRL_SHIFT: Modifiers = Modifiers::CTRL.union(Modifiers::SHIFT);

    #[test]
    fn bindings_map_onto_the_service_config() {
        assert_eq!(
            setup_for(&AudioSettings::default(), KeyboardBackend::Hotkey),
            None,
            "voice activity"
        );
        let setup = setup_for(&ptt(200, false), KeyboardBackend::Portal).unwrap();
        assert_eq!(
            setup.config,
            PttConfig {
                key: Some("ctrl+shift+KeyT".parse().unwrap()),
                gamepad: Some("south".parse().unwrap()),
                keyboard: KeyboardBackend::Portal,
            }
        );
        assert!(!setup.toggle && setup.bad.is_empty());

        let mut none = ptt(200, true);
        none.push_to_talk = PushToTalkBindings::default();
        let setup = setup_for(&none, KeyboardBackend::WindowOnly).unwrap();
        assert!(setup.toggle);
        assert_eq!((setup.config.key, setup.config.gamepad), (None, None));
        assert!(setup.bad.is_empty(), "nothing bound is not an error");

        let mut bad = ptt(200, false);
        bad.push_to_talk.key = Some("ctrl+Banana".into());
        let setup = setup_for(&bad, KeyboardBackend::Hotkey).unwrap();
        assert_eq!(setup.config.key, None);
        assert!(setup.config.gamepad.is_some(), "the button still works");
        assert_eq!(setup.bad, [BadBinding::Key]);
        bad.push_to_talk.gamepad_button = Some("paddle9".into());
        assert_eq!(
            setup_for(&bad, KeyboardBackend::Hotkey).unwrap().bad,
            [BadBinding::Key, BadBinding::Button]
        );
    }

    #[test]
    fn holding_and_toggling() {
        let mut hold = Talk::new(false);
        assert_eq!(hold.held(true), Some(true));
        assert_eq!(hold.held(true), None);
        assert_eq!(hold.held(false), Some(false));
        assert_eq!(hold.stop(), None);
        assert_eq!(hold.held(true), Some(true));
        assert_eq!(hold.stop(), Some(false));

        let mut toggle = Talk::new(true);
        assert_eq!(toggle.held(true), Some(true));
        assert_eq!(toggle.held(false), None, "a release keeps talking");
        assert_eq!(toggle.held(true), Some(false));
        assert_eq!(toggle.held(false), None);
        assert_eq!(toggle.held(true), Some(true));
        assert_eq!(toggle.stop(), Some(false));
        assert_eq!(toggle.stop(), None);
    }

    #[test]
    fn the_service_runs_only_while_a_call_the_test_or_the_card_needs_it() {
        let mut rig = Rig::new(ptt(200, false));
        rig.app.ui_settings.advanced_settings = true;
        assert_eq!(rig.fake.started(), 0, "push-to-talk alone grabs nothing");

        rig.update(Message::Audio(AudioMessage::Open));
        assert_eq!(rig.fake.running(), 1, "the card shows the sources");
        rig.update(Message::Audio(AudioMessage::Close));
        assert_eq!(rig.fake.running(), 0);

        rig.join();
        assert_eq!(rig.fake.running(), 1, "in a call");
        rig.update(Message::Audio(AudioMessage::Open));
        rig.update(Message::Audio(AudioMessage::Close));
        assert_eq!(rig.fake.started(), 2, "the card changes nothing in a call");
        rig.leave();
        assert_eq!(rig.fake.running(), 0);

        rig.update(Message::Audio(AudioMessage::Open));
        rig.update(Message::Audio(AudioMessage::MicTest));
        rig.update(Message::Audio(AudioMessage::Close));
        assert_eq!(rig.fake.running(), 0, "closing the card stops the test");

        // Voice activity never starts it.
        rig.update(core(CoreEvent::AudioSettings(AudioSettings::default())));
        rig.join();
        rig.update(Message::Audio(AudioMessage::Open));
        assert_eq!(rig.fake.running(), 0);

        // Push-to-talk again during the call: it starts, logging out stops it.
        rig.update(core(CoreEvent::AudioSettings(ptt(200, false))));
        assert_eq!(rig.fake.running(), 1);
        rig.update(core(CoreEvent::LoggedOut));
        assert_eq!(rig.fake.running(), 0);
    }

    #[test]
    fn a_changed_binding_restarts_the_service_once() {
        let mut rig = Rig::new(ptt(200, false));
        rig.join();
        assert_eq!(rig.fake.started(), 1);
        rig.update(core(CoreEvent::AudioSettings(ptt(500, false))));
        assert_eq!(rig.fake.started(), 1, "the delay is the core's");
        let mut other = ptt(500, false);
        other.push_to_talk.key = Some("F13".into());
        rig.update(core(CoreEvent::AudioSettings(other.clone())));
        rig.update(core(CoreEvent::AudioSettings(other)));
        assert_eq!((rig.fake.started(), rig.fake.running()), (2, 1));
        let configs = rig.fake.0.lock().unwrap().configs.clone();
        assert_eq!(configs[1].key, Some("F13".parse().unwrap()));
        assert_eq!(configs[1].keyboard, KeyboardBackend::Hotkey);
        rig.update(core(CoreEvent::AudioSettings(ptt(500, true))));
        assert_eq!(rig.fake.started(), 3, "toggling is a new service");
    }

    #[test]
    fn transmit_reaches_the_core_without_update() {
        let mut rig = Rig::new(ptt(200, false));
        rig.join();
        rig.fake.report(PttEvent::Transmit(true));
        assert!(rig.next_ptt());
        rig.fake.report(PttEvent::Transmit(false));
        assert!(!rig.next_ptt());
        rig.fake.report(PttEvent::Transmit(true));
        assert!(rig.next_ptt());
        // Leaving the call drops the service; the core is told to stop.
        rig.leave();
        assert!(!rig.next_ptt());
        rig.quiet();
    }

    #[test]
    fn toggle_to_talk_flips_on_each_press() {
        let mut rig = Rig::new(ptt(200, true));
        rig.join();
        rig.fake.report(PttEvent::Transmit(true));
        assert!(rig.next_ptt());
        rig.fake.report(PttEvent::Transmit(false));
        rig.quiet();
        rig.fake.report(PttEvent::Transmit(true));
        assert!(!rig.next_ptt(), "the second press stops");
        rig.fake.report(PttEvent::Transmit(false));
        rig.fake.report(PttEvent::Transmit(true));
        assert!(rig.next_ptt());
        // The service lets go when it stops (here, the call ends).
        rig.leave();
        assert!(!rig.next_ptt());
        rig.quiet();
    }

    #[test]
    fn a_new_audio_session_starts_let_go() {
        let mut rig = Rig::new(ptt(200, true));
        rig.update(Message::Audio(AudioMessage::Open));
        rig.update(Message::Audio(AudioMessage::MicTest));
        rig.fake.report(PttEvent::Transmit(true));
        assert!(rig.next_ptt());
        // The call takes over from the mic test: a new session, still the
        // same service (the card stays open).
        rig.join();
        assert_eq!(rig.fake.started(), 1);
        assert!(!rig.next_ptt(), "toggled off with the old session");
        rig.fake.report(PttEvent::Transmit(false));
        rig.fake.report(PttEvent::Transmit(true));
        assert!(rig.next_ptt(), "a press starts the new session");
    }

    #[test]
    fn the_bound_key_in_the_window_is_a_source() {
        let mut rig = Rig::new(ptt(200, false));
        rig.key(Code::KeyT, CTRL_SHIFT, true);
        assert!(rig.fake.window().is_empty(), "no service, no key");
        rig.join();
        rig.key(Code::KeyT, Modifiers::CTRL, true);
        rig.key(Code::KeyY, CTRL_SHIFT, true);
        rig.key(Code::ControlLeft, CTRL_SHIFT, true);
        assert!(rig.fake.window().is_empty(), "not the binding");
        rig.key(Code::KeyT, CTRL_SHIFT, true);
        rig.key(Code::KeyT, CTRL_SHIFT, true);
        // Ctrl let go first: the release still counts.
        rig.key(Code::KeyT, Modifiers::SHIFT, false);
        rig.key(Code::KeyT, Modifiers::empty(), false);
        assert_eq!(rig.fake.window(), [true, false]);
        rig.key(Code::KeyT, CTRL_SHIFT, true);
        rig.update(Message::WindowFocus(false));
        rig.update(Message::WindowFocus(false));
        rig.key(Code::KeyT, CTRL_SHIFT, false);
        assert_eq!(rig.fake.window(), [true, false], "losing focus lets go");

        let mut no_key = ptt(200, false);
        no_key.push_to_talk.key = None;
        rig.update(core(CoreEvent::AudioSettings(no_key)));
        rig.key(Code::KeyT, CTRL_SHIFT, true);
        assert!(rig.fake.window().is_empty(), "no key bound");
    }

    #[test]
    fn window_key_events_become_messages_only_for_real_keys() {
        use iced::keyboard::{Event, Key, Location};
        let id = iced::window::Id::unique();
        let status = iced::event::Status::Captured;
        let press = |physical_key, repeat| {
            iced::Event::Keyboard(Event::KeyPressed {
                key: Key::Unidentified,
                modified_key: Key::Unidentified,
                physical_key,
                location: Location::Standard,
                modifiers: Modifiers::CTRL,
                text: None,
                repeat,
            })
        };
        let t = Physical::Code(Code::KeyT);
        assert!(
            matches!(
                window_key_message(press(t, false), status, id),
                Some(Message::Ptt(PttMessage::WindowKey(WindowKey {
                    code: Code::KeyT,
                    pressed: true,
                    ..
                })))
            ),
            "captured by a widget still counts"
        );
        assert!(window_key_message(press(t, true), status, id).is_none());
        let unknown = Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified);
        assert!(window_key_message(press(unknown, false), status, id).is_none());
        let release = iced::Event::Keyboard(Event::KeyReleased {
            key: Key::Unidentified,
            modified_key: Key::Unidentified,
            physical_key: t,
            location: Location::Standard,
            modifiers: Modifiers::empty(),
        });
        assert!(matches!(
            window_key_message(release, status, id),
            Some(Message::Ptt(PttMessage::WindowKey(WindowKey {
                pressed: false,
                ..
            })))
        ));
    }

    /// The window's updates, as the subscription would deliver them.
    fn deliver(rig: &mut Rig, expected: usize) {
        let mut receiver = rig.app.ptt.updates.take().expect("the receiver is there");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.len() < expected {
            match receiver.try_recv() {
                Ok(update) => got.push(update),
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => panic!("{} updates, wanted {expected}", got.len()),
            }
        }
        rig.app.ptt.updates.put(receiver);
        for update in got {
            rig.update(Message::Ptt(PttMessage::Update(update)));
        }
    }

    #[test]
    fn each_source_reports_on_its_own() {
        let mut rig = Rig::new(ptt(200, false));
        rig.join();
        rig.fake.report(PttEvent::Ready {
            source: Source::Portal,
            trigger: "Ctrl+Shift+T".into(),
        });
        rig.fake.report(PttEvent::Unavailable {
            source: Source::Gamepad,
            reason: "no gamepad support".into(),
        });
        rig.fake.report(PttEvent::Transmit(true));
        deliver(&mut rig, 3);
        assert_eq!(
            rig.app.ptt.sources,
            [
                (Source::Portal, SourceState::Ready("Ctrl+Shift+T".into())),
                (
                    Source::Gamepad,
                    SourceState::Unavailable("no gamepad support".into())
                ),
            ]
        );
        assert!(rig.app.ptt.talking);

        // A restart forgets them; the old service's late reports are dropped.
        let stale = rig.fake.0.lock().unwrap().sinks[0].clone();
        let mut other = ptt(200, false);
        other.push_to_talk.key = Some("F13".into());
        rig.update(core(CoreEvent::AudioSettings(other)));
        assert!(rig.app.ptt.sources.is_empty());
        stale(PttEvent::Ready {
            source: Source::Hotkey,
            trigger: "old".into(),
        });
        rig.fake.report(PttEvent::Ready {
            source: Source::Hotkey,
            trigger: "F13".into(),
        });
        // The restart let go (one Talking update), then the new source.
        deliver(&mut rig, 2);
        assert_eq!(
            rig.app.ptt.sources,
            [(Source::Hotkey, SourceState::Ready("F13".into()))]
        );
        assert!(!rig.app.ptt.talking);
    }

    /// Iced builds the subscriptions once at boot, before any service: the
    /// stream it takes then must carry the updates of a service started
    /// later.
    #[test]
    fn the_stream_taken_at_boot_carries_later_updates() {
        let mut rig = Rig::new(ptt(200, false));
        let mut stream = updates_stream(&rig.app.ptt.updates);
        rig.join();
        rig.fake.report(PttEvent::Ready {
            source: Source::Hotkey,
            trigger: "F13".into(),
        });
        let first = iced::futures::executor::block_on(stream.next());
        assert!(matches!(first, Some(PttUpdate::Ready { .. })), "{first:?}");
    }

    /// A late event from a stopped service (a portal task outlives it) is
    /// not forwarded, also when no new service starts.
    #[test]
    fn a_stopped_service_cannot_start_talking() {
        let mut rig = Rig::new(ptt(200, true));
        rig.join();
        let stale = rig.fake.0.lock().unwrap().sinks[0].clone();
        rig.leave();
        stale(PttEvent::Transmit(true));
        rig.quiet();
    }

    #[test]
    fn the_subscription_listens_to_keys_only_while_the_service_runs() {
        // Iced subscriptions are opaque; their count is what can be seen.
        use iced::advanced::subscription::into_recipes;
        let mut rig = Rig::new(ptt(200, false));
        let idle = into_recipes(rig.app.ptt.subscription()).len();
        rig.join();
        let running = into_recipes(rig.app.ptt.subscription()).len();
        assert_eq!(running, idle + 1);
    }

    #[test]
    fn the_forwarding_thread_ends_with_the_app() {
        let Rig { mut app, fake, .. } = Rig::new(ptt(200, false));
        let _ = app.update(core(CoreEvent::CallState(CallState::Joining {
            room_id: RoomId::from("!a:x"),
        })));
        let mut updates = app.ptt.updates.take().expect("the receiver is there");
        // The fake keeps the sinks, as a portal task may for a while.
        drop(app);
        drop(fake);
        // The thread's end closes its channel to the window.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match updates.try_recv() {
                Err(ui_mpsc::TryRecvError::Closed) => break,
                _ if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                other => panic!("the thread still runs: {other:?}"),
            }
        }
    }
}
