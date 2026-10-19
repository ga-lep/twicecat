//! Push-to-talk end to end on the real bridge and core, with virtual silent
//! audio and no homeserver (plan Task 3): a fake source's sink, the app's
//! forwarding thread, `Command::PushToTalk`, the core's gate and its release
//! tail, seen in the mic test's `InputLevel::transmitting`.
//!
//! Times are measured in audio, frames of 10 ms the virtual microphone gave:
//! the gate's tail is counted in frames, and on a busy CI runner the virtual
//! device falls behind the clock and skips the frames it missed (a release
//! tail of 1000 ms once took 3.2 s on macOS).
//!
//! The app is put on its main screen with made-up session events (the core
//! has no session); the audio commands work without one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::futures::StreamExt as _;
use iced::futures::stream::BoxStream;
use tokio::sync::mpsc;
use twicecat_app::audio_settings::AudioMessage;
use twicecat_app::bridge::{BridgeEvent, events_stream};
use twicecat_app::ptt::{PttHandle, PttStarter};
use twicecat_app::{App, Bridge, Message, Screen, Startup};
use twicecat_core::CoreConfig;
use twicecat_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, AudioSettings, Command, CoreEvent, DeviceId, InputMode,
    ListDiff, PushToTalkBindings, RoomId, RoomSummary, UserId, VirtualAudio,
};
use twicecat_core::paths::DataPaths;
use twicecat_core::secrets::InMemorySecretStore;
use twicecat_ptt::{KeyboardBackend, PttConfig, PttEvent, Sink};

/// Silent virtual devices: no real microphone is opened. Counts the frames
/// captured.
struct Silence(Arc<AtomicU64>);

impl VirtualAudio for Silence {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        frame.fill(0.0);
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn playback(&self, _: &[f32; AUDIO_FRAME_LEN]) {}
}

/// A service with no real source: the test calls its sink.
#[derive(Clone, Default)]
struct FakeSource(Arc<Mutex<Option<Sink>>>);

struct Nothing;

impl PttHandle for Nothing {
    fn window_key(&self, _pressed: bool) {}
    fn window_mouse(&self, _pressed: bool) {}
}

impl PttStarter for FakeSource {
    fn start(&self, _config: PttConfig, sink: Sink) -> Box<dyn PttHandle> {
        *self.0.lock().unwrap() = Some(sink);
        Box::new(Nothing)
    }
}

impl FakeSource {
    fn report(&self, event: PttEvent) {
        let sink = self.0.lock().unwrap().clone().expect("the service started");
        sink(event);
    }
}

struct Rig {
    app: App,
    source: FakeSource,
    commands: mpsc::Sender<Command>,
    pump: tokio::runtime::Runtime,
    events: BoxStream<'static, BridgeEvent>,
    dir: std::path::PathBuf,
    frames: Arc<AtomicU64>,
}

/// A level the core reported, and when the test saw it: on the clock and in
/// frames captured.
struct Level {
    at: Instant,
    frame: u64,
    transmitting: bool,
}

/// Audio time between two frame counts.
fn audio(from: u64, to: u64) -> Duration {
    Duration::from_millis(to.saturating_sub(from) * 10)
}

