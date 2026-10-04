//! Call media end to end on the local stack (`docker compose up -d --wait
//! && scripts/create-test-users.sh`): alice's (virtual) microphone plays a
//! 1 kHz tone, and it must come out of carol's (virtual) speakers through
//! the LiveKit SFU, Opus and both voice pipelines. Muting alice stops it.
//! Multi-SFU: with carol publishing on the second call service, each still
//! hears the other.
//!
//! Uses alice and carol; bob stays untouched.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_encrypted_room, create_room, expect_event, raw_login,
    start_core_with_audio, start_core_with_call_service, unique_room_name, wait_for_room,
};
use serde_json::Value;
use tessera_core::CoreHandle;
use tessera_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, AudioProcessing, AudioSettings, CallIntent, CallState,
    CleaningStatus, Command, CoreEvent, InputMode, ListDiff, NoiseSuppression, Participant,
    Password, RoomId, SkipReason, SyncState, UserId, VirtualAudio, VoiceCleaning,
};
use tessera_core::secrets::InMemorySecretStore;

const TONE_HZ: f32 = 1000.0;
const RATE: f32 = 48_000.0;

/// A tone sent at 0.3 is heard at 0.23 to 0.29 (through Opus and the
/// voice chain) when nothing is lost. The level is measured coherently over
/// its window, so gaps and phase jumps pull it down: this bar catches
/// dropouts (call audio starved on a busy runtime gave 0.03 to 0.27, mostly
/// under 0.15), not only silence.
const HEARD: f32 = 0.15;

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

/// Amplitude of a `hz` tone in the last `seconds` of `played`.
fn level_of(played: &[f32], hz: f32, seconds: f32) -> f32 {
    let n = ((seconds * RATE) as usize).min(played.len());
    let tail = &played[played.len() - n..];
    let w = 2.0 * std::f32::consts::PI * hz / RATE;
    let (mut re, mut im) = (0.0_f32, 0.0_f32);
    for (i, s) in tail.iter().enumerate() {
        re += s * (w * i as f32).cos();
        im += s * (w * i as f32).sin();
    }
    2.0 * (re * re + im * im).sqrt() / n.max(1) as f32
}

impl Recorder {
    /// Amplitude of the tone in the last `seconds` of playback.
    fn tone_level(&self, seconds: f32) -> f32 {
        level_of(&self.played.lock().unwrap(), TONE_HZ, seconds)
    }
}

/// Plays a `hz` tone into the microphone and records the speakers.
struct Duplex {
    hz: f32,
    n: AtomicU64,
    played: Mutex<Vec<f32>>,
}

impl Duplex {
    fn new(hz: f32) -> Self {
        Self {
            hz,
            n: AtomicU64::new(0),
            played: Mutex::default(),
        }
    }

    fn level(&self, hz: f32, seconds: f32) -> f32 {
        level_of(&self.played.lock().unwrap(), hz, seconds)
    }
}

impl VirtualAudio for Duplex {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        let start = self.n.fetch_add(AUDIO_FRAME_LEN as u64, Ordering::Relaxed);
        for (i, s) in frame.iter_mut().enumerate() {
            let t = (start + i as u64) as f32 / RATE;
            *s = 0.3 * (2.0 * std::f32::consts::PI * self.hz * t).sin();
        }
    }
    fn playback(&self, frame: &[f32; AUDIO_FRAME_LEN]) {
        self.played.lock().unwrap().extend_from_slice(frame);
    }
}

/// Settings that send a pure tone as is: a denoiser takes it for noise,
/// a gain control and a gate would shape it.
fn tone_settings() -> AudioSettings {
    AudioSettings {
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
    }
}

async fn logged_in(dir: &std::path::Path, n: u8, audio: AudioBackend) -> CoreHandle {
    login(
        start_core_with_audio(dir, Arc::new(InMemorySecretStore::new()), audio),
        n,
    )
    .await
}

async fn login(mut core: CoreHandle, n: u8) -> CoreHandle {
    let env = TestEnv::load();
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
    join_expecting(core, room_id, false).await;
}

