//! Audio settings through the core API: defaults, validation, persistence
//! across restarts, per-user volume, a corrupt file, device listing, and the
//! call commands that are not available yet. Needs no server or audio device.

mod common;

use std::sync::Arc;

use common::{expect_event, silence, start_core, start_core_with_audio, start_core_with_video};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    AudioSettings, CallIntent, Command, CoreError, CoreEvent, InputMode, PushToTalkBindings,
    RoomId, SoundSource, TEST_PATTERN_ID, UserId, VideoBackend, VideoQuality, VideoSettings,
    VideoTrackId,
};
use twicecat_core::paths::DataPaths;
use twicecat_core::secrets::InMemorySecretStore;

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
            toggle: true,
        },
        voice_bitrate_kbps: 96,
        push_to_talk: PushToTalkBindings {
            key: Some("ctrl+shift+KeyT".into()),
            gamepad_button: Some("left_thumb".into()),
        },
        ..AudioSettings::default()
    };
    send(&handle, Command::UpdateAudioSettings(wanted.clone())).await;
    let echoed = settings(&mut handle).await;
    assert_eq!(echoed, wanted.validated());
    assert_eq!(
        echoed.input_mode,
        InputMode::PushToTalk {
            release_delay_ms: 2000,
            toggle: true,
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
            source: SoundSource::Voice,
            percent: 300,
        },
    )
    .await;
    assert_eq!(settings(&mut handle).await.user_volume(&bob), 200);
    send(
        &handle,
        Command::SetUserVolume {
            user_id: bob.clone(),
            source: SoundSource::ScreenShare,
            percent: 30,
        },
    )
    .await;
    let s = settings(&mut handle).await;
    assert_eq!(s.user_share_volume(&bob), 30);
    assert_eq!(s.user_volume(&bob), 200, "the voice keeps its own");
    send(
        &handle,
        Command::SetUserVolume {
            user_id: bob.clone(),
            source: SoundSource::Voice,
            percent: 100,
        },
    )
    .await;
    let s = settings(&mut handle).await;
    assert_eq!(s.user_volume(&bob), 100);
    assert!(s.user_volumes.is_empty(), "100 % is not stored");
    assert_eq!(s.user_share_volume(&bob), 30);
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

/// The error a command should answer with.
enum Answer {
    NotSupported(&'static str),
    NotInCall,
}

/// The video commands without a call and without real cameras or screens
/// (Phase 9 Task 5): what needs a call says so, what needs hardware, the
/// settings or our encoders answers `NotSupported` until its task.
#[tokio::test]
async fn video_commands_answer_without_a_call_or_hardware() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    use Answer::{NotInCall, NotSupported};
    let cases = [
        (Command::ListVideoDevices, NotSupported("cameras")),
        (Command::ListScreenSources, NotSupported("screen sharing")),
        (
            Command::StartCamera {
                device: None,
                quality: VideoQuality::Auto,
            },
            NotSupported("cameras"),
        ),
        (Command::StopCamera, NotInCall),
        (
            Command::StartScreenShare {
                source: None,
                quality: VideoQuality::Uhd2160p60,
                audio: true,
            },
            NotSupported("screen-share sound"),
        ),
        (Command::StopScreenShare, NotInCall),
        (
            Command::SetVideoView {
                track: VideoTrackId::from("t"),
                width: 640,
                height: 360,
                visible: true,
            },
            NotInCall,
        ),
        (Command::GetVideoSettings, NotSupported("video")),
        (
            Command::UpdateVideoSettings(VideoSettings::default()),
            NotSupported("video"),
        ),
        (Command::ProbeVideoEncoders, NotSupported("video")),
        (
            Command::StartCameraPreview {
                device: None,
                quality: VideoQuality::Hd720p30,
            },
            NotSupported("video"),
        ),
        (Command::StopCameraPreview, NotSupported("video")),
    ];
    for (command, answer) in cases {
        let name = format!("{command:?}");
        send(&handle, command).await;
        let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::Error(_))).await;
        let ok = match answer {
            NotSupported(what) => {
                matches!(&e, CoreEvent::Error(CoreError::NotSupported(w)) if w == what)
            }
            NotInCall => matches!(e, CoreEvent::Error(CoreError::NotInCall)),
        };
        assert!(ok, "{name}: {e:?}");
    }
    send(&handle, Command::Shutdown).await;
}

/// With the test pattern as camera and screen, both are listed, and
/// sending one needs a call.
#[tokio::test]
async fn the_test_pattern_is_the_camera_and_the_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = start_core_with_video(
        dir.path(),
        Arc::new(InMemorySecretStore::new()),
        silence(),
        None,
        VideoBackend::TestPattern,
    );
    send(&handle, Command::ListVideoDevices).await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::VideoDevices(_))).await;
    let CoreEvent::VideoDevices(devices) = e else {
        unreachable!("filtered by the predicate")
    };
    assert_eq!(devices.cameras.len(), 1);
    assert_eq!(devices.cameras[0].id.as_str(), TEST_PATTERN_ID);
    assert!(devices.cameras[0].is_default);
    send(&handle, Command::ListScreenSources).await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::ScreenSources(_))).await;
    let CoreEvent::ScreenSources(sources) = e else {
        unreachable!("filtered by the predicate")
    };
    assert!(!sources.system_picker);
    assert_eq!(sources.sources[0].id.as_str(), TEST_PATTERN_ID);
    send(
        &handle,
        Command::StartCamera {
            device: None,
            quality: VideoQuality::Hd720p30,
        },
    )
    .await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(matches!(e, CoreEvent::Error(CoreError::NotInCall)), "{e:?}");
    send(
        &handle,
        Command::StartCamera {
            device: Some("usb-cam".into()),
            quality: VideoQuality::Hd720p30,
        },
    )
    .await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(matches!(e, CoreEvent::Error(CoreError::Call(_))), "{e:?}");
    send(&handle, Command::Shutdown).await;
}

#[tokio::test]
async fn listing_audio_devices_answers_without_assuming_hardware() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    send(&handle, Command::ListAudioDevices).await;
    let answer = expect_event(&mut handle, |e| {
        matches!(
            e,
            CoreEvent::AudioDevices(_) | CoreEvent::Error(CoreError::Audio(_))
        )
    })
    .await;
    if let CoreEvent::AudioDevices(devices) = answer {
        // At most one default per direction.
        assert!(devices.inputs.iter().filter(|d| d.is_default).count() <= 1);
        assert!(devices.outputs.iter().filter(|d| d.is_default).count() <= 1);
    }
    send(&handle, Command::Shutdown).await;
}

/// Human-run (AGENTS.md rule 9): opens the real microphone for about a
/// second. Run with `cargo test -p twicecat-core -- --ignored hardware_`.
#[tokio::test]
#[ignore = "needs a microphone; opens it for about a second (human-run)"]
async fn hardware_mic_test_reports_levels() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = start_core_with_audio(
        dir.path(),
        Arc::new(InMemorySecretStore::new()),
        twicecat_core::api::AudioBackend::System,
    );
    send(&handle, Command::StartMicTest { loopback: false }).await;
    for _ in 0..10 {
        let e = expect_event(&mut handle, |e| {
            matches!(e, CoreEvent::InputLevel(_) | CoreEvent::Error(_))
        })
        .await;
        assert!(matches!(e, CoreEvent::InputLevel(_)), "{e:?}");
    }
    send(&handle, Command::StopMicTest).await;
    send(&handle, Command::Shutdown).await;
}