impl Rig {
    fn start() -> Self {
        let tag = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(format!("twicecat-app-ptt-test-{tag}"));
        let frames = Arc::new(AtomicU64::new(0));
        let (bridge, slot) = Bridge::start(
            CoreConfig {
                data: DataPaths::at(&dir),
                homeserver_override: None,
                room_list_page_size: twicecat_core::DEFAULT_ROOM_LIST_PAGE_SIZE,
                max_upload_bytes_override: None,
                audio: AudioBackend::Virtual(Arc::new(Silence(frames.clone()))),
                video: twicecat_core::api::VideoBackend::System,
                call_service_override: None,
            },
            Arc::new(InMemorySecretStore::new()),
        )
        .expect("start the bridge");
        let commands = bridge.command_sender();
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(bridge),
            events: slot.clone(),
        });
        let source = FakeSource::default();
        app.set_ptt_starter(Box::new(source.clone()), KeyboardBackend::WindowOnly);
        for event in [
            CoreEvent::NoSession,
            CoreEvent::LoggedIn {
                user_id: UserId::from("@a:x"),
                device_id: DeviceId::from("D"),
                restored: false,
            },
            CoreEvent::RoomListChanged(vec![ListDiff::Reset {
                values: vec![RoomSummary::new(RoomId::from("!a:x"), "a".into())],
            }]),
        ] {
            let _ = app.update(Message::Core(Box::new(event)));
        }
        assert_eq!(app.screen, Screen::Main);
        let pump = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("pump runtime");
        Self {
            app,
            source,
            commands,
            pump,
            events: events_stream(&slot).boxed(),
            dir,
            frames,
        }
    }

    /// Frames the virtual microphone has captured so far.
    fn frame(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// Sends a command to the core directly (as the forwarding thread does).
    fn send(&self, command: Command) {
        self.commands.blocking_send(command).expect("the core runs");
    }

    /// The next core event within `within`. Only the audio events are given
    /// to the app: the core has no session, the app is told it has one.
    fn next(&mut self, within: Duration) -> Option<CoreEvent> {
        let event = self
            .pump
            .block_on(async { tokio::time::timeout(within, self.events.next()).await });
        match event {
            Ok(Some(BridgeEvent::Core(event))) => {
                let event = *event;
                if matches!(
                    event,
                    CoreEvent::AudioSettings(_) | CoreEvent::InputLevel(_) | CoreEvent::Error(_)
                ) {
                    let _ = self.app.update(Message::Core(Box::new(event.clone())));
                }
                Some(event)
            }
            Ok(Some(BridgeEvent::Closed)) => panic!("the core stopped"),
            Ok(None) | Err(_) => None,
        }
    }

    /// Waits for the core's echo of `settings`.
    fn settle(&mut self, settings: AudioSettings) {
        self.send(Command::UpdateAudioSettings(settings.clone()));
        let limit = Instant::now() + Duration::from_secs(10);
        while Instant::now() < limit {
            if let Some(CoreEvent::AudioSettings(echo)) = self.next(Duration::from_secs(1))
                && echo == settings.clone().validated()
            {
                return;
            }
        }
        panic!("no settings echo");
    }

    /// The levels reported until `done` holds for one, at most `secs`.
    fn levels_until(&mut self, secs: u64, done: impl Fn(&Level) -> bool) -> Vec<Level> {
        let limit = Instant::now() + Duration::from_secs(secs);
        let mut seen = Vec::new();
        while Instant::now() < limit {
            match self.next(Duration::from_millis(500)) {
                Some(CoreEvent::InputLevel(level)) => {
                    let level = Level {
                        at: Instant::now(),
                        frame: self.frame(),
                        transmitting: level.transmitting,
                    };
                    let stop = done(&level);
                    seen.push(level);
                    if stop {
                        return seen;
                    }
                }
                Some(CoreEvent::Error(e)) => panic!("the core refused: {e:?}"),
                _ => {}
            }
        }
        panic!("the levels never got there; saw {} levels", seen.len());
    }

    fn close(mut self) {
        let _ = self.app.update(Message::Audio(AudioMessage::Close));
        let dir = self.dir.clone();
        drop(self);
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn push_to_talk(release_delay_ms: u32) -> AudioSettings {
    AudioSettings {
        input_mode: InputMode::PushToTalk {
            release_delay_ms,
            toggle: false,
        },
        push_to_talk: PushToTalkBindings {
            key: Some("F13".into()),
            mouse_button: None,
            gamepad_button: None,
        },
        ..AudioSettings::default()
    }
}

#[test]
fn a_release_keeps_transmitting_for_the_delay_then_stops() {
    let mut rig = Rig::start();
    rig.settle(push_to_talk(1000));
    let _ = rig.app.update(Message::Audio(AudioMessage::Open));
    let _ = rig.app.update(Message::Audio(AudioMessage::MicTest));
    assert!(rig.app.ptt.is_running(), "the card and the test need it");

    // Closed until pressed.
    rig.levels_until(10, |l| !l.transmitting);
    rig.source.report(PttEvent::Transmit(true));
    rig.levels_until(5, |l| l.transmitting);

    rig.source.report(PttEvent::Transmit(false));
    let (released, released_at) = (rig.frame(), Instant::now());
    let levels = rig.levels_until(30, |l| !l.transmitting);
    let last = levels.last().unwrap();
    let closed = audio(released, last.frame);
    let on_the_clock = last.at.duration_since(released_at);
    let last_open = levels
        .iter()
        .filter(|l| l.transmitting)
        .map(|l| audio(released, l.frame))
        .max()
        .unwrap_or_default();
    // A 1000 ms tail: still open well after the release, closed soon after
    // the tail (the levels come every 50 ms).
    assert!(
        last_open >= Duration::from_millis(500),
        "open for {last_open:?} of audio after the release"
    );
    assert!(
        closed >= Duration::from_millis(700) && closed <= Duration::from_secs(3),
        "closed {closed:?} of audio ({on_the_clock:?} on the clock) after the release"
    );
    rig.close();
}

#[test]
fn without_a_delay_a_release_stops_at_once_and_voice_activity_ignores_presses() {
    let mut rig = Rig::start();
    rig.settle(push_to_talk(0));
    let _ = rig.app.update(Message::Audio(AudioMessage::Open));
    let _ = rig.app.update(Message::Audio(AudioMessage::MicTest));
    rig.levels_until(10, |l| !l.transmitting);
    rig.source.report(PttEvent::Transmit(true));
    rig.levels_until(5, |l| l.transmitting);
    rig.source.report(PttEvent::Transmit(false));
    let released = rig.frame();
    let levels = rig.levels_until(15, |l| !l.transmitting);
    let closed = audio(released, levels.last().unwrap().frame);
    assert!(
        closed < Duration::from_millis(500),
        "closed after {closed:?} of audio"
    );

    // Voice activity, silent microphone: a press (the fake service keeps
    // forwarding) opens nothing.
    rig.settle(AudioSettings::default());
    rig.source.report(PttEvent::Transmit(true));
    let start = Instant::now();
    let levels = rig.levels_until(5, |l| {
        l.at.duration_since(start) > Duration::from_millis(500)
    });
    assert!(levels.iter().all(|l| !l.transmitting));
    rig.close();
}
