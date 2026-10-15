//! The Video page of Settings (Phase 9 Task 11, and the two levels of
//! Settings): the camera and the picture quality for people who want it to
//! work, and, with **Show advanced settings** on, every knob the core and
//! LiveKit allow for the camera and for screen sharing: size, frame rate,
//! bitrate, codec, encoder, layers, SVC, what gives way under pressure, the
//! camera's own capture mode, and the upload limit.
//!
//! Edits are made on a copy of the settings the core last echoed and sent
//! whole ([`App::edit_video`]), as the audio settings are. The core clamps
//! them and says what it changed (`VideoSettingsAdjusted`), which the page
//! shows in words.

use std::collections::{HashMap, HashSet};
use std::fmt;

use iced::Task;
use twicecat_core::api::{
    CameraFormat, Command, DegradationChoice, EncoderBackend, EncoderChoice, LayerTuning,
    VideoCodec, VideoCodecChoice, VideoDevice, VideoDeviceId, VideoKind, VideoMode, VideoSettings,
    VideoTuning,
};

use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder};
use crate::video::QualityOption;

/// Which number of a layer a control sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dim {
    /// Width and height.
    Size,
    /// Frames per second.
    Fps,
    /// Bitrate, kb/s.
    Bitrate,
}

/// One number of one layer: layer 0 is the top one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Field {
    /// 0 for the top layer, 1 and 2 for the layers under it.
    pub layer: u8,
    /// Which number.
    pub dim: Dim,
}

impl Field {
    /// The top layer's.
    pub const fn top(dim: Dim) -> Self {
        Self { layer: 0, dim }
    }
}

/// What a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    /// Left to the core.
    Auto,
    /// A frame rate or a bitrate.
    Number(u32),
    /// A width and a height.
    Size(u32, u32),
}

/// What `field` of `tuning` holds now.
pub fn get(tuning: &VideoTuning, field: Field) -> Value {
    let (w, h, fps, kbps) = if field.layer == 0 {
        (tuning.width, tuning.height, tuning.fps, tuning.bitrate_kbps)
    } else {
        let layer = tuning
            .lower_layers
            .get(usize::from(field.layer) - 1)
            .copied()
            .unwrap_or_default();
        (layer.width, layer.height, layer.fps, layer.bitrate_kbps)
    };
    match field.dim {
        Dim::Size => w.zip(h).map_or(Value::Auto, |(w, h)| Value::Size(w, h)),
        Dim::Fps => fps.map_or(Value::Auto, Value::Number),
        Dim::Bitrate => kbps.map_or(Value::Auto, Value::Number),
    }
}

/// Sets `field` of `tuning`. Layers under the top that end up all automatic
/// are dropped, so the file stays as short as what was chosen.
pub fn set(tuning: &mut VideoTuning, field: Field, value: Value) {
    if field.layer > 0 {
        let at = usize::from(field.layer) - 1;
        while tuning.lower_layers.len() <= at {
            tuning.lower_layers.push(LayerTuning::default());
        }
    }
    let layer = if field.layer == 0 {
        None
    } else {
        Some(usize::from(field.layer) - 1)
    };
    let (w, h, fps, kbps): (
        &mut Option<u32>,
        &mut Option<u32>,
        &mut Option<u32>,
        &mut Option<u32>,
    ) = match layer {
        None => (
            &mut tuning.width,
            &mut tuning.height,
            &mut tuning.fps,
            &mut tuning.bitrate_kbps,
        ),
        Some(i) => {
            let l = &mut tuning.lower_layers[i];
            (&mut l.width, &mut l.height, &mut l.fps, &mut l.bitrate_kbps)
        }
    };
    match (field.dim, value) {
        (Dim::Size, Value::Size(sw, sh)) => {
            *w = Some(sw);
            *h = Some(sh);
        }
        (Dim::Size, _) => {
            *w = None;
            *h = None;
        }
        (Dim::Fps, Value::Number(n)) => *fps = Some(n),
        (Dim::Fps, _) => *fps = None,
        (Dim::Bitrate, Value::Number(n)) => *kbps = Some(n),
        (Dim::Bitrate, _) => *kbps = None,
    }
    while tuning
        .lower_layers
        .last()
        .is_some_and(|l| *l == LayerTuning::default())
    {
        tuning.lower_layers.pop();
    }
}

/// Sizes a menu offers.
pub const SIZES: [(u32, u32); 8] = [
    (426, 240),
    (640, 360),
    (854, 480),
    (1280, 720),
    (1600, 900),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
];
/// Frame rates a menu offers.
pub const RATES: [u32; 9] = [5, 10, 15, 24, 30, 48, 60, 90, 120];
/// Bitrates a menu offers, kb/s.
pub const BITRATES: [u32; 10] = [
    250, 500, 1_000, 2_000, 4_000, 6_000, 8_000, 12_000, 20_000, 40_000,
];
/// Upload limits a menu offers, kb/s.
pub const CAPS: [u32; 7] = [500, 1_000, 2_000, 4_000, 8_000, 16_000, 32_000];

/// An entry of a numbers menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Left to the core.
    Auto,
    /// A listed number.
    Number(u32),
    /// A listed size.
    Size(u32, u32),
    /// Type it.
    Custom,
}

