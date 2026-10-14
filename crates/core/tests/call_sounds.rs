//! The call cues end to end on the local stack (`scripts/stack.sh up -d
//! --wait && scripts/create-test-users.sh`): the rising chime when joining
//! (or starting) a call, the falling one when leaving, and the cases that
//! stay silent. Virtual speakers record what each person's call plays, and a
//! cue is recognised by the energy of its two notes (E5 at 659 Hz, A5 at
//! 880 Hz) in 10 ms windows.
//!
//! Uses alice and carol; bob stays untouched. Microphones are silent, so any
//! cue that reaches another person's speakers went through the room.

mod common;

use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_room, expect_event, expect_event_within, raw_login,
    start_core_with_audio, start_core_with_call_service, unique_room_name, wait_for_room,
};
use serde_json::Value;
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    AUDIO_FRAME_LEN, AudioBackend, AudioSettings, CallEndReason, CallIntent, CallSounds, CallState,
    Command, CoreError, CoreEvent, ListDiff, Participant, Password, RoomId, SyncState,
    VirtualAudio,
};
use twicecat_core::secrets::InMemorySecretStore;

const RATE: f32 = 48_000.0;
const E5: f32 = 659.25;
const A5: f32 = 880.0;
/// One analysis window: 10 ms.
const WINDOW: usize = 480;
/// A note is heard above this amplitude (-38 dBFS; the cue peaks at 0.06 at
/// the default 60 %).
const HEARD: f32 = 0.012;
/// "No cue energy": below -60 dBFS.
const SILENT: f32 = 0.001;

/// A silent microphone; records what it plays.
#[derive(Default)]
struct Speaker {
    played: Mutex<Vec<f32>>,
}

impl VirtualAudio for Speaker {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        frame.fill(0.0);
    }
    fn playback(&self, frame: &[f32; AUDIO_FRAME_LEN]) {
        self.played.lock().unwrap().extend_from_slice(frame);
    }
}

impl Speaker {
    fn len(&self) -> usize {
        self.played.lock().unwrap().len()
    }

    /// Everything played from sample `from` on.
    fn since(&self, from: usize) -> Vec<f32> {
        self.played.lock().unwrap()[from..].to_vec()
    }
}

/// Amplitude of `hz` in `s` (Goertzel).
fn level(s: &[f32], hz: f32) -> f32 {
    let w = TAU * hz / RATE;
    let coeff = 2.0 * w.cos();
    let (mut a, mut b) = (0.0_f32, 0.0_f32);
    for x in s {
        let next = x + coeff * a - b;
        b = a;
        a = next;
    }
    2.0 * (a * a + b * b - coeff * a * b).max(0.0).sqrt() / s.len().max(1) as f32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Note {
    E,
    A,
}

/// What one burst of sound was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Burst {
    /// E5 then A5: the join cue.
    Rising,
    /// A5 then E5: the leave cue.
    Falling,
    /// Anything else with cue energy.
    Other,
}

/// The cue-like bursts in `played`: windows dominated by E5 or A5, grouped
/// into bursts separated by 100 ms of neither.
fn bursts(played: &[f32]) -> Vec<Burst> {
    let mut out = Vec::new();
    let mut notes: Vec<Note> = Vec::new();
    let mut quiet = 0;
    let mut close = |notes: &mut Vec<Note>| {
        if !notes.is_empty() {
            out.push(match notes.as_slice() {
                [Note::E, Note::A] => Burst::Rising,
                [Note::A, Note::E] => Burst::Falling,
                _ => Burst::Other,
            });
            notes.clear();
        }
    };
    for window in played.chunks_exact(WINDOW) {
        let (e, a) = (level(window, E5), level(window, A5));
        let note = if e > HEARD && e > 2.0 * a {
            Some(Note::E)
        } else if a > HEARD && a > 2.0 * e {
            Some(Note::A)
        } else {
            None
        };
        match note {
            Some(note) => {
                quiet = 0;
                if notes.last() != Some(&note) {
                    notes.push(note);
                }
            }
            None => {
                quiet += 1;
                if quiet == 10 {
                    close(&mut notes);
                }
            }
        }
    }
    close(&mut notes);
    out
}

