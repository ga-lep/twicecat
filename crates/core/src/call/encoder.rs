//! Which encoder sends our video, and how (plan Phase 9 Task 6; Q6 as
//! amended 2026-10-14): LiveKit's own encoders, with their simulcast.
//!
//! - NVENC (H.264, three layers) when LiveKit's CUDA build finds it at run
//!   time: NVIDIA on Linux with the proprietary driver.
//! - VideoToolbox (H.264) on macOS.
//! - Otherwise libwebrtc's software VP8, at most 1080p30.
//!
//! We always name the encoder: LiveKit's `Auto` and `Hardware` would pick
//! VA-API on an Intel or AMD machine, and LiveKit's VA-API encoder crashes
//! (the plan's encoder spike). VA-API comes back when that is fixed.
//!
//! NVENC is told H.264 level 3.1 by LiveKit's factory, which it refuses at
//! 4K60, so the hardware modes stop at 4K30 and 1440p60 for now.
//! `TWICECAT_VIDEO_UNCAPPED=1` lifts that cap, to try a LiveKit build that
//! takes 4K60 (and, today, to see the software fallback when NVENC refuses
//! it).

use livekit::options::VideoEncoderBackend;
use twicecat_video::ladder::{self, Codec, Content, Layer, Source};
use twicecat_video::probe::{Probe, Vendor};

use crate::api::{
    CodecLimits, EncoderBackend, EncoderChoice, VideoCodec, VideoEncoderInfo, VideoEncoders,
    VideoKind, VideoQuality,
};

/// The software path's largest picture and rate (plan Q3 a).
const SOFTWARE_MAX: (u32, u32, u32) = (1920, 1080, 30);

/// What LiveKit can encode with on this machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Available {
    /// NVENC, with the name of the GPU it runs on when the probe found one.
    nvenc: Option<Option<String>>,
    /// VideoToolbox (macOS).
    video_toolbox: bool,
}

impl Available {
    /// Asks LiveKit and the GPU probe (blocking: the probe opens the
    /// drivers' libraries; a fraction of a second).
    pub(crate) fn detect() -> Self {
        let backends: Vec<_> = VideoEncoderBackend::list_available().into_iter().collect();
        Self::from_parts(&backends, &twicecat_video::probe::probe())
    }

    /// From LiveKit's list and the probe. LiveKit's list decides: it says
    /// what its build has and what loaded at run time. The probe names the
    /// GPU (LiveKit uses CUDA's first device; with one NVIDIA GPU that is
    /// the one the probe finds).
    pub(crate) fn from_parts(backends: &[VideoEncoderBackend], probe: &Probe) -> Self {
        let nvidia = probe
            .gpus
            .iter()
            .find(|g| g.vendor == Vendor::Nvidia && g.nvidia.is_some())
            .map(|g| g.name.clone());
        Self {
            nvenc: backends
                .contains(&VideoEncoderBackend::Nvenc)
                .then_some(nvidia),
            video_toolbox: backends.contains(&VideoEncoderBackend::VideoToolbox),
        }
    }

    /// The encoders, best first: what `ProbeVideoEncoders` answers.
    pub(crate) fn encoders(&self, in_use: Option<EncoderBackend>) -> VideoEncoders {
        let mut encoders = Vec::new();
        if let Some(gpu) = &self.nvenc {
            encoders.push(VideoEncoderInfo {
                backend: EncoderBackend::Nvenc,
                gpu: gpu.clone(),
                hardware: true,
                codecs: hardware_limits(),
            });
        }
        if self.video_toolbox {
            encoders.push(VideoEncoderInfo {
                backend: EncoderBackend::VideoToolbox,
                gpu: None,
                hardware: true,
                codecs: hardware_limits(),
            });
        }
        let (max_width, max_height, max_fps) = SOFTWARE_MAX;
        encoders.push(VideoEncoderInfo {
            backend: EncoderBackend::Software,
            gpu: None,
            hardware: false,
            codecs: vec![CodecLimits {
                codec: VideoCodec::Vp8,
                max_width,
                max_height,
                max_fps,
            }],
        });
        VideoEncoders { encoders, in_use }
    }

