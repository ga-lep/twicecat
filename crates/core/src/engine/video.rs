//! The engine's video commands (Phase 9). The test pattern stands for the
//! camera and the screen with [`VideoBackend::TestPattern`]; with
//! [`VideoBackend::System`] the screen is captured (Task 8: the ScreenCast
//! portal on Wayland, our own picker and MIT-SHM on X11, with the sound of
//! other applications) and cameras answer `NotSupported` until Task 9, as
//! do previews. Either is encoded by the encoder the settings and this
//! machine give (Task 6).
//!
//! The portal's picker can stay open for as long as the person likes, so it
//! runs in a task the engine's loop waits on beside its commands
//! ([`next_capture_event`]); the share is published when it answers.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use twicecat_video::capture::{CaptureEnd, CaptureError};
#[cfg(target_os = "linux")]
use twicecat_video::capture::{ScreenBackend, portal, x11};

use super::{Engine, blocking};
use crate::api::{
    Command, CoreError, CoreEvent, EncoderBackend, LocalVideo, RoomId, ScreenSource,
    ScreenSourceKind, ScreenSources, TEST_PATTERN_ID, VideoAdjustment, VideoBackend, VideoDevice,
    VideoDeviceId, VideoDevices, VideoKind, VideoMode, VideoQuality, VideoSettings,
};
#[cfg(target_os = "linux")]
use crate::call::{Capture, CaptureSink, PusherSlot, next_serial};
use crate::call::{CaptureEnded, Captured, Sending, VideoEncodersAvailable, VideoSource};
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

/// A capture failure as the front end hears it; `None` when the person
/// cancelled (nothing to say).
fn capture_error(e: &CaptureError) -> Option<CoreError> {
    match e {
        CaptureError::Cancelled => None,
        CaptureError::NoSuchSource => Some(CoreError::Call(
            "that screen or window is not available".into(),
        )),
        CaptureError::Unavailable(why) => Some(CoreError::Call(format!(
            "screen sharing is not available here ({why})"
        ))),
        CaptureError::Failed(why) => {
            Some(CoreError::Call(format!("screen sharing failed ({why})")))
        }
    }
}

/// How big a thumbnail in our own picker is at most.
#[cfg(target_os = "linux")]
const THUMBNAIL_MAX: (u32, u32) = (320, 180);

/// An X11 source for the front end; its thumbnail as RGBA.
#[cfg(target_os = "linux")]
fn x11_source(source: x11::X11Source) -> ScreenSource {
    let thumbnail = source.thumbnail.as_ref().map(|t| {
        let plane = &t.planes()[0];
        let mut rgba = Vec::with_capacity(t.width() as usize * t.height() as usize * 4);
        for row in 0..t.height() as usize {
            let line = &plane.data[row * plane.stride..][..t.width() as usize * 4];
            for px in line.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        crate::api::ScreenThumbnail {
            width: t.width(),
            height: t.height(),
            rgba: rgba.into(),
        }
    });
    ScreenSource {
        id: source.id.into(),
        kind: match source.kind {
            x11::X11SourceKind::Monitor => ScreenSourceKind::Monitor,
            x11::X11SourceKind::Window => ScreenSourceKind::Window,
        },
        title: source.title,
        width: source.width,
        height: source.height,
        thumbnail,
    }
}

/// A portal share being opened: the picker is up.
struct PendingShare {
    task: JoinHandle<Result<(Captured, Option<String>), CaptureError>>,
    quality: VideoQuality,
    audio: bool,
    sending: Sending,
    changes: Vec<VideoAdjustment>,
}

/// The engine's captures: ends reported by capturers, and a portal share
/// waiting for the picker.
pub(super) struct Captures {
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "capturers are Linux only so far")
    )]
    ended_tx: mpsc::UnboundedSender<CaptureEnded>,
    ended: mpsc::UnboundedReceiver<CaptureEnded>,
    pending: Option<PendingShare>,
}

impl Default for Captures {
    fn default() -> Self {
        let (ended_tx, ended) = mpsc::unbounded_channel();
        Self {
            ended_tx,
            ended,
            pending: None,
        }
    }
}

