//! Audio settings through the core API: defaults, validation, persistence
//! across restarts, per-user volume, a corrupt file, and the call commands
//! that are not available yet. Needs no server.

mod common;

use std::sync::Arc;

use common::{expect_event, start_core};
use tessera_core::CoreHandle;
use tessera_core::api::{
    AudioSettings, CallIntent, Command, CoreError, CoreEvent, InputMode, RoomId, UserId,
};
use tessera_core::paths::DataPaths;
use tessera_core::secrets::InMemorySecretStore;

fn core(dir: &std::path::Path) -> CoreHandle {
    start_core(dir, Arc::new(InMemorySecretStore::new()))
}

async fn settings(handle: &mut CoreHandle) -> AudioSettings {
    match expect_event(handle, |e| matches!(e, CoreEvent::AudioSettings(_))).await {
        CoreEvent::AudioSettings(s) => s,
        _ => unreachable!("filtered by the predicate"),
    }
}

async fn send(handle: &CoreHandle, cmd: Command) {
    handle.commands.send(cmd).await.expect("engine is running");
}

#[tokio::test]
async fn defaults_without_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    send(&handle, Command::GetAudioSettings).await;
    assert_eq!(settings(&mut handle).await, AudioSettings::default());
    assert!(!DataPaths::at(dir.path()).call_settings_file().exists());
}

#[tokio::test]
async fn update_is_validated_saved_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    let wanted = AudioSettings {
        input_mode: InputMode::PushToTalk {
            release_delay_ms: 99_999,
        },
        voice_bitrate_kbps: 96,
        ..AudioSettings::default()
    };
    send(&handle, Command::UpdateAudioSettings(wanted.clone())).await;
    let echoed = settings(&mut handle).await;
    assert_eq!(echoed, wanted.validated());
    assert_eq!(
        echoed.input_mode,
        InputMode::PushToTalk {
            release_delay_ms: 2000
        }
    );
    send(&handle, Command::Shutdown).await;
    drop(handle);

    let mut handle = core(dir.path());
    send(&handle, Command::GetAudioSettings).await;
    assert_eq!(settings(&mut handle).await, echoed);
}

#[tokio::test]
async fn user_volume_is_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    let bob = UserId::from("@bob:localhost");
    send(
        &handle,
        Command::SetUserVolume {
            user_id: bob.clone(),
            percent: 300,
        },
    )
    .await;
    assert_eq!(settings(&mut handle).await.user_volume(&bob), 200);
    send(
        &handle,
        Command::SetUserVolume {
            user_id: bob.clone(),
            percent: 100,
        },
    )
    .await;
    let s = settings(&mut handle).await;
    assert_eq!(s.user_volume(&bob), 100);
    assert!(s.user_volumes.is_empty(), "100 % is not stored");
}

#[tokio::test]
async fn corrupt_file_is_reported_once_and_defaults_apply() {
    let dir = tempfile::tempdir().unwrap();
    DataPaths::at(dir.path())
        .write_call_settings(b"not json")
        .unwrap();
    let mut handle = core(dir.path());
    send(&handle, Command::GetAudioSettings).await;
    let error = expect_event(&mut handle, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(matches!(error, CoreEvent::Error(CoreError::Storage(_))));
    assert_eq!(settings(&mut handle).await, AudioSettings::default());
    // The second request neither reloads nor reports again.
    send(&handle, Command::GetAudioSettings).await;
    let next = expect_event(&mut handle, |_| true).await;
    assert!(matches!(next, CoreEvent::AudioSettings(_)), "{next:?}");
}

#[tokio::test]
async fn call_commands_need_a_session_or_a_call() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    send(
        &handle,
        Command::JoinCall {
            room_id: RoomId::from("!r:localhost"),
            intent: CallIntent::Audio,
        },
    )
    .await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(
        matches!(e, CoreEvent::Error(CoreError::NotLoggedIn)),
        "{e:?}"
    );
    send(&handle, Command::SetMicMuted(true)).await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(matches!(e, CoreEvent::Error(CoreError::NotInCall)), "{e:?}");
}