    /// The encoder for `choice`: the one preferred when this machine has
    /// it, else the best one.
    pub(crate) fn choose(&self, choice: EncoderChoice) -> EncoderBackend {
        let has = |backend| match backend {
            EncoderBackend::Nvenc => self.nvenc.is_some(),
            EncoderBackend::VideoToolbox => self.video_toolbox,
            EncoderBackend::Software => true,
            // Not offered by LiveKit, or not safe yet (VA-API, Windows).
            EncoderBackend::VulkanVideo
            | EncoderBackend::MediaFoundation
            | EncoderBackend::V4l2 => false,
        };
        match choice {
            EncoderChoice::Prefer(backend) if has(backend) => backend,
            _ if self.nvenc.is_some() => EncoderBackend::Nvenc,
            _ if self.video_toolbox => EncoderBackend::VideoToolbox,
            _ => EncoderBackend::Software,
        }
    }
}

/// What a hardware encoder takes through LiveKit today: 4K at 30 fps, and
/// 60 fps up to 1440p (H.264 level 3.1, see the module docs).
fn hardware_limits() -> Vec<CodecLimits> {
    vec![
        CodecLimits {
            codec: VideoCodec::H264,
            max_width: 3840,
            max_height: 2160,
            max_fps: 30,
        },
        CodecLimits {
            codec: VideoCodec::H264,
            max_width: 2560,
            max_height: 1440,
            max_fps: 60,
        },
    ]
}

/// LiveKit's name for one of our encoders.
pub(crate) fn livekit_backend(backend: EncoderBackend) -> VideoEncoderBackend {
    match backend {
        EncoderBackend::Nvenc => VideoEncoderBackend::Nvenc,
        EncoderBackend::VideoToolbox => VideoEncoderBackend::VideoToolbox,
        _ => VideoEncoderBackend::Software,
    }
}

/// Whether LiveKit's encoder report (`encoderImplementation`) names the
/// hardware encoder we asked for: "NVIDIA H264 Encoder" (inside
/// `SimulcastEncoderAdapter (…)` when there are layers) or VideoToolbox's.
pub(crate) fn is_running(backend: EncoderBackend, implementation: &str) -> bool {
    match backend {
        EncoderBackend::Nvenc => implementation.contains("NVIDIA"),
        EncoderBackend::VideoToolbox => implementation.contains("VideoToolbox"),
        _ => true,
    }
}

/// How one track is sent: its size and rate, codec, encoder, and the
/// simulcast layers (top first, bitrates from the plan's ladder, Q12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Sending {
    pub backend: EncoderBackend,
    pub codec: VideoCodec,
    /// Top first; the first is the track's own size and rate.
    pub layers: Vec<Layer>,
}

impl Sending {
    /// The track's size and rate.
    pub(crate) fn mode(&self) -> (u32, u32, u32) {
        let top = self.layers[0];
        (top.width, top.height, top.fps)
    }
}

/// How `kind` at `quality` is sent with `backend`.
pub(crate) fn sending(backend: EncoderBackend, kind: VideoKind, quality: VideoQuality) -> Sending {
    let hardware = backend != EncoderBackend::Software;
    let (width, height, fps) = if hardware {
        hardware_mode(quality, uncapped())
    } else {
        software_mode(quality)
    };
    let (codec, ladder_codec) = if hardware {
        (VideoCodec::H264, Codec::H264)
    } else {
        (VideoCodec::Vp8, Codec::Vp8)
    };
    let content = match kind {
        VideoKind::Camera => Content::Camera,
        VideoKind::Screen => Content::Screen,
    };
    let top = Layer {
        width,
        height,
        fps,
        bitrate_bps: ladder::bitrate_bps(content, ladder_codec, height, fps),
    };
    let source = Source { width, height, fps };
    Sending {
        backend,
        codec,
        layers: ladder::layers(content, ladder_codec, source, top),
    }
}

/// The size and rate asked for. `Auto` is 1440p60 (plan Q12's cap until the
/// human raises it); 4K60 is sent at 4K30 until LiveKit's NVENC takes it,
/// unless `uncapped`.
fn hardware_mode(quality: VideoQuality, uncapped: bool) -> (u32, u32, u32) {
    match quality {
        VideoQuality::Hd720p30 => (1280, 720, 30),
        VideoQuality::Hd1080p60 => (1920, 1080, 60),
        VideoQuality::Qhd1440p60 | VideoQuality::Auto => (2560, 1440, 60),
        VideoQuality::Uhd2160p60 if uncapped => (3840, 2160, 60),
        VideoQuality::Uhd2160p30 | VideoQuality::Uhd2160p60 => (3840, 2160, 30),
    }
}