/// The loudest cue note in any 10 ms window of `played`.
fn cue_level(played: &[f32]) -> f32 {
    played
        .chunks_exact(WINDOW)
        .map(|w| level(w, E5).max(level(w, A5)))
        .fold(0.0, f32::max)
}

fn energy(s: &[f32]) -> f32 {
    s.iter().map(|x| x * x).sum()
}

/// Lets the audio thread play a cue out (240 ms) and the recorder catch up.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(700)).await;
}

async fn logged_in(
    dir: &std::path::Path,
    n: u8,
    speaker: &Arc<Speaker>,
    call_service: Option<String>,
) -> CoreHandle {
    let env = TestEnv::load();
    let audio = AudioBackend::Virtual(speaker.clone() as Arc<dyn VirtualAudio>);
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = match call_service {
        Some(service) => start_core_with_call_service(dir, secrets, audio, Some(service)),
        None => start_core_with_audio(dir, secrets, audio),
    };
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

async fn send_join(core: &mut CoreHandle, room_id: &str) {
    core.commands
        .send(Command::JoinCall {
            room_id: RoomId::from(room_id),
            intent: CallIntent::Audio,
        })
        .await
        .unwrap();
}

/// Waits for `Connected` in `room_id`.
async fn connected(core: &mut CoreHandle, room_id: &str) {
    let state = expect_event(core, |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Connected { .. } | CallState::Left { .. })
        ) && !matches!(
            e,
            CoreEvent::CallState(CallState::Left {
                reason: CallEndReason::Hangup,
                ..
            })
        )
    })
    .await;
    assert!(
        matches!(&state, CoreEvent::CallState(CallState::Connected { room_id: r, .. }) if r.as_str() == room_id),
        "{state:?}"
    );
}

async fn join(core: &mut CoreHandle, room_id: &str) {
    send_join(core, room_id).await;
    connected(core, room_id).await;
}

async fn apply_settings(core: &mut CoreHandle, settings: AudioSettings) {
    core.commands
        .send(Command::UpdateAudioSettings(settings))
        .await
        .unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::AudioSettings(_))).await;
}

fn sounds(call_sounds: CallSounds) -> AudioSettings {
    AudioSettings {
        call_sounds,
        ..AudioSettings::default()
    }
}

async fn leave(core: &mut CoreHandle) {
    core.commands.send(Command::LeaveCall).await.unwrap();
    expect_event(core, |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Left {
                reason: CallEndReason::Hangup,
                ..
            })
        )
    })
    .await;
}

