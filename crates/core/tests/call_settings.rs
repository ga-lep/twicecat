//! Audio and video settings through the core API: defaults, validation,
//! persistence across restarts, per-user volume, a corrupt file, device
//! listing, the video encoders, and the call commands that are not
//! available yet. Needs no server or audio device.

mod common;

use std::sync::Arc;

use common::{expect_event, silence, start_core, start_core_with_audio, start_core_with_video};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    AudioSettings, CallIntent, Command, CoreError, CoreEvent, EncoderBackend, EncoderChoice,
    InputMode, PushToTalkBindings, RoomId, SoundSource, TEST_PATTERN_ID, UserId, VideoBackend,
    VideoQuality, VideoSettings, VideoTrackId,
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

async fn video_settings(handle: &mut CoreHandle) -> VideoSettings {
    match expect_event(handle, |e| matches!(e, CoreEvent::VideoSettings(_))).await {
        CoreEvent::VideoSettings(s) => s,
        _ => unreachable!("filtered by the predicate"),
    }
}

/// Video settings start at their defaults, are saved, and come back after a
/// restart; a forced software encoder stays forced.
#[tokio::test]
async fn video_settings_persist_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    send(&handle, Command::GetVideoSettings).await;
    assert_eq!(video_settings(&mut handle).await, VideoSettings::default());
    assert!(!DataPaths::at(dir.path()).video_settings_file().exists());
    let chosen = VideoSettings {
        encoder: EncoderChoice::Prefer(EncoderBackend::Software),
        screen_quality: VideoQuality::Uhd2160p30,
        ..VideoSettings::default()
    };
    send(&handle, Command::UpdateVideoSettings(chosen.clone())).await;
    assert_eq!(video_settings(&mut handle).await, chosen);
    send(&handle, Command::Shutdown).await;
    while handle.events.recv().await.is_some() {}

    let mut handle = core(dir.path());
    send(&handle, Command::GetVideoSettings).await;
    assert_eq!(video_settings(&mut handle).await, chosen);
    send(&handle, Command::Shutdown).await;
}

/// Advanced video settings out of bounds are brought inside, saved that way,
/// echoed, and said in a `VideoSettingsAdjusted` after the echo; settings in
/// bounds say nothing, and an old file without the new fields still loads.
#[tokio::test]
async fn advanced_video_settings_are_validated_and_reported() {
    use twicecat_core::api::{VideoTuning, video_limits};
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    let asked = VideoSettings {
        screen_tuning: VideoTuning {
            fps: Some(900),
            bitrate_kbps: Some(7_000),
            ..VideoTuning::default()
        },
        ..VideoSettings::default()
    };
    send(&handle, Command::UpdateVideoSettings(asked)).await;
    let echoed = video_settings(&mut handle).await;
    assert_eq!(echoed.screen_tuning.fps, Some(video_limits::FPS.1));
    assert_eq!(echoed.screen_tuning.bitrate_kbps, Some(7_000));
    match expect_event(&mut handle, |e| {
        matches!(e, CoreEvent::VideoSettingsAdjusted(_))
    })
    .await
    {
        CoreEvent::VideoSettingsAdjusted(changes) => {
            assert_eq!(changes.len(), 1, "{changes:?}");
            assert_eq!(changes[0].setting, "screen.fps");
        }
        _ => unreachable!("filtered by the predicate"),
    }
    // In bounds: no report.
    send(&handle, Command::UpdateVideoSettings(echoed.clone())).await;
    assert_eq!(video_settings(&mut handle).await, echoed);
    send(&handle, Command::GetVideoSettings).await;
    assert_eq!(video_settings(&mut handle).await, echoed);
    send(&handle, Command::Shutdown).await;
    while let Some(event) = handle.events.recv().await {
        assert!(
            !matches!(event, CoreEvent::VideoSettingsAdjusted(_)),
            "nothing was out of bounds the second time"
        );
    }
    // Saved inside the bounds: a restart reads them back as echoed.
    let mut handle = core(dir.path());
    send(&handle, Command::GetVideoSettings).await;
    assert_eq!(video_settings(&mut handle).await, echoed);
    send(&handle, Command::Shutdown).await;
}

/// A settings file from before the advanced fields existed loads, with
/// everything new automatic.
#[tokio::test]
async fn an_old_video_settings_file_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    DataPaths::at(dir.path())
        .write_video_settings(
            br#"{"version": 1, "settings": {"encoder": {"prefer": "software"}, "upload_cap_kbps": 6000}}"#,
        )
        .unwrap();
    let mut handle = core(dir.path());
    send(&handle, Command::GetVideoSettings).await;
    let loaded = video_settings(&mut handle).await;
    assert_eq!(loaded.upload_cap_kbps, Some(6_000));
    assert!(loaded.camera_tuning.is_automatic() && loaded.screen_tuning.is_automatic());
    send(&handle, Command::Shutdown).await;
}

/// The encoder list always ends with the software path, and names no
/// encoder in use outside a call.
#[tokio::test]
async fn the_encoders_end_with_software() {
    let dir = tempfile::tempdir().unwrap();
    let mut handle = core(dir.path());
    send(&handle, Command::ProbeVideoEncoders).await;
    let e = expect_event(&mut handle, |e| matches!(e, CoreEvent::VideoEncoders(_))).await;
    let CoreEvent::VideoEncoders(encoders) = e else {
        unreachable!("filtered by the predicate")
    };
    let last = encoders
        .encoders
        .last()
        .expect("at least the software path");
    assert_eq!(last.backend, EncoderBackend::Software);
    assert!(!last.hardware);
    assert!(
        encoders
            .encoders
            .iter()
            .all(|e| e.hardware == (e.backend != EncoderBackend::Software)),
        "{encoders:?}"
    );
    assert_eq!(encoders.in_use, None);
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