/// `TWICECAT_VIDEO_UNCAPPED=1`: see the module docs.
fn uncapped() -> bool {
    std::env::var_os("TWICECAT_VIDEO_UNCAPPED").is_some_and(|v| v == "1")
}

/// The size and rate asked for, capped at 1080p30. `Auto` is the cap.
fn software_mode(quality: VideoQuality) -> (u32, u32, u32) {
    let (w, h, fps) = match quality {
        VideoQuality::Hd720p30 => (1280, 720, 30),
        VideoQuality::Hd1080p60 => (1920, 1080, 60),
        VideoQuality::Qhd1440p60 => (2560, 1440, 60),
        VideoQuality::Uhd2160p30 => (3840, 2160, 30),
        VideoQuality::Uhd2160p60 => (3840, 2160, 60),
        VideoQuality::Auto => SOFTWARE_MAX,
    };
    let (max_w, max_h, max_fps) = SOFTWARE_MAX;
    if h > max_h {
        (max_w, max_h, fps.min(max_fps))
    } else {
        (w, h, fps.min(max_fps))
    }
}

#[cfg(test)]
mod tests {
    use twicecat_video::probe::{Gpu, GpuKind, Nvidia, VulkanVideo};

    use super::*;

    fn probe_with(vendor: Vendor, nvidia: bool) -> Probe {
        Probe {
            gpus: vec![Gpu {
                name: "Test GPU".into(),
                vendor,
                kind: GpuKind::Discrete,
                uuid: [0; 16],
                render_node: None,
                vulkan: VulkanVideo::default(),
                nvidia: nvidia.then(Nvidia::default),
            }],
            vulkan_error: None,
        }
    }

    const BASE: [VideoEncoderBackend; 3] = [
        VideoEncoderBackend::Auto,
        VideoEncoderBackend::Software,
        VideoEncoderBackend::PreEncoded,
    ];

    fn with(extra: &[VideoEncoderBackend]) -> Vec<VideoEncoderBackend> {
        BASE.iter().chain(extra).copied().collect()
    }

    #[test]
    fn nvenc_is_chosen_when_livekit_has_it() {
        let a = Available::from_parts(
            &with(&[VideoEncoderBackend::Nvenc, VideoEncoderBackend::Vaapi]),
            &probe_with(Vendor::Nvidia, true),
        );
        assert_eq!(a.choose(EncoderChoice::Auto), EncoderBackend::Nvenc);
        let list = a.encoders(None);
        assert_eq!(list.encoders[0].backend, EncoderBackend::Nvenc);
        assert_eq!(list.encoders[0].gpu.as_deref(), Some("Test GPU"));
        assert!(list.encoders[0].hardware);
        assert_eq!(
            list.encoders.last().map(|e| e.backend),
            Some(EncoderBackend::Software)
        );
    }

    #[test]
    fn vaapi_is_never_chosen() {
        // An Intel or AMD machine: LiveKit lists VA-API, which crashes.
        for choice in [
            EncoderChoice::Auto,
            EncoderChoice::Prefer(EncoderBackend::VulkanVideo),
        ] {
            let a = Available::from_parts(
                &with(&[VideoEncoderBackend::Vaapi, VideoEncoderBackend::Hardware]),
                &probe_with(Vendor::Intel, false),
            );
            assert_eq!(a.choose(choice), EncoderBackend::Software);
            assert_eq!(a.encoders(None).encoders.len(), 1);
        }
    }

    #[test]
    fn the_software_choice_wins_and_a_missing_one_falls_back() {
        let nvenc = Available::from_parts(
            &with(&[VideoEncoderBackend::Nvenc]),
            &probe_with(Vendor::Nvidia, true),
        );
        assert_eq!(
            nvenc.choose(EncoderChoice::Prefer(EncoderBackend::Software)),
            EncoderBackend::Software
        );
        assert_eq!(
            nvenc.choose(EncoderChoice::Prefer(EncoderBackend::VideoToolbox)),
            EncoderBackend::Nvenc
        );
        let none = Available::from_parts(&with(&[]), &probe_with(Vendor::Nvidia, true));
        assert_eq!(
            none.choose(EncoderChoice::Prefer(EncoderBackend::Nvenc)),
            EncoderBackend::Software
        );
    }

