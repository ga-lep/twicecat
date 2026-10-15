//! Video types of the core API (Phase 9): cameras, screen sources, the
//! video settings, the call's video tracks, the frames handed to the front
//! end ([`VideoFeed`]), the encoders this machine has and the statistics.
//! None of them names an SDK, LiveKit or capture type.
//!
//! Frames, window titles and device identifiers (they can hold a serial
//! number) are user data: no `Debug` here prints them.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::call::ParticipantId;
use super::types::{DeviceId, UserId, string_newtype};

string_newtype!(
    /// One video track in the current call, ours or another participant's.
    /// Opaque; stable for the track's life.
    VideoTrackId
);

/// Writes `Name(…)` with the length only: the value can identify a device
/// or a window.
macro_rules! private_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Borrows the inner string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "(<{} bytes>)"), self.0.len())
            }
        }
    };
}

private_newtype!(
    /// A camera, for [`VideoSettings::camera`] and the start commands.
    /// Opaque; can hold the device's serial number, so `Debug` hides it.
    VideoDeviceId
);

private_newtype!(
    /// A monitor or window offered by [`Command::ListScreenSources`](super::Command::ListScreenSources).
    /// Opaque, valid until the next listing.
    ScreenSourceId
);

/// Where cameras and screens come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoBackend {
    /// The system's cameras and screens.
    #[default]
    System,
    /// A moving test pattern instead, as a camera and as a screen (tests,
    /// the CLI): each frame carries its number and send time in a stamp
    /// at the top that survives encoding and scaling.
    TestPattern,
}

/// The identifier of the test pattern, as a camera and as a screen
/// ([`VideoBackend::TestPattern`]).
pub const TEST_PATTERN_ID: &str = "test-pattern";

/// What a video track carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoKind {
    /// A camera.
    Camera,
    /// A screen or window share (one per participant).
    Screen,
}

/// The quality asked for a camera or a share. `Auto` is the highest the
/// encoder and the measured uplink allow, capped at 1440p60 (plan Q12).
/// Under load a screen share keeps its resolution and drops framerate; a
/// camera keeps its framerate and drops resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoQuality {
    /// Chosen by the core.
    #[default]
    Auto,
    /// 1280 × 720 at 30 fps.
    Hd720p30,
    /// 1920 × 1080 at 60 fps.
    Hd1080p60,
    /// 2560 × 1440 at 60 fps.
    Qhd1440p60,
    /// 3840 × 2160 at 30 fps.
    Uhd2160p30,
    /// 3840 × 2160 at 60 fps.
    Uhd2160p60,
}

/// A video codec, as sent or received.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    /// H.264 (what we send by default).
    H264,
    /// H.265 (received only).
    H265,
    /// VP8 (Element Call's default; our no-hardware fallback).
    Vp8,
    /// VP9 (received only).
    Vp9,
    /// AV1 (opt-in, see [`VideoCodecChoice`]).
    Av1,
}

/// Which codec we send (plan Q4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodecChoice {
    /// H.264: every browser decodes it, also in encrypted calls.
    #[default]
    H264,
    /// AV1 where the encoder has it and every receiver can decode it:
    /// Element Call cannot decrypt AV1, so in practice Twicecat-only and
    /// unencrypted calls. H.264 otherwise.
    Av1WhenPossible,
}

/// Where video is encoded (plan Q6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderBackend {
    /// NVIDIA's NVENC through CUDA (Ada and newer by default).
    Nvenc,
    /// Vulkan Video (Intel, AMD, NVIDIA before Ada).
    VulkanVideo,
    /// Windows' Media Foundation.
    MediaFoundation,
    /// Apple's VideoToolbox.
    VideoToolbox,
    /// V4L2 (the Steam Frame).
    V4l2,
    /// WebRTC's own encoder on the CPU (VP8, up to 1080p30).
    Software,
}

/// Which encoder to use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderChoice {
    /// The best this machine has, from [`Command::ProbeVideoEncoders`](super::Command::ProbeVideoEncoders).
    #[default]
    Auto,
    /// This one, when available; the next best otherwise.
    Prefer(EncoderBackend),
}

