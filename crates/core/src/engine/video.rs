//! The engine's video commands (Phase 9). We send the test pattern as
//! camera and screen ([`VideoBackend::TestPattern`]), encoded by the
//! encoder the settings and this machine give (Task 6); real cameras and
//! screens answer `NotSupported` until their tasks, as do previews.

use super::{Engine, blocking};
use crate::api::{
    Command, CoreError, CoreEvent, EncoderBackend, LocalVideo, RoomId, ScreenSource,
    ScreenSourceKind, ScreenSources, TEST_PATTERN_ID, VideoBackend, VideoDevice, VideoDeviceId,
    VideoDevices, VideoKind, VideoMode, VideoQuality, VideoSettings,
};
use crate::call::VideoEncodersAvailable;
use crate::video_settings;

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
            Command::ProbeVideoEncoders => {
                let in_use = self.video_in_use();
                let encoders = self.video_encoders().await.encoders(in_use);
                self.emit(CoreEvent::VideoEncoders(encoders)).await;
            }
            Command::GetVideoSettings => {
                let settings = self.video_settings().await.clone();
                self.emit(CoreEvent::VideoSettings(settings)).await;
            }
            Command::UpdateVideoSettings(settings) => self.store_video_settings(settings).await,
            // Plan Tasks 9 and 11.
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
        if self.call.is_none() {
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return;
        }
        let choice = self.video_settings().await.encoder;
        let backend = self.video_encoders().await.choose(choice);
        self.send_video(kind, quality, backend).await;
    }

    /// Publishes our `kind` at `quality` on `backend`, and says what is sent
    /// now (and which encoder, when that changed).
    async fn send_video(
        &mut self,
        kind: VideoKind,
        quality: VideoQuality,
        backend: EncoderBackend,
    ) {
        let before = self.video_in_use();
        let Some(call) = self.call.as_mut() else {
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return;
        };
        let sending = crate::call::sending(backend, kind, quality);
        let result = call.start_pattern(kind, quality, &sending).await;
        let local = call.local_video();
        match result {
            Ok(_) => self.emit(CoreEvent::LocalVideo(local)).await,
            Err(e) => self.emit(CoreEvent::Error(e)).await,
        }
        self.report_encoder_change(before).await;
    }

    /// Our hardware encoder for `kind` stopped or was replaced: the track is
    /// sent again on the software path, once (plan Task 6).
    pub(super) async fn video_encoder_lost(&mut self, kind: VideoKind) {
        let Some((quality, backend)) = self.call.as_ref().and_then(|c| c.video_sending(kind))
        else {
            return;
        };
        if backend == EncoderBackend::Software {
            return;
        }
        self.send_video(kind, quality, EncoderBackend::Software)
            .await;
    }

    /// The encoder our video uses now: the camera's, else the screen's.
    fn video_in_use(&self) -> Option<EncoderBackend> {
        let call = self.call.as_ref()?;
        call.video_sending(VideoKind::Camera)
            .or_else(|| call.video_sending(VideoKind::Screen))
            .map(|(_, backend)| backend)
    }

    /// Tells the front end when the encoder in use is no longer `before`.
    async fn report_encoder_change(&mut self, before: Option<EncoderBackend>) {
        let now = self.video_in_use();
        if now != before {
            let encoders = self.video_encoders().await.encoders(now);
            self.emit(CoreEvent::VideoEncoders(encoders)).await;
        }
    }

    /// What LiveKit can encode with here, found once (off the engine's
    /// thread: the probe opens the GPU drivers).
    async fn video_encoders(&mut self) -> &VideoEncodersAvailable {
        if self.video_encoders.is_none() {
            let found = blocking(VideoEncodersAvailable::detect)
                .await
                .unwrap_or_default();
            self.video_encoders = Some(found);
        }
        self.video_encoders.as_ref().expect("found above")
    }

    /// The video settings, loaded from disk on first use. A file that cannot
    /// be used is reported once and replaced by the defaults in memory.
    async fn video_settings(&mut self) -> &VideoSettings {
        if self.video_settings.is_none() {
            let paths = self.config.data.clone();
            let loaded = blocking(move || video_settings::load(&paths))
                .await
                .unwrap_or_else(|e| video_settings::Loaded {
                    settings: VideoSettings::default(),
                    problem: Some(e),
                });
            if let Some(problem) = loaded.problem {
                self.emit(CoreEvent::Error(problem)).await;
            }
            self.video_settings = Some(loaded.settings);
        }
        self.video_settings.as_ref().expect("loaded above")
    }

    /// Adopts and saves new settings, and echoes them back. A failed save is
    /// reported; the settings still apply for this run. Tracks being sent
    /// keep their encoder until they are started again.
    async fn store_video_settings(&mut self, settings: VideoSettings) {
        let paths = self.config.data.clone();
        let to_save = settings.clone();
        let saved = blocking(move || video_settings::save(&paths, &to_save))
            .await
            .and_then(|r| r);
        self.video_settings = Some(settings.clone());
        if let Err(e) = saved {
            self.emit(CoreEvent::Error(e)).await;
        }
        self.emit(CoreEvent::VideoSettings(settings)).await;
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
