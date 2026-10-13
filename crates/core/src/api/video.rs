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
        }
    }
}

/// A size and rate a camera offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