/// Video settings; saved with the account's other settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoSettings {
    /// The camera to use; the system's default when `None` or missing.
    pub camera: Option<VideoDeviceId>,
    /// Quality of the camera.
    pub camera_quality: VideoQuality,
    /// Quality of screen shares.
    pub screen_quality: VideoQuality,
    /// Which codec we send.
    pub codec: VideoCodecChoice,
    /// Which encoder to use.
    pub encoder: EncoderChoice,
    /// On battery, encode on the integrated GPU of a laptop with two.
    pub prefer_integrated_gpu_on_battery: bool,
    /// The most our video may send in total, kb/s; `None`: no cap.
    pub upload_cap_kbps: Option<u32>,
    /// Remember the screen or window picked in the system's picker
    /// (Wayland's portal), so the next share starts without asking.
    pub remember_screen_choice: bool,
    /// How the camera is sent, where the person does not leave it to the
    /// quality above.
    pub camera_tuning: VideoTuning,
    /// How a screen share is sent, as `camera_tuning`.
    pub screen_tuning: VideoTuning,
    /// How the camera is opened, apart from what is sent.
    pub camera_capture: CameraCapture,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            camera: None,
            camera_quality: VideoQuality::Auto,
            screen_quality: VideoQuality::Auto,
            codec: VideoCodecChoice::H264,
            encoder: EncoderChoice::Auto,
            prefer_integrated_gpu_on_battery: true,
            upload_cap_kbps: None,
            remember_screen_choice: false,
            camera_tuning: VideoTuning::default(),
            screen_tuning: VideoTuning::default(),
            camera_capture: CameraCapture::default(),
        }
    }
}

/// How a track gives way when the network or the encoder cannot keep up
/// (`degradation_preference` in WebRTC).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationChoice {
    /// Keep the frame rate, lower the resolution (the camera's default).
    KeepFrameRate,
    /// Keep the resolution, drop frames (a screen share's default: text must
    /// stay sharp).
    KeepResolution,
    /// A bit of both.
    Balanced,
}

/// One simulcast layer below the top one, as the person set it. Every field
/// `None` is left to the quality ladder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerTuning {
    /// Width in pixels (with `height`: both or neither).
    pub width: Option<u32>,
    /// Height in pixels.
    pub height: Option<u32>,
    /// Frames per second.
    pub fps: Option<u32>,
    /// Bitrate, kb/s.
    pub bitrate_kbps: Option<u32>,
}

/// What one kind of video (the camera, or screen shares) is sent as, when
/// the person does not want the automatic choice. Every field that is
/// `None` (or empty) is automatic, so the default changes nothing. The core
/// checks and clamps what is here ([`VideoSettings::validated`]) and the
/// call's [`LocalVideo`] says what is really sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoTuning {
    /// Width of the picture sent (with `height`: both or neither).
    pub width: Option<u32>,
    /// Height of the picture sent.
    pub height: Option<u32>,
    /// Frames per second.
    pub fps: Option<u32>,
    /// The most the top layer may send, kb/s.
    pub bitrate_kbps: Option<u32>,
    /// The codec; automatic is what the encoder does best (H.264 on a
    /// hardware encoder, VP8 in software), or AV1 where
    /// [`VideoSettings::codec`] asks for it.
    pub codec: Option<VideoCodec>,
    /// The encoder, for this kind only; the one in [`VideoSettings::encoder`]
    /// when `None`.
    pub encoder: Option<EncoderChoice>,
    /// How many simulcast layers go out, 1 to 3 (1 is no simulcast); the
    /// ladder's (up to 3) when `None`.
    pub layers: Option<u8>,
    /// The layers below the top, the next one first; what is not named is
    /// the ladder's.
    pub lower_layers: Vec<LayerTuning>,
    /// For VP9 and AV1: one SVC stream with this scalability mode (for
    /// example `L3T3_KEY`) instead of simulcast. One of [`SCALABILITY_MODES`].
    pub scalability_mode: Option<String>,
    /// What gives way under pressure; the kind's own default when `None`.
    pub degradation: Option<DegradationChoice>,
}

/// The scalability modes [`VideoTuning::scalability_mode`] may name.
pub const SCALABILITY_MODES: [&str; 8] = [
    "L1T2", "L1T3", "L2T2", "L2T3", "L3T3", "L2T2_KEY", "L2T3_KEY", "L3T3_KEY",
];

/// The pixel format a camera is opened in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraFormat {
    /// The best the camera offers for the mode.
    #[default]
    Auto,
    /// NV12.
    Nv12,
    /// YUYV (YUY2).
    Yuyv,
    /// Motion JPEG.
    Mjpeg,
}

/// How the camera is opened, apart from what is sent: the size and rate it
/// captures at, and its pixel format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraCapture {
    /// A mode from the camera's [`VideoDevice::modes`]; the best one for the
    /// quality when `None`.
    pub mode: Option<VideoMode>,
    /// The pixel format.
    pub format: CameraFormat,
}