/// A [`Pick`] with the unit it is in, for the menu's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PickOption {
    /// What the menu is about.
    pub dim: Dim,
    /// The entry.
    pub pick: Pick,
}

impl PickOption {
    /// The entries of the menu for `dim`: automatic, the list, custom.
    pub fn menu(dim: Dim) -> Vec<Self> {
        let mut out = vec![Self {
            dim,
            pick: Pick::Auto,
        }];
        match dim {
            Dim::Size => out.extend(SIZES.map(|(w, h)| Self {
                dim,
                pick: Pick::Size(w, h),
            })),
            Dim::Fps => out.extend(RATES.map(|n| Self {
                dim,
                pick: Pick::Number(n),
            })),
            Dim::Bitrate => out.extend(BITRATES.map(|n| Self {
                dim,
                pick: Pick::Number(n),
            })),
        }
        out.push(Self {
            dim,
            pick: Pick::Custom,
        });
        out
    }

    /// The entry that shows `value`: a listed one, else custom. `typing`
    /// keeps custom shown while its field is being filled in.
    pub fn showing(dim: Dim, value: Value, typing: bool) -> Self {
        let pick = match value {
            _ if typing => Pick::Custom,
            Value::Auto => Pick::Auto,
            Value::Number(n) => {
                let listed = match dim {
                    Dim::Fps => RATES.contains(&n),
                    _ => BITRATES.contains(&n),
                };
                if listed {
                    Pick::Number(n)
                } else {
                    Pick::Custom
                }
            }
            Value::Size(w, h) => {
                if SIZES.contains(&(w, h)) {
                    Pick::Size(w, h)
                } else {
                    Pick::Custom
                }
            }
        };
        Self { dim, pick }
    }
}

impl fmt::Display for PickOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.dim, self.pick) {
            (_, Pick::Auto) => f.write_str("Automatic"),
            (_, Pick::Custom) => f.write_str("Custom\u{2026}"),
            (_, Pick::Size(w, h)) => write!(f, "{w} \u{d7} {h} ({h}p)"),
            (Dim::Fps, Pick::Number(n)) => write!(f, "{n} frames a second"),
            (_, Pick::Number(n)) => write!(f, "{n} kb/s"),
        }
    }
}

/// What typed text says for `dim`: a size such as `1600x900`, or a whole
/// number. `None` when it says nothing usable.
pub fn parse(dim: Dim, text: &str) -> Option<Value> {
    let text = text.trim();
    match dim {
        Dim::Size => {
            let (w, h) = text.split_once(['x', 'X', '\u{d7}'])?;
            let side = |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n > 0);
            Some(Value::Size(side(w)?, side(h)?))
        }
        Dim::Fps | Dim::Bitrate => text
            .parse::<u32>()
            .ok()
            .filter(|n| *n > 0)
            .map(Value::Number),
    }
}

/// How a value reads back in a field that is being typed in.
pub fn text_of(value: Value) -> String {
    match value {
        Value::Auto => String::new(),
        Value::Number(n) => n.to_string(),
        Value::Size(w, h) => format!("{w}x{h}"),
    }
}

/// The camera menu's entries.
#[derive(Clone, Debug)]
pub struct CameraChoice {
    /// What the setting stores; `None` follows the system's default.
    pub id: Option<VideoDeviceId>,
    /// What the menu shows.
    pub label: String,
}

impl PartialEq for CameraChoice {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl fmt::Display for CameraChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// The camera menu and the entry that is chosen: "System default", the
/// cameras, and the saved camera when it is not plugged in now (it stays
/// chosen and comes back).
pub fn camera_menu(
    cameras: &[VideoDevice],
    chosen: Option<&VideoDeviceId>,
) -> (Vec<CameraChoice>, CameraChoice) {
    let mut menu = vec![CameraChoice {
        id: None,
        label: "System default".to_owned(),
    }];
    menu.extend(cameras.iter().map(|c| CameraChoice {
        id: Some(c.id.clone()),
        label: name_or_placeholder(&c.name, MAX_NAME_CHARS),
    }));
    let selected = match chosen {
        None => menu[0].clone(),
        Some(id) => match menu.iter().find(|c| c.id.as_ref() == Some(id)) {
            Some(found) => found.clone(),
            None => {
                let missing = CameraChoice {
                    id: Some(id.clone()),
                    label: "Saved camera (not connected)".to_owned(),
                };
                menu.push(missing.clone());
                missing
            }
        },
    };
    (menu, selected)
}

/// An encoder menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EncoderOption(pub EncoderChoice);

impl fmt::Display for EncoderOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            EncoderChoice::Auto => "Automatic",
            EncoderChoice::Prefer(EncoderBackend::Nvenc) => "NVIDIA graphics card (NVENC)",
            EncoderChoice::Prefer(EncoderBackend::VideoToolbox) => "Apple (VideoToolbox)",
            EncoderChoice::Prefer(EncoderBackend::VulkanVideo) => "Graphics card (Vulkan Video)",
            EncoderChoice::Prefer(EncoderBackend::MediaFoundation) => "Windows (Media Foundation)",
            EncoderChoice::Prefer(EncoderBackend::V4l2) => "Hardware (V4L2)",
            EncoderChoice::Prefer(EncoderBackend::Software) => "Processor (software)",
        })
    }
}