/// Joins and checks that the call is (not) end-to-end encrypted.
async fn join_expecting(core: &mut CoreHandle, room_id: &str, encrypted: bool) {
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
        matches!(state, CoreEvent::CallState(CallState::Connected { encrypted: e, .. }) if e == encrypted),
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

/// The call service `user`'s membership publishes on (`foci_preferred[0]`).
async fn call_service_of(reader: &matrix_sdk::Client, room_id: &str, user: &str) -> Option<String> {
    use matrix_sdk::ruma::api::client::state::get_state_events;
    let reply = reader
        .send(get_state_events::v3::Request::new(
            room_id.try_into().unwrap(),
        ))
        .await
        .ok()?;
    reply.room_state.iter().find_map(|raw| {
        let event: Value = raw.deserialize_as().ok()?;
        (event["type"] == "org.matrix.msc3401.call.member" && event["sender"] == user)
            .then(|| {
                event["content"]["foci_preferred"][0]["livekit_service_url"]
                    .as_str()
                    .map(str::to_owned)
            })
            .flatten()
    })
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
        heard > HEARD,
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

/// Carol publishes on the second call service (another homeserver's, in
/// Element Call's multi-SFU mode), alice on the first. Each subscribes on
/// the other's SFU: alice hears carol's 600 Hz tone, carol alice's 1 kHz.
#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn members_on_different_sfus_hear_each_other() {
    const ALICE_HZ: f32 = 1000.0;
    const CAROL_HZ: f32 = 600.0;
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

    let (alice_audio, carol_audio) = (
        Arc::new(Duplex::new(ALICE_HZ)),
        Arc::new(Duplex::new(CAROL_HZ)),
    );
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1, AudioBackend::Virtual(alice_audio.clone())).await;
    let service_b = env.get("LK_JWT_URL_B").to_owned();
    let mut carol = login(
        start_core_with_call_service(
            dir_c.path(),
            Arc::new(InMemorySecretStore::new()),
            AudioBackend::Virtual(carol_audio.clone()),
            Some(service_b.clone()),
        ),
        3,
    )
    .await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    for core in [&mut alice, &mut carol] {
        core.commands
            .send(Command::UpdateAudioSettings(tone_settings()))
            .await
            .unwrap();
        expect_event(core, |e| matches!(e, CoreEvent::AudioSettings(_))).await;
    }

    join(&mut alice, &room_id).await;
    join(&mut carol, &room_id).await;

    // Carol advertises the second service, as a member of another homeserver would.
    assert_eq!(
        call_service_of(&alice_raw, &room_id, &env.user_id(3))
            .await
            .as_deref(),
        Some(service_b.as_str())
    );
    assert_eq!(
        call_service_of(&alice_raw, &room_id, &env.user_id(1))
            .await
            .as_deref(),
        Some(env.get("LK_JWT_URL"))
    );

    alice_as_seen_by(&mut carol, &env.user_id(1), |p| p.speaking).await;
    alice_as_seen_by(&mut alice, &env.user_id(3), |p| p.speaking).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let alice_hears = alice_audio.level(CAROL_HZ, 2.0);
    let carol_hears = carol_audio.level(ALICE_HZ, 2.0);
    eprintln!(
        "alice hears carol (via SFU B): {alice_hears:.4}; carol hears alice (via SFU A): {carol_hears:.4} (sent 0.3)"
    );
    assert!(
        alice_hears > HEARD,
        "alice hears carol: amplitude {alice_hears:.4}"
    );
    assert!(
        carol_hears > HEARD,
        "carol hears alice: amplitude {carol_hears:.4}"
    );

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// Joins the call's LiveKit room as `client`'s user with a made-up device
/// (a token from the call service, no call membership) and publishes a
/// `hz` tone: what anyone with a Matrix account could do.
async fn intrude(client: &matrix_sdk::Client, room_id: &str, hz: f32) -> livekit::prelude::Room {
    use livekit::options::TrackPublishOptions;
    use livekit::prelude::{LocalAudioTrack, LocalTrack, Room, RoomOptions, TrackSource};
    use livekit::webrtc::audio_frame::AudioFrame;
    use livekit::webrtc::audio_source::native::NativeAudioSource;
    use livekit::webrtc::audio_source::{AudioSourceOptions, RtcAudioSource};
    use matrix_sdk::ruma::api::client::account::request_openid_token;

    let env = TestEnv::load();
    let openid = client
        .send(request_openid_token::v3::Request::new(
            client.user_id().unwrap().to_owned(),
        ))
        .await
        .unwrap();
    let body = serde_json::json!({
        "room": room_id,
        "openid_token": {
            "access_token": openid.access_token,
            "token_type": openid.token_type,
            "matrix_server_name": openid.matrix_server_name,
            "expires_in": openid.expires_in.as_secs(),
        },
        "device_id": "INTRUDER",
    });
    let reply = matrix_sdk::reqwest::Client::new()
        .post(format!("{}/sfu/get", env.get("LK_JWT_URL")))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let (room, _events) = Room::connect(
        reply["url"].as_str().unwrap(),
        reply["jwt"].as_str().unwrap(),
        RoomOptions::default(),
    )
    .await
    .expect("intruder connects");
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 50);
    let track = LocalAudioTrack::create_audio_track("tone", RtcAudioSource::Native(source.clone()));
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..TrackPublishOptions::default()
            },
        )
        .await
        .expect("intruder publishes");
    tokio::spawn(async move {
        let mut n = 0u64;
        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        loop {
            ticker.tick().await;
            let data: Vec<i16> = (0..480)
                .map(|i| {
                    let t = (n + i) as f32 / RATE;
                    (0.3 * (2.0 * std::f32::consts::PI * hz * t).sin() * f32::from(i16::MAX)) as i16
                })
                .collect();
            n += 480;
            let frame = AudioFrame {
                data: data.into(),
                sample_rate: 48_000,
                num_channels: 1,
                samples_per_channel: 480,
            };
            if source.capture_frame(&frame).await.is_err() {
                return;
            }
        }
    });
    room
}