/// Bounds enforced by [`VideoSettings::validated`].
pub mod video_limits {
    /// Width of a picture sent, pixels.
    pub const WIDTH: (u32, u32) = (160, 7680);
    /// Height of a picture sent, pixels.
    pub const HEIGHT: (u32, u32) = (90, 4320);
    /// Frames per second.
    pub const FPS: (u32, u32) = (1, 120);
    /// A layer's bitrate, kb/s.
    pub const BITRATE_KBPS: (u32, u32) = (50, 150_000);
    /// The upload cap, kb/s.
    pub const UPLOAD_CAP_KBPS: (u32, u32) = (100, 300_000);
    /// Simulcast layers.
    pub const LAYERS: (u8, u8) = (1, 3);
}

/// One thing [`VideoSettings::validated`] (or the encoder) changed, so the
/// front end can say so instead of silently sending something else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoAdjustment {
    /// Which setting, as a path such as `screen.fps` or `camera.layer2.height`.
    pub setting: String,
    /// What was asked, in words.
    pub asked: String,
    /// What is used, in words.
    pub used: String,
}

impl VideoAdjustment {
    fn new(setting: impl Into<String>, asked: impl fmt::Display, used: impl fmt::Display) -> Self {
        Self {
            setting: setting.into(),
            asked: asked.to_string(),
            used: used.to_string(),
        }
    }
}

/// Clamps `value` into `range`; records the change.
fn clamp_to(
    changes: &mut Vec<VideoAdjustment>,
    setting: &str,
    value: &mut Option<u32>,
    range: (u32, u32),
    even: bool,
) {
    let Some(asked) = *value else {
        return;
    };
    let mut used = asked.clamp(range.0, range.1);
    if even {
        used &= !1;
    }
    if used != asked {
        changes.push(VideoAdjustment::new(setting, asked, used));
        *value = Some(used);
    }
}

/// A custom size needs both sides: one alone is dropped.
fn size_pair(
    changes: &mut Vec<VideoAdjustment>,
    setting: &str,
    w: &mut Option<u32>,
    h: &mut Option<u32>,
) {
    clamp_to(
        changes,
        &format!("{setting}.width"),
        w,
        video_limits::WIDTH,
        true,
    );
    clamp_to(
        changes,
        &format!("{setting}.height"),
        h,
        video_limits::HEIGHT,
        true,
    );
    match (*w, *h) {
        (Some(w0), None) => {
            changes.push(VideoAdjustment::new(
                format!("{setting}.width"),
                w0,
                "automatic",
            ));
            *w = None;
        }
        (None, Some(h0)) => {
            changes.push(VideoAdjustment::new(
                format!("{setting}.height"),
                h0,
                "automatic",
            ));
            *h = None;
        }
        _ => {}
    }
}

impl VideoTuning {
    /// Whether everything is automatic.
    pub fn is_automatic(&self) -> bool {
        *self == Self::default()
    }

    fn validate(&mut self, name: &str, changes: &mut Vec<VideoAdjustment>) {
        size_pair(changes, name, &mut self.width, &mut self.height);
        clamp_to(
            changes,
            &format!("{name}.fps"),
            &mut self.fps,
            video_limits::FPS,
            false,
        );
        clamp_to(
            changes,
            &format!("{name}.bitrate"),
            &mut self.bitrate_kbps,
            video_limits::BITRATE_KBPS,
            false,
        );
        // H.265 is received only (plan Q4).
        if self.codec == Some(VideoCodec::H265) {
            changes.push(VideoAdjustment::new(
                format!("{name}.codec"),
                "H.265",
                "automatic",
            ));
            self.codec = None;
        }
        let (min_layers, max_layers) = video_limits::LAYERS;
        if let Some(layers) = self.layers {
            let used = layers.clamp(min_layers, max_layers);
            if used != layers {
                changes.push(VideoAdjustment::new(format!("{name}.layers"), layers, used));
                self.layers = Some(used);
            }
        }
        // The layers below the top: at most two, and a size of both sides.
        let room = usize::from(max_layers) - 1;
        if self.lower_layers.len() > room {
            changes.push(VideoAdjustment::new(
                format!("{name}.layers"),
                self.lower_layers.len() + 1,
                max_layers,
            ));
            self.lower_layers.truncate(room);
        }
        for (i, layer) in self.lower_layers.iter_mut().enumerate() {
            let at = format!("{name}.layer{}", i + 2);
            size_pair(changes, &at, &mut layer.width, &mut layer.height);
            clamp_to(
                changes,
                &format!("{at}.fps"),
                &mut layer.fps,
                video_limits::FPS,
                false,
            );
            clamp_to(
                changes,
                &format!("{at}.bitrate"),
                &mut layer.bitrate_kbps,
                video_limits::BITRATE_KBPS,
                false,
            );
        }
        // A layer is never bigger than the one above it (a custom top size
        // is the one to compare with).
        if let (Some(w), Some(h)) = (self.width, self.height) {
            let mut above = (w, h);
            for (i, layer) in self.lower_layers.iter_mut().enumerate() {
                if let (Some(lw), Some(lh)) = (layer.width, layer.height) {
                    if lw > above.0 || lh > above.1 {
                        changes.push(VideoAdjustment::new(
                            format!("{name}.layer{}.size", i + 2),
                            format!("{lw}\u{d7}{lh}"),
                            format!("{}\u{d7}{}", above.0, above.1),
                        ));
                        layer.width = Some(lw.min(above.0) & !1);
                        layer.height = Some(lh.min(above.1) & !1);
                    }
                    above = (layer.width.unwrap_or(lw), layer.height.unwrap_or(lh));
                }
            }
        }
        if let Some(mode) = self.scalability_mode.clone() {
            let known = SCALABILITY_MODES.contains(&mode.as_str());
            let codec_ok = matches!(self.codec, None | Some(VideoCodec::Vp9 | VideoCodec::Av1));
            if !known || !codec_ok {
                changes.push(VideoAdjustment::new(
                    format!("{name}.svc"),
                    mode.chars().take(16).collect::<String>(),
                    "automatic",
                ));
                self.scalability_mode = None;
            }
        }
    }
}