/// A codec menu entry: automatic, or one codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodecOption(pub Option<VideoCodec>);

impl CodecOption {
    /// The codecs that can be sent (H.265 is received only).
    pub const ALL: [Self; 5] = [
        Self(None),
        Self(Some(VideoCodec::H264)),
        Self(Some(VideoCodec::Vp8)),
        Self(Some(VideoCodec::Vp9)),
        Self(Some(VideoCodec::Av1)),
    ];
}

impl fmt::Display for CodecOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            None => "Automatic",
            Some(VideoCodec::H264) => "H.264",
            Some(VideoCodec::H265) => "H.265",
            Some(VideoCodec::Vp8) => "VP8",
            Some(VideoCodec::Vp9) => "VP9",
            Some(VideoCodec::Av1) => "AV1",
        })
    }
}

/// How many simulcast layers: automatic, or 1 to 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayersOption(pub Option<u8>);

impl LayersOption {
    /// The menu.
    pub const ALL: [Self; 4] = [Self(None), Self(Some(1)), Self(Some(2)), Self(Some(3))];
}

impl fmt::Display for LayersOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("Automatic"),
            Some(1) => f.write_str("1 (one stream, no simulcast)"),
            Some(n) => write!(f, "{n} layers"),
        }
    }
}

/// The scalability mode menu: none (simulcast), or one of the modes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvcOption(pub Option<String>);

impl SvcOption {
    /// The menu.
    pub fn all() -> Vec<Self> {
        std::iter::once(Self(None))
            .chain(
                twicecat_core::api::SCALABILITY_MODES
                    .iter()
                    .map(|m| Self(Some((*m).to_owned()))),
            )
            .collect()
    }
}

impl fmt::Display for SvcOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            None => f.write_str("Off (simulcast layers)"),
            Some(mode) => f.write_str(mode),
        }
    }
}

/// What gives way under pressure: the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DegradationOption(pub Option<DegradationChoice>);

impl DegradationOption {
    /// The menu.
    pub const ALL: [Self; 4] = [
        Self(None),
        Self(Some(DegradationChoice::KeepFrameRate)),
        Self(Some(DegradationChoice::KeepResolution)),
        Self(Some(DegradationChoice::Balanced)),
    ];
}

impl fmt::Display for DegradationOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            None => "Automatic (smooth camera, sharp share)",
            Some(DegradationChoice::KeepFrameRate) => "Keep it smooth (lower the sharpness)",
            Some(DegradationChoice::KeepResolution) => "Keep it sharp (drop frames)",
            Some(DegradationChoice::Balanced) => "A bit of both",
        })
    }
}

/// The general codec menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeneralCodecOption(pub VideoCodecChoice);

impl GeneralCodecOption {
    /// The menu.
    pub const ALL: [Self; 2] = [
        Self(VideoCodecChoice::H264),
        Self(VideoCodecChoice::Av1WhenPossible),
    ];
}

impl fmt::Display for GeneralCodecOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            VideoCodecChoice::H264 => "H.264 (every browser and app decodes it)",
            VideoCodecChoice::Av1WhenPossible => "AV1 where the encoder has it",
        })
    }
}

/// The camera's pixel format menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatOption(pub CameraFormat);

impl FormatOption {
    /// The menu.
    pub const ALL: [Self; 4] = [
        Self(CameraFormat::Auto),
        Self(CameraFormat::Nv12),
        Self(CameraFormat::Yuyv),
        Self(CameraFormat::Mjpeg),
    ];
}

impl fmt::Display for FormatOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            CameraFormat::Auto => "Automatic",
            CameraFormat::Nv12 => "NV12",
            CameraFormat::Yuyv => "YUYV",
            CameraFormat::Mjpeg => "Motion JPEG",
        })
    }
}

/// The camera's capture mode menu: automatic, or a mode the camera lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeOption(pub Option<VideoMode>);

impl fmt::Display for ModeOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            None => f.write_str("Automatic (the best for the quality)"),
            Some(m) => write!(f, "{} \u{d7} {} at {} fps", m.width, m.height, m.fps),
        }
    }
}

/// An upload limit menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapOption {
    /// No limit.
    None,
    /// A listed limit, kb/s.
    Limit(u32),
    /// Type it.
    Custom,
}

impl CapOption {
    /// The menu.
    pub fn all() -> Vec<Self> {
        let mut out = vec![Self::None];
        out.extend(CAPS.map(Self::Limit));
        out.push(Self::Custom);
        out
    }

    /// The entry that shows `cap`.
    pub fn showing(cap: Option<u32>, typing: bool) -> Self {
        match cap {
            _ if typing => Self::Custom,
            None => Self::None,
            Some(n) if CAPS.contains(&n) => Self::Limit(n),
            Some(_) => Self::Custom,
        }
    }
}

impl fmt::Display for CapOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("No limit"),
            Self::Limit(n) => write!(f, "{n} kb/s"),
            Self::Custom => f.write_str("Custom\u{2026}"),
        }
    }
}