async fn finish(cores: &[&CoreHandle]) {
    for core in cores {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// Waits until `who` appears in `core`'s participants.
async fn sees(core: &mut CoreHandle, who: &str) {
    let mut list: Vec<Participant> = Vec::new();
    let wait = async {
        loop {
            match core.events.recv().await {
                Some(CoreEvent::CallParticipants { diffs, .. }) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut list);
                    }
                    if list.iter().any(|p| p.user_id.as_str() == who) {
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
        .expect("timed out waiting for the participant");
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

/// A room of alice's and carol's, where both may call.
async fn shared_room(env: &TestEnv) -> String {
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
    room_id
}

/// A room only alice is in.
async fn own_room() -> String {
    create_room(&raw_login(1).await, &unique_room_name(), None)
        .await
        .to_string()
}

#[test]
fn the_detector_tells_rising_from_falling_and_nothing() {
    let note = |hz: f32, ms: usize| -> Vec<f32> {
        (0..ms * 48)
            .map(|i| 0.06 * (TAU * hz * i as f32 / RATE).sin())
            .collect()
    };
    let silence = vec![0.0; 48 * 300];
    let rising = [note(E5, 120), note(A5, 120)].concat();
    let falling = [note(A5, 120), note(E5, 120)].concat();
    assert_eq!(bursts(&silence), []);
    assert_eq!(bursts(&rising), [Burst::Rising]);
    assert_eq!(bursts(&falling), [Burst::Falling]);
    let both = [
        rising.clone(),
        silence.clone(),
        rising,
        silence.clone(),
        falling,
    ]
    .concat();
    assert_eq!(
        bursts(&both),
        [Burst::Rising, Burst::Rising, Burst::Falling]
    );
    assert!(cue_level(&note(A5, 100)) > 0.05);
    assert!(cue_level(&silence) < SILENT);
}

/// Joining plays a rising cue for whoever joins: alice starting the call,
/// then carol joining it.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn joining_plays_a_rising_cue_to_the_one_who_joins() {
    let env = TestEnv::load();
    let room_id = shared_room(&env).await;
    let (a, c) = (Arc::new(Speaker::default()), Arc::new(Speaker::default()));
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1, &a, None).await;
    let mut carol = logged_in(dir_c.path(), 3, &c, None).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;

    join(&mut alice, &room_id).await;
    settle().await;
    assert_eq!(
        bursts(&a.since(0)),
        [Burst::Rising],
        "alice starts the call"
    );

    join(&mut carol, &room_id).await;
    settle().await;
    assert_eq!(bursts(&c.since(0)), [Burst::Rising], "carol joins it");

    finish(&[&alice, &carol]).await;
}

/// A second `JoinCall` for the call we are in answers `Connected` and plays
/// nothing.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_repeat_join_is_connected_without_a_cue() {
    let room_id = own_room().await;
    let a = Arc::new(Speaker::default());
    let dir = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir.path(), 1, &a, None).await;
    wait_for_room(&mut alice, &room_id).await;

    join(&mut alice, &room_id).await;
    settle().await;
    assert_eq!(bursts(&a.since(0)), [Burst::Rising]);
    let mark = a.len();

    join(&mut alice, &room_id).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        cue_level(&a.since(mark)) < SILENT,
        "a repeat join played a cue"
    );

    finish(&[&alice]).await;
}

/// Deafened, alice still hears the falling cue when she leaves; `Left`
/// does not wait for it, the devices close after it, and a second leave is
/// refused and silent.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn leaving_deafened_plays_the_falling_cue_without_delaying_left() {
    let room_id = own_room().await;
    let a = Arc::new(Speaker::default());
    let dir = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir.path(), 1, &a, None).await;
    wait_for_room(&mut alice, &room_id).await;
    join(&mut alice, &room_id).await;
    alice
        .commands
        .send(Command::SetDeafened(true))
        .await
        .unwrap();
    settle().await;
    let mark = a.len();

    alice.commands.send(Command::LeaveCall).await.unwrap();
    expect_event(&mut alice, |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Left {
                reason: CallEndReason::Hangup,
                ..
            })
        )
    })
    .await;
    let at_left = a.len();
    tokio::time::sleep(Duration::from_millis(800)).await;
    let cue = a.since(mark);
    assert_eq!(bursts(&cue), [Burst::Falling], "the falling cue, deafened");
    let after_left = a.since(at_left);
    assert!(
        energy(&after_left) > 0.5 * energy(&cue),
        "Left waited for the cue: {:.4} of {:.4} played after it",
        energy(&after_left),
        energy(&cue)
    );

    // The devices are closed some time after the cue, not kept open.
    let closed_at = a.len();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(a.len(), closed_at, "the devices are still playing");

    // A second leave: refused, and silent.
    alice.commands.send(Command::LeaveCall).await.unwrap();
    let refused = expect_event(&mut alice, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(
        matches!(refused, CoreEvent::Error(CoreError::NotInCall)),
        "{refused:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(a.len(), closed_at, "no audio session for a refused leave");

    alice.commands.send(Command::Shutdown).await.unwrap();
}

/// The settings silence the cues: all off, then only leaving off.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn the_settings_turn_the_cues_off() {
    let room_id = own_room().await;
    let a = Arc::new(Speaker::default());
    let dir = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir.path(), 1, &a, None).await;
    wait_for_room(&mut alice, &room_id).await;

    apply_settings(
        &mut alice,
        sounds(CallSounds {
            enabled: false,
            ..CallSounds::default()
        }),
    )
    .await;
    join(&mut alice, &room_id).await;
    settle().await;
    leave(&mut alice).await;
    settle().await;
    assert!(a.len() > 0, "the call's audio ran");
    assert!(cue_level(&a.since(0)) < SILENT, "disabled sounds played");

    apply_settings(
        &mut alice,
        sounds(CallSounds {
            leaving: false,
            ..CallSounds::default()
        }),
    )
    .await;
    let mark = a.len();
    join(&mut alice, &room_id).await;
    settle().await;
    assert_eq!(
        bursts(&a.since(mark)),
        [Burst::Rising],
        "joining still plays"
    );
    leave(&mut alice).await;
    settle().await;
    assert_eq!(
        bursts(&a.since(mark)),
        [Burst::Rising],
        "leaving played with `leaving` off"
    );

    alice.commands.send(Command::Shutdown).await.unwrap();
}