impl VideoSettings {
    /// These settings with every value inside its bounds (see
    /// [`video_limits`]), and what was changed, for the front end to say.
    /// Settings that are already in bounds come back as they are, with
    /// nothing changed.
    ///
    /// ```
    /// use twicecat_core::api::{VideoSettings, VideoTuning};
    /// let settings = VideoSettings {
    ///     screen_tuning: VideoTuning { fps: Some(500), ..VideoTuning::default() },
    ///     ..VideoSettings::default()
    /// };
    /// let (used, changes) = settings.validated();
    /// assert_eq!(used.screen_tuning.fps, Some(120));
    /// assert_eq!(changes.len(), 1);
    /// ```
    pub fn validated(mut self) -> (Self, Vec<VideoAdjustment>) {
        let mut changes = Vec::new();
        clamp_to(
            &mut changes,
            "upload_cap",
            &mut self.upload_cap_kbps,
            video_limits::UPLOAD_CAP_KBPS,
            false,
        );
        self.camera_tuning.validate("camera", &mut changes);
        self.screen_tuning.validate("screen", &mut changes);
        if let Some(mode) = self.camera_capture.mode {
            let (mut w, mut h, mut f) = (Some(mode.width), Some(mode.height), Some(mode.fps));
            clamp_to(
                &mut changes,
                "capture.width",
                &mut w,
                video_limits::WIDTH,
                true,
            );
            clamp_to(
                &mut changes,
                "capture.height",
                &mut h,
                video_limits::HEIGHT,
                true,
            );
            clamp_to(
                &mut changes,
                "capture.fps",
                &mut f,
                video_limits::FPS,
                false,
            );
            self.camera_capture.mode = Some(VideoMode {
                width: w.unwrap_or(mode.width),
                height: h.unwrap_or(mode.height),
                fps: f.unwrap_or(mode.fps),
            });
        }
        (self, changes)
    }

    /// The tuning of one kind.
    pub fn tuning(&self, kind: VideoKind) -> &VideoTuning {
        match kind {
            VideoKind::Camera => &self.camera_tuning,
            VideoKind::Screen => &self.screen_tuning,
        }
    }
}

/// A size and rate a camera offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VideoMode {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frames per second.
    pub fps: u32,
}

/// One camera.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoDevice {
    /// Identifier for [`VideoSettings::camera`] and the start commands.
    pub id: VideoDeviceId,
    /// Human-readable name.
    pub name: String,
    /// The system's default camera.
    pub is_default: bool,
    /// What it can send, largest first.
    pub modes: Vec<VideoMode>,
}

/// Every camera found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoDevices {
    /// The cameras.
    pub cameras: Vec<VideoDevice>,
}

/// What a screen source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenSourceKind {
    /// A whole monitor.
    Monitor,
    /// One window.
    Window,
}

/// A small picture of a screen source, RGBA, for a picker.
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenThumbnail {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, rows top first.
    pub rgba: Arc<[u8]>,
}

impl fmt::Debug for ScreenThumbnail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenThumbnail")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// A monitor or window that can be shared, for our own picker (X11 and
/// Windows).
#[derive(Clone, PartialEq, Eq)]
pub struct ScreenSource {
    /// Identifier for [`Command::StartScreenShare`](super::Command::StartScreenShare).
    pub id: ScreenSourceId,
    /// A monitor or a window.
    pub kind: ScreenSourceKind,
    /// The monitor's name or the window's title.
    pub title: String,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
    /// A picture of it, when one could be taken.
    pub thumbnail: Option<ScreenThumbnail>,
}

