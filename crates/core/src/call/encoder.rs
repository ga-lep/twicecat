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
    CodecLimits, DegradationChoice, EncoderBackend, EncoderChoice, VideoAdjustment, VideoCodec,
    VideoCodecChoice, VideoEncoderInfo, VideoEncoders, VideoKind, VideoQuality, VideoTuning,
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
                codecs: codec_limits(EncoderBackend::Nvenc),
            });
        }
        if self.video_toolbox {
            encoders.push(VideoEncoderInfo {
                backend: EncoderBackend::VideoToolbox,
                gpu: None,
                hardware: true,
                codecs: codec_limits(EncoderBackend::VideoToolbox),
            });
        }
        encoders.push(VideoEncoderInfo {
            backend: EncoderBackend::Software,
            gpu: None,
            hardware: false,
            codecs: codec_limits(EncoderBackend::Software),
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

/// What `backend` encodes through LiveKit today, by codec.
///
/// - NVENC and VideoToolbox: H.264 at 4K30 and 1440p60 (level 3.1, see the
///   module docs); NVENC also AV1 at 4K60 (the encoder spike), which
///   livekit-client cannot decrypt, so in practice it is for calls between
///   Twicecat clients and unencrypted ones.
/// - Software: VP8 up to 1080p30, the path that is tested. VP9 (libvpx),
///   H.264 (OpenH264, where the libwebrtc build has it) and AV1 (libaom,
///   where it has it) are in libwebrtc's encoder factory and are offered at
///   the same size (AV1 at 720p30: it is slow), but have not been tried on
///   a call here, so `tested` is off.
pub(crate) fn codec_limits(backend: EncoderBackend) -> Vec<CodecLimits> {
    let limits = |codec, max_width, max_height, max_fps, tested| CodecLimits {
        codec,
        max_width,
        max_height,
        max_fps,
        tested,
    };
    match backend {
        EncoderBackend::Nvenc => vec![
            limits(VideoCodec::H264, 3840, 2160, 30, true),
            limits(VideoCodec::H264, 2560, 1440, 60, true),
            limits(VideoCodec::Av1, 3840, 2160, 60, true),
        ],
        EncoderBackend::VideoToolbox => vec![
            limits(VideoCodec::H264, 3840, 2160, 30, false),
            limits(VideoCodec::H264, 2560, 1440, 60, false),
        ],
        _ => {
            let (w, h, fps) = SOFTWARE_MAX;
            vec![
                limits(VideoCodec::Vp8, w, h, fps, true),
                limits(VideoCodec::Vp9, w, h, fps, false),
                limits(VideoCodec::H264, w, h, fps, false),
                limits(VideoCodec::Av1, 1280, 720, 30, false),
            ]
        }
    }
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
    /// VP9 or AV1 with one SVC stream (`L3T3_KEY`, …) in place of simulcast.
    pub scalability_mode: Option<String>,
    /// What gives way under pressure; LiveKit's default for the kind when
    /// `None`.
    pub degradation: Option<DegradationChoice>,
}

impl Sending {
    /// The track's size and rate.
    pub(crate) fn mode(&self) -> (u32, u32, u32) {
        let top = self.layers[0];
        (top.width, top.height, top.fps)
    }
}

/// How `kind` at `quality` is sent with `backend`, as the quality ladder
/// says: nothing tuned.
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
        scalability_mode: None,
        degradation: None,
    }
}

/// The ladder's codec for ours.
fn ladder_codec(codec: VideoCodec) -> Codec {
    match codec {
        VideoCodec::Av1 => Codec::Av1,
        VideoCodec::Vp9 => Codec::Vp9,
        VideoCodec::Vp8 => Codec::Vp8,
        // H.265 is never sent; it never gets here.
        VideoCodec::H264 | VideoCodec::H265 => Codec::H264,
    }
}

fn codec_words(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "H.264",
        VideoCodec::H265 => "H.265",
        VideoCodec::Vp8 => "VP8",
        VideoCodec::Vp9 => "VP9",
        VideoCodec::Av1 => "AV1",
    }
}

