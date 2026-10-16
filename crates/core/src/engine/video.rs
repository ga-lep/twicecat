//! The engine's video commands (Phase 9). The test pattern stands for the
//! camera and the screen with [`VideoBackend::TestPattern`]; with
//! [`VideoBackend::System`] the screen is captured (Task 8: the ScreenCast
//! portal on Wayland, our own picker and MIT-SHM on X11, with the sound of
//! other applications) and so are cameras (Task 9: PipeWire's camera nodes,
//! through the Camera portal inside Flatpak, in the mode the settings name
//! or the best for the quality; a preview without a call; cameras that
//! come and go are said). Either is encoded by the encoder the settings and
//! this machine give (Task 6).
//!
//! The portal's picker can stay open for as long as the person likes, so it
//! runs in a task the engine's loop waits on beside its commands
//! ([`next_capture_event`]); the share is published when it answers.

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
#[cfg(target_os = "linux")]
use twicecat_video::capture::camera::{self, CameraInfo, CameraMode, CameraWatcher};
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
use crate::call::{Capture, CaptureSink, PreviewSink, PusherSlot, next_serial};
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
fn capture_error(kind: VideoKind, e: &CaptureError) -> Option<CoreError> {
    let what = match kind {
        VideoKind::Camera => "the camera",
        VideoKind::Screen => "screen sharing",
    };
    match e {
        CaptureError::Cancelled => None,
        CaptureError::NoSuchSource => Some(CoreError::Call(
            match kind {
                VideoKind::Camera => "that camera is not available",
                VideoKind::Screen => "that screen or window is not available",
            }
            .into(),
        )),
        CaptureError::Unavailable(why) => Some(CoreError::Call(format!(
            "{what} is not available here ({why})"
        ))),
        CaptureError::Failed(why) => Some(CoreError::Call(format!("{what} failed ({why})"))),
    }
}

/// What the front end hears when a capture stopped by itself: a camera
/// unplugged or taken away, any capture that failed (a share the person
/// stopped from the compositor is no news).
fn ended_error(kind: VideoKind, end: CaptureEnd) -> Option<CoreError> {
    let why = match (kind, end) {
        (VideoKind::Screen, CaptureEnd::Stopped) => return None,
        (VideoKind::Screen, CaptureEnd::Lost) => "screen sharing stopped: the capture failed",
        (VideoKind::Camera, CaptureEnd::Stopped) => "the camera was disconnected",
        (VideoKind::Camera, CaptureEnd::Lost) => "the camera stopped: the capture failed",
    };
    Some(CoreError::Call(why.into()))
}

/// A camera for the front end: its modes as sizes and rates (each once,
/// largest first; the pixel format is a setting of its own).
#[cfg(target_os = "linux")]
fn video_device(camera: &CameraInfo) -> VideoDevice {
    let mut modes: Vec<VideoMode> = Vec::new();
    for m in &camera.modes {
        let mode = VideoMode {
            width: m.width,
            height: m.height,
            fps: m.fps,
        };
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    VideoDevice {
        id: VideoDeviceId::from(camera.id.as_str()),
        name: camera.name.clone(),
        is_default: camera.is_default,
        modes,
    }
}

/// The camera to open: the one asked for, else the one the settings name
/// when it is here, else the system's default.
#[cfg(target_os = "linux")]
fn pick_camera<'a>(
    cameras: &'a [CameraInfo],
    asked: Option<&str>,
    saved: Option<&str>,
) -> Result<&'a CameraInfo, CoreError> {
    if let Some(asked) = asked {
        return cameras
            .iter()
            .find(|c| c.id == asked)
            .ok_or_else(|| CoreError::Call("that camera is not available".into()));
    }
    saved
        .and_then(|saved| cameras.iter().find(|c| c.id == saved))
        .or_else(|| cameras.iter().find(|c| c.is_default))
        .or_else(|| cameras.first())
        .ok_or_else(|| CoreError::Call("no camera was found".into()))
}

/// The mode to open `camera` in to send `(width, height, fps)`, with the
/// capture the settings ask.
#[cfg(target_os = "linux")]
fn camera_mode(
    camera: &CameraInfo,
    target: (u32, u32, u32),
    capture: crate::api::CameraCapture,
) -> Result<CameraMode, CoreError> {
    use crate::api::CameraFormat as Asked;
    use twicecat_video::capture::camera::CameraFormat;
    let format = match capture.format {
        Asked::Auto => None,
        Asked::Nv12 => Some(CameraFormat::Nv12),
        Asked::Yuyv => Some(CameraFormat::Yuyv),
        Asked::Mjpeg => Some(CameraFormat::Mjpeg),
    };
    let wanted = capture.mode.map(|m| (m.width, m.height, m.fps));
    camera::choose_mode(&camera.modes, target, wanted, format)
        .ok_or_else(|| CoreError::Call("that camera sends nothing we can read".into()))
}