// Manual impl: a window title can be anything the user has open.
impl fmt::Debug for ScreenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScreenSource")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("title", &format_args!("<{} bytes>", self.title.len()))
            .field("width", &self.width)
            .field("height", &self.height)
            .field("thumbnail", &self.thumbnail)
            .finish()
    }
}

/// What can be shared, answering [`Command::ListScreenSources`](super::Command::ListScreenSources).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScreenSources {
    /// The system shows its own picker when a share starts (Wayland's
    /// portal, macOS): `sources` is empty and
    /// [`Command::StartScreenShare`](super::Command::StartScreenShare)
    /// takes no source.
    pub system_picker: bool,
    /// The monitors and windows, for our own picker.
    pub sources: Vec<ScreenSource>,
}

/// One video track of the current call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoTrackView {
    /// Stable for the track's life.
    pub id: VideoTrackId,
    /// Whose device sends it (the same as in [`Participant`](super::Participant)).
    pub participant: ParticipantId,
    /// Account.
    pub user_id: UserId,
    /// Device.
    pub device_id: DeviceId,
    /// This device's own track.
    pub is_local: bool,
    /// A camera or a screen share.
    pub kind: VideoKind,
    /// Width of the picture now, pixels (0 before the first frame).
    pub width: u32,
    /// Height of the picture now, pixels (0 before the first frame).
    pub height: u32,
    /// Frames per second now, when known.
    pub fps: Option<u32>,
    /// The codec, when known.
    pub codec: Option<VideoCodec>,
    /// Its sender turned it off for now (a camera "off" that is still
    /// published).
    pub muted: bool,
}

/// How a frame's pixels are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoPixelFormat {
    /// A Y plane, then one interleaved UV plane at half size.
    Nv12,
    /// Y, U and V planes, U and V at half size.
    I420,
}

/// The YUV matrix of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoMatrix {
    /// Standard definition.
    Bt601,
    /// HD and above.
    #[default]
    Bt709,
}

/// The range of a frame's YUV values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum VideoRange {
    /// Y in 16..=235.
    #[default]
    Limited,
    /// Y in 0..=255.
    Full,
}

/// One plane of a frame: its rows, `stride` bytes apart.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoPlane {
    /// The bytes.
    pub data: Arc<[u8]>,
    /// Bytes per row (at least the plane's width).
    pub stride: usize,
}

impl fmt::Debug for VideoPlane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoPlane")
            .field("len", &self.data.len())
            .field("stride", &self.stride)
            .finish()
    }
}

/// A decoded frame, ready to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    /// Layout of `planes`.
    pub format: VideoPixelFormat,
    /// Width in pixels (even).
    pub width: u32,
    /// Height in pixels (even).
    pub height: u32,
    /// Y first.
    pub planes: Vec<VideoPlane>,
    /// The matrix to convert to RGB with.
    pub matrix: VideoMatrix,
    /// The range to convert to RGB with.
    pub range: VideoRange,
    /// When it should be shown, microseconds on the core's clock.
    pub timestamp_us: i64,
}

/// The newest frame of one track, for the front end to draw at its own
/// pace: a slot that holds only the latest frame (older ones are dropped,
/// never queued) and a wake-up when a new one arrives. Cheap to clone; each
/// clone has its own wake-up.
#[derive(Clone)]
pub struct VideoFeed {
    rx: watch::Receiver<Option<Arc<VideoFrame>>>,
}

impl VideoFeed {
    /// A feed and the sender that fills it.
    pub fn channel() -> (VideoFeedSender, Self) {
        let (tx, rx) = watch::channel(None);
        (VideoFeedSender { tx }, Self { rx })
    }

    /// The newest frame, if any arrived yet.
    pub fn latest(&self) -> Option<Arc<VideoFrame>> {
        self.rx.borrow().clone()
    }

    /// Waits for a frame newer than the last one this handle saw. Returns
    /// `false` once the track has ended (its sender is gone).
    pub async fn changed(&mut self) -> bool {
        self.rx.changed().await.is_ok()
    }

    /// Whether the track has ended.
    pub fn is_ended(&self) -> bool {
        self.rx.has_changed().is_err()
    }
}

impl fmt::Debug for VideoFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let frame = self.rx.borrow();
        f.debug_struct("VideoFeed")
            .field("size", &frame.as_ref().map(|fr| (fr.width, fr.height)))
            .field("ended", &self.is_ended())
            .finish()
    }
}

/// Fills a [`VideoFeed`]; dropping it ends the feed.
#[derive(Debug)]
pub struct VideoFeedSender {
    tx: watch::Sender<Option<Arc<VideoFrame>>>,
}

