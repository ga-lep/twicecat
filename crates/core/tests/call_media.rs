//! Call media end to end on the local stack (`docker compose up -d --wait
//! && scripts/create-test-users.sh`): alice's (virtual) microphone plays a
//! 1 kHz tone, and it must come out of carol's (virtual) speakers through
//! the LiveKit SFU, Opus and both voice pipelines. Muting alice stops it.
//!
//! Uses alice and carol; bob stays untouched.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_room, expect_event, raw_login, start_core_with_audio,
    unique_room_name, wait_for_room,
};
use serde_json::Value;
use tessera_core::CoreHandle;
use tessera_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, AudioProcessing, AudioSettings, CallIntent, CallState, Command,
    CoreEvent, InputMode, ListDiff, NoiseSuppression, Participant, Password, RoomId, SyncState,
    VirtualAudio,
};
use tessera_core::secrets::InMemorySecretStore;

const TONE_HZ: f32 = 1000.0;
const RATE: f32 = 48_000.0;

/// A microphone playing a steady tone.
#[derive(Default)]
struct ToneMic {
    n: AtomicU64,
}

impl VirtualAudio for ToneMic {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        let start = self.n.fetch_add(AUDIO_FRAME_LEN as u64, Ordering::Relaxed);
        for (i, s) in frame.iter_mut().enumerate() {
            let t = (start + i as u64) as f32 / RATE;
            *s = 0.3 * (2.0 * std::f32::consts::PI * TONE_HZ * t).sin();
        }
    }
    fn playback(&self, _: &[f32; AUDIO_FRAME_LEN]) {}
}

/// Silent microphone, records what it plays.
#[derive(Default)]
struct Recorder {
    played: Mutex<Vec<f32>>,
}

impl VirtualAudio for Recorder {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        frame.fill(0.0);
    }
    fn playback(&self, frame: &[f32; AUDIO_FRAME_LEN]) {
        self.played.lock().unwrap().extend_from_slice(frame);
    }
}

impl Recorder {
    /// Amplitude of the tone in the last `seconds` of playback.
    fn tone_level(&self, seconds: f32) -> f32 {
        let played = self.played.lock().unwrap();
        let n = ((seconds * RATE) as usize).min(played.len());
        let tail = &played[played.len() - n..];
        let w = 2.0 * std::f32::consts::PI * TONE_HZ / RATE;
        let (mut re, mut im) = (0.0_f32, 0.0_f32);
        for (i, s) in tail.iter().enumerate() {
            re += s * (w * i as f32).cos();
            im += s * (w * i as f32).sin();
        }
        2.0 * (re * re + im * im).sqrt() / n.max(1) as f32
    }
}

async fn logged_in(dir: &std::path::Path, n: u8, audio: AudioBackend) -> CoreHandle {
    let env = TestEnv::load();
    let mut core = start_core_with_audio(dir, Arc::new(InMemorySecretStore::new()), audio);
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
    core
}

async fn join(core: &mut CoreHandle, room_id: &str) {
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
        matches!(state, CoreEvent::CallState(CallState::Connected { .. })),
        "{state:?}"
    );
}

/// Waits until carol's view of alice satisfies `done`, applying every
/// participant update on the way.
async fn alice_as_seen_by(core: &mut CoreHandle, alice: &str, done: impl Fn(&Participant) -> bool) {
    let mut list: Vec<Participant> = Vec::new();
    let wait = async {
        loop {
            match core.events.recv().await {
                Some(CoreEvent::CallParticipants { diffs, .. }) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut list);
                    }
                    if list.iter().any(|p| p.user_id.as_str() == alice && done(p)) {
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
        .expect("timed out waiting for alice's participant state");
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

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn alices_tone_reaches_carols_speakers_and_mute_stops_it() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();

    let recorder = Arc::new(Recorder::default());
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(
        dir_a.path(),
        1,
        AudioBackend::Virtual(Arc::new(ToneMic::default())),
    )
    .await;
    let mut carol = logged_in(dir_c.path(), 3, AudioBackend::Virtual(recorder.clone())).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;

    // A pure tone is "noise" to a denoiser and steady to a gate: send it as is.
    alice
        .commands
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
    expect_event(&mut alice, |e| matches!(e, CoreEvent::AudioSettings(_))).await;

    join(&mut alice, &room_id).await;
    join(&mut carol, &room_id).await;

    let alice_id = env.user_id(1);
    alice_as_seen_by(&mut carol, &alice_id, |p| p.speaking).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let heard = recorder.tone_level(2.0);
    eprintln!("tone amplitude at carol's speakers: {heard:.4} (sent 0.3)");
    assert!(
        heard > 0.05,
        "tone at carol's speakers: amplitude {heard:.4}"
    );

    alice
        .commands
        .send(Command::SetMicMuted(true))
        .await
        .unwrap();
    alice_as_seen_by(&mut carol, &alice_id, |p| p.muted).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let after_mute = recorder.tone_level(1.0);
    eprintln!("after mute: {after_mute:.5}");
    assert!(
        after_mute < heard / 10.0,
        "muted: amplitude {after_mute:.4} (was {heard:.4})"
    );

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}