/// A control of the Video page.
#[derive(Clone, Debug, PartialEq)]
pub enum VideoSettingsMessage {
    /// The camera menu.
    Camera(Option<VideoDeviceId>),
    /// The camera's quality.
    CameraQuality(QualityOption),
    /// The screen share's quality.
    ScreenQuality(QualityOption),
    /// "Use the graphics card to encode video".
    Hardware(bool),
    /// "Remember the screen I pick".
    RememberScreen(bool),
    /// "Use the integrated GPU on battery".
    PreferIntegrated(bool),
    /// The general codec menu.
    GeneralCodec(GeneralCodecOption),
    /// The upload limit menu.
    Cap(CapOption),
    /// The upload limit's field was typed in.
    CapText(String),
    /// **Show my camera** / **Hide my camera**.
    Preview(bool),
    /// A numbers menu of a kind's tuning.
    Number(VideoKind, Field, Pick),
    /// A custom field was typed in.
    Text(VideoKind, Field, String),
    /// Enter in a custom field: it is applied.
    Apply(VideoKind, Field),
    /// The codec menu of a kind.
    Codec(VideoKind, CodecOption),
    /// The encoder menu of a kind.
    Encoder(VideoKind, EncoderOption),
    /// The layers menu of a kind.
    Layers(VideoKind, LayersOption),
    /// The SVC menu of a kind.
    Svc(VideoKind, SvcOption),
    /// The degradation menu of a kind.
    Degradation(VideoKind, DegradationOption),
    /// **Reset to automatic** for a kind.
    Reset(VideoKind),
    /// The camera's capture mode menu.
    CaptureMode(ModeOption),
    /// The camera's pixel format menu.
    CaptureFormat(FormatOption),
}

/// The state of the Video page.
#[derive(Debug, Default)]
pub struct VideoSettingsUi {
    /// The Video page of Settings is on screen.
    pub inline: bool,
    /// The camera preview was asked for and not stopped.
    pub preview: bool,
    /// The custom fields shown (their menu says Custom).
    pub typing: HashSet<(VideoKind, Field)>,
    /// What is typed in a custom field.
    pub text: HashMap<(VideoKind, Field), String>,
    /// Fields whose typed text said nothing usable.
    pub bad: HashSet<(VideoKind, Field)>,
    /// The upload limit's custom field is shown.
    pub cap_typing: bool,
    /// What is typed in it.
    pub cap_text: String,
}

/// Most characters of a number typed in a custom field.
pub const MAX_TYPED: usize = 16;

impl App {
    /// The Video page of Settings comes on screen or goes: it asks the core
    /// for what it shows, and a camera preview it started stops with it.
    pub(crate) fn video_inline(&mut self, shown: bool) -> Task<Message> {
        if self.calls.video.ui.inline == shown {
            return Task::none();
        }
        self.calls.video.ui.inline = shown;
        if shown {
            return Task::batch([
                self.send(Command::GetVideoSettings),
                self.send(Command::ListVideoDevices),
                self.send(Command::ProbeVideoEncoders),
            ]);
        }
        self.stop_preview()
    }

    fn stop_preview(&mut self) -> Task<Message> {
        let video = &mut self.calls.video;
        video.preview = None;
        if std::mem::take(&mut video.ui.preview) {
            self.send(Command::StopCameraPreview)
        } else {
            Task::none()
        }
    }

    /// Copies the echoed settings, changes them and sends the whole value
    /// when it changed. Before the first echo there is nothing to change: the
    /// settings are asked for again.
    pub(crate) fn edit_video(&mut self, change: impl FnOnce(&mut VideoSettings)) -> Task<Message> {
        let Some(now) = &self.calls.video.settings else {
            return self.send(Command::GetVideoSettings);
        };
        let mut next = now.clone();
        change(&mut next);
        if next == *now {
            return Task::none();
        }
        // Shown at once; the core's echo replaces it with what it used.
        self.calls.video.settings = Some(next.clone());
        self.send(Command::UpdateVideoSettings(next))
    }

    fn edit_tuning(
        &mut self,
        kind: VideoKind,
        change: impl FnOnce(&mut VideoTuning),
    ) -> Task<Message> {
        self.edit_video(|s| {
            change(match kind {
                VideoKind::Camera => &mut s.camera_tuning,
                VideoKind::Screen => &mut s.screen_tuning,
            });
        })
    }

