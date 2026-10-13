//! The engine's video commands (Phase 9). This task's sending is the
//! software path with the test pattern as camera and screen
//! ([`VideoBackend::TestPattern`]); real cameras, screens, the settings and
//! our encoders answer `NotSupported` until their tasks.

use super::Engine;
use crate::api::{
    Command, CoreError, CoreEvent, LocalVideo, RoomId, ScreenSource, ScreenSourceKind,
    ScreenSources, TEST_PATTERN_ID, VideoBackend, VideoDevice, VideoDeviceId, VideoDevices,
    VideoKind, VideoMode, VideoQuality,
};

/// The test pattern as a camera: every size and rate the qualities name.
fn pattern_camera() -> VideoDevice {
    VideoDevice {
        id: VideoDeviceId::from(TEST_PATTERN_ID),
        name: "Test pattern".into(),
        is_default: true,
        modes: [
            (3840, 2160, 60),
            (2560, 1440, 60),
            (1920, 1080, 60),
            (1280, 720, 30),
        ]
        .into_iter()
        .map(|(width, height, fps)| VideoMode { width, height, fps })
        .collect(),
    }
}

/// The test pattern as a screen.
fn pattern_screen() -> ScreenSource {
    ScreenSource {
        id: TEST_PATTERN_ID.into(),
        kind: ScreenSourceKind::Monitor,
        title: "Test pattern".into(),
        width: 1920,
        height: 1080,
        thumbnail: None,
    }
}

/// What the kind is called in errors.
fn feature(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => "cameras",
        VideoKind::Screen => "screen sharing",
    }
}

impl Engine {
    /// Handles one of the video commands.
    pub(super) async fn video_command(&mut self, command: Command) {
        let pattern = self.config.video == VideoBackend::TestPattern;
        match command {
            Command::ListVideoDevices if pattern => {
                let devices = VideoDevices {
                    cameras: vec![pattern_camera()],
                };
                self.emit(CoreEvent::VideoDevices(devices)).await;
            }
            Command::ListScreenSources if pattern => {
                let sources = ScreenSources {
                    system_picker: false,
                    sources: vec![pattern_screen()],
                };
                self.emit(CoreEvent::ScreenSources(sources)).await;
            }
            Command::StartCamera { device, quality } => {
                let id = device.as_ref().map(VideoDeviceId::as_str);
                self.start_video(VideoKind::Camera, id, quality).await;
            }
            Command::StartScreenShare {
                source,
                quality,
                audio,
            } => {
                if audio {
                    // Plan Task 8.
                    self.not_supported("screen-share sound").await;
                    return;
                }
                let id = source.as_ref().map(|s| s.as_str());
                self.start_video(VideoKind::Screen, id, quality).await;
            }
            Command::StopCamera => self.stop_video(VideoKind::Camera).await,
            Command::StopScreenShare => self.stop_video(VideoKind::Screen).await,
            Command::SetVideoView {
                track,
                width,
                height,
                visible,
            } => match &self.call {
                // A track that just ended is no error: the front end has not
                // heard yet.
                Some(call) => {
                    if !call.set_video_view(&track, width, height, visible) {
                        tracing::debug!("a video view for a track that is not received");
                    }
                }
                None => self.emit(CoreEvent::Error(CoreError::NotInCall)).await,
            },
            Command::ListVideoDevices => self.not_supported(feature(VideoKind::Camera)).await,
            Command::ListScreenSources => self.not_supported(feature(VideoKind::Screen)).await,
            // Plan Tasks 6, 9 and 11.
            _ => self.not_supported("video").await,
        }
    }

    async fn not_supported(&self, what: &str) {
        self.emit(CoreEvent::Error(CoreError::NotSupported(what.into())))
            .await;
    }

    /// Sends our camera or screen share in the current call.
    async fn start_video(&mut self, kind: VideoKind, id: Option<&str>, quality: VideoQuality) {
        if self.config.video != VideoBackend::TestPattern {
            self.not_supported(feature(kind)).await;
            return;
        }
        if id.is_some_and(|id| id != TEST_PATTERN_ID) {
            let what = match kind {
                VideoKind::Camera => "that camera is not available",
                VideoKind::Screen => "that screen or window is not available",
            };
            self.emit(CoreEvent::Error(CoreError::Call(what.into())))
                .await;
            return;
        }
        let Some(call) = self.call.as_mut() else {
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return;
        };
        let mode = crate::call::software_mode(quality);
        match call.start_pattern(kind, mode).await {
            Ok(_) => {
                let local = call.local_video();
                self.emit(CoreEvent::LocalVideo(local)).await;
            }
            Err(e) => self.emit(CoreEvent::Error(e)).await,
        }
    }

    async fn stop_video(&mut self, kind: VideoKind) {
        let Some(call) = self.call.as_mut() else {
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return;
        };
        call.stop_video(kind).await;
        let local = call.local_video();
        self.emit(CoreEvent::LocalVideo(local)).await;
    }

    /// After a call with video ended: no tracks, nothing sent.
    pub(super) async fn video_ended(&self, room_id: RoomId, had: (bool, LocalVideo)) {
        let (tracks, local) = had;
        if tracks {
            self.emit(CoreEvent::VideoTracks {
                room_id,
                tracks: Vec::new(),
            })
            .await;
        }
        if local != LocalVideo::default() {
            self.emit(CoreEvent::LocalVideo(LocalVideo::default()))
                .await;
        }
    }
}