    #[test]
    fn nvenc_without_the_probe_has_no_gpu_name() {
        // nouveau would not list NVENC; a driver the probe misses still
        // encodes, unnamed.
        let a = Available::from_parts(
            &with(&[VideoEncoderBackend::Nvenc]),
            &probe_with(Vendor::Intel, false),
        );
        assert_eq!(a.choose(EncoderChoice::Auto), EncoderBackend::Nvenc);
        assert_eq!(a.encoders(None).encoders[0].gpu, None);
    }

    #[test]
    fn video_toolbox_on_macos() {
        let a = Available::from_parts(
            &with(&[VideoEncoderBackend::VideoToolbox]),
            &Probe::default(),
        );
        assert_eq!(a.choose(EncoderChoice::Auto), EncoderBackend::VideoToolbox);
    }

    #[test]
    fn software_caps_at_1080p30() {
        assert_eq!(software_mode(VideoQuality::Hd720p30), (1280, 720, 30));
        assert_eq!(software_mode(VideoQuality::Hd1080p60), (1920, 1080, 30));
        assert_eq!(software_mode(VideoQuality::Uhd2160p60), (1920, 1080, 30));
        assert_eq!(software_mode(VideoQuality::Qhd1440p60), (1920, 1080, 30));
        assert_eq!(software_mode(VideoQuality::Auto), (1920, 1080, 30));
    }

    #[test]
    fn hardware_stops_at_4k30_and_1440p60() {
        let mode = |q| sending(EncoderBackend::Nvenc, VideoKind::Camera, q).mode();
        assert_eq!(mode(VideoQuality::Uhd2160p60), (3840, 2160, 30));
        assert_eq!(mode(VideoQuality::Uhd2160p30), (3840, 2160, 30));
        assert_eq!(mode(VideoQuality::Qhd1440p60), (2560, 1440, 60));
        assert_eq!(mode(VideoQuality::Auto), (2560, 1440, 60));
        assert_eq!(mode(VideoQuality::Hd1080p60), (1920, 1080, 60));
    }

    #[test]
    fn uncapped_hardware_sends_4k60() {
        assert_eq!(
            hardware_mode(VideoQuality::Uhd2160p60, true),
            (3840, 2160, 60)
        );
        assert_eq!(
            hardware_mode(VideoQuality::Uhd2160p30, true),
            (3840, 2160, 30)
        );
        assert_eq!(
            hardware_mode(VideoQuality::Uhd2160p60, false),
            (3840, 2160, 30)
        );
    }

    #[test]
    fn layers_follow_the_ladder_and_codec_the_encoder() {
        let camera = sending(
            EncoderBackend::Nvenc,
            VideoKind::Camera,
            VideoQuality::Hd1080p60,
        );
        assert_eq!(camera.codec, VideoCodec::H264);
        let sizes: Vec<_> = camera
            .layers
            .iter()
            .map(|l| (l.width, l.height, l.fps, l.bitrate_bps))
            .collect();
        assert_eq!(
            sizes,
            [
                (1920, 1080, 60, 8_000_000),
                (1280, 720, 30, 1_700_000),
                (640, 360, 30, 450_000)
            ]
        );
        let screen = sending(
            EncoderBackend::Nvenc,
            VideoKind::Screen,
            VideoQuality::Uhd2160p30,
        );
        let heights: Vec<_> = screen.layers.iter().map(|l| (l.height, l.fps)).collect();
        assert_eq!(heights, [(2160, 30), (1080, 30), (540, 15)]);
        let software = sending(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
        );
        assert_eq!(software.codec, VideoCodec::Vp8);
        assert_eq!(software.mode(), (1920, 1080, 30));
    }

    #[test]
    fn a_fallen_back_encoder_is_noticed() {
        let nvenc = "SimulcastEncoderAdapter (NVIDIA H264 Encoder, NVIDIA H264 Encoder)";
        assert!(is_running(EncoderBackend::Nvenc, nvenc));
        assert!(!is_running(EncoderBackend::Nvenc, "OpenH264"));
        assert!(!is_running(EncoderBackend::Nvenc, "libvpx"));
        assert!(is_running(EncoderBackend::Software, "libvpx"));
    }
}