/// The call services give a LiveKit token to any Matrix user, so a
/// participant on the SFU without a call membership is not played; a real
/// member next to it is.
#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_participant_without_a_membership_is_not_played() {
    const CAROL_HZ: f32 = 600.0;
    const INTRUDER_HZ: f32 = 800.0;
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

    let alice_audio = Arc::new(Duplex::new(TONE_HZ));
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1, AudioBackend::Virtual(alice_audio.clone())).await;
    let mut carol = logged_in(
        dir_c.path(),
        3,
        AudioBackend::Virtual(Arc::new(Duplex::new(CAROL_HZ))),
    )
    .await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    carol
        .commands
        .send(Command::UpdateAudioSettings(tone_settings()))
        .await
        .unwrap();
    expect_event(&mut carol, |e| matches!(e, CoreEvent::AudioSettings(_))).await;
    join(&mut alice, &room_id).await;
    join(&mut carol, &room_id).await;
    let intruder = intrude(&carol_raw, &room_id, INTRUDER_HZ).await;

    alice_as_seen_by(&mut alice, &env.user_id(3), |p| p.speaking).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let member = alice_audio.level(CAROL_HZ, 3.0);
    let stranger = alice_audio.level(INTRUDER_HZ, 3.0);
    eprintln!("alice hears carol's device {member:.4}, the intruder {stranger:.5} (each sent 0.3)");
    assert!(member > HEARD, "member: amplitude {member:.4}");
    assert!(stranger < 0.01, "intruder: amplitude {stranger:.4}");

    let _ = intruder.close().await;
    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// Joins the call's LiveKit room as `client`'s user with a made-up device,