/// `mode` (width, height, fps) brought inside what `limits` allow. A mode
/// that one entry takes is left alone; one whose size fits but whose rate is
/// too high gets the highest rate those entries take; a size too big for
/// every entry is scaled down (same shape) to the biggest one.
fn fit_limits(limits: &[CodecLimits], mode: (u32, u32, u32)) -> (u32, u32, u32) {
    let (w, h, fps) = mode;
    if limits.is_empty()
        || limits
            .iter()
            .any(|l| w <= l.max_width && h <= l.max_height && fps <= l.max_fps)
    {
        return mode;
    }
    let sized: Vec<&CodecLimits> = limits
        .iter()
        .filter(|l| w <= l.max_width && h <= l.max_height)
        .collect();
    if let Some(rate) = sized.iter().map(|l| l.max_fps).max() {
        return (w, h, fps.min(rate));
    }
    let biggest = limits
        .iter()
        .max_by_key(|l| u64::from(l.max_width) * u64::from(l.max_height))
        .expect("limits is not empty");
    let scale = (f64::from(biggest.max_width) / f64::from(w))
        .min(f64::from(biggest.max_height) / f64::from(h));
    let even = |v: f64| (v.floor() as u32) & !1;
    (
        even(f64::from(w) * scale).max(2),
        even(f64::from(h) * scale).max(2),
        fps.min(biggest.max_fps),
    )
}

