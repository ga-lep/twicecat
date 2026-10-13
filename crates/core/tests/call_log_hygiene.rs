//! No call key reaches the logs: an encrypted call between alice and carol
//! on the local stack (`scripts/stack.sh up -d --wait &&
//! scripts/create-test-users.sh`) under a global capturing subscriber.
//! libwebrtc's frame cryptor writes the secret and the derived key into
//! libwebrtc's own log each time a key is set (webrtc-sdk/webrtc#308), and
//! that log reaches `tracing` at `debug`; the core's filter must drop it
//! even when `RUST_LOG` asks for everything at `debug`.
//!
//! One layer takes libwebrtc's log as it comes, to prove the capture sees
//! it; it is never printed. This test prints only counts.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_encrypted_room, expect_event, raw_login,
    start_core_with_audio, unique_room_name, wait_for_room,
};
use serde_json::Value;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, AudioProcessing, AudioSettings, CallIntent, CallState, Command,
    CoreEvent, InputMode, ListDiff, NoiseSuppression, Participant, Password, RoomId, SyncState,
    VirtualAudio,
};
use twicecat_core::logging::{DEFAULT_FILTER, filter};
use twicecat_core::secrets::InMemorySecretStore;

/// The target libwebrtc's log arrives under.
const LIBWEBRTC: &str = "libwebrtc";
/// Words of the frame cryptor's key messages.
const KEY_WORDS: [&str; 3] = ["derived_key", "raw_key ", "secret "];

/// An in-memory log sink.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    /// The captured lines from `target` (fmt writes `LEVEL target: message`),
    /// and how many of them hold a key word.
    fn count(&self, target: &str) -> (usize, usize) {
        let bytes = self.0.lock().expect("capture lock");
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains(&format!(" {target}:")))
            .collect();
        let keys = lines
            .iter()
            .filter(|l| KEY_WORDS.iter().any(|w| l.contains(w)))
            .count();
        (lines.len(), keys)
    }
}

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl MakeWriter<'_> for Capture {
    type Writer = Capture;

    fn make_writer(&self) -> Capture {
        self.clone()
    }
}

/// A microphone playing a steady tone, so the other side sees it speak.
#[derive(Default)]
struct ToneMic {
    n: AtomicU64,
}

impl VirtualAudio for ToneMic {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        let start = self.n.fetch_add(AUDIO_FRAME_LEN as u64, Ordering::Relaxed);
        for (i, s) in frame.iter_mut().enumerate() {
            let t = (start + i as u64) as f32 / 48_000.0;
            *s = 0.3 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
        }
    }
    fn playback(&self, _: &[f32; AUDIO_FRAME_LEN]) {}
}

/// Installs the global subscriber: libwebrtc's log as it comes, and the
/// core's filter as `RUST_LOG=debug` makes it.
fn install_capture() -> (Capture, Capture) {
    let (raw, ours) = (Capture::default(), Capture::default());
    let layer = |capture: &Capture, filter: EnvFilter| {
        tracing_subscriber::fmt::layer()
            .with_writer(capture.clone())
            .with_ansi(false)
            .with_filter(filter)
            .boxed()
    };
    let layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>,
    > = vec![
        layer(&raw, EnvFilter::new(format!("warn,{LIBWEBRTC}=trace"))),
        layer(&ours, filter(DEFAULT_FILTER, Some("debug")).unwrap()),
    ];
    tracing_subscriber::registry()
        .with(layers)
        .try_init()
        .expect("no other global subscriber in this test binary");
    (raw, ours)
}

async fn logged_in(dir: &std::path::Path, n: u8) -> CoreHandle {
    let env = TestEnv::load();
    let mut core = start_core_with_audio(
        dir,
        Arc::new(InMemorySecretStore::new()),
        AudioBackend::Virtual(Arc::new(ToneMic::default())),
    );
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    expect_event(&mut core, |e| {
        matches!(e, CoreEvent::SyncState(SyncState::Running))
    })
    .await;
    // A pure tone is "noise" to a denoiser and steady to a gate: send it as is.
    core.commands
        .send(Command::UpdateAudioSettings(AudioSettings {
            processing: AudioProcessing {
                noise_suppression: NoiseSuppression::Off,
                auto_gain: false,
                ..AudioProcessing::default()
            },
            input_mode: InputMode::VoiceActivity {
                automatic: false,
                threshold_db: -100.0,
            },
            ..AudioSettings::default()
        }))
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::AudioSettings(_))).await;
    core
}