impl VideoFeedSender {
    /// Replaces the feed's frame and wakes its readers.
    pub fn send(&self, frame: VideoFrame) {
        self.tx.send_replace(Some(Arc::new(frame)));
    }
}

/// Our own video in the current call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalVideo {
    /// Our camera track, while the camera is on.
    pub camera: Option<VideoTrackId>,
    /// Our screen share, while sharing.
    pub screen: Option<VideoTrackId>,
    /// The share sends its sound too.
    pub screen_audio: bool,
}

/// The largest picture an encoder takes for one codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodecLimits {
    /// The codec.
    pub codec: VideoCodec,
    /// Widest picture, pixels.
    pub max_width: u32,
    /// Tallest picture, pixels.
    pub max_height: u32,
    /// Highest framerate at that size.
    pub max_fps: u32,
    /// Checked on a real call on this project's test machine. A codec that is
    /// not is offered all the same, and sent as asked, but the front end says
    /// it has not been tried.
    pub tested: bool,
}

/// One encoder found on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoEncoderInfo {
    /// Which API.
    pub backend: EncoderBackend,
    /// The GPU's name, for a hardware encoder.
    pub gpu: Option<String>,
    /// Encodes in hardware.
    pub hardware: bool,
    /// What it encodes.
    pub codecs: Vec<CodecLimits>,
}

/// The encoders this machine has, best first, answering
/// [`Command::ProbeVideoEncoders`](super::Command::ProbeVideoEncoders) and
/// whenever the one in use changes (a GPU lost mid-call).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VideoEncoders {
    /// Everything found, best first.
    pub encoders: Vec<VideoEncoderInfo>,
    /// The one our video uses now, in a call.
    pub in_use: Option<EncoderBackend>,
}

/// Whether a track is sent or received.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VideoDirection {
    /// Ours, going out.
    Sent,
    /// Someone else's, coming in.
    Received,
}

/// What holds a sent track's quality back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum QualityLimitation {
    /// Nothing.
    #[default]
    None,
    /// The encoder or the CPU cannot keep up.
    Cpu,
    /// The network cannot carry more.
    Bandwidth,
    /// Something else.
    Other,
}

/// One track's numbers over the last second.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoTrackStats {
    /// The track.
    pub track: VideoTrackId,
    /// Sent or received.
    pub direction: VideoDirection,
    /// Picture size, pixels.
    pub width: u32,
    /// Picture size, pixels.
    pub height: u32,
    /// Frames sent or received per second.
    pub fps: f32,
    /// Bitrate, kb/s.
    pub bitrate_kbps: u32,
    /// Mean time to encode (sent) or decode (received) a frame, ms.
    pub frame_ms: Option<f32>,
    /// What holds a sent track back (`None` for received ones).
    pub limitation: QualityLimitation,
    /// Pictures that froze (a pause over 250 ms) since the track started.
    pub freezes: u32,
    /// The encoder (sent) or decoder (received) as the WebRTC stack names it
    /// (`NVIDIA H264 Encoder`, `libvpx`, `dav1d`, …), once known.
    pub implementation: Option<String>,
    /// That encoder or decoder runs in hardware.
    pub hardware: bool,
}