/// The camera preview: its camera, and the serial its end comes with.
#[cfg(target_os = "linux")]
struct Preview {
    camera: camera::Camera,
    serial: crate::call::CaptureSerial,
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

/// The engine's captures: ends reported by capturers, a portal share
/// waiting for the picker, the camera preview, and cameras coming and going.
pub(super) struct Captures {
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "capturers are Linux only so far")
    )]
    ended_tx: mpsc::UnboundedSender<CaptureEnded>,
    ended: mpsc::UnboundedReceiver<CaptureEnded>,
    pending: Option<PendingShare>,
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "cameras are Linux only so far")
    )]
    cameras_tx: mpsc::UnboundedSender<()>,
    cameras_changed: mpsc::UnboundedReceiver<()>,
    /// Started by the first camera listing.
    #[cfg(target_os = "linux")]
    watcher: Option<CameraWatcher>,
    #[cfg(target_os = "linux")]
    preview: Option<Preview>,
}

impl Default for Captures {
    fn default() -> Self {
        let (ended_tx, ended) = mpsc::unbounded_channel();
        let (cameras_tx, cameras_changed) = mpsc::unbounded_channel();
        Self {
            ended_tx,
            ended,
            pending: None,
            cameras_tx,
            cameras_changed,
            #[cfg(target_os = "linux")]
            watcher: None,
            #[cfg(target_os = "linux")]
            preview: None,
        }
    }
}