/// Lets members at power level 0 publish call memberships (as Element does).
async fn allow_calls(admin: &matrix_sdk::Client, room_id: &str) {
    use matrix_sdk::ruma::api::client::state::{get_state_event_for_key, send_state_event};
    use matrix_sdk::ruma::events::StateEventType;
    let room: matrix_sdk::ruma::OwnedRoomId = room_id.try_into().unwrap();
    let reply = admin
        .send(get_state_event_for_key::v3::Request::new(
            room.clone(),
            StateEventType::RoomPowerLevels,
            String::new(),
        ))
        .await
        .unwrap();
    let mut levels: Value = serde_json::from_str(reply.event_or_content.get()).unwrap();
    levels["events"]["org.matrix.msc3401.call.member"] = serde_json::json!(0);
    admin
        .send(send_state_event::v3::Request::new_raw(
            room,
            StateEventType::RoomPowerLevels,
            String::new(),
            matrix_sdk::ruma::serde::Raw::new(&levels)
                .unwrap()
                .cast_unchecked(),
        ))
        .await
        .unwrap();
}

/// Joins and checks that the call is end-to-end encrypted.
async fn join_encrypted(core: &mut CoreHandle, room_id: &str) {
    core.commands
        .send(Command::JoinCall {
            room_id: RoomId::from(room_id),
            intent: CallIntent::Audio,
        })
        .await
        .unwrap();
    let state = expect_event(core, |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Connected { .. } | CallState::Left { .. })
        )
    })
    .await;
    assert!(
        matches!(
            state,
            CoreEvent::CallState(CallState::Connected {
                encrypted: true,
                ..
            })
        ),
        "{state:?}"
    );
}

/// Waits until `core` hears `user` speak: their key is set and works.
async fn hears(core: &mut CoreHandle, user: &str) {
    let mut list: Vec<Participant> = Vec::new();
    let wait = async {
        loop {
            match core.events.recv().await {
                Some(CoreEvent::CallParticipants { diffs, .. }) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut list);
                    }
                    if list
                        .iter()
                        .any(|p| p.user_id.as_str() == user && p.speaking)
                    {
                        return;
                    }
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for the other side to speak");
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn call_keys_never_reach_the_logs() {
    let (raw, ours) = install_capture();
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_encrypted_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();

    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1).await;
    let mut carol = logged_in(dir_c.path(), 3).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    join_encrypted(&mut alice, &room_id).await;
    join_encrypted(&mut carol, &room_id).await;
    hears(&mut alice, &env.user_id(3)).await;
    hears(&mut carol, &env.user_id(1)).await;
    // Carol leaves, so alice sets a new key, and comes back.
    carol.commands.send(Command::LeaveCall).await.unwrap();
    expect_event(&mut carol, |e| {
        matches!(e, CoreEvent::CallState(CallState::Left { .. }))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    join_encrypted(&mut carol, &room_id).await;
    hears(&mut carol, &env.user_id(1)).await;

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
    let (raw_lines, raw_keys) = raw.count(LIBWEBRTC);
    let (our_lines, our_keys) = ours.count(LIBWEBRTC);
    eprintln!(
        "libwebrtc log: {raw_lines} lines as it comes ({raw_keys} with key words); \
         {our_lines} through the core's filter at debug ({our_keys} with key words)"
    );
    assert!(raw_lines > 0, "the capture saw nothing from libwebrtc");
    assert_eq!(our_keys, 0, "a call key passed the core's filter");
    assert_eq!(our_lines, 0, "libwebrtc's log passed the core's filter");
}