    /// A control of the Video page.
    pub(crate) fn on_video_settings(&mut self, message: VideoSettingsMessage) -> Task<Message> {
        use VideoSettingsMessage as M;
        if !self.calls.video.ui.inline {
            return Task::none();
        }
        match message {
            M::Camera(id) => {
                // A preview of the old camera ends; the person starts the
                // next one.
                let stop = self.stop_preview();
                Task::batch([stop, self.edit_video(|s| s.camera = id)])
            }
            M::CameraQuality(QualityOption(q)) => self.edit_video(|s| s.camera_quality = q),
            M::ScreenQuality(QualityOption(q)) => self.edit_video(|s| s.screen_quality = q),
            M::Hardware(on) => self.edit_video(|s| {
                s.encoder = if on {
                    EncoderChoice::Auto
                } else {
                    EncoderChoice::Prefer(EncoderBackend::Software)
                };
            }),
            M::RememberScreen(on) => self.edit_video(|s| s.remember_screen_choice = on),
            M::PreferIntegrated(on) => self.edit_video(|s| s.prefer_integrated_gpu_on_battery = on),
            M::GeneralCodec(GeneralCodecOption(choice)) => self.edit_video(|s| s.codec = choice),
            M::Cap(CapOption::None) => {
                let ui = &mut self.calls.video.ui;
                ui.cap_typing = false;
                self.edit_video(|s| s.upload_cap_kbps = None)
            }
            M::Cap(CapOption::Limit(n)) => {
                self.calls.video.ui.cap_typing = false;
                self.edit_video(|s| s.upload_cap_kbps = Some(n))
            }
            M::Cap(CapOption::Custom) => {
                let now = self
                    .calls
                    .video
                    .settings
                    .as_ref()
                    .and_then(|s| s.upload_cap_kbps);
                let ui = &mut self.calls.video.ui;
                ui.cap_typing = true;
                ui.cap_text = now.map(|n| n.to_string()).unwrap_or_default();
                Task::none()
            }
            M::CapText(text) => {
                let ui = &mut self.calls.video.ui;
                ui.cap_text = text.chars().take(MAX_TYPED).collect();
                // Applied as it is typed: a usable number is the setting.
                match ui.cap_text.trim().parse::<u32>() {
                    Ok(n) if n > 0 => self.edit_video(|s| s.upload_cap_kbps = Some(n)),
                    _ => Task::none(),
                }
            }
            M::Preview(on) => {
                let video = &mut self.calls.video;
                if on == video.ui.preview {
                    return Task::none();
                }
                if !on {
                    return self.stop_preview();
                }
                let settings = video.settings_or_default();
                video.ui.preview = true;
                video.preview = None;
                self.send(Command::StartCameraPreview {
                    device: settings.camera,
                    quality: settings.camera_quality,
                })
            }
            M::Number(kind, field, pick) => self.pick_number(kind, field, pick),
            M::Text(kind, field, text) => {
                let ui = &mut self.calls.video.ui;
                ui.text
                    .insert((kind, field), text.chars().take(MAX_TYPED).collect());
                ui.bad.remove(&(kind, field));
                Task::none()
            }
            M::Apply(kind, field) => self.apply_typed(kind, field),
            M::Codec(kind, CodecOption(codec)) => self.edit_tuning(kind, |t| {
                t.codec = codec;
                // An SVC mode needs VP9 or AV1.
                if !matches!(codec, None | Some(VideoCodec::Vp9 | VideoCodec::Av1)) {
                    t.scalability_mode = None;
                }
            }),
            M::Encoder(kind, EncoderOption(choice)) => self.edit_tuning(kind, |t| {
                t.encoder = (choice != EncoderChoice::Auto).then_some(choice);
            }),
            M::Layers(kind, LayersOption(n)) => self.edit_tuning(kind, |t| {
                t.layers = n;
                // Layers that are not sent need no settings.
                let keep = n.map_or(2, |n| usize::from(n).saturating_sub(1));
                t.lower_layers.truncate(keep);
                while t
                    .lower_layers
                    .last()
                    .is_some_and(|l| *l == LayerTuning::default())
                {
                    t.lower_layers.pop();
                }
                if n.is_some() {
                    t.scalability_mode = None;
                }
            }),
            M::Svc(kind, SvcOption(mode)) => self.edit_tuning(kind, |t| {
                t.scalability_mode = mode;
                if t.scalability_mode.is_some() {
                    // One stream.
                    t.layers = Some(1);
                    t.lower_layers.clear();
                    if !matches!(t.codec, Some(VideoCodec::Vp9 | VideoCodec::Av1)) {
                        t.codec = Some(VideoCodec::Vp9);
                    }
                }
            }),
            M::Degradation(kind, DegradationOption(choice)) => {
                self.edit_tuning(kind, |t| t.degradation = choice)
            }
            M::Reset(kind) => {
                let ui = &mut self.calls.video.ui;
                ui.typing.retain(|(k, _)| *k != kind);
                ui.text.retain(|(k, _), _| *k != kind);
                ui.bad.retain(|(k, _)| *k != kind);
                self.edit_tuning(kind, |t| *t = VideoTuning::default())
            }
            // A camera already open keeps its mode until it is opened again.
            M::CaptureMode(ModeOption(mode)) => self.edit_video(|s| s.camera_capture.mode = mode),
            M::CaptureFormat(FormatOption(format)) => {
                self.edit_video(|s| s.camera_capture.format = format)
            }
        }
    }

    fn pick_number(&mut self, kind: VideoKind, field: Field, pick: Pick) -> Task<Message> {
        let value = match pick {
            Pick::Auto => Value::Auto,
            Pick::Number(n) => Value::Number(n),
            Pick::Size(w, h) => Value::Size(w, h),
            Pick::Custom => {
                // The field opens with what is set now; nothing is sent yet.
                let now = self
                    .calls
                    .video
                    .settings
                    .as_ref()
                    .map_or(Value::Auto, |s| get(s.tuning(kind), field));
                let ui = &mut self.calls.video.ui;
                ui.typing.insert((kind, field));
                ui.text.insert((kind, field), text_of(now));
                ui.bad.remove(&(kind, field));
                return Task::none();
            }
        };
        let ui = &mut self.calls.video.ui;
        ui.typing.remove(&(kind, field));
        ui.text.remove(&(kind, field));
        ui.bad.remove(&(kind, field));
        self.edit_tuning(kind, |t| set(t, field, value))
    }