/// How `kind` is sent with `backend` when the person tuned it: the quality
/// gives the starting size, rate, codec and layers, and what the tuning names
/// replaces them. Whatever the encoder cannot do is brought inside what it
/// can, and said in the returned adjustments (never silently). `codec_choice`
/// is the settings' general codec choice, used where the tuning names none.
pub(crate) fn tuned(
    backend: EncoderBackend,
    kind: VideoKind,
    quality: VideoQuality,
    tuning: &VideoTuning,
    codec_choice: VideoCodecChoice,
) -> (Sending, Vec<VideoAdjustment>) {
    let name = match kind {
        VideoKind::Camera => "camera",
        VideoKind::Screen => "screen",
    };
    let mut notes = Vec::new();
    let base = sending(backend, kind, quality);
    let limits = codec_limits(backend);
    // The codec: the tuning's, else AV1 where the general choice asks for it
    // and the encoder has it, else what the encoder does best.
    let wanted = tuning.codec.or(match codec_choice {
        // "When possible" is a hardware encoder that has it: software AV1 is
        // too slow to be a default, and is only sent when asked for.
        VideoCodecChoice::Av1WhenPossible if backend != EncoderBackend::Software => {
            Some(VideoCodec::Av1)
        }
        _ => None,
    });
    let codec = match wanted {
        Some(c) if c == base.codec || limits.iter().any(|l| l.codec == c) => c,
        Some(c) => {
            // Only a codec the tuning asks for is worth a word: the general
            // choice says "when possible".
            if tuning.codec.is_some() {
                notes.push(VideoAdjustment {
                    setting: format!("{name}.codec"),
                    asked: codec_words(c).to_owned(),
                    used: codec_words(base.codec).to_owned(),
                });
            }
            base.codec
        }
        None => base.codec,
    };
    // The top picture.
    let (quality_w, quality_h, quality_fps) = base.mode();
    let custom = tuning.width.zip(tuning.height);
    let asked = (
        custom.map_or(quality_w, |(w, _)| w),
        custom.map_or(quality_h, |(_, h)| h),
        tuning.fps.unwrap_or(quality_fps),
    );
    let own: Vec<CodecLimits> = limits
        .iter()
        .copied()
        .filter(|l| l.codec == codec)
        .collect();
    let (width, height, fps) = if uncapped() {
        asked
    } else {
        fit_limits(&own, asked)
    };
    if (width, height) != (asked.0, asked.1) {
        notes.push(VideoAdjustment {
            setting: format!("{name}.size"),
            asked: format!("{}\u{d7}{}", asked.0, asked.1),
            used: format!("{width}\u{d7}{height}"),
        });
    }
    if fps != asked.2 {
        notes.push(VideoAdjustment {
            setting: format!("{name}.fps"),
            asked: asked.2.to_string(),
            used: fps.to_string(),
        });
    }
    let content = match kind {
        VideoKind::Camera => Content::Camera,
        VideoKind::Screen => Content::Screen,
    };
    let lcodec = ladder_codec(codec);
    let top = Layer {
        width,
        height,
        fps,
        bitrate_bps: tuning.bitrate_kbps.map_or_else(
            || ladder::bitrate_bps(content, lcodec, height, fps),
            |kbps| kbps.saturating_mul(1000),
        ),
    };
    // One SVC stream, or the simulcast layers.
    let svc = tuning.scalability_mode.clone().filter(|_| {
        let ok = matches!(codec, VideoCodec::Vp9 | VideoCodec::Av1);
        if !ok {
            notes.push(VideoAdjustment {
                setting: format!("{name}.svc"),
                asked: tuning.scalability_mode.clone().unwrap_or_default(),
                used: format!("not with {}", codec_words(codec)),
            });
        }
        ok
    });
    let mut layers = vec![top];
    if svc.is_none() {
        let source = Source { width, height, fps };
        let mut lower = ladder::layers(content, lcodec, source, top);
        let count = tuning
            .layers
            .map_or(lower.len(), usize::from)
            .clamp(1, ladder::MAX_LAYERS);
        // More layers than the ladder makes: each is half the one above.
        while lower.len() < count {
            let above = lower[lower.len() - 1];
            let (w, h) = (
                (above.width / 2).max(160) & !1,
                (above.height / 2).max(90) & !1,
            );
            if (w, h) == (above.width, above.height) {
                break;
            }
            let fps = above.fps.clamp(15, 30).min(above.fps);
            lower.push(Layer {
                width: w,
                height: h,
                fps,
                bitrate_bps: ladder::bitrate_bps(content, lcodec, h, fps),
            });
        }
        lower.truncate(count);
        for (i, layer) in lower.iter_mut().enumerate().skip(1) {
            if let Some(set) = tuning.lower_layers.get(i - 1) {
                if let Some((w, h)) = set.width.zip(set.height) {
                    layer.width = w;
                    layer.height = h;
                }
                if let Some(fps) = set.fps {
                    layer.fps = fps;
                }
                if let Some(kbps) = set.bitrate_kbps {
                    layer.bitrate_bps = kbps.saturating_mul(1000);
                }
            }
        }
        // No layer is bigger or faster than the one above it.
        for i in 1..lower.len() {
            let above = lower[i - 1];
            let at = format!("{name}.layer{}", i + 1);
            let layer = &mut lower[i];
            if layer.width > above.width || layer.height > above.height {
                notes.push(VideoAdjustment {
                    setting: format!("{at}.size"),
                    asked: format!("{}\u{d7}{}", layer.width, layer.height),
                    used: format!("{}\u{d7}{}", above.width, above.height),
                });
                layer.width = above.width;
                layer.height = above.height;
            }
            if layer.fps > above.fps {
                notes.push(VideoAdjustment {
                    setting: format!("{at}.fps"),
                    asked: layer.fps.to_string(),
                    used: above.fps.to_string(),
                });
                layer.fps = above.fps;
            }
        }
        layers = lower;
    }
    (
        Sending {
            backend,
            codec,
            layers,
            scalability_mode: svc,
            degradation: tuning.degradation,
        },
        notes,
    )
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
    use crate::api::LayerTuning;

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

    fn tuning() -> VideoTuning {
        VideoTuning::default()
    }

    fn tuned_with(
        backend: EncoderBackend,
        kind: VideoKind,
        quality: VideoQuality,
        tuning: &VideoTuning,
    ) -> (Sending, Vec<VideoAdjustment>) {
        tuned(backend, kind, quality, tuning, VideoCodecChoice::H264)
    }

    fn says(notes: &[VideoAdjustment], setting: &str) -> bool {
        notes.iter().any(|n| n.setting == setting)
    }

    #[test]
    fn an_untuned_kind_is_sent_as_the_ladder_says() {
        for backend in [EncoderBackend::Nvenc, EncoderBackend::Software] {
            for kind in [VideoKind::Camera, VideoKind::Screen] {
                for quality in [
                    VideoQuality::Auto,
                    VideoQuality::Hd720p30,
                    VideoQuality::Hd1080p60,
                    VideoQuality::Uhd2160p60,
                ] {
                    let (sent, notes) = tuned_with(backend, kind, quality, &tuning());
                    assert_eq!(
                        sent,
                        sending(backend, kind, quality),
                        "{backend:?} {kind:?}"
                    );
                    assert!(notes.is_empty(), "{notes:?}");
                }
            }
        }
    }

    #[test]
    fn a_custom_size_rate_and_bitrate_replace_the_qualitys() {
        let t = VideoTuning {
            width: Some(1280),
            height: Some(720),
            fps: Some(24),
            bitrate_kbps: Some(2_500),
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Screen,
            VideoQuality::Hd1080p60,
            &t,
        );
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(sent.mode(), (1280, 720, 24));
        assert_eq!(sent.layers[0].bitrate_bps, 2_500_000);
        // The layers under it follow the ladder from the new top.
        assert!(sent.layers.len() >= 2 && sent.layers[1].height < 720);
    }

    #[test]
    fn a_rate_alone_keeps_the_qualitys_size() {
        let t = VideoTuning {
            fps: Some(15),
            ..tuning()
        };
        let (sent, _) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Hd720p30,
            &t,
        );
        assert_eq!(sent.mode(), (1280, 720, 15));
    }

    #[test]
    fn what_the_encoder_cannot_do_is_brought_inside_and_said() {
        // NVENC through LiveKit stops at 4K30: 4K60 loses its rate.
        let t = VideoTuning {
            width: Some(3840),
            height: Some(2160),
            fps: Some(60),
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Screen,
            VideoQuality::Auto,
            &t,
        );
        assert_eq!(sent.mode(), (3840, 2160, 30));
        assert!(
            says(&notes, "screen.fps") && !says(&notes, "screen.size"),
            "{notes:?}"
        );
        // A size beyond every limit is scaled down, keeping its shape.
        let big = VideoTuning {
            width: Some(7680),
            height: Some(4320),
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &big,
        );
        assert_eq!(sent.mode(), (1920, 1080, 30));
        assert!(says(&notes, "camera.size"), "{notes:?}");
        // The software path cannot do 60 fps at 1080p.
        let fast = VideoTuning {
            fps: Some(60),
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &fast,
        );
        assert_eq!(sent.mode().2, 30);
        assert!(says(&notes, "camera.fps"), "{notes:?}");
    }

    #[test]
    fn a_codec_is_used_where_the_encoder_has_it_and_replaced_and_said_where_not() {
        let codec = |c| VideoTuning {
            codec: Some(c),
            ..tuning()
        };
        let (vp9, notes) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &codec(VideoCodec::Vp9),
        );
        assert_eq!(vp9.codec, VideoCodec::Vp9);
        assert!(notes.is_empty(), "{notes:?}");
        // VP9 needs less than VP8 at the same size.
        let (vp8, _) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &tuning(),
        );
        assert!(vp9.layers[0].bitrate_bps < vp8.layers[0].bitrate_bps);
        // A hardware encoder has no VP9: H.264 instead, with a word.
        let (sent, notes) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Camera,
            VideoQuality::Auto,
            &codec(VideoCodec::Vp9),
        );
        assert_eq!(sent.codec, VideoCodec::H264);
        assert!(says(&notes, "camera.codec"), "{notes:?}");
        // AV1 on NVENC is there, at 4K60 as the spike measured.
        let (av1, _) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Screen,
            VideoQuality::Uhd2160p60,
            &codec(VideoCodec::Av1),
        );
        assert_eq!(av1.codec, VideoCodec::Av1);
        assert_eq!(av1.mode(), (3840, 2160, 30), "the quality's own cap stays");
    }

    #[test]
    fn the_general_av1_choice_is_for_hardware_and_never_noisy() {
        let ask = |backend| {
            tuned(
                backend,
                VideoKind::Screen,
                VideoQuality::Auto,
                &tuning(),
                VideoCodecChoice::Av1WhenPossible,
            )
        };
        let (nvenc, notes) = ask(EncoderBackend::Nvenc);
        assert_eq!(nvenc.codec, VideoCodec::Av1);
        assert!(notes.is_empty());
        let (software, notes) = ask(EncoderBackend::Software);
        assert_eq!(software.codec, VideoCodec::Vp8);
        assert!(notes.is_empty());
        let (toolbox, notes) = ask(EncoderBackend::VideoToolbox);
        assert_eq!(toolbox.codec, VideoCodec::H264, "no AV1 in VideoToolbox");
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn the_number_of_layers_and_each_layer_can_be_set() {
        let one = VideoTuning {
            layers: Some(1),
            ..tuning()
        };
        let (sent, _) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Camera,
            VideoQuality::Hd1080p60,
            &one,
        );
        assert_eq!(sent.layers.len(), 1);
        let two = VideoTuning {
            layers: Some(2),
            ..tuning()
        };
        let (sent, _) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Camera,
            VideoQuality::Hd1080p60,
            &two,
        );
        assert_eq!(sent.layers.len(), 2);
        assert_eq!(sent.layers[1].height, 720);
        // A source of 360p has no ladder layers; three are made by halving.
        let three = VideoTuning {
            layers: Some(3),
            width: Some(640),
            height: Some(360),
            fps: Some(30),
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &three,
        );
        let sizes: Vec<_> = sent.layers.iter().map(|l| (l.width, l.height)).collect();
        assert_eq!(sizes, [(640, 360), (320, 180), (160, 90)], "{notes:?}");
        // Settings of a layer replace the ladder's, and a layer bigger than
        // the one above is brought down and said.
        let set = VideoTuning {
            lower_layers: vec![
                LayerTuning {
                    width: Some(960),
                    height: Some(540),
                    fps: Some(20),
                    bitrate_kbps: Some(900),
                },
                LayerTuning {
                    width: Some(1280),
                    height: Some(720),
                    ..LayerTuning::default()
                },
            ],
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Nvenc,
            VideoKind::Camera,
            VideoQuality::Hd1080p60,
            &set,
        );
        assert_eq!(sent.layers[1].width, 960);
        assert_eq!(sent.layers[1].fps, 20);
        assert_eq!(sent.layers[1].bitrate_bps, 900_000);
        assert_eq!((sent.layers[2].width, sent.layers[2].height), (960, 540));
        assert!(says(&notes, "camera.layer3.size"), "{notes:?}");
    }

    #[test]
    fn an_svc_mode_makes_one_stream_for_vp9_and_av1_only() {
        let svc = |codec| VideoTuning {
            codec,
            scalability_mode: Some("L3T3_KEY".into()),
            ..tuning()
        };
        let (sent, notes) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Screen,
            VideoQuality::Auto,
            &svc(Some(VideoCodec::Vp9)),
        );
        assert_eq!(sent.layers.len(), 1);
        assert_eq!(sent.scalability_mode.as_deref(), Some("L3T3_KEY"));
        assert!(notes.is_empty(), "{notes:?}");
        // With the default codec of the software path, VP8, it is dropped.
        let (sent, notes) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Screen,
            VideoQuality::Auto,
            &svc(None),
        );
        assert_eq!(sent.scalability_mode, None);
        assert!(sent.layers.len() > 1);
        assert!(says(&notes, "screen.svc"), "{notes:?}");
    }

    #[test]
    fn the_degradation_choice_reaches_the_sending() {
        let t = VideoTuning {
            degradation: Some(DegradationChoice::Balanced),
            ..tuning()
        };
        let (sent, _) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &t,
        );
        assert_eq!(sent.degradation, Some(DegradationChoice::Balanced));
        let (plain, _) = tuned_with(
            EncoderBackend::Software,
            VideoKind::Camera,
            VideoQuality::Auto,
            &tuning(),
        );
        assert_eq!(plain.degradation, None);
    }

    #[test]
    fn the_offered_codecs_say_which_were_tried() {
        let nvenc = codec_limits(EncoderBackend::Nvenc);
        assert!(nvenc.iter().all(|l| l.tested));
        let software = codec_limits(EncoderBackend::Software);
        let tested: Vec<_> = software
            .iter()
            .filter(|l| l.tested)
            .map(|l| l.codec)
            .collect();
        assert_eq!(tested, [VideoCodec::Vp8]);
        assert!(software.iter().any(|l| l.codec == VideoCodec::Vp9));
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