/// Something a capture has to say.
pub(super) enum CaptureEvent {
    /// A capture stopped by itself.
    Ended(CaptureEnded),
    /// The portal's picker was answered (or its task failed).
    Opened(Result<Result<(Captured, Option<String>), CaptureError>, tokio::task::JoinError>),
}

/// The next capture event; never resolves while there is nothing to wait
/// for. Safe to drop and call again.
pub(super) async fn next_capture_event(captures: &mut Captures) -> CaptureEvent {
    let pending = async {
        match captures.pending.as_mut() {
            Some(p) => (&mut p.task).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        Some(ended) = captures.ended.recv() => CaptureEvent::Ended(ended),
        opened = pending => CaptureEvent::Opened(opened),
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
            Command::ListScreenSources => self.list_screen_sources().await,
            Command::StartCamera { device, quality } => {
                let id = device.as_ref().map(VideoDeviceId::as_str);
                self.start_pattern(VideoKind::Camera, id, quality).await;
            }
            Command::StartScreenShare {
                source,
                quality,
                audio,
            } if pattern => {
                if audio {
                    // The pattern has no sound.
                    self.not_supported("screen-share sound").await;
                    return;
                }
                let id = source.as_ref().map(|s| s.as_str());
                self.start_pattern(VideoKind::Screen, id, quality).await;
            }
            Command::StartScreenShare {
                source,
                quality,
                audio,
            } => {
                let id = source.as_ref().map(|s| s.as_str().to_owned());
                self.start_screen_share(id, quality, audio).await;
            }
            Command::StopCamera => self.stop_video(VideoKind::Camera).await,
            Command::StopScreenShare => {
                if let Some(pending) = self.captures.pending.take() {
                    // The picker is still up: closing the request is the
                    // answer (the portal session goes with the task).
                    pending.task.abort();
                }
                self.stop_video(VideoKind::Screen).await;
            }
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

    /// What can be shared: the system's picker on Wayland, our own list
    /// (with pictures) on X11.
    async fn list_screen_sources(&mut self) {
        #[cfg(target_os = "linux")]
        match ScreenBackend::detect() {
            Some(ScreenBackend::Portal) => {
                let sources = ScreenSources {
                    system_picker: true,
                    sources: Vec::new(),
                };
                self.emit(CoreEvent::ScreenSources(sources)).await;
                return;
            }
            Some(ScreenBackend::X11) => {
                match blocking(|| x11::list_sources(THUMBNAIL_MAX)).await {
                    Ok(Ok(found)) => {
                        let sources = ScreenSources {
                            system_picker: false,
                            sources: found.into_iter().map(x11_source).collect(),
                        };
                        self.emit(CoreEvent::ScreenSources(sources)).await;
                    }
                    Ok(Err(e)) => {
                        if let Some(e) = capture_error(&e) {
                            self.emit(CoreEvent::Error(e)).await;
                        }
                    }
                    Err(e) => self.emit(CoreEvent::Error(e)).await,
                }
                return;
            }
            None => {}
        }
        self.not_supported(feature(VideoKind::Screen)).await;
    }

    /// Sends the test pattern as our camera or screen share in the current
    /// call.
    async fn start_pattern(&mut self, kind: VideoKind, id: Option<&str>, quality: VideoQuality) {
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
        let (sending, changes) = self.plan_video(kind, quality, None).await;
        self.publish_video(kind, quality, sending, changes, VideoSource::Pattern)
            .await;
    }

    /// How `kind` at `quality` is sent: the encoder the settings and this
    /// machine give (or `backend`), and what the settings ask, with what the
    /// encoder could not do.
    async fn plan_video(
        &mut self,
        kind: VideoKind,
        quality: VideoQuality,
        backend: Option<EncoderBackend>,
    ) -> (Sending, Vec<VideoAdjustment>) {
        let settings = self.video_settings().await.clone();
        let backend = match backend {
            Some(backend) => backend,
            None => {
                let choice = settings.tuning(kind).encoder.unwrap_or(settings.encoder);
                self.video_encoders().await.choose(choice)
            }
        };
        crate::call::tuned(
            backend,
            kind,
            quality,
            settings.tuning(kind),
            settings.codec,
        )
    }

    /// Shares the screen: the portal's picker (Wayland) or the source picked
    /// in our own list (X11), with the sound of other applications when
    /// `audio`.
    async fn start_screen_share(&mut self, id: Option<String>, quality: VideoQuality, audio: bool) {
        if self.call.is_none() {
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return;
        }
        #[cfg(target_os = "linux")]
        match ScreenBackend::detect() {
            Some(ScreenBackend::Portal) => {
                self.open_portal_share(quality, audio).await;
                return;
            }
            Some(ScreenBackend::X11) => {
                self.start_x11_share(id, quality, audio).await;
                return;
            }
            None => {}
        }
        let _ = (id, quality, audio);
        self.not_supported(feature(VideoKind::Screen)).await;
    }

    /// Asks the portal for a screen or window; the share is published when
    /// the person has picked ([`Engine::on_capture_event`]).
    #[cfg(target_os = "linux")]
    async fn open_portal_share(&mut self, quality: VideoQuality, audio: bool) {
        if self.captures.pending.is_some() {
            // The picker is up already.
            self.emit(CoreEvent::Error(CoreError::Busy)).await;
            return;
        }
        let (sending, changes) = self.plan_video(VideoKind::Screen, quality, None).await;
        let remember = self.video_settings().await.remember_screen_choice;
        let paths = self.config.data.clone();
        let restore_token = if remember {
            blocking(move || paths.read_screen_restore_token())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let (width, height, fps) = sending.mode();
        let request = portal::ShareRequest {
            remember,
            restore_token,
            max_size: (width, height),
            max_fps: fps,
        };
        let slot = PusherSlot::default();
        let serial = next_serial();
        let sink = CaptureSink {
            slot: slot.clone(),
            kind: VideoKind::Screen,
            serial,
            ended: self.captures.ended_tx.clone(),
        };
        let task = tokio::spawn(async move {
            let share = portal::ScreenShare::start(&request, Box::new(sink)).await?;
            let token = share.restore_token().map(str::to_owned);
            let size = share.size();
            Ok((
                Captured {
                    capture: Capture::Portal(share),
                    slot,
                    serial,
                    size,
                },
                token,
            ))
        });
        self.captures.pending = Some(PendingShare {
            task,
            quality,
            audio,
            sending,
            changes,
        });
    }

    /// Captures the X11 source `id` (from `ListScreenSources`) and shares it.
    #[cfg(target_os = "linux")]
    async fn start_x11_share(&mut self, id: Option<String>, quality: VideoQuality, audio: bool) {
        let Some(id) = id else {
            self.emit(CoreEvent::Error(CoreError::Call(
                "pick a screen or window to share".into(),
            )))
            .await;
            return;
        };
        let (sending, changes) = self.plan_video(VideoKind::Screen, quality, None).await;
        let fps = sending.mode().2;
        let slot = PusherSlot::default();
        let serial = next_serial();
        let sink = CaptureSink {
            slot: slot.clone(),
            kind: VideoKind::Screen,
            serial,
            ended: self.captures.ended_tx.clone(),
        };
        let started = blocking(move || x11::X11Capture::start(&id, fps, Box::new(sink))).await;
        let capture = match started {
            Ok(Ok(capture)) => capture,
            Ok(Err(e)) => {
                if let Some(e) = capture_error(&e) {
                    self.emit(CoreEvent::Error(e)).await;
                }
                return;
            }
            Err(e) => {
                self.emit(CoreEvent::Error(e)).await;
                return;
            }
        };
        let captured = Captured {
            size: Some(capture.size()),
            capture: Capture::X11(capture),
            slot,
            serial,
        };
        self.publish_share(quality, audio, sending, changes, captured)
            .await;
    }

    /// Publishes a started screen capture as our share (replacing the one
    /// we send), then its sound when asked.
    async fn publish_share(
        &mut self,
        quality: VideoQuality,
        audio: bool,
        sending: Sending,
        changes: Vec<VideoAdjustment>,
        captured: Captured,
    ) {
        if let Some(call) = self.call.as_mut() {
            call.stop_video(VideoKind::Screen).await;
        }
        let published = self
            .publish_video(
                VideoKind::Screen,
                quality,
                sending,
                changes,
                VideoSource::Captured(captured),
            )
            .await;
        if !published || !audio {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let Some(call) = self.call.as_mut() else {
                return;
            };
            match call.start_share_sound().await {
                Ok(()) => {
                    let local = call.local_video();
                    self.emit(CoreEvent::LocalVideo(local)).await;
                }
                // The picture goes on without its sound.
                Err(e) => self.emit(CoreEvent::Error(e)).await,
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.not_supported("screen-share sound").await;
    }

    /// Reacts to a capture: one that ended by itself stops its share; the
    /// portal's answer publishes the share (or says why not).
    pub(super) async fn on_capture_event(&mut self, event: CaptureEvent) {
        match event {
            CaptureEvent::Ended((kind, serial, end)) => {
                let Some(call) = self.call.as_mut() else {
                    return;
                };
                // An older capture of a share since replaced is no news.
                if call.capture_serial(kind) != Some(serial) {
                    return;
                }
                call.stop_video(kind).await;
                let local = call.local_video();
                self.emit(CoreEvent::LocalVideo(local)).await;
                if end == CaptureEnd::Lost {
                    let error =
                        CoreError::Call("screen sharing stopped: the capture failed".into());
                    self.emit(CoreEvent::Error(error)).await;
                }
            }
            CaptureEvent::Opened(result) => {
                let Some(pending) = self.captures.pending.take() else {
                    return;
                };
                let opened = match result {
                    Ok(Ok(opened)) => opened,
                    Ok(Err(e)) => {
                        self.share_not_started(capture_error(&e)).await;
                        return;
                    }
                    // Aborted by `StopScreenShare`, which answered already.
                    Err(e) if e.is_cancelled() => return,
                    Err(_) => {
                        let error = CoreError::Internal("the screen picker task failed".into());
                        self.share_not_started(Some(error)).await;
                        return;
                    }
                };
                let (captured, token) = opened;
                self.keep_restore_token(token).await;
                if self.call.is_none() {
                    // The call ended while the picker was up.
                    captured.capture.stop().await;
                    return;
                }
                self.publish_share(
                    pending.quality,
                    pending.audio,
                    pending.sending,
                    pending.changes,
                    captured,
                )
                .await;
            }
        }
    }

    /// The share did not start: says why (nothing when cancelled), and what
    /// we send now, so the front end stops waiting.
    async fn share_not_started(&mut self, error: Option<CoreError>) {
        if let Some(error) = error {
            self.emit(CoreEvent::Error(error)).await;
        }
        let local = self
            .call
            .as_ref()
            .map(|c| c.local_video())
            .unwrap_or_default();
        self.emit(CoreEvent::LocalVideo(local)).await;
    }

    /// Keeps the portal's restore token while "remember" is on, and forgets
    /// any otherwise. A failure only means the picker shows next time.
    async fn keep_restore_token(&mut self, token: Option<String>) {
        let remember = self.video_settings().await.remember_screen_choice;
        let paths = self.config.data.clone();
        let _ = blocking(move || match token.filter(|_| remember) {
            Some(token) => paths.write_screen_restore_token(&token),
            None if remember => Ok(()),
            None => paths.remove_screen_restore_token(),
        })
        .await;
    }

    /// Publishes our `kind` from `source` as `sending` says, and says what
    /// is sent now (and which encoder, when that changed). `true` when it
    /// is sent.
    async fn publish_video(
        &mut self,
        kind: VideoKind,
        quality: VideoQuality,
        sending: Sending,
        changes: Vec<VideoAdjustment>,
        source: VideoSource,
    ) -> bool {
        let before = self.video_in_use();
        let Some(call) = self.call.as_mut() else {
            if let VideoSource::Captured(c) = source {
                c.capture.stop().await;
            }
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return false;
        };
        let result = call.start_video(kind, quality, &sending, source).await;
        let local = call.local_video();
        let sent = result.is_ok();
        match result {
            Ok(_) => {
                self.emit(CoreEvent::LocalVideo(local)).await;
                // What is sent is not what the settings say: say so.
                if !changes.is_empty() {
                    self.emit(CoreEvent::VideoSending { kind, changes }).await;
                }
            }
            Err(e) => self.emit(CoreEvent::Error(e)).await,
        }
        self.report_encoder_change(before).await;
        sent
    }

    /// Our hardware encoder for `kind` stopped or was replaced: the track is
    /// sent again on the software path, once (plan Task 6), from the same
    /// source (a capture keeps running).
    pub(super) async fn video_encoder_lost(&mut self, kind: VideoKind) {
        let Some((quality, backend)) = self.call.as_ref().and_then(|c| c.video_sending(kind))
        else {
            return;
        };
        if backend == EncoderBackend::Software {
            return;
        }
        let before = self.video_in_use();
        let (sending, changes) = self
            .plan_video(kind, quality, Some(EncoderBackend::Software))
            .await;
        let Some(call) = self.call.as_mut() else {
            return;
        };
        let result = call.resend_video(kind, quality, &sending).await;
        let local = call.local_video();
        match result {
            Ok(_) => {
                self.emit(CoreEvent::LocalVideo(local)).await;
                if !changes.is_empty() {
                    self.emit(CoreEvent::VideoSending { kind, changes }).await;
                }
            }
            Err(e) => {
                self.emit(CoreEvent::Error(e)).await;
                self.emit(CoreEvent::LocalVideo(local)).await;
            }
        }
        self.report_encoder_change(before).await;
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
            // A file edited by hand may hold values out of bounds.
            let (settings, changes) = loaded.settings.validated();
            if !changes.is_empty() {
                self.emit(CoreEvent::VideoSettingsAdjusted(changes)).await;
            }
            self.video_settings = Some(settings);
        }
        self.video_settings.as_ref().expect("loaded above")
    }

    /// Adopts and saves new settings, and echoes them back. A failed save is
    /// reported; the settings still apply for this run. Tracks being sent
    /// keep their encoder until they are started again. Turning "remember
    /// my screen choice" off forgets the remembered choice.
    async fn store_video_settings(&mut self, settings: VideoSettings) {
        // Out-of-bounds values are brought inside them, and said.
        let (settings, changes) = settings.validated();
        let paths = self.config.data.clone();
        let to_save = settings.clone();
        let saved = blocking(move || {
            if !to_save.remember_screen_choice {
                // Nothing to report: a token that stays only shows no picker
                // while "remember" is off, which it is never read for.
                let _ = paths.remove_screen_restore_token();
            }
            video_settings::save(&paths, &to_save)
        })
        .await
        .and_then(|r| r);
        self.video_settings = Some(settings.clone());
        if let Err(e) = saved {
            self.emit(CoreEvent::Error(e)).await;
        }
        self.emit(CoreEvent::VideoSettings(settings)).await;
        if !changes.is_empty() {
            self.emit(CoreEvent::VideoSettingsAdjusted(changes)).await;
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

    /// After a call with video ended: no tracks, nothing sent, no picker
    /// waiting.
    pub(super) async fn video_ended(&mut self, room_id: RoomId, had: (bool, LocalVideo)) {
        if let Some(pending) = self.captures.pending.take() {
            pending.task.abort();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancelled_picker_is_not_an_error() {
        assert_eq!(capture_error(&CaptureError::Cancelled), None);
        assert!(matches!(
            capture_error(&CaptureError::Unavailable("no screen-cast portal")),
            Some(CoreError::Call(why)) if why.contains("no screen-cast portal")
        ));
        assert!(matches!(
            capture_error(&CaptureError::NoSuchSource),
            Some(CoreError::Call(_))
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn x11_thumbnails_reach_the_front_end_as_rgba() {
        let frame = twicecat_video::CpuFrame::new(
            twicecat_video::PixelFormat::Bgra,
            2,
            1,
            vec![twicecat_video::Plane {
                // Blue, then red, as B, G, R, x.
                data: vec![255, 0, 0, 0, 0, 0, 255, 0].into(),
                stride: 8,
            }],
            twicecat_video::ColorSpace::default(),
            std::time::Duration::ZERO,
        )
        .expect("frame");
        let source = x11_source(x11::X11Source {
            id: "x11-window:7".into(),
            kind: x11::X11SourceKind::Window,
            title: "A window".into(),
            width: 2,
            height: 1,
            thumbnail: Some(frame),
        });
        assert_eq!(source.kind, ScreenSourceKind::Window);
        let thumbnail = source.thumbnail.expect("thumbnail");
        assert_eq!(&thumbnail.rgba[..], &[0, 0, 255, 255, 255, 0, 0, 255]);
    }
}