    fn apply_typed(&mut self, kind: VideoKind, field: Field) -> Task<Message> {
        let ui = &mut self.calls.video.ui;
        let text = ui.text.get(&(kind, field)).cloned().unwrap_or_default();
        match parse(field.dim, &text) {
            Some(value) => {
                ui.bad.remove(&(kind, field));
                self.edit_tuning(kind, |t| set(t, field, value))
            }
            None => {
                ui.bad.insert((kind, field));
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;
    use twicecat_core::api::{CoreEvent, VideoDevices, VideoMode};

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::settings::SettingsMessage;
    use crate::state::Startup;

    fn tuning_with(f: impl FnOnce(&mut VideoTuning)) -> VideoTuning {
        let mut t = VideoTuning::default();
        f(&mut t);
        t
    }

    #[test]
    fn a_field_reads_and_writes_the_top_and_the_lower_layers() {
        let mut t = VideoTuning::default();
        let top = Field::top(Dim::Size);
        assert_eq!(get(&t, top), Value::Auto);
        set(&mut t, top, Value::Size(1280, 720));
        assert_eq!((t.width, t.height), (Some(1280), Some(720)));
        assert_eq!(get(&t, top), Value::Size(1280, 720));
        let second = Field {
            layer: 1,
            dim: Dim::Bitrate,
        };
        let third_fps = Field {
            layer: 2,
            dim: Dim::Fps,
        };
        set(&mut t, third_fps, Value::Number(15));
        // The second layer exists, empty, so the third is at its place.
        assert_eq!(t.lower_layers.len(), 2);
        assert_eq!(get(&t, second), Value::Auto);
        assert_eq!(get(&t, third_fps), Value::Number(15));
        set(&mut t, second, Value::Number(900));
        assert_eq!(t.lower_layers[0].bitrate_kbps, Some(900));
        // Back to automatic: a layer with nothing left is dropped, and so
        // are the empty ones before it.
        set(&mut t, third_fps, Value::Auto);
        set(&mut t, second, Value::Auto);
        assert!(t.lower_layers.is_empty());
        set(&mut t, top, Value::Auto);
        assert!(t.is_automatic());
    }

    #[test]
    fn typed_text_is_read_as_a_size_or_a_number() {
        assert_eq!(parse(Dim::Size, "1600x900"), Some(Value::Size(1600, 900)));
        assert_eq!(
            parse(Dim::Size, " 800 \u{d7} 600 "),
            Some(Value::Size(800, 600))
        );
        assert_eq!(parse(Dim::Size, "1600"), None);
        assert_eq!(parse(Dim::Size, "0x900"), None);
        assert_eq!(parse(Dim::Size, "axb"), None);
        assert_eq!(parse(Dim::Fps, "24"), Some(Value::Number(24)));
        assert_eq!(parse(Dim::Bitrate, " 3500 "), Some(Value::Number(3500)));
        assert_eq!(parse(Dim::Fps, "0"), None);
        assert_eq!(parse(Dim::Fps, "-5"), None);
        assert_eq!(parse(Dim::Bitrate, "12k"), None);
        assert_eq!(text_of(Value::Size(1280, 720)), "1280x720");
        assert_eq!(text_of(Value::Auto), "");
    }

    #[test]
    fn a_menu_shows_a_listed_value_or_custom() {
        let shows = |dim, value, typing| PickOption::showing(dim, value, typing).pick;
        assert_eq!(shows(Dim::Fps, Value::Number(30), false), Pick::Number(30));
        assert_eq!(shows(Dim::Fps, Value::Number(31), false), Pick::Custom);
        assert_eq!(shows(Dim::Fps, Value::Auto, true), Pick::Custom);
        assert_eq!(
            shows(Dim::Size, Value::Size(1280, 720), false),
            Pick::Size(1280, 720)
        );
        assert_eq!(
            shows(Dim::Size, Value::Size(1281, 721), false),
            Pick::Custom
        );
        assert_eq!(shows(Dim::Bitrate, Value::Auto, false), Pick::Auto);
        // Every entry has words, and the menu starts automatic, ends custom.
        for dim in [Dim::Size, Dim::Fps, Dim::Bitrate] {
            let menu = PickOption::menu(dim);
            assert_eq!(menu.first().map(|o| o.pick), Some(Pick::Auto));
            assert_eq!(menu.last().map(|o| o.pick), Some(Pick::Custom));
            assert!(menu.iter().all(|o| !o.to_string().is_empty()));
        }
        assert_eq!(
            PickOption::menu(Dim::Size)[4].to_string(),
            "1280 \u{d7} 720 (720p)"
        );
        assert_eq!(
            CapOption::showing(Some(4_000), false),
            CapOption::Limit(4_000)
        );
        assert_eq!(CapOption::showing(Some(4_001), false), CapOption::Custom);
        assert_eq!(CapOption::showing(None, false), CapOption::None);
    }

    #[test]
    fn the_camera_menu_keeps_a_saved_camera_that_is_unplugged() {
        let cameras = vec![VideoDevice {
            id: VideoDeviceId::from("cam-a"),
            name: "Desk camera".into(),
            is_default: true,
            modes: vec![VideoMode {
                width: 1280,
                height: 720,
                fps: 30,
            }],
        }];
        let (menu, chosen) = camera_menu(&cameras, None);
        assert_eq!(menu.len(), 2);
        assert_eq!(chosen.id, None);
        let (_, chosen) = camera_menu(&cameras, Some(&VideoDeviceId::from("cam-a")));
        assert_eq!(chosen.label, "Desk camera");
        let (menu, chosen) = camera_menu(&cameras, Some(&VideoDeviceId::from("cam-gone")));
        assert_eq!(menu.len(), 3);
        assert!(chosen.label.contains("not connected"));
    }

    // ---- through App::update --------------------------------------------

    fn core(event: CoreEvent) -> Message {
        Message::Core(Box::new(event))
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// An app on the main screen, Settings open on the Video page, the
    /// video settings known, commands drained.
    fn on_the_page() -> (App, mpsc::Receiver<Command>) {
        let (tx, mut rx) = mpsc::channel(64);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let _ = app.update(core(CoreEvent::NoSession));
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: twicecat_core::api::UserId::from("@a:x"),
            device_id: twicecat_core::api::DeviceId::from("D"),
            restored: false,
        }));
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![
            twicecat_core::api::ListDiff::Reset {
                values: vec![twicecat_core::api::RoomSummary::new(
                    twicecat_core::api::RoomId::from("!a:x"),
                    "a".to_owned(),
                )],
            },
        ])));
        let _ = app.update(Message::Settings(SettingsMessage::Open));
        let _ = app.update(Message::Settings(SettingsMessage::Page(
            crate::settings::SettingsPage::Video,
        )));
        let _ = app.update(core(CoreEvent::VideoSettings(VideoSettings::default())));
        drain(&mut rx);
        (app, rx)
    }

    fn updates(sent: &[Command]) -> Vec<&VideoSettings> {
        sent.iter()
            .filter_map(|c| match c {
                Command::UpdateVideoSettings(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn opening_the_page_asks_for_what_it_shows_and_leaving_it_stops_the_preview() {
        let (tx, mut rx) = mpsc::channel(64);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let _ = app.update(core(CoreEvent::NoSession));
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: twicecat_core::api::UserId::from("@a:x"),
            device_id: twicecat_core::api::DeviceId::from("D"),
            restored: false,
        }));
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![
            twicecat_core::api::ListDiff::Reset {
                values: vec![twicecat_core::api::RoomSummary::new(
                    twicecat_core::api::RoomId::from("!a:x"),
                    "a".to_owned(),
                )],
            },
        ])));
        let _ = app.update(Message::Settings(SettingsMessage::Open));
        drain(&mut rx);
        let _ = app.update(Message::Settings(SettingsMessage::Page(
            crate::settings::SettingsPage::Video,
        )));
        let sent = drain(&mut rx);
        for wanted in ["GetVideoSettings", "ListVideoDevices", "ProbeVideoEncoders"] {
            assert!(
                sent.iter().any(|c| format!("{c:?}").starts_with(wanted)),
                "{wanted} not in {sent:?}"
            );
        }
        let _ = app.update(core(CoreEvent::VideoSettings(VideoSettings::default())));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Preview(true)));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::StartCameraPreview { .. }))
        );
        // Another page: the preview ends.
        let _ = app.update(Message::Settings(SettingsMessage::Page(
            crate::settings::SettingsPage::Keyboard,
        )));
        assert!(
            drain(&mut rx)
                .iter()
                .any(|c| matches!(c, Command::StopCameraPreview))
        );
        assert!(!app.calls.video.ui.inline && !app.calls.video.ui.preview);
        // Controls of a page that is not shown do nothing.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Hardware(
            false,
        )));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_simple_choices_edit_the_settings_and_send_them_whole() {
        let (mut app, mut rx) = on_the_page();
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::CameraQuality(
            QualityOption(twicecat_core::api::VideoQuality::Hd1080p60),
        )));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Hardware(
            false,
        )));
        let _ = app.update(Message::VideoSettings(
            VideoSettingsMessage::RememberScreen(true),
        ));
        let sent = drain(&mut rx);
        let all = updates(&sent);
        assert_eq!(all.len(), 3, "{sent:?}");
        // Each builds on the one before, before any echo.
        let last = all[2];
        assert_eq!(
            last.camera_quality,
            twicecat_core::api::VideoQuality::Hd1080p60
        );
        assert_eq!(
            last.encoder,
            EncoderChoice::Prefer(EncoderBackend::Software)
        );
        assert!(last.remember_screen_choice);
        // The same choice again sends nothing.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Hardware(
            false,
        )));
        assert!(drain(&mut rx).is_empty());
        // Hardware on again: automatic.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Hardware(true)));
        assert_eq!(updates(&drain(&mut rx))[0].encoder, EncoderChoice::Auto);
    }

    #[test]
    fn a_menu_choice_sets_a_field_and_custom_waits_for_typed_text() {
        let (mut app, mut rx) = on_the_page();
        let fps = Field::top(Dim::Fps);
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Number(
            VideoKind::Screen,
            fps,
            Pick::Number(15),
        )));
        let sent = drain(&mut rx);
        assert_eq!(updates(&sent)[0].screen_tuning.fps, Some(15));
        assert!(updates(&sent)[0].camera_tuning.is_automatic());
        // Custom: nothing is sent until a usable number is applied.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Number(
            VideoKind::Screen,
            fps,
            Pick::Custom,
        )));
        assert!(drain(&mut rx).is_empty());
        assert!(
            app.calls
                .video
                .ui
                .typing
                .contains(&(VideoKind::Screen, fps))
        );
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Text(
            VideoKind::Screen,
            fps,
            "abc".into(),
        )));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Apply(
            VideoKind::Screen,
            fps,
        )));
        assert!(drain(&mut rx).is_empty());
        assert!(app.calls.video.ui.bad.contains(&(VideoKind::Screen, fps)));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Text(
            VideoKind::Screen,
            fps,
            "22".into(),
        )));
        assert!(!app.calls.video.ui.bad.contains(&(VideoKind::Screen, fps)));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Apply(
            VideoKind::Screen,
            fps,
        )));
        assert_eq!(
            updates(&drain(&mut rx))[0].screen_tuning.fps,
            Some(22),
            "the typed number"
        );
        // Typed text is capped.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Text(
            VideoKind::Screen,
            fps,
            "1".repeat(200),
        )));
        assert_eq!(
            app.calls.video.ui.text[&(VideoKind::Screen, fps)].len(),
            MAX_TYPED
        );
    }

    #[test]
    fn layers_svc_and_codec_keep_each_other_consistent() {
        let (mut app, mut rx) = on_the_page();
        let cam = VideoKind::Camera;
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Svc(
            cam,
            SvcOption(Some("L3T3_KEY".into())),
        )));
        let t = updates(&drain(&mut rx))[0].camera_tuning.clone();
        // An SVC mode makes one stream and needs VP9 or AV1.
        assert_eq!(t.scalability_mode.as_deref(), Some("L3T3_KEY"));
        assert_eq!(t.layers, Some(1));
        assert_eq!(t.codec, Some(VideoCodec::Vp9));
        // H.264 does not do SVC: the mode goes.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Codec(
            cam,
            CodecOption(Some(VideoCodec::H264)),
        )));
        let t = updates(&drain(&mut rx))[0].camera_tuning.clone();
        assert_eq!(t.scalability_mode, None);
        // Choosing a number of layers drops the SVC mode and the layers that
        // are not sent.
        let second = Field {
            layer: 2,
            dim: Dim::Bitrate,
        };
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Number(
            cam,
            second,
            Pick::Number(500),
        )));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Layers(
            cam,
            LayersOption(Some(2)),
        )));
        let sent = drain(&mut rx);
        let t = updates(&sent)
            .last()
            .expect("an update")
            .camera_tuning
            .clone();
        assert_eq!(t.layers, Some(2));
        assert!(t.lower_layers.is_empty(), "{t:?}");
    }

    #[test]
    fn reset_brings_back_automatic_for_one_kind_only() {
        let (mut app, mut rx) = on_the_page();
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Number(
            VideoKind::Camera,
            Field::top(Dim::Fps),
            Pick::Number(24),
        )));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Number(
            VideoKind::Screen,
            Field::top(Dim::Fps),
            Pick::Number(15),
        )));
        drain(&mut rx);
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Reset(
            VideoKind::Camera,
        )));
        let sent = drain(&mut rx);
        let s = updates(&sent)[0];
        assert!(s.camera_tuning.is_automatic());
        assert_eq!(s.screen_tuning.fps, Some(15));
    }

    #[test]
    fn the_upload_cap_and_the_capture_choices_reach_the_settings() {
        let (mut app, mut rx) = on_the_page();
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Cap(
            CapOption::Limit(8_000),
        )));
        assert_eq!(updates(&drain(&mut rx))[0].upload_cap_kbps, Some(8_000));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Cap(
            CapOption::Custom,
        )));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::CapText(
            "3000".into(),
        )));
        assert_eq!(updates(&drain(&mut rx))[0].upload_cap_kbps, Some(3_000));
        // Text that is no number changes nothing.
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::CapText(
            "x".into(),
        )));
        assert!(drain(&mut rx).is_empty());
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Cap(
            CapOption::None,
        )));
        assert_eq!(updates(&drain(&mut rx))[0].upload_cap_kbps, None);
        let mode = VideoMode {
            width: 1920,
            height: 1080,
            fps: 30,
        };
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::CaptureMode(
            ModeOption(Some(mode)),
        )));
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::CaptureFormat(
            FormatOption(CameraFormat::Mjpeg),
        )));
        let sent = drain(&mut rx);
        let last = updates(&sent).last().expect("an update").camera_capture;
        assert_eq!(last.mode, Some(mode));
        assert_eq!(last.format, CameraFormat::Mjpeg);
    }

    #[test]
    fn before_the_first_echo_a_change_asks_for_the_settings_instead() {
        let (tx, mut rx) = mpsc::channel(64);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        app.calls.video.ui.inline = true;
        let _ = app.update(Message::VideoSettings(VideoSettingsMessage::Hardware(
            false,
        )));
        let sent = drain(&mut rx);
        assert!(sent.iter().any(|c| matches!(c, Command::GetVideoSettings)));
        assert!(updates(&sent).is_empty());
        let _ = tuning_with(|_| {});
        let _ = VideoDevices::default();
    }
}