/// A join that fails (the call service is a closed local port) ends in
/// `Left { Failed }` and plays no cue.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_failed_join_plays_no_cue() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port())
    };
    let room_id = own_room().await;
    let a = Arc::new(Speaker::default());
    let dir = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir.path(), 1, &a, Some(closed)).await;
    wait_for_room(&mut alice, &room_id).await;

    send_join(&mut alice, &room_id).await;
    let left = expect_event_within(&mut alice, Duration::from_secs(60), |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Left { .. } | CallState::Connected { .. })
        )
    })
    .await;
    assert!(
        matches!(
            left,
            CoreEvent::CallState(CallState::Left {
                reason: CallEndReason::Failed(_),
                ..
            })
        ),
        "{left:?}"
    );
    settle().await;
    assert!(
        cue_level(&a.since(0)) < SILENT,
        "a failed join played a cue"
    );

    alice.commands.send(Command::Shutdown).await.unwrap();
}

/// Switching from one room's call to another's plays the new room's join
/// cue only: no leave cue for the call that ends.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn switching_rooms_plays_only_the_new_join_cue() {
    let (room_a, room_b) = (own_room().await, own_room().await);
    let a = Arc::new(Speaker::default());
    let dir = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir.path(), 1, &a, None).await;
    wait_for_room(&mut alice, &room_a).await;
    wait_for_room(&mut alice, &room_b).await;

    join(&mut alice, &room_a).await;
    settle().await;
    let started = Instant::now();
    send_join(&mut alice, &room_b).await;
    connected(&mut alice, &room_b).await;
    eprintln!("switching took {:?}", started.elapsed());
    settle().await;
    assert_eq!(
        bursts(&a.since(0)),
        [Burst::Rising, Burst::Rising],
        "one join cue for each room, no leave cue between"
    );

    finish(&[&alice]).await;
}

/// The cue is not sent to the room: with alice's microphone silent (so no
/// echo path), carol's speakers get no cue energy above -60 dBFS while
/// alice joins, and alice's own speakers do play it.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn the_cue_never_reaches_the_room() {
    let env = TestEnv::load();
    let room_id = shared_room(&env).await;
    let (a, c) = (Arc::new(Speaker::default()), Arc::new(Speaker::default()));
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1, &a, None).await;
    let mut carol = logged_in(dir_c.path(), 3, &c, None).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    // Carol's own cues would play on her speakers: leave them off, so what
    // she records is what the room sends her.
    apply_settings(
        &mut carol,
        sounds(CallSounds {
            enabled: false,
            ..CallSounds::default()
        }),
    )
    .await;

    join(&mut carol, &room_id).await;
    settle().await;
    let mark = c.len();
    join(&mut alice, &room_id).await;
    sees(&mut carol, &env.user_id(1)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert_eq!(bursts(&a.since(0)), [Burst::Rising], "alice hears her cue");
    let heard = c.since(mark);
    assert!(
        heard.len() > 3 * 48_000,
        "carol's speakers ran: {}",
        heard.len()
    );
    let loudest = cue_level(&heard);
    assert!(
        loudest < SILENT,
        "carol heard cue energy {loudest:.5} from the room"
    );

    finish(&[&alice, &carol]).await;
}