/// Something a capture has to say.
pub(super) enum CaptureEvent {
    /// A capture stopped by itself.
    Ended(CaptureEnded),
    /// A camera came or went.
    CamerasChanged,
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
        Some(()) = captures.cameras_changed.recv() => {
            // A device often adds or removes several nodes at once: one
            // listing for all.
            while captures.cameras_changed.try_recv().is_ok() {}
            CaptureEvent::CamerasChanged
        }
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
            Command::StartCamera { device, quality } if pattern => {
                let id = device.as_ref().map(VideoDeviceId::as_str);
                self.start_pattern(VideoKind::Camera, id, quality).await;
            }
            Command::StartCamera { device, quality } => {
                self.start_camera(device, quality).await;
            }
            Command::ListVideoDevices => self.list_cameras().await,
            Command::StartCameraPreview { .. } if pattern => {
                self.not_supported("camera previews of the test pattern")
                    .await;
            }
            Command::StartCameraPreview { device, quality } => {
                self.start_preview(device, quality).await;
            }
            Command::StopCameraPreview => self.stop_preview().await,
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
                        if let Some(e) = capture_error(VideoKind::Screen, &e) {
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
                if let Some(e) = capture_error(VideoKind::Screen, &e) {
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
                #[cfg(target_os = "linux")]
                if kind == VideoKind::Camera
                    && self
                        .captures
                        .preview
                        .as_ref()
                        .is_some_and(|p| p.serial == serial)
                {
                    // The preview's camera: its feed ends with it.
                    self.stop_preview().await;
                    if let Some(error) = ended_error(kind, end) {
                        self.emit(CoreEvent::Error(error)).await;
                    }
                    return;
                }
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
                if let Some(error) = ended_error(kind, end) {
                    self.emit(CoreEvent::Error(error)).await;
                }
            }
            CaptureEvent::CamerasChanged =>
            {
                #[cfg(target_os = "linux")]
                if let Ok(cameras) = self.find_cameras().await {
                    let cameras = cameras.iter().map(video_device).collect();
                    self.emit(CoreEvent::VideoDevices(VideoDevices { cameras }))
                        .await;
                }
            }
            CaptureEvent::Opened(result) => {
                let Some(pending) = self.captures.pending.take() else {
                    return;
                };
                let opened = match result {
                    Ok(Ok(opened)) => opened,
                    Ok(Err(e)) => {
                        self.share_not_started(capture_error(VideoKind::Screen, &e))
                            .await;
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

    /// The cameras here, with their modes (blocking work off the engine's
    /// threads).
    #[cfg(target_os = "linux")]
    async fn find_cameras(&mut self) -> Result<Vec<CameraInfo>, CoreError> {
        let remote = camera::remote()
            .await
            .map_err(|e| capture_error(VideoKind::Camera, &e).unwrap_or(CoreError::Cancelled))?;
        blocking(move || camera::list(remote))
            .await?
            .map_err(|e| capture_error(VideoKind::Camera, &e).unwrap_or(CoreError::Cancelled))
    }

    /// Lists the cameras, and from the first listing on says when one comes
    /// or goes (`VideoDevices` again).
    async fn list_cameras(&mut self) {
        #[cfg(target_os = "linux")]
        {
            match self.find_cameras().await {
                Ok(cameras) => {
                    let cameras = cameras.iter().map(video_device).collect();
                    self.emit(CoreEvent::VideoDevices(VideoDevices { cameras }))
                        .await;
                }
                Err(e) => {
                    self.emit(CoreEvent::Error(e)).await;
                    return;
                }
            }
            if self.captures.watcher.is_none() {
                let changed = self.captures.cameras_tx.clone();
                let started = match camera::remote().await {
                    Ok(remote) => blocking(move || {
                        CameraWatcher::start(
                            remote,
                            Box::new(move || {
                                let _ = changed.send(());
                            }),
                        )
                    })
                    .await
                    .ok()
                    .and_then(Result::ok),
                    Err(_) => None,
                };
                if started.is_none() {
                    // The list stands; only hot-plug is missing.
                    tracing::info!("cameras coming and going will not be noticed");
                }
                self.captures.watcher = started;
            }
        }
        #[cfg(not(target_os = "linux"))]
        self.not_supported(feature(VideoKind::Camera)).await;
    }

    /// Opens the camera `device` (the settings' or the default when `None`)
    /// in the mode for `quality` (or the settings'), with frames to `sink`
    /// made from the serial.
    #[cfg(target_os = "linux")]
    async fn open_camera(
        &mut self,
        device: Option<VideoDeviceId>,
        target: (u32, u32, u32),
        sink: impl FnOnce(crate::call::CaptureSerial) -> Box<dyn twicecat_video::capture::FrameSink>,
    ) -> Result<(camera::Camera, CameraMode, crate::call::CaptureSerial), CoreError> {
        let settings = self.video_settings().await.clone();
        let cameras = self.find_cameras().await?;
        let chosen = pick_camera(
            &cameras,
            device.as_ref().map(VideoDeviceId::as_str),
            settings.camera.as_ref().map(VideoDeviceId::as_str),
        )?;
        let mode = camera_mode(chosen, target, settings.camera_capture)?;
        let id = chosen.id.clone();
        let serial = next_serial();
        let sink = sink(serial);
        let remote = camera::remote()
            .await
            .map_err(|e| capture_error(VideoKind::Camera, &e).unwrap_or(CoreError::Cancelled))?;
        let opened = blocking(move || camera::Camera::open(remote, &id, mode, sink)).await?;
        let camera = opened
            .map_err(|e| capture_error(VideoKind::Camera, &e).unwrap_or(CoreError::Cancelled))?;
        Ok((camera, mode, serial))
    }

    /// Sends a camera as our camera in the current call (replacing the one
    /// we send, and the preview: a camera streams in one mode at a time).
    async fn start_camera(&mut self, device: Option<VideoDeviceId>, quality: VideoQuality) {
        if self.call.is_none() {
            self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
            return;
        }
        #[cfg(target_os = "linux")]
        {
            self.stop_preview().await;
            if let Some(call) = self.call.as_mut() {
                call.stop_video(VideoKind::Camera).await;
            }
            let (sending, changes) = self.plan_video(VideoKind::Camera, quality, None).await;
            let slot = PusherSlot::default();
            let ended = self.captures.ended_tx.clone();
            let opened = self
                .open_camera(device, sending.mode(), |serial| {
                    Box::new(CaptureSink {
                        slot: slot.clone(),
                        kind: VideoKind::Camera,
                        serial,
                        ended,
                    })
                })
                .await;
            let (camera, mode, serial) = match opened {
                Ok(opened) => opened,
                Err(e) => {
                    self.share_not_started(Some(e)).await;
                    return;
                }
            };
            let captured = Captured {
                capture: Capture::Camera(camera),
                slot,
                serial,
                size: Some((mode.width, mode.height)),
            };
            self.publish_video(
                VideoKind::Camera,
                quality,
                sending,
                changes,
                VideoSource::Captured(captured),
            )
            .await;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (device, quality);
            self.not_supported(feature(VideoKind::Camera)).await;
        }
    }

    /// Shows a camera without sending it (`CameraPreview`), in the mode
    /// `quality` would send; replaces a preview already shown.
    async fn start_preview(&mut self, device: Option<VideoDeviceId>, quality: VideoQuality) {
        #[cfg(target_os = "linux")]
        {
            self.stop_preview().await;
            // The mode the camera would send in at that quality.
            let target = self
                .plan_video(VideoKind::Camera, quality, None)
                .await
                .0
                .mode();
            let (tx, feed) = crate::api::VideoFeed::channel();
            let ended = self.captures.ended_tx.clone();
            let opened = self
                .open_camera(device, target, |serial| {
                    Box::new(PreviewSink {
                        feed: tx,
                        serial,
                        ended,
                    })
                })
                .await;
            match opened {
                Ok((camera, _, serial)) => {
                    self.captures.preview = Some(Preview { camera, serial });
                    self.emit(CoreEvent::CameraPreview(feed)).await;
                }
                Err(e) => self.emit(CoreEvent::Error(e)).await,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (device, quality);
            self.not_supported(feature(VideoKind::Camera)).await;
        }
    }

    /// Stops the preview (its feed ends); nothing to do without one.
    async fn stop_preview(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(preview) = self.captures.preview.take() {
            let _ = blocking(move || drop(preview.camera)).await;
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
        // A preview needs no call: it stays.
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
        assert_eq!(
            capture_error(VideoKind::Screen, &CaptureError::Cancelled),
            None
        );
        assert!(matches!(
            capture_error(VideoKind::Screen, &CaptureError::Unavailable("no screen-cast portal")),
            Some(CoreError::Call(why)) if why.contains("no screen-cast portal")
        ));
        assert!(matches!(
            capture_error(VideoKind::Camera, &CaptureError::NoSuchSource),
            Some(CoreError::Call(why)) if why.contains("camera")
        ));
    }

    #[test]
    fn a_camera_that_stops_by_itself_is_said_a_share_stopped_is_not() {
        assert_eq!(ended_error(VideoKind::Screen, CaptureEnd::Stopped), None);
        assert!(matches!(
            ended_error(VideoKind::Camera, CaptureEnd::Stopped),
            Some(CoreError::Call(why)) if why.contains("disconnected")
        ));
        assert!(ended_error(VideoKind::Screen, CaptureEnd::Lost).is_some());
        assert!(ended_error(VideoKind::Camera, CaptureEnd::Lost).is_some());
    }

    #[cfg(target_os = "linux")]
    fn cameras() -> Vec<CameraInfo> {
        use twicecat_video::capture::camera::CameraFormat;
        let mode = |width, height, fps, format| CameraMode {
            width,
            height,
            fps,
            format,
        };
        vec![
            CameraInfo {
                id: "front".into(),
                name: "Front".into(),
                is_default: false,
                modes: vec![
                    mode(1920, 1080, 30, CameraFormat::Mjpeg),
                    mode(1280, 720, 30, CameraFormat::Mjpeg),
                    mode(1280, 720, 30, CameraFormat::Yuyv),
                ],
            },
            CameraInfo {
                id: "built-in".into(),
                name: "Built-in".into(),
                is_default: true,
                modes: vec![mode(640, 480, 30, CameraFormat::Yuyv)],
            },
        ]
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_camera_asked_wins_then_the_saved_one_then_the_default() {
        let cameras = cameras();
        let id = |r: Result<&CameraInfo, CoreError>| r.map(|c| c.id.clone());
        assert_eq!(
            id(pick_camera(&cameras, Some("front"), None)),
            Ok("front".into())
        );
        assert!(pick_camera(&cameras, Some("gone"), Some("front")).is_err());
        assert_eq!(
            id(pick_camera(&cameras, None, Some("front"))),
            Ok("front".into())
        );
        // A saved camera that is not plugged in: the default.
        assert_eq!(
            id(pick_camera(&cameras, None, Some("gone"))),
            Ok("built-in".into())
        );
        assert_eq!(id(pick_camera(&cameras, None, None)), Ok("built-in".into()));
        assert!(pick_camera(&[], None, None).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_camera_s_modes_reach_the_front_end_once_each_and_the_settings_pick() {
        use crate::api::{CameraCapture, CameraFormat as Asked};
        use twicecat_video::capture::camera::CameraFormat;
        let cameras = cameras();
        let device = video_device(&cameras[0]);
        assert_eq!(device.id.as_str(), "front");
        assert_eq!(
            device.modes,
            vec![
                VideoMode {
                    width: 1920,
                    height: 1080,
                    fps: 30
                },
                VideoMode {
                    width: 1280,
                    height: 720,
                    fps: 30
                },
            ]
        );
        let auto = CameraCapture::default();
        assert_eq!(
            camera_mode(&cameras[0], (1280, 720, 30), auto).map(|m| m.format),
            Ok(CameraFormat::Yuyv)
        );
        let mjpeg = CameraCapture {
            mode: None,
            format: Asked::Mjpeg,
        };
        assert_eq!(
            camera_mode(&cameras[0], (1280, 720, 30), mjpeg).map(|m| m.format),
            Ok(CameraFormat::Mjpeg)
        );
        let full = CameraCapture {
            mode: Some(VideoMode {
                width: 1920,
                height: 1080,
                fps: 30,
            }),
            format: Asked::Auto,
        };
        assert_eq!(
            camera_mode(&cameras[0], (1280, 720, 30), full).map(|m| m.width),
            Ok(1920)
        );
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