/// Video statistics, about once a second during a call with video.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoStats {
    /// One entry per track with video.
    pub tracks: Vec<VideoTrackStats>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_and_source_ids_and_titles_stay_out_of_debug() {
        let id = VideoDeviceId::from("usb-046d_BRIO_SERIAL1234");
        assert_eq!(format!("{id:?}"), "VideoDeviceId(<24 bytes>)");
        let source = ScreenSource {
            id: ScreenSourceId::from("window:0x4400007"),
            kind: ScreenSourceKind::Window,
            title: "Bank statement - Firefox".into(),
            width: 800,
            height: 600,
            thumbnail: Some(ScreenThumbnail {
                width: 2,
                height: 1,
                rgba: vec![9; 8].into(),
            }),
        };
        let text = format!("{source:?}");
        assert!(
            !text.contains("Bank") && !text.contains("0x4400007"),
            "{text}"
        );
        assert!(text.contains("Window") && text.contains("800"), "{text}");
    }

    #[test]
    fn frames_stay_out_of_debug() {
        let frame = VideoFrame {
            format: VideoPixelFormat::Nv12,
            width: 2,
            height: 2,
            planes: vec![
                VideoPlane {
                    data: vec![0xAB; 4].into(),
                    stride: 2,
                },
                VideoPlane {
                    data: vec![0xCD; 2].into(),
                    stride: 2,
                },
            ],
            matrix: VideoMatrix::Bt709,
            range: VideoRange::Limited,
            timestamp_us: 5,
        };
        let text = format!("{frame:?}");
        assert!(!text.contains("171") && !text.contains("205"), "{text}");
        let (tx, feed) = VideoFeed::channel();
        tx.send(frame);
        assert_eq!(
            format!("{feed:?}"),
            "VideoFeed { size: Some((2, 2)), ended: false }"
        );
    }

    #[tokio::test]
    async fn a_feed_keeps_only_the_newest_frame_and_ends_with_its_sender() {
        let frame = |ts| VideoFrame {
            format: VideoPixelFormat::I420,
            width: 2,
            height: 2,
            planes: Vec::new(),
            matrix: VideoMatrix::Bt709,
            range: VideoRange::Limited,
            timestamp_us: ts,
        };
        let (tx, mut feed) = VideoFeed::channel();
        assert!(feed.latest().is_none());
        tx.send(frame(1));
        tx.send(frame(2));
        assert!(feed.changed().await);
        assert_eq!(feed.latest().unwrap().timestamp_us, 2);
        let mut other = feed.clone();
        drop(tx);
        assert!(!feed.changed().await);
        assert!(!other.changed().await);
        assert!(feed.is_ended());
        // The last frame stays readable.
        assert_eq!(other.latest().unwrap().timestamp_us, 2);
    }

    fn says(changes: &[VideoAdjustment], setting: &str) -> bool {
        changes.iter().any(|c| c.setting == setting)
    }

    #[test]
    fn settings_in_bounds_come_back_unchanged() {
        let settings = VideoSettings {
            upload_cap_kbps: Some(8_000),
            camera_tuning: VideoTuning {
                width: Some(1280),
                height: Some(720),
                fps: Some(30),
                bitrate_kbps: Some(1_700),
                codec: Some(VideoCodec::Vp9),
                layers: Some(2),
                lower_layers: vec![LayerTuning {
                    width: Some(640),
                    height: Some(360),
                    fps: Some(15),
                    bitrate_kbps: Some(400),
                }],
                scalability_mode: Some("L2T3_KEY".into()),
                degradation: Some(DegradationChoice::Balanced),
                ..VideoTuning::default()
            },
            camera_capture: CameraCapture {
                mode: Some(VideoMode {
                    width: 1920,
                    height: 1080,
                    fps: 30,
                }),
                format: CameraFormat::Mjpeg,
            },
            ..VideoSettings::default()
        };
        let (used, changes) = settings.clone().validated();
        assert_eq!(used, settings);
        assert!(changes.is_empty(), "{changes:?}");
        assert_eq!(VideoSettings::default().validated().1, Vec::new());
    }

    #[test]
    fn every_number_is_clamped_and_each_change_is_reported() {
        let settings = VideoSettings {
            upload_cap_kbps: Some(5),
            screen_tuning: VideoTuning {
                width: Some(99_999),
                height: Some(10),
                fps: Some(0),
                bitrate_kbps: Some(9_999_999),
                layers: Some(9),
                ..VideoTuning::default()
            },
            camera_capture: CameraCapture {
                mode: Some(VideoMode {
                    width: 100_000,
                    height: 1,
                    fps: 1_000,
                }),
                ..CameraCapture::default()
            },
            ..VideoSettings::default()
        };
        let (used, changes) = settings.validated();
        assert_eq!(used.upload_cap_kbps, Some(video_limits::UPLOAD_CAP_KBPS.0));
        let t = &used.screen_tuning;
        assert_eq!(t.width, Some(video_limits::WIDTH.1));
        assert_eq!(t.height, Some(video_limits::HEIGHT.0));
        assert_eq!(t.fps, Some(1));
        assert_eq!(t.bitrate_kbps, Some(video_limits::BITRATE_KBPS.1));
        assert_eq!(t.layers, Some(3));
        let mode = used.camera_capture.mode.expect("kept");
        assert_eq!((mode.width, mode.height, mode.fps), (7680, 90, 120));
        for setting in [
            "upload_cap",
            "screen.width",
            "screen.height",
            "screen.fps",
            "screen.bitrate",
            "screen.layers",
            "capture.width",
            "capture.height",
            "capture.fps",
        ] {
            assert!(
                says(&changes, setting),
                "{setting} missing from {changes:?}"
            );
        }
        // What is used is itself in bounds.
        assert!(used.clone().validated().1.is_empty());
    }

    #[test]
    fn odd_sizes_become_even_and_half_a_size_is_dropped() {
        let settings = VideoSettings {
            camera_tuning: VideoTuning {
                width: Some(1281),
                height: Some(721),
                ..VideoTuning::default()
            },
            screen_tuning: VideoTuning {
                width: Some(1920),
                ..VideoTuning::default()
            },
            ..VideoSettings::default()
        };
        let (used, changes) = settings.validated();
        assert_eq!(
            (used.camera_tuning.width, used.camera_tuning.height),
            (Some(1280), Some(720))
        );
        assert_eq!(
            (used.screen_tuning.width, used.screen_tuning.height),
            (None, None)
        );
        assert!(says(&changes, "camera.width") && says(&changes, "screen.width"));
    }

    #[test]
    fn layers_codecs_and_svc_modes_are_checked() {
        let settings = VideoSettings {
            camera_tuning: VideoTuning {
                codec: Some(VideoCodec::H265),
                scalability_mode: Some("L9T9".into()),
                lower_layers: vec![LayerTuning::default(); 5],
                ..VideoTuning::default()
            },
            screen_tuning: VideoTuning {
                codec: Some(VideoCodec::H264),
                scalability_mode: Some("L3T3_KEY".into()),
                ..VideoTuning::default()
            },
            ..VideoSettings::default()
        };
        let (used, changes) = settings.validated();
        assert_eq!(used.camera_tuning.codec, None, "H.265 is receive-only");
        assert_eq!(used.camera_tuning.scalability_mode, None);
        assert_eq!(used.camera_tuning.lower_layers.len(), 2);
        // SVC needs VP9 or AV1.
        assert_eq!(used.screen_tuning.scalability_mode, None);
        for setting in ["camera.codec", "camera.svc", "camera.layers", "screen.svc"] {
            assert!(
                says(&changes, setting),
                "{setting} missing from {changes:?}"
            );
        }
    }

    #[test]
    fn a_lower_layer_is_never_bigger_than_the_one_above() {
        let settings = VideoSettings {
            camera_tuning: VideoTuning {
                width: Some(1280),
                height: Some(720),
                lower_layers: vec![
                    LayerTuning {
                        width: Some(1920),
                        height: Some(1080),
                        ..LayerTuning::default()
                    },
                    LayerTuning {
                        width: Some(1600),
                        height: Some(900),
                        ..LayerTuning::default()
                    },
                ],
                ..VideoTuning::default()
            },
            ..VideoSettings::default()
        };
        let (used, changes) = settings.validated();
        let sizes: Vec<_> = used
            .camera_tuning
            .lower_layers
            .iter()
            .map(|l| (l.width, l.height))
            .collect();
        assert_eq!(sizes, [(Some(1280), Some(720)), (Some(1280), Some(720))]);
        assert!(says(&changes, "camera.layer2.size"), "{changes:?}");
    }

    #[test]
    fn an_old_settings_file_loads_with_the_new_fields_automatic() {
        let old = r#"{"codec":"h264","camera_quality":"hd720p30","upload_cap_kbps":4000}"#;
        let settings: VideoSettings = serde_json::from_str(old).unwrap();
        assert!(settings.camera_tuning.is_automatic());
        assert!(settings.screen_tuning.is_automatic());
        assert_eq!(settings.camera_capture, CameraCapture::default());
        assert_eq!(settings.camera_quality, VideoQuality::Hd720p30);
        // And a new one reads back as written.
        let tuned = VideoSettings {
            screen_tuning: VideoTuning {
                fps: Some(15),
                degradation: Some(DegradationChoice::KeepFrameRate),
                encoder: Some(EncoderChoice::Prefer(EncoderBackend::Software)),
                ..VideoTuning::default()
            },
            ..settings
        };
        let json = serde_json::to_string(&tuned).unwrap();
        assert_eq!(serde_json::from_str::<VideoSettings>(&json).unwrap(), tuned);
    }

    #[test]
    fn the_tuning_of_a_kind_is_picked_by_kind() {
        let settings = VideoSettings {
            camera_tuning: VideoTuning {
                fps: Some(24),
                ..VideoTuning::default()
            },
            ..VideoSettings::default()
        };
        assert_eq!(settings.tuning(VideoKind::Camera).fps, Some(24));
        assert!(settings.tuning(VideoKind::Screen).is_automatic());
    }

    #[test]
    fn settings_default_and_read_back_with_missing_fields() {
        let settings = VideoSettings::default();
        assert!(settings.prefer_integrated_gpu_on_battery);
        assert!(!settings.remember_screen_choice);
        let back: VideoSettings =
            serde_json::from_str(r#"{"codec":"av1_when_possible","encoder":{"prefer":"nvenc"}}"#)
                .unwrap();
        assert_eq!(back.codec, VideoCodecChoice::Av1WhenPossible);
        assert_eq!(back.encoder, EncoderChoice::Prefer(EncoderBackend::Nvenc));
        assert_eq!(back.camera_quality, VideoQuality::Auto);
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<VideoSettings>(&json).unwrap(),
            settings
        );
    }
}