/// without any E2EE keys, and records what alice's track decodes to.
async fn eavesdrop(
    client: &matrix_sdk::Client,
    room_id: &str,
    alice: &str,
) -> (
    livekit::prelude::Room,
    Arc<Mutex<Vec<f32>>>,
    Arc<Mutex<Option<String>>>,
) {
    use futures_util::StreamExt as _;
    use livekit::prelude::{RemoteTrack, Room, RoomEvent, RoomOptions};
    use livekit::webrtc::audio_stream::native::NativeAudioStream;
    use matrix_sdk::ruma::api::client::account::request_openid_token;

    let env = TestEnv::load();
    let openid = client
        .send(request_openid_token::v3::Request::new(
            client.user_id().unwrap().to_owned(),
        ))
        .await
        .unwrap();
    let body = serde_json::json!({
        "room": room_id,
        "openid_token": {
            "access_token": openid.access_token,
            "token_type": openid.token_type,
            "matrix_server_name": openid.matrix_server_name,
            "expires_in": openid.expires_in.as_secs(),
        },
        "device_id": "LISTENER",
    });
    let reply = matrix_sdk::reqwest::Client::new()
        .post(format!("{}/sfu/get", env.get("LK_JWT_URL")))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let (room, mut events) = Room::connect(
        reply["url"].as_str().unwrap(),
        reply["jwt"].as_str().unwrap(),
        RoomOptions::default(),
    )
    .await
    .expect("eavesdropper connects");
    let heard = Arc::new(Mutex::new(Vec::new()));
    let encryption = Arc::new(Mutex::new(None));
    let (heard_tx, encryption_tx, alice) = (heard.clone(), encryption.clone(), alice.to_owned());
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let RoomEvent::TrackSubscribed {
                track: RemoteTrack::Audio(track),
                publication,
                participant,
            } = event
            {
                if !participant.identity().as_str().starts_with(&alice) {
                    continue;
                }
                *encryption_tx.lock().unwrap() =
                    Some(format!("{:?}", publication.encryption_type()));
                let mut stream = NativeAudioStream::new(track.rtc_track(), 48_000, 1);
                let heard = heard_tx.clone();
                tokio::spawn(async move {
                    while let Some(frame) = stream.next().await {
                        heard
                            .lock()
                            .unwrap()
                            .extend(frame.data.iter().map(|s| f32::from(*s) / 32768.0));
                    }
                });
            }
        }
    });
    (room, heard, encryption)
}

/// In an encrypted room, media is end-to-end encrypted as Element Call
/// does it: alice and carol hear each other with keys sent over Olm, a
/// listener on the SFU with a valid token but no keys hears nothing usable,
/// and after carol leaves (alice rotates her key) and rejoins, both hear
/// each other again.
#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn encrypted_room_calls_are_end_to_end_encrypted() {
    const ALICE_HZ: f32 = 1000.0;
    const CAROL_HZ: f32 = 600.0;
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

    let (alice_audio, carol_audio) = (
        Arc::new(Duplex::new(ALICE_HZ)),
        Arc::new(Duplex::new(CAROL_HZ)),
    );
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1, AudioBackend::Virtual(alice_audio.clone())).await;
    let mut carol = logged_in(dir_c.path(), 3, AudioBackend::Virtual(carol_audio.clone())).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    for core in [&mut alice, &mut carol] {
        core.commands
            .send(Command::UpdateAudioSettings(tone_settings()))
            .await
            .unwrap();
        expect_event(core, |e| matches!(e, CoreEvent::AudioSettings(_))).await;
    }

    join_expecting(&mut alice, &room_id, true).await;
    join_expecting(&mut carol, &room_id, true).await;
    let (listener, overheard, encryption) = eavesdrop(&carol_raw, &room_id, &env.user_id(1)).await;

    alice_as_seen_by(&mut carol, &env.user_id(1), |p| p.speaking).await;
    alice_as_seen_by(&mut alice, &env.user_id(3), |p| p.speaking).await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    let alice_hears = alice_audio.level(CAROL_HZ, 2.0);
    let carol_hears = carol_audio.level(ALICE_HZ, 2.0);
    let stranger = level_of(&overheard.lock().unwrap(), ALICE_HZ, 2.0);
    let samples = overheard.lock().unwrap().len();
    let encryption = encryption.lock().unwrap().clone();
    eprintln!(
        "encrypted call: alice hears carol {alice_hears:.4}, carol hears alice {carol_hears:.4}; \
         listener without keys: {stranger:.5} over {samples} samples, publication encryption {encryption:?}"
    );
    assert!(alice_hears > HEARD, "alice hears carol: {alice_hears:.4}");
    assert!(carol_hears > HEARD, "carol hears alice: {carol_hears:.4}");
    assert_eq!(
        encryption.as_deref(),
        Some("Gcm"),
        "alice publishes an encrypted track"
    );
    // Decoding ciphertext gives some noise (seen up to 0.008), not the tone.
    assert!(stranger < 0.03, "listener without keys: {stranger:.4}");
    let _ = listener.close().await;

    // Carol leaves (alice rotates her key) and comes back.
    carol.commands.send(Command::LeaveCall).await.unwrap();
    expect_event(&mut carol, |e| {
        matches!(e, CoreEvent::CallState(CallState::Left { .. }))
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    join_expecting(&mut carol, &room_id, true).await;
    // Both directions need the other's new key (to-device) and the new
    // subscription: wait until both hear the other, up to 20 s, rather than
    // measuring once after a fixed delay (6 s was once not enough in a gate
    // run; the time it takes is printed).
    let rejoined = std::time::Instant::now();
    let (mut alice_hears, mut carol_hears);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        alice_hears = alice_audio.level(CAROL_HZ, 2.0);
        carol_hears = carol_audio.level(ALICE_HZ, 2.0);
        let heard = alice_hears > HEARD && carol_hears > HEARD;
        if (heard && rejoined.elapsed() >= Duration::from_secs(3))
            || rejoined.elapsed() > Duration::from_secs(20)
        {
            break;
        }
    }
    eprintln!(
        "after carol rejoined ({:?}): alice hears carol {alice_hears:.4}, carol hears alice {carol_hears:.4}",
        rejoined.elapsed()
    );
    assert!(
        alice_hears > HEARD,
        "after rejoin, alice hears carol: {alice_hears:.4}"
    );
    assert!(
        carol_hears > HEARD,
        "after rejoin, carol hears alice (rotated key): {carol_hears:.4}"
    );

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// Cleaning other people's voices (Task 16): with it on, alice's device
/// runs DeepFilterNet3 on carol's voice, which takes her steady tone for
/// noise; set to never for carol, her voice plays as it came.
#[tokio::test]
#[ignore = "needs local homeserver and scripts/fetch-models.sh"]
async fn remote_voices_are_cleaned_when_asked() {
    const CAROL_HZ: f32 = 600.0;
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

    let alice_audio = Arc::new(Duplex::new(TONE_HZ));
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1, AudioBackend::Virtual(alice_audio.clone())).await;
    let mut carol = logged_in(
        dir_c.path(),
        3,
        AudioBackend::Virtual(Arc::new(Duplex::new(CAROL_HZ))),
    )
    .await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    carol
        .commands
        .send(Command::UpdateAudioSettings(tone_settings()))
        .await
        .unwrap();
    expect_event(&mut carol, |e| matches!(e, CoreEvent::AudioSettings(_))).await;
    join(&mut alice, &room_id).await;
    join(&mut carol, &room_id).await;
    let carol_id = env.user_id(3);
    alice_as_seen_by(&mut alice, &carol_id, |p| {
        p.speaking && p.cleaning == CleaningStatus::Off
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let plain = alice_audio.level(CAROL_HZ, 2.0);

    let mut settings = AudioSettings::default();
    settings.remote_cleaning.enabled = true;
    alice
        .commands
        .send(Command::UpdateAudioSettings(settings))
        .await
        .unwrap();
    alice_as_seen_by(&mut alice, &carol_id, |p| {
        p.cleaning == CleaningStatus::Cleaned
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let cleaned = alice_audio.level(CAROL_HZ, 2.0);

    alice
        .commands
        .send(Command::SetUserCleaning {
            user_id: UserId::from(carol_id.as_str()),
            cleaning: VoiceCleaning::Never,
        })
        .await
        .unwrap();
    alice_as_seen_by(&mut alice, &carol_id, |p| {
        p.cleaning == CleaningStatus::Skipped(SkipReason::Never)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let skipped = alice_audio.level(CAROL_HZ, 2.0);
    eprintln!(
        "carol's tone at alice: {plain:.4} plain, {cleaned:.5} cleaned, {skipped:.4} set to never"
    );
    assert!(plain > HEARD, "plain: {plain:.4}");
    assert!(cleaned < plain / 10.0, "cleaned: {cleaned:.5}");
    assert!(skipped > HEARD, "set to never: {skipped:.4}");

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}
