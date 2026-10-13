//! The Voice & audio card (Phase 6 call UI plan, Task 6, Q5): devices,
//! when the microphone sends (voice activity, push-to-talk, toggle-to-talk;
//! the push-to-talk plan's Task 4, #15), microphone processing, levels, the
//! mic test, the ringtone, cleaning other voices and what others learn
//! about ours.
//!
//! The card shows the audio settings as the core last echoed them
//! ([`crate::calls::CallModel::settings`]). Every change copies that value,
//! changes one field and sends it whole with `UpdateAudioSettings`; the
//! core validates, saves and echoes it back, so a clamped value snaps back
//! on screen. Pure: only `App`'s handlers here send commands.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use iced::Task;
use iced::keyboard::Modifiers;
use iced::keyboard::key::{Code, Physical};
use twicecat_core::api::{
    AudioDevice, AudioDeviceId, AudioDevices, AudioSettings, AudioStats, Command, EnhancerBackend,
    GpuBackend, GpuEnhancement, InputLevel, InputMode, NoiseSuppression, NsLevel, SkipCleaned,
    limits,
};
use twicecat_ptt::{GamepadButton, KeyBinding};

use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder};

/// A slider moved by the wheel or the keys has no release: its value is
/// sent after this pause, as for the participants' volumes.
pub const SLIDER_SETTLE: Duration = Duration::from_millis(400);
/// The bitrates offered, kb/s (the core accepts 16 to 510).
pub const BITRATES: [u16; 7] = [16, 24, 32, 48, 64, 96, 128];
/// The level meter's floor, dBFS: quieter reads as empty.
pub const METER_FLOOR_DB: f32 = -60.0;
/// The DeepFilterNet3 attenuation limit given when it is picked.
pub const DEFAULT_DF_LIMIT_DB: f32 = 100.0;
/// The release delay push-to-talk starts with, coming from voice activity
/// (the roadmap's "about 200 ms", so word endings are not cut).
pub const DEFAULT_RELEASE_MS: u32 = 200;
/// The voice activity threshold shown before one is set, dBFS (the core's
/// default).
pub const DEFAULT_THRESHOLD_DB: f32 = -50.0;
/// Said when a captured key cannot be a push-to-talk key.
pub const UNBINDABLE_KEY: &str = "This key cannot be used for push-to-talk. Try another, or Esc.";

/// A slider of the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slider {
    /// Gain after processing, dB.
    InputGain,
    /// Master playback volume, percent.
    MasterVolume,
    /// DeepFilterNet3's attenuation limit on the microphone, dB.
    MicCleaningLimit,
    /// The attenuation limit for other voices, dB.
    OthersCleaningLimit,
    /// The most automatic gain may boost, dB.
    AutoGainMax,
    /// The voice activity threshold, dBFS.
    Threshold,
    /// How long push-to-talk keeps sending after a release, ms.
    ReleaseDelay,
}

impl Slider {
    /// The slider's value in `settings`.
    pub fn value(self, settings: &AudioSettings) -> f32 {
        match self {
            Self::InputGain => settings.input_gain_db,
            Self::MasterVolume => f32::from(settings.output_volume_percent),
            Self::MicCleaningLimit => match settings.processing.noise_suppression {
                NoiseSuppression::DeepFilter {
                    attenuation_limit_db,
                    ..
                } => attenuation_limit_db,
                _ => DEFAULT_DF_LIMIT_DB,
            },
            Self::OthersCleaningLimit => settings.remote_cleaning.attenuation_limit_db,
            Self::AutoGainMax => settings.processing.auto_gain_max_db,
            Self::Threshold => match settings.input_mode {
                InputMode::VoiceActivity { threshold_db, .. } => threshold_db,
                InputMode::PushToTalk { .. } => DEFAULT_THRESHOLD_DB,
            },
            Self::ReleaseDelay => match settings.input_mode {
                InputMode::PushToTalk {
                    release_delay_ms, ..
                } => release_delay_ms as f32,
                InputMode::VoiceActivity { .. } => DEFAULT_RELEASE_MS as f32,
            },
        }
    }

    /// The slider's bounds.
    pub fn range(self) -> (f32, f32) {
        match self {
            Self::InputGain => limits::INPUT_GAIN_DB,
            Self::MasterVolume => (0.0, f32::from(limits::VOLUME_PERCENT)),
            Self::MicCleaningLimit | Self::OthersCleaningLimit => limits::DF_ATTENUATION_DB,
            Self::AutoGainMax => limits::AUTO_GAIN_MAX_DB,
            Self::Threshold => limits::VAD_THRESHOLD_DB,
            Self::ReleaseDelay => (0.0, limits::PTT_RELEASE_DELAY_MS as f32),
        }
    }

    /// What **−** and **+** beside it change.
    pub fn step(self) -> f32 {
        match self {
            Self::InputGain => 1.0,
            Self::MasterVolume => 10.0,
            Self::MicCleaningLimit | Self::OthersCleaningLimit | Self::AutoGainMax => 5.0,
            Self::Threshold => 1.0,
            Self::ReleaseDelay => 50.0,
        }
    }

    /// Sets the slider's value in `settings`; the core clamps it.
    pub fn apply(self, settings: &mut AudioSettings, value: f32) {
        match self {
            Self::InputGain => settings.input_gain_db = value,
            Self::MasterVolume => {
                settings.output_volume_percent = value.round().clamp(0.0, 200.0) as u16;
            }
            Self::MicCleaningLimit => {
                if let NoiseSuppression::DeepFilter {
                    attenuation_limit_db,
                    ..
                } = &mut settings.processing.noise_suppression
                {
                    *attenuation_limit_db = value;
                }
            }
            Self::OthersCleaningLimit => settings.remote_cleaning.attenuation_limit_db = value,
            Self::AutoGainMax => settings.processing.auto_gain_max_db = value,
            Self::Threshold => {
                if let InputMode::VoiceActivity { threshold_db, .. } = &mut settings.input_mode {
                    *threshold_db = value;
                }
            }
            Self::ReleaseDelay => {
                if let InputMode::PushToTalk {
                    release_delay_ms, ..
                } = &mut settings.input_mode
                {
                    *release_delay_ms = value.round().max(0.0) as u32;
                }
            }
        }
    }
}

/// When the microphone sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeChoice {
    /// While a voice is heard.
    VoiceActivity,
    /// While the key or button is held.
    PushToTalk,
    /// From one press to the next.
    ToggleToTalk,
}

impl ModeChoice {
    /// The menu, in order.
    pub const ALL: [Self; 3] = [Self::VoiceActivity, Self::PushToTalk, Self::ToggleToTalk];

    /// The entry of a setting.
    pub fn of(mode: &InputMode) -> Self {
        match mode {
            InputMode::VoiceActivity { .. } => Self::VoiceActivity,
            InputMode::PushToTalk { toggle: false, .. } => Self::PushToTalk,
            InputMode::PushToTalk { toggle: true, .. } => Self::ToggleToTalk,
        }
    }

    /// The setting for this entry. Between push-to-talk and toggle-to-talk
    /// the release delay is kept; from voice activity it starts at
    /// [`DEFAULT_RELEASE_MS`]; back to voice activity the threshold is
    /// automatic.
    pub fn setting(self, now: &InputMode) -> InputMode {
        let release_delay_ms = match now {
            InputMode::PushToTalk {
                release_delay_ms, ..
            } => *release_delay_ms,
            InputMode::VoiceActivity { .. } => DEFAULT_RELEASE_MS,
        };
        match self {
            Self::VoiceActivity => match now {
                InputMode::VoiceActivity { .. } => *now,
                InputMode::PushToTalk { .. } => InputMode::VoiceActivity {
                    automatic: true,
                    threshold_db: DEFAULT_THRESHOLD_DB,
                },
            },
            Self::PushToTalk | Self::ToggleToTalk => InputMode::PushToTalk {
                release_delay_ms,
                toggle: self == Self::ToggleToTalk,
            },
        }
    }
}

impl fmt::Display for ModeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::VoiceActivity => "Voice activity (sends when you speak)",
            Self::PushToTalk => "Push-to-talk (sends while held)",
            Self::ToggleToTalk => "Toggle-to-talk (press to start, press to stop)",
        })
    }
}

/// An entry of the gamepad button menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonChoice {
    /// No button.
    None,
    /// A button.
    Button(GamepadButton),
    /// The saved button is not one Twicecat knows (shown, not offered).
    Unknown,
}

impl ButtonChoice {
    /// The menu: none, then every button; plus the unknown saved one.
    pub fn menu(saved: Option<&str>) -> (Vec<Self>, Self) {
        let mut menu = vec![Self::None];
        menu.extend(
            GamepadButton::names()
                .filter_map(|name| name.parse().ok())
                .map(Self::Button),
        );
        let chosen = match saved {
            None => Self::None,
            Some(text) => match text.parse() {
                Ok(button) => Self::Button(button),
                Err(_) => {
                    menu.push(Self::Unknown);
                    Self::Unknown
                }
            },
        };
        (menu, chosen)
    }
}

impl fmt::Display for ButtonChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let button = match self {
            Self::None => return f.write_str("None"),
            Self::Unknown => return f.write_str("Saved button (not recognised)"),
            Self::Button(button) => *button,
        };
        let name = button.to_string();
        // By position, then what common pads print on it.
        let label = match name.as_str() {
            "south" => "Bottom face button (A, \u{2715})",
            "east" => "Right face button (B, \u{25cb})",
            "north" => "Top face button (Y, \u{25b3})",
            "west" => "Left face button (X, \u{25a1})",
            "c" => "C button",
            "z" => "Z button",
            "left_trigger" => "Left bumper (LB, L1)",
            "left_trigger2" => "Left trigger (LT, L2)",
            "right_trigger" => "Right bumper (RB, R1)",
            "right_trigger2" => "Right trigger (RT, R2)",
            "select" => "Select (View, Share)",
            "start" => "Start (Menu, Options)",
            "mode" => "Guide (Xbox, PS, Steam)",
            "left_thumb" => "Left stick press (L3)",
            "right_thumb" => "Right stick press (R3)",
            "dpad_up" => "D-pad up",
            "dpad_down" => "D-pad down",
            "dpad_left" => "D-pad left",
            "dpad_right" => "D-pad right",
            other => other,
        };
        f.write_str(label)
    }
}

/// A stored key binding as people write it: `Ctrl+Shift+T`, `F13`. A
/// binding that does not parse is shown as stored.
pub fn key_label(binding: &str) -> String {
    let Ok(parsed) = binding.parse::<KeyBinding>() else {
        return binding.to_owned();
    };
    let text = parsed.to_string();
    let mut parts: Vec<&str> = text.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let mut out = String::new();
    let logo = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Super"
    };
    for (written, shown) in [
        ("control", "Ctrl"),
        ("alt", "Alt"),
        ("shift", "Shift"),
        ("super", logo),
    ] {
        if parts.iter().any(|p| p.eq_ignore_ascii_case(written)) {
            out.push_str(shown);
            out.push('+');
        }
    }
    let key = key
        .strip_prefix("Key")
        .or_else(|| key.strip_prefix("Digit"))
        .unwrap_or(key);
    out.push_str(key);
    out
}

/// A key pressed while the card waits for the push-to-talk key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturedKey {
    /// The physical key.
    pub code: Code,
    /// The modifiers held.
    pub modifiers: Modifiers,
}

/// What a captured key means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capture {
    /// A modifier alone: keep waiting for the key.
    Wait,
    /// Esc: stop waiting.
    Cancel,
    /// The binding, in the stored syntax.
    Bound(String),
    /// Not a key push-to-talk can use.
    Refused,
}

/// Turns a captured key into a binding.
pub fn capture(key: CapturedKey) -> Capture {
    use Code as C;
    match key.code {
        C::Escape => Capture::Cancel,
        C::ShiftLeft
        | C::ShiftRight
        | C::ControlLeft
        | C::ControlRight
        | C::AltLeft
        | C::AltRight
        | C::SuperLeft
        | C::SuperRight
        | C::Meta
        | C::Hyper
        | C::Fn
        | C::FnLock => Capture::Wait,
        code => match KeyBinding::from_key(
            &crate::ptt::code_name(code),
            crate::ptt::held(key.modifiers),
        ) {
            Ok(binding) => Capture::Bound(binding.to_string()),
            Err(_) => Capture::Refused,
        },
    }
}

/// The capture message for a window event: key presses (not repeats) of
/// identified keys.
pub fn capture_message(event: &iced::Event) -> Option<Message> {
    let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        physical_key: Physical::Code(code),
        modifiers,
        repeat: false,
        ..
    }) = event
    else {
        return None;
    };
    Some(Message::Audio(AudioMessage::Captured(CapturedKey {
        code: *code,
        modifiers: *modifiers,
    })))
}

/// An entry of a device menu: a device, or the system default (`id: None`).
#[derive(Clone, Debug)]
pub struct DeviceChoice {
    /// What the setting stores.
    pub id: Option<AudioDeviceId>,
    /// What the menu shows.
    pub label: String,
}

impl PartialEq for DeviceChoice {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl fmt::Display for DeviceChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// The menu "System default" and the devices, plus the saved device when it
/// is not connected now (it stays chosen and comes back when plugged in);
/// and the entry that is chosen.
pub fn device_menu(
    devices: &[AudioDevice],
    chosen: Option<&AudioDeviceId>,
    names: &HashMap<AudioDeviceId, String>,
) -> (Vec<DeviceChoice>, DeviceChoice) {
    let mut menu = vec![DeviceChoice {
        id: None,
        label: "System default".to_owned(),
    }];
    menu.extend(devices.iter().map(|d| DeviceChoice {
        id: Some(d.id.clone()),
        label: name_or_placeholder(&d.name, MAX_NAME_CHARS),
    }));
    let selected = match chosen {
        None => menu[0].clone(),
        Some(id) => match menu.iter().find(|c| c.id.as_ref() == Some(id)) {
            Some(found) => found.clone(),
            None => {
                let name = names.get(id).map_or("Saved device", String::as_str);
                let missing = DeviceChoice {
                    id: Some(id.clone()),
                    label: format!(
                        "{} (not connected)",
                        name_or_placeholder(name, MAX_NAME_CHARS)
                    ),
                };
                menu.push(missing.clone());
                missing
            }
        },
    };
    (menu, selected)
}

/// The noise suppression menu. Each entry says where it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseChoice {
    /// None.
    Off,
    /// WebRTC's classic suppressor (CPU).
    WebRtc,
    /// RNNoise (CPU).
    RnNoise,
    /// DeepFilterNet3 on the CPU.
    DeepFilter,
    /// DeepFilterNet3 on the GPU, with the backend it starts on when the
    /// probe has named one.
    DeepFilterGpu(Option<GpuBackend>),
}

impl NoiseChoice {
    /// The menu, in order. The GPU entry is there when the probe found a
    /// GPU, or when it is the setting (a GPU set up on another machine).
    pub fn menu(gpu: Option<&GpuEnhancement>, now: &NoiseSuppression) -> Vec<Self> {
        let mut menu = vec![Self::Off, Self::WebRtc, Self::RnNoise, Self::DeepFilter];
        let set = matches!(now, NoiseSuppression::DeepFilter { gpu: true, .. });
        if set || gpu_used(gpu).is_some() {
            menu.push(Self::DeepFilterGpu(gpu_used(gpu)));
        }
        menu
    }

    /// The entry of a setting.
    pub fn of(ns: &NoiseSuppression, gpu: Option<&GpuEnhancement>) -> Self {
        match ns {
            NoiseSuppression::Off => Self::Off,
            NoiseSuppression::WebRtc { .. } => Self::WebRtc,
            NoiseSuppression::RnNoise => Self::RnNoise,
            NoiseSuppression::DeepFilter { gpu: false, .. } => Self::DeepFilter,
            NoiseSuppression::DeepFilter { gpu: true, .. } => Self::DeepFilterGpu(gpu_used(gpu)),
        }
    }

    /// The setting for this entry. Picking the one already set keeps its
    /// options, and moving DeepFilterNet3 between the CPU and the GPU keeps
    /// its limit.
    pub fn setting(self, now: &NoiseSuppression) -> NoiseSuppression {
        let on_gpu = match self {
            Self::Off => return NoiseSuppression::Off,
            Self::WebRtc => {
                return match now {
                    NoiseSuppression::WebRtc { .. } => *now,
                    _ => NoiseSuppression::WebRtc {
                        level: NsLevel::Moderate,
                    },
                };
            }
            Self::RnNoise => return NoiseSuppression::RnNoise,
            Self::DeepFilter => false,
            Self::DeepFilterGpu(_) => true,
        };
        let attenuation_limit_db = match now {
            NoiseSuppression::DeepFilter {
                attenuation_limit_db,
                ..
            } => *attenuation_limit_db,
            _ => DEFAULT_DF_LIMIT_DB,
        };
        NoiseSuppression::DeepFilter {
            attenuation_limit_db,
            gpu: on_gpu,
        }
    }
}

impl fmt::Display for NoiseChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => f.write_str("Off"),
            Self::WebRtc => f.write_str("WebRTC (light, CPU)"),
            Self::RnNoise => f.write_str("RNNoise (CPU, default)"),
            Self::DeepFilter => f.write_str("DeepFilterNet3 (best, CPU)"),
            Self::DeepFilterGpu(Some(backend)) => {
                write!(f, "DeepFilterNet3 on the GPU ({})", gpu_name(*backend))
            }
            Self::DeepFilterGpu(None) => f.write_str("DeepFilterNet3 on the GPU"),
        }
    }
}

/// The backend a GPU choice starts on: the probe lists the backends in the
/// core's order of preference, and the core takes the first that starts.
pub fn gpu_used(gpu: Option<&GpuEnhancement>) -> Option<GpuBackend> {
    gpu.and_then(|gpu| gpu.available.first().copied())
}

/// The card's GPU line: what the probe found, or why there is no GPU.
pub fn gpu_status(gpu: Option<&GpuEnhancement>) -> String {
    let Some(found) = gpu else {
        return "GPU: looking for one\u{2026}".to_owned();
    };
    let Some((used, others)) = found.available.split_first() else {
        return format!("GPU: {}", no_gpu_note(gpu).unwrap_or(NO_GPU_FOUND));
    };
    let mut line = format!("GPU: {} is used.", gpu_name(*used));
    if !others.is_empty() {
        let others: Vec<&str> = others.iter().map(|b| gpu_name(*b)).collect();
        line.push_str(&format!(" Also here: {}.", others.join(", ")));
    }
    line
}

/// The WebRTC suppressor's strength menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelChoice(pub NsLevel);

impl LevelChoice {
    /// The menu, in order.
    pub const ALL: [Self; 4] = [
        Self(NsLevel::Low),
        Self(NsLevel::Moderate),
        Self(NsLevel::High),
        Self(NsLevel::VeryHigh),
    ];
}

impl fmt::Display for LevelChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            NsLevel::Low => "Low (about 6 dB)",
            NsLevel::Moderate => "Moderate (about 12 dB)",
            NsLevel::High => "High (about 18 dB)",
            NsLevel::VeryHigh => "Very high (about 21 dB)",
        })
    }
}

/// Which voices cleaning leaves alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkipChoice(pub SkipCleaned);

impl SkipChoice {
    /// The menu, in order.
    pub const ALL: [Self; 3] = [
        Self(SkipCleaned::Neural),
        Self(SkipCleaned::AnySuppressor),
        Self(SkipCleaned::Never),
    ];
}

impl fmt::Display for SkipChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            SkipCleaned::Neural => "Skip voices cleaned by a neural suppressor (default)",
            SkipCleaned::AnySuppressor => "Skip voices cleaned by any suppressor",
            SkipCleaned::Never => "Clean every voice",
        })
    }
}

/// A voice bitrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bitrate(pub u16);

impl fmt::Display for Bitrate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} kb/s", self.0)
    }
}

/// The bitrate menu: [`BITRATES`], plus the current value when it is not one
/// of them (a value set elsewhere, up to 510), in order.
pub fn bitrate_menu(current: u16) -> Vec<Bitrate> {
    let mut menu: Vec<u16> = BITRATES.to_vec();
    if !menu.contains(&current) {
        menu.push(current);
        menu.sort_unstable();
    }
    menu.into_iter().map(Bitrate).collect()
}

/// Where cleaning runs, in words.
pub fn backend_label(backend: EnhancerBackend) -> String {
    match backend {
        EnhancerBackend::Cpu => "Running on the CPU.".to_owned(),
        EnhancerBackend::Gpu { backend } => format!("Running on the GPU ({}).", gpu_name(backend)),
        EnhancerBackend::CpuAfterGpuError => "Running on the CPU: the GPU failed.".to_owned(),
        EnhancerBackend::CpuAfterGpuTooSlow => {
            "Running on the CPU for now: the GPU fell behind (a game may be using it). \
             It is tried again in a while."
                .to_owned()
        }
    }
}

/// The light build has no GPU support at all.
pub const NO_GPU_IN_BUILD: &str = "This is the light build: it cleans on the CPU only. The full build (cargo feature gpu) can use the GPU.";
/// The full build found no GPU it can use.
pub const NO_GPU_FOUND: &str =
    "No usable GPU found. The GPU needs ONNX Runtime and the GPU model (see docs/building.md).";

/// Why DeepFilterNet3 offers no GPU, once the probe has answered with none;
/// `None` while a GPU is offered or before the answer.
pub fn no_gpu_note(gpu: Option<&GpuEnhancement>) -> Option<&'static str> {
    match gpu {
        Some(gpu) if gpu.available.is_empty() => Some(if cfg!(feature = "gpu") {
            NO_GPU_FOUND
        } else {
            NO_GPU_IN_BUILD
        }),
        _ => None,
    }
}

/// A backend by vendor and API: NVIDIA's own (CUDA, TensorRT) or one that
/// runs on any vendor's GPU.
pub fn gpu_name(backend: GpuBackend) -> &'static str {
    match backend {
        GpuBackend::Cuda => "NVIDIA \u{b7} CUDA",
        GpuBackend::TensorRt => "NVIDIA \u{b7} TensorRT",
        GpuBackend::WebGpu => WEBGPU_NAME,
        GpuBackend::DirectMl => "any GPU \u{b7} DirectML",
        GpuBackend::CoreMl => "Apple \u{b7} Core ML",
    }
}

/// WebGPU runs on the system's own graphics API.
const WEBGPU_NAME: &str = if cfg!(target_os = "windows") {
    "any GPU \u{b7} WebGPU on Direct3D 12"
} else if cfg!(target_os = "macos") {
    "any GPU \u{b7} WebGPU on Metal"
} else {
    "any GPU \u{b7} WebGPU on Vulkan"
};

/// The level meter: how full (0 to 1, from [`METER_FLOOR_DB`] to 0 dBFS),
/// and its words.
pub fn meter(level: InputLevel) -> (f32, String) {
    let fill = |db: f32| {
        if db.is_nan() {
            0.0
        } else {
            ((db - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0)
        }
    };
    let gate = if level.transmitting {
        "sending"
    } else {
        "not sending"
    };
    (
        fill(level.rms_db),
        format!(
            "{:.0} dB, peak {:.0} dB, {gate}",
            level.rms_db.max(METER_FLOOR_DB),
            level.peak_db.max(METER_FLOOR_DB)
        ),
    )
}

/// The diagnostics lines of the latest statistics.
pub fn diagnostics(stats: &AudioStats) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(erle) = stats.echo_return_loss_enhancement_db {
        lines.push(format!("Echo removed: {erle:.0} dB"));
    }
    if let Some(likelihood) = stats.residual_echo_likelihood {
        lines.push(format!(
            "Echo left, likelihood: {:.0} %",
            likelihood * 100.0
        ));
    }
    if let Some(delay) = stats.echo_delay_ms {
        lines.push(format!("Echo delay: {delay} ms"));
    }
    lines.push(format!(
        "Processing: {} \u{b5}s per 10 ms",
        stats.processing_us
    ));
    lines.push(format!(
        "Late wake-ups: {} (longest {} \u{b5}s)",
        stats.late_wakes, stats.max_wake_us
    ));
    lines.push(format!(
        "Capture overruns: {}, playback underruns: {}",
        stats.capture_overruns, stats.playback_underruns
    ));
    lines
}

/// The card's own state; the settings, devices and meters are the call
/// model's.
#[derive(Debug, Default)]
pub struct AudioUi {
    /// The card is open.
    pub open: bool,
    /// The mic test this card started: `Some(loopback)` while it runs.
    pub mic_test: Option<bool>,
    /// "Hear myself" for the mic test.
    pub loopback: bool,
    /// A slider being moved and its value, until it is sent.
    pub drag: Option<(Slider, f32)>,
    /// Counts slider moves, so only the last pause sends.
    pub drag_seq: u64,
    /// The diagnostics lines are shown.
    pub diagnostics: bool,
    /// Device names seen this session, for a saved device that is not
    /// connected now.
    pub names: HashMap<AudioDeviceId, String>,
    /// Waiting for the push-to-talk key: the shortcuts and the window key
    /// are off meanwhile.
    pub capturing: bool,
    /// Why the last captured key was refused.
    pub capture_note: Option<&'static str>,
}

impl AudioUi {
    /// The value a slider shows: the one being moved, else the setting.
    pub fn shown(&self, slider: Slider, settings: &AudioSettings) -> f32 {
        match self.drag {
            Some((moving, value)) if moving == slider => value,
            _ => slider.value(settings),
        }
    }

    /// Remembers the names of the devices listed.
    pub fn learn_names(&mut self, devices: &AudioDevices) {
        for device in devices.inputs.iter().chain(&devices.outputs) {
            self.names.insert(device.id.clone(), device.name.clone());
        }
    }
}

/// A control of the Voice & audio card.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioMessage {
    /// Open the card (Settings, the call bar).
    Open,
    /// **Close**, Esc.
    Close,
    /// The microphone menu.
    InputDevice(DeviceChoice),
    /// The speakers menu.
    OutputDevice(DeviceChoice),
    /// The noise suppression menu.
    Noise(NoiseChoice),
    /// The WebRTC strength menu.
    Level(LevelChoice),
    /// Echo cancellation.
    EchoCancellation(bool),
    /// Also cancel other apps' sound.
    CancelSystemAudio(bool),
    /// Automatic gain.
    AutoGain(bool),
    /// Remove low rumble.
    HighPass(bool),
    /// A slider moved.
    Slide(Slider, f32),
    /// A slider was let go.
    SlideReleased,
    /// **−** or **+** beside a slider: one step, sent at once.
    Step(Slider, bool),
    /// The pause after a slider move ended; stale numbers do nothing.
    SlideDue(u64),
    /// The bitrate menu.
    Bitrate(Bitrate),
    /// **Test the microphone** / **Stop the test**.
    MicTest,
    /// "Hear myself".
    Loopback(bool),
    /// The ringtone.
    Ringtone(bool),
    /// Clean other voices.
    CleanOthers(bool),
    /// Which voices cleaning leaves alone.
    Skip(SkipChoice),
    /// Clean other voices on a GPU.
    CleanOthersGpu(bool),
    /// Tell others our microphone's cleaning.
    Announce(bool),
    /// Our microphone is cleaned outside Twicecat.
    CleanedOutside(bool),
    /// Show or hide the diagnostics.
    Diagnostics,
    /// The input mode menu.
    Mode(ModeChoice),
    /// Pick the voice activity threshold automatically.
    AutoThreshold(bool),
    /// **Set key…**: wait for the push-to-talk key.
    CaptureKey,
    /// **Cancel** while waiting.
    CancelCapture,
    /// A key pressed while waiting.
    Captured(CapturedKey),
    /// **Clear** beside the key.
    ClearKey,
    /// The gamepad button menu.
    Button(ButtonChoice),
}

impl App {
    /// A control of the Voice & audio card.
    pub(crate) fn on_audio(&mut self, message: AudioMessage) -> Task<Message> {
        use AudioMessage as M;
        if !self.audio_ui.open && message != M::Open {
            return Task::none();
        }
        match message {
            M::Open => {
                self.audio_ui.open = true;
                Task::batch([
                    self.send(Command::GetAudioSettings),
                    self.send(Command::ListAudioDevices),
                    self.send(Command::ProbeGpuEnhancement),
                ])
            }
            M::Close => self.close_audio_card(),
            M::InputDevice(choice) => self.edit_audio(|s| s.input_device = choice.id),
            M::OutputDevice(choice) => self.edit_audio(|s| s.output_device = choice.id),
            M::Noise(choice) => self.edit_audio(|s| {
                let p = &mut s.processing;
                p.noise_suppression = choice.setting(&p.noise_suppression);
            }),
            M::Level(LevelChoice(new)) => self.edit_audio(|s| {
                if let NoiseSuppression::WebRtc { level } = &mut s.processing.noise_suppression {
                    *level = new;
                }
            }),
            M::EchoCancellation(on) => self.edit_audio(|s| {
                s.processing.echo_cancellation = on;
                // It extends echo cancellation and does nothing without it.
                if !on {
                    s.processing.cancel_system_audio = false;
                }
            }),
            M::CancelSystemAudio(on) => self.edit_audio(|s| {
                if s.processing.echo_cancellation {
                    s.processing.cancel_system_audio = on;
                }
            }),
            M::AutoGain(on) => self.edit_audio(|s| s.processing.auto_gain = on),
            M::HighPass(on) => self.edit_audio(|s| s.processing.high_pass = on),
            M::Slide(slider, value) => {
                self.audio_ui.drag = Some((slider, value));
                self.audio_ui.drag_seq = self.audio_ui.drag_seq.wrapping_add(1);
                let seq = self.audio_ui.drag_seq;
                self.timer(SLIDER_SETTLE, Message::Audio(M::SlideDue(seq)))
            }
            M::SlideReleased => self.send_slider(),
            M::Step(slider, up) => {
                let Some(settings) = &self.calls.settings else {
                    return self.send(Command::GetAudioSettings);
                };
                let now = self.audio_ui.shown(slider, settings);
                let (lo, hi) = slider.range();
                let next = if up {
                    now + slider.step()
                } else {
                    now - slider.step()
                }
                .clamp(lo, hi);
                self.audio_ui.drag = None;
                self.edit_audio(|s| slider.apply(s, next))
            }
            M::SlideDue(seq) if seq == self.audio_ui.drag_seq => self.send_slider(),
            M::SlideDue(_) => Task::none(),
            M::Bitrate(Bitrate(kbps)) => self.edit_audio(|s| s.voice_bitrate_kbps = kbps),
            M::MicTest => self.toggle_mic_test(),
            M::Loopback(on) => {
                self.audio_ui.loopback = on;
                // A running test starts again with the new choice (the core
                // replaces it).
                if self.audio_ui.mic_test.is_some() {
                    self.audio_ui.mic_test = Some(on);
                    self.send(Command::StartMicTest { loopback: on })
                } else {
                    Task::none()
                }
            }
            M::Ringtone(on) => self.edit_audio(|s| s.ringtone = on),
            M::CleanOthers(on) => self.edit_audio(|s| s.remote_cleaning.enabled = on),
            M::Skip(SkipChoice(skip)) => self.edit_audio(|s| s.remote_cleaning.skip_cleaned = skip),
            M::CleanOthersGpu(on) => self.edit_audio(|s| s.remote_cleaning.gpu = on),
            M::Announce(on) => self.edit_audio(|s| s.announce_cleaning.enabled = on),
            M::CleanedOutside(on) => self.edit_audio(|s| s.announce_cleaning.cleaned_outside = on),
            M::Diagnostics => {
                self.audio_ui.diagnostics = !self.audio_ui.diagnostics;
                Task::none()
            }
            M::Mode(choice) => self.edit_audio(|s| s.input_mode = choice.setting(&s.input_mode)),
            M::AutoThreshold(on) => self.edit_audio(|s| {
                if let InputMode::VoiceActivity { automatic, .. } = &mut s.input_mode {
                    *automatic = on;
                }
            }),
            M::CaptureKey => {
                self.audio_ui.capturing = true;
                self.audio_ui.capture_note = None;
                Task::none()
            }
            M::CancelCapture => {
                self.stop_capture();
                Task::none()
            }
            M::Captured(_) if !self.audio_ui.capturing => Task::none(),
            M::Captured(key) => match capture(key) {
                Capture::Wait => Task::none(),
                Capture::Cancel => {
                    self.stop_capture();
                    Task::none()
                }
                Capture::Refused => {
                    self.audio_ui.capture_note = Some(UNBINDABLE_KEY);
                    Task::none()
                }
                Capture::Bound(binding) => {
                    self.stop_capture();
                    self.edit_audio(|s| s.push_to_talk.key = Some(binding))
                }
            },
            M::ClearKey => self.edit_audio(|s| s.push_to_talk.key = None),
            M::Button(choice) => self.edit_audio(|s| {
                s.push_to_talk.gamepad_button = match choice {
                    ButtonChoice::None => None,
                    ButtonChoice::Button(button) => Some(button.to_string()),
                    // Not offered; picking it again keeps what is saved.
                    ButtonChoice::Unknown => s.push_to_talk.gamepad_button.take(),
                };
            }),
        }
    }

    /// Stops waiting for the push-to-talk key.
    pub(crate) fn stop_capture(&mut self) {
        self.audio_ui.capturing = false;
        self.audio_ui.capture_note = None;
    }

    /// Copies the echoed settings, changes them, and sends the whole value
    /// when it changed. Before the first echo there is nothing to change:
    /// the settings are asked for again.
    fn edit_audio(&mut self, change: impl FnOnce(&mut AudioSettings)) -> Task<Message> {
        let Some(now) = &self.calls.settings else {
            return self.send(Command::GetAudioSettings);
        };
        let mut next = now.clone();
        change(&mut next);
        if next == *now {
            return Task::none();
        }
        // Kept at once, so a second change made before the core's echo (a
        // join in progress holds it up) builds on this one; the echo then
        // replaces it with the effective value.
        self.calls.settings = Some(next.clone());
        self.send(Command::UpdateAudioSettings(next))
    }

    /// Sends the moved slider's value, once.
    fn send_slider(&mut self) -> Task<Message> {
        let Some((slider, value)) = self.audio_ui.drag.take() else {
            return Task::none();
        };
        self.edit_audio(|s| slider.apply(s, value))
    }

    fn toggle_mic_test(&mut self) -> Task<Message> {
        if self.audio_ui.mic_test.take().is_some() {
            return self.send(Command::StopMicTest);
        }
        if self.calls.call.is_some() {
            // The core refuses it during a call; the button is off too.
            return Task::none();
        }
        let loopback = self.audio_ui.loopback;
        self.audio_ui.mic_test = Some(loopback);
        self.calls.level = None;
        self.send(Command::StartMicTest { loopback })
    }

    /// Closes the card: a moved slider is sent, and the mic test stops.
    pub(crate) fn close_audio_card(&mut self) -> Task<Message> {
        let slider = self.send_slider();
        let stop = if self.audio_ui.mic_test.take().is_some() {
            self.send(Command::StopMicTest)
        } else {
            Task::none()
        };
        self.audio_ui.open = false;
        self.stop_capture();
        Task::batch([slider, stop])
    }

    /// A refused call or audio command: a mic test the core could not start
    /// (no device, a busy one) is not running.
    pub(crate) fn audio_card_error(&mut self, error: &twicecat_core::api::CoreError) {
        if matches!(error, twicecat_core::api::CoreError::Audio(_)) {
            self.audio_ui.mic_test = None;
        }
    }

    /// What the card follows in the core's events: device names, and a call
    /// that replaced the mic test.
    pub(crate) fn audio_card_saw(&mut self, event: &twicecat_core::api::CoreEvent) {
        use twicecat_core::api::{CallState, CoreEvent};
        match event {
            CoreEvent::AudioDevices(devices) => self.audio_ui.learn_names(devices),
            // Joining a call takes the devices over from the mic test.
            CoreEvent::CallState(CallState::Joining { .. }) => self.audio_ui.mic_test = None,
            _ => {}
        }
    }
}

/// The bounds of the input gain slider, dB.
pub const INPUT_GAIN_RANGE: (f32, f32) = limits::INPUT_GAIN_DB;
/// The bounds of the attenuation limit sliders, dB.
pub const CLEANING_LIMIT_RANGE: (f32, f32) = limits::DF_ATTENUATION_DB;
/// The bounds of the automatic gain's most boost, dB.
pub const AUTO_GAIN_MAX_RANGE: (f32, f32) = limits::AUTO_GAIN_MAX_DB;
/// The bounds of the voice activity threshold, dBFS.
pub const THRESHOLD_RANGE: (f32, f32) = limits::VAD_THRESHOLD_DB;
/// The bounds of the release delay, ms.
pub const RELEASE_DELAY_RANGE: (f32, f32) = (0.0, limits::PTT_RELEASE_DELAY_MS as f32);

#[cfg(test)]
mod tests {
    //! The Voice & audio card through `App::update` (plan Task 6).

    use tokio::sync::mpsc;
    use twicecat_core::api::{
        AudioDevice, AudioDeviceId, AudioDevices, AudioSettings, AudioStats, CallState, Command,
        CoreEvent, DeviceId, EnhancerBackend, GpuBackend, GpuEnhancement, InputLevel, InputMode,
        ListDiff, NoiseSuppression, NsLevel, PushToTalkBindings, RoomId, RoomSummary, SkipCleaned,
        UserId,
    };

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::keys::Shortcut;
    use crate::state::{App, Message, Screen, Startup};

    fn core(event: CoreEvent) -> Message {
        Message::Core(Box::new(event))
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    /// Settings with push-to-talk and bindings (left to #15), so tests see
    /// them pass through.
    fn stored() -> AudioSettings {
        AudioSettings {
            input_mode: InputMode::PushToTalk {
                release_delay_ms: 300,
                toggle: false,
            },
            push_to_talk: PushToTalkBindings {
                key: Some("ctrl+shift+KeyT".into()),
                gamepad_button: Some("south".into()),
            },
            ..AudioSettings::default()
        }
    }

    /// The main screen with the card open and the stored settings echoed.
    fn card() -> (App, mpsc::Receiver<Command>) {
        let (tx, mut rx) = mpsc::channel(64);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let _ = app.update(core(CoreEvent::NoSession));
        let _ = app.update(core(CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }));
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
            values: vec![RoomSummary::new(RoomId::from("!a:x"), "a".into())],
        }])));
        assert_eq!(app.screen, Screen::Main);
        let _ = app.update(core(CoreEvent::AudioSettings(stored())));
        let _ = app.update(audio(AudioMessage::Open));
        drain(&mut rx);
        (app, rx)
    }

    fn audio(message: AudioMessage) -> Message {
        Message::Audio(message)
    }

    /// The one `UpdateAudioSettings` sent, checked for the untouched input
    /// mode and bindings (only the Talking section changes them).
    fn sent_settings(rx: &mut mpsc::Receiver<Command>) -> AudioSettings {
        let settings = sent_one(rx);
        assert_eq!(
            settings.input_mode,
            stored().input_mode,
            "not this control's"
        );
        assert_eq!(
            settings.push_to_talk,
            stored().push_to_talk,
            "not this control's"
        );
        settings
    }

    /// The one `UpdateAudioSettings` sent.
    fn sent_one(rx: &mut mpsc::Receiver<Command>) -> AudioSettings {
        let sent = drain(rx);
        let [Command::UpdateAudioSettings(settings)] = sent.as_slice() else {
            panic!("expected one UpdateAudioSettings, got {sent:?}");
        };
        settings.clone()
    }

    fn device(id: &str, name: &str) -> AudioDevice {
        AudioDevice {
            id: AudioDeviceId::from(id),
            name: name.into(),
            is_default: false,
        }
    }

    #[test]
    fn opening_asks_for_settings_devices_and_the_gpu() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Close));
        drain(&mut rx);
        let _ = app.update(audio(AudioMessage::Open));
        let sent = drain(&mut rx);
        assert!(
            matches!(
                sent.as_slice(),
                [
                    Command::GetAudioSettings,
                    Command::ListAudioDevices,
                    Command::ProbeGpuEnhancement
                ]
            ),
            "{sent:?}"
        );
        assert!(app.audio_ui.open);
        let _ = app.view();
    }

    #[test]
    fn controls_do_nothing_while_the_card_is_closed() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Close));
        let _ = app.update(audio(AudioMessage::AutoGain(false)));
        let _ = app.update(audio(AudioMessage::MicTest));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn each_control_sends_the_whole_settings_with_one_change() {
        type Check = fn(&AudioSettings) -> bool;
        let cases: Vec<(AudioMessage, Check)> = vec![
            (
                AudioMessage::InputDevice(DeviceChoice {
                    id: Some(AudioDeviceId::from("usb")),
                    label: "USB".into(),
                }),
                |s| s.input_device == Some(AudioDeviceId::from("usb")),
            ),
            (
                AudioMessage::OutputDevice(DeviceChoice {
                    id: Some(AudioDeviceId::from("hs")),
                    label: "Headset".into(),
                }),
                |s| s.output_device == Some(AudioDeviceId::from("hs")),
            ),
            (AudioMessage::Noise(NoiseChoice::Off), |s| {
                s.processing.noise_suppression == NoiseSuppression::Off
            }),
            (AudioMessage::Noise(NoiseChoice::WebRtc), |s| {
                s.processing.noise_suppression
                    == NoiseSuppression::WebRtc {
                        level: NsLevel::Moderate,
                    }
            }),
            (AudioMessage::Noise(NoiseChoice::DeepFilter), |s| {
                s.processing.noise_suppression
                    == NoiseSuppression::DeepFilter {
                        attenuation_limit_db: DEFAULT_DF_LIMIT_DB,
                        gpu: false,
                    }
            }),
            (
                AudioMessage::Noise(NoiseChoice::DeepFilterGpu(Some(GpuBackend::Cuda))),
                |s| {
                    s.processing.noise_suppression
                        == NoiseSuppression::DeepFilter {
                            attenuation_limit_db: DEFAULT_DF_LIMIT_DB,
                            gpu: true,
                        }
                },
            ),
            (AudioMessage::EchoCancellation(false), |s| {
                !s.processing.echo_cancellation
            }),
            (AudioMessage::CancelSystemAudio(true), |s| {
                s.processing.cancel_system_audio
            }),
            (AudioMessage::AutoGain(false), |s| !s.processing.auto_gain),
            (AudioMessage::HighPass(false), |s| !s.processing.high_pass),
            (AudioMessage::Bitrate(Bitrate(32)), |s| {
                s.voice_bitrate_kbps == 32
            }),
            (AudioMessage::Ringtone(false), |s| !s.ringtone),
            (AudioMessage::CleanOthers(true), |s| {
                s.remote_cleaning.enabled
            }),
            (AudioMessage::Skip(SkipChoice(SkipCleaned::Never)), |s| {
                s.remote_cleaning.skip_cleaned == SkipCleaned::Never
            }),
            (AudioMessage::CleanOthersGpu(true), |s| {
                s.remote_cleaning.gpu
            }),
            (AudioMessage::Announce(false), |s| {
                !s.announce_cleaning.enabled
            }),
            (AudioMessage::CleanedOutside(true), |s| {
                s.announce_cleaning.cleaned_outside
            }),
        ];
        for (message, check) in cases {
            let (mut app, mut rx) = card();
            let name = format!("{message:?}");
            let _ = app.update(audio(message));
            let sent = sent_settings(&mut rx);
            assert!(check(&sent), "{name}: {sent:?}");
            let mut rest = sent.clone();
            rest.input_mode = stored().input_mode;
            assert_ne!(rest, stored(), "{name} changed something");
        }
    }

    #[test]
    fn a_choice_already_set_sends_nothing() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Noise(NoiseChoice::RnNoise)));
        let _ = app.update(audio(AudioMessage::AutoGain(true)));
        let _ = app.update(audio(AudioMessage::Bitrate(Bitrate(64))));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_level_and_gpu_options_apply_only_to_their_suppressor() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Level(LevelChoice(NsLevel::High))));
        assert!(drain(&mut rx).is_empty(), "RNNoise has no strength");
        let mut webrtc = stored();
        webrtc.processing.noise_suppression = NoiseSuppression::WebRtc {
            level: NsLevel::Low,
        };
        let _ = app.update(core(CoreEvent::AudioSettings(webrtc)));
        let _ = app.update(audio(AudioMessage::Level(LevelChoice(NsLevel::High))));
        assert_eq!(
            sent_settings(&mut rx).processing.noise_suppression,
            NoiseSuppression::WebRtc {
                level: NsLevel::High
            }
        );
        // Picking WebRTC again keeps its strength.
        let _ = app.update(audio(AudioMessage::Noise(NoiseChoice::WebRtc)));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn system_audio_needs_echo_cancellation() {
        let (mut app, mut rx) = card();
        let mut both = stored();
        both.processing.cancel_system_audio = true;
        let _ = app.update(core(CoreEvent::AudioSettings(both)));
        let _ = app.update(audio(AudioMessage::EchoCancellation(false)));
        let sent = sent_settings(&mut rx);
        assert!(!sent.processing.echo_cancellation && !sent.processing.cancel_system_audio);
        let _ = app.update(core(CoreEvent::AudioSettings(sent)));
        let _ = app.update(audio(AudioMessage::CancelSystemAudio(true)));
        assert!(drain(&mut rx).is_empty(), "off without echo cancellation");
    }

    #[test]
    fn before_the_first_echo_a_change_asks_for_the_settings() {
        let (mut app, mut rx) = card();
        app.calls.settings = None;
        let _ = app.update(audio(AudioMessage::AutoGain(false)));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::GetAudioSettings]
        ));
    }

    #[test]
    fn sliders_send_once_on_release_or_after_the_last_pause() {
        let (mut app, mut rx) = card();
        for gain in [2.0, 4.5, 6.0] {
            let _ = app.update(audio(AudioMessage::Slide(Slider::InputGain, gain)));
        }
        assert!(drain(&mut rx).is_empty(), "nothing while it moves");
        let settings = app.calls.settings.clone().expect("echoed");
        assert_eq!(app.audio_ui.shown(Slider::InputGain, &settings), 6.0);
        let _ = app.update(audio(AudioMessage::SlideReleased));
        let _ = app.update(audio(AudioMessage::SlideReleased));
        assert_eq!(sent_settings(&mut rx).input_gain_db, 6.0);
        let _ = app.update(audio(AudioMessage::Slide(Slider::MasterVolume, 150.0)));
        let stale = app.audio_ui.drag_seq;
        let _ = app.update(audio(AudioMessage::Slide(Slider::MasterVolume, 120.0)));
        let _ = app.update(audio(AudioMessage::SlideDue(stale)));
        assert!(drain(&mut rx).is_empty(), "an older pause sends nothing");
        let seq = app.audio_ui.drag_seq;
        let _ = app.update(audio(AudioMessage::SlideDue(seq)));
        assert_eq!(sent_settings(&mut rx).output_volume_percent, 120);
    }

    #[test]
    fn the_steps_beside_a_slider_send_at_once_and_clamp() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Step(Slider::MasterVolume, true)));
        assert_eq!(sent_settings(&mut rx).output_volume_percent, 110);
        let mut loud = stored();
        loud.output_volume_percent = 195;
        loud.input_gain_db = -19.5;
        let _ = app.update(core(CoreEvent::AudioSettings(loud)));
        let _ = app.update(audio(AudioMessage::Step(Slider::MasterVolume, true)));
        assert_eq!(sent_settings(&mut rx).output_volume_percent, 200);
        let _ = app.update(audio(AudioMessage::Step(Slider::InputGain, false)));
        assert_eq!(sent_settings(&mut rx).input_gain_db, -20.0);
        // A moved slider steps from where it is, and is not sent again.
        let _ = app.update(audio(AudioMessage::Slide(Slider::InputGain, 3.0)));
        let _ = app.update(audio(AudioMessage::Step(Slider::InputGain, true)));
        assert_eq!(sent_settings(&mut rx).input_gain_db, 4.0);
        let _ = app.update(audio(AudioMessage::SlideReleased));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_cleaning_limits_follow_their_sliders() {
        let (mut app, mut rx) = card();
        let mut df = stored();
        df.processing.noise_suppression = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 100.0,
            gpu: true,
        };
        let _ = app.update(core(CoreEvent::AudioSettings(df)));
        let _ = app.update(audio(AudioMessage::Slide(Slider::MicCleaningLimit, 30.0)));
        let _ = app.update(audio(AudioMessage::SlideReleased));
        assert_eq!(
            sent_settings(&mut rx).processing.noise_suppression,
            NoiseSuppression::DeepFilter {
                attenuation_limit_db: 30.0,
                gpu: true
            }
        );
        let _ = app.update(audio(AudioMessage::Slide(
            Slider::OthersCleaningLimit,
            12.0,
        )));
        let _ = app.update(audio(AudioMessage::SlideReleased));
        assert_eq!(
            sent_settings(&mut rx).remote_cleaning.attenuation_limit_db,
            12.0
        );
    }

    #[test]
    fn the_most_boost_follows_its_slider_and_steps() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Slide(Slider::AutoGainMax, 12.0)));
        let _ = app.update(audio(AudioMessage::SlideReleased));
        assert_eq!(sent_settings(&mut rx).processing.auto_gain_max_db, 12.0);
        let mut low = stored();
        low.processing.auto_gain_max_db = 3.0;
        let _ = app.update(core(CoreEvent::AudioSettings(low)));
        let _ = app.update(audio(AudioMessage::Step(Slider::AutoGainMax, false)));
        assert_eq!(
            sent_settings(&mut rx).processing.auto_gain_max_db,
            1.0,
            "- stops at AGC2's least"
        );
    }

    #[test]
    fn the_echo_wins_over_what_was_sent() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Slide(Slider::InputGain, 30.0)));
        let _ = app.update(audio(AudioMessage::SlideReleased));
        let sent = sent_settings(&mut rx);
        // The core clamps 30 dB to 20 and echoes that.
        let echoed = sent.clone().validated();
        let _ = app.update(core(CoreEvent::AudioSettings(echoed)));
        let settings = app.calls.settings.clone().expect("echoed");
        assert_eq!(app.audio_ui.shown(Slider::InputGain, &settings), 20.0);
    }

    #[test]
    fn closing_sends_a_moved_slider_and_stops_the_mic_test() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::MicTest));
        let _ = app.update(audio(AudioMessage::Slide(Slider::MasterVolume, 80.0)));
        drain(&mut rx);
        let _ = app.update(audio(AudioMessage::Close));
        let sent = drain(&mut rx);
        assert!(
            matches!(
                sent.as_slice(),
                [Command::UpdateAudioSettings(s), Command::StopMicTest] if s.output_volume_percent == 80
            ),
            "{sent:?}"
        );
        assert!(!app.audio_ui.open && app.audio_ui.mic_test.is_none());
    }

    #[test]
    fn the_mic_test_starts_stops_and_follows_hear_myself() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::MicTest));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::StartMicTest { loopback: false }]
        ));
        let _ = app.update(audio(AudioMessage::Loopback(true)));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::StartMicTest { loopback: true }]
        ));
        let _ = app.update(audio(AudioMessage::MicTest));
        assert!(matches!(drain(&mut rx).as_slice(), [Command::StopMicTest]));
        // Off, "Hear myself" only changes the next test.
        let _ = app.update(audio(AudioMessage::Loopback(false)));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_mic_test_is_refused_in_a_call_and_ends_when_one_starts() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::MicTest));
        assert!(app.audio_ui.mic_test.is_some());
        let _ = app.update(core(CoreEvent::CallState(CallState::Joining {
            room_id: RoomId::from("!a:x"),
        })));
        assert!(app.audio_ui.mic_test.is_none(), "the call took the devices");
        drain(&mut rx);
        let _ = app.update(audio(AudioMessage::MicTest));
        assert!(drain(&mut rx).is_empty());
        let _ = app.view();
    }

    #[test]
    fn two_changes_before_the_echo_both_count() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::EchoCancellation(false)));
        let _ = app.update(audio(AudioMessage::AutoGain(false)));
        let sent = drain(&mut rx);
        let [
            Command::UpdateAudioSettings(_),
            Command::UpdateAudioSettings(last),
        ] = sent.as_slice()
        else {
            panic!("{sent:?}");
        };
        assert!(!last.processing.echo_cancellation && !last.processing.auto_gain);
        let _ = app.update(audio(AudioMessage::Step(Slider::MasterVolume, true)));
        let _ = app.update(audio(AudioMessage::Step(Slider::MasterVolume, true)));
        assert_eq!(sent_settings_last(&mut rx).output_volume_percent, 120);
    }

    fn sent_settings_last(rx: &mut mpsc::Receiver<Command>) -> AudioSettings {
        match drain(rx).pop() {
            Some(Command::UpdateAudioSettings(settings)) => settings,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_refused_mic_test_is_not_shown_as_running() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::MicTest));
        drain(&mut rx);
        let _ = app.update(core(CoreEvent::Error(
            twicecat_core::api::CoreError::Audio("no input device".into()),
        )));
        assert!(app.audio_ui.mic_test.is_none());
        assert_eq!(app.calls.notice.as_deref(), Some("Audio: no input device"));
        let _ = app.view();
        // The button starts it again.
        let _ = app.update(audio(AudioMessage::MicTest));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::StartMicTest { .. }]
        ));
    }

    fn found(available: Vec<GpuBackend>) -> GpuEnhancement {
        GpuEnhancement {
            available,
            running: None,
        }
    }

    #[test]
    fn the_card_says_why_no_gpu_is_offered() {
        let expected = if cfg!(feature = "gpu") {
            NO_GPU_FOUND
        } else {
            NO_GPU_IN_BUILD
        };
        assert_eq!(
            gpu_status(Some(&found(Vec::new()))),
            format!("GPU: {expected}")
        );
        assert_eq!(gpu_status(None), "GPU: looking for one\u{2026}");
        // No GPU: the menu has no GPU entry.
        let rnnoise = NoiseSuppression::RnNoise;
        assert!(
            !NoiseChoice::menu(Some(&found(Vec::new())), &rnnoise)
                .iter()
                .any(|c| matches!(c, NoiseChoice::DeepFilterGpu(_)))
        );
    }

    #[test]
    fn the_menu_names_the_gpu_backend_that_runs() {
        let gpu = found(vec![GpuBackend::Cuda, GpuBackend::WebGpu]);
        let menu = NoiseChoice::menu(Some(&gpu), &NoiseSuppression::RnNoise);
        let last = menu.last().copied().expect("entries");
        assert_eq!(last, NoiseChoice::DeepFilterGpu(Some(GpuBackend::Cuda)));
        assert_eq!(
            last.to_string(),
            "DeepFilterNet3 on the GPU (NVIDIA \u{b7} CUDA)"
        );
        // Every entry says where it runs.
        for choice in &menu[1..] {
            let label = choice.to_string();
            assert!(label.contains("CPU") || label.contains("GPU"), "{label}");
        }
        // The first backend is used; the others are listed.
        assert_eq!(
            gpu_status(Some(&gpu)),
            format!(
                "GPU: NVIDIA \u{b7} CUDA is used. Also here: {}.",
                gpu_name(GpuBackend::WebGpu)
            )
        );
        assert!(gpu_name(GpuBackend::WebGpu).starts_with("any GPU"));
        // The setting picks the same entry the menu shows.
        let on_gpu = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 30.0,
            gpu: true,
        };
        assert!(menu.contains(&NoiseChoice::of(&on_gpu, Some(&gpu))));
        // A GPU setting from another machine stays in the menu.
        let none = found(Vec::new());
        assert!(
            NoiseChoice::menu(Some(&none), &on_gpu)
                .contains(&NoiseChoice::of(&on_gpu, Some(&none)))
        );
    }

    #[test]
    fn moving_deepfilter_between_cpu_and_gpu_keeps_its_limit() {
        let cpu = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 42.0,
            gpu: false,
        };
        assert_eq!(
            NoiseChoice::DeepFilterGpu(None).setting(&cpu),
            NoiseSuppression::DeepFilter {
                attenuation_limit_db: 42.0,
                gpu: true,
            }
        );
        let gpu = NoiseChoice::DeepFilterGpu(None).setting(&cpu);
        assert_eq!(NoiseChoice::DeepFilter.setting(&gpu), cpu);
    }

    #[test]
    fn escape_follows_the_layers_over_the_card() {
        let (mut app, _rx) = card();
        // A layer drawn over the card (the switcher here) closes first.
        app.switcher.open = true;
        let esc = |app: &mut App| {
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Escape,
                captured: false,
            });
        };
        esc(&mut app);
        assert!(!app.switcher.open && app.audio_ui.open);
        esc(&mut app);
        assert!(!app.audio_ui.open);
    }

    #[test]
    fn hot_plug_keeps_a_saved_device_chosen_as_not_connected() {
        let (mut app, _rx) = card();
        let _ = app.update(core(CoreEvent::AudioDevices(AudioDevices {
            inputs: vec![device("mic", "Desk mic"), device("usb", "USB headset")],
            outputs: Vec::new(),
        })));
        let names = app.audio_ui.names.clone();
        let usb = AudioDeviceId::from("usb");
        let (menu, chosen) = device_menu(
            &app.calls.devices.as_ref().expect("listed").inputs,
            Some(&usb),
            &names,
        );
        assert_eq!(chosen.label, "USB headset");
        assert_eq!(menu.len(), 3);
        // Unplugged: the list loses it, the choice stays, by name.
        let _ = app.update(core(CoreEvent::AudioDevices(AudioDevices {
            inputs: vec![device("mic", "Desk mic")],
            outputs: Vec::new(),
        })));
        let (menu, chosen) = device_menu(
            &app.calls.devices.as_ref().expect("listed").inputs,
            Some(&usb),
            &app.audio_ui.names,
        );
        assert_eq!(chosen.id, Some(usb.clone()));
        assert_eq!(chosen.label, "USB headset (not connected)");
        assert_eq!(
            menu.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(),
            ["System default", "Desk mic", "USB headset (not connected)"]
        );
        // Never seen: a placeholder name.
        let (_, chosen) = device_menu(&[], Some(&AudioDeviceId::from("old")), &HashMap::new());
        assert_eq!(chosen.label, "Saved device (not connected)");
        let (_, chosen) = device_menu(&[], None, &HashMap::new());
        assert_eq!(chosen.label, "System default");
        let _ = app.view();
    }

    #[test]
    fn bitrates_keep_an_unusual_current_value() {
        assert_eq!(bitrate_menu(64).len(), BITRATES.len());
        let menu = bitrate_menu(510);
        assert_eq!(menu.last(), Some(&Bitrate(510)));
        assert_eq!(bitrate_menu(40)[3], Bitrate(40));
        assert_eq!(Bitrate(64).to_string(), "64 kb/s");
    }

    #[test]
    fn the_meter_reads_the_level() {
        let (fill, words) = meter(InputLevel {
            rms_db: -30.0,
            peak_db: -12.4,
            transmitting: true,
        });
        assert!((fill - 0.5).abs() < 1e-6);
        assert_eq!(words, "-30 dB, peak -12 dB, sending");
        let (fill, words) = meter(InputLevel {
            rms_db: -120.0,
            peak_db: f32::NAN,
            transmitting: false,
        });
        assert_eq!(fill, 0.0);
        assert!(words.ends_with("not sending"), "{words}");
    }

    #[test]
    fn diagnostics_and_backends_read_well() {
        let lines = diagnostics(&AudioStats {
            echo_return_loss_enhancement_db: Some(31.6),
            echo_delay_ms: Some(48),
            processing_us: 95,
            ..AudioStats::default()
        });
        assert_eq!(lines[0], "Echo removed: 32 dB");
        assert_eq!(lines[1], "Echo delay: 48 ms");
        assert!(lines.contains(&"Processing: 95 \u{b5}s per 10 ms".to_owned()));
        assert_eq!(
            backend_label(EnhancerBackend::Gpu {
                backend: GpuBackend::TensorRt
            }),
            "Running on the GPU (NVIDIA \u{b7} TensorRT)."
        );
        assert!(backend_label(EnhancerBackend::CpuAfterGpuTooSlow).contains("fell behind"));
    }

    #[test]
    fn the_view_builds_with_every_section_shown() {
        let (mut app, _rx) = card();
        let mut all = stored();
        all.processing.noise_suppression = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 50.0,
            gpu: true,
        };
        all.remote_cleaning.enabled = true;
        let _ = app.update(core(CoreEvent::AudioSettings(all)));
        let _ = app.update(core(CoreEvent::GpuEnhancement(GpuEnhancement {
            available: vec![GpuBackend::WebGpu],
            running: Some(EnhancerBackend::Gpu {
                backend: GpuBackend::WebGpu,
            }),
        })));
        let _ = app.update(core(CoreEvent::AudioStats(AudioStats {
            remote_cleaning: Some(EnhancerBackend::Cpu),
            ..AudioStats::default()
        })));
        let _ = app.update(core(CoreEvent::InputLevel(InputLevel {
            rms_db: -20.0,
            peak_db: -6.0,
            transmitting: true,
        })));
        let _ = app.update(audio(AudioMessage::MicTest));
        let _ = app.update(audio(AudioMessage::Diagnostics));
        let _ = app.view();
        app.calls.settings = None;
        let _ = app.view();
    }

    #[test]
    fn escape_closes_the_card_and_room_shortcuts_wait() {
        let (mut app, _rx) = card();
        assert!(app.any_overlay_open());
        let _ = app.update(Message::Shortcut {
            shortcut: Shortcut::Escape,
            captured: false,
        });
        assert!(!app.audio_ui.open);
    }

    #[test]
    fn settings_opens_the_card_and_closes_itself() {
        let (mut app, mut rx) = card();
        let _ = app.update(audio(AudioMessage::Close));
        let _ = app.update(Message::Settings(crate::settings::SettingsMessage::Open));
        assert!(app.settings_ui.open);
        drain(&mut rx);
        let _ = app.update(Message::Settings(
            crate::settings::SettingsMessage::OpenVoiceAudio,
        ));
        assert!(!app.settings_ui.open && app.audio_ui.open);
        assert_eq!(drain(&mut rx).len(), 3);
        // Settings waits while the card is open.
        let _ = app.update(Message::Settings(crate::settings::SettingsMessage::Open));
        assert!(!app.settings_ui.open);
    }

    #[test]
    fn logging_out_closes_the_card() {
        let (mut app, _rx) = card();
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert!(!app.audio_ui.open);
    }

    /// The Talking section (push-to-talk plan, Task 4).
    mod talking {
        use iced::keyboard::Modifiers;
        use iced::keyboard::key::Code;
        use twicecat_ptt::{KeyBinding, KeyboardBackend, Source};

        use super::*;
        use crate::ptt::{PttMessage, PttUpdate, SourceState};

        fn voice(automatic: bool, threshold_db: f32) -> AudioSettings {
            AudioSettings {
                input_mode: InputMode::VoiceActivity {
                    automatic,
                    threshold_db,
                },
                ..stored()
            }
        }

        fn key(code: Code, modifiers: Modifiers) -> Message {
            audio(AudioMessage::Captured(CapturedKey { code, modifiers }))
        }

        fn binding(text: &str) -> KeyBinding {
            text.parse().unwrap()
        }

        #[test]
        fn the_mode_menu_keeps_what_each_mode_needs() {
            let (mut app, mut rx) = card();
            let _ = app.update(audio(AudioMessage::Mode(ModeChoice::ToggleToTalk)));
            let s = sent_one(&mut rx);
            assert_eq!(
                s.input_mode,
                InputMode::PushToTalk {
                    release_delay_ms: 300,
                    toggle: true
                },
                "the delay is kept"
            );
            assert_eq!(s.push_to_talk, stored().push_to_talk);
            let _ = app.update(core(CoreEvent::AudioSettings(s)));
            let _ = app.update(audio(AudioMessage::Mode(ModeChoice::VoiceActivity)));
            let s = sent_one(&mut rx);
            assert_eq!(
                s.input_mode,
                InputMode::VoiceActivity {
                    automatic: true,
                    threshold_db: DEFAULT_THRESHOLD_DB
                }
            );
            assert_eq!(s.push_to_talk, stored().push_to_talk, "bindings are kept");
            let _ = app.update(core(CoreEvent::AudioSettings(voice(false, -42.0))));
            let _ = app.update(audio(AudioMessage::Mode(ModeChoice::VoiceActivity)));
            assert!(drain(&mut rx).is_empty(), "already set");
            let _ = app.update(audio(AudioMessage::Mode(ModeChoice::PushToTalk)));
            assert_eq!(
                sent_one(&mut rx).input_mode,
                InputMode::PushToTalk {
                    release_delay_ms: DEFAULT_RELEASE_MS,
                    toggle: false
                },
                "from voice activity: 200 ms"
            );
            assert_eq!(
                ModeChoice::of(&voice(true, -50.0).input_mode),
                ModeChoice::VoiceActivity
            );
        }

        #[test]
        fn the_voice_activity_level_follows_its_switch_and_slider() {
            let (mut app, mut rx) = card();
            let _ = app.update(core(CoreEvent::AudioSettings(voice(true, -50.0))));
            let _ = app.update(audio(AudioMessage::AutoThreshold(false)));
            let s = sent_one(&mut rx);
            assert_eq!(s.input_mode, voice(false, -50.0).input_mode);
            let _ = app.update(core(CoreEvent::AudioSettings(s)));
            let _ = app.update(audio(AudioMessage::Step(Slider::Threshold, true)));
            assert_eq!(sent_one(&mut rx).input_mode, voice(false, -49.0).input_mode);
            let _ = app.update(audio(AudioMessage::Slide(Slider::Threshold, -61.0)));
            let _ = app.update(audio(AudioMessage::SlideReleased));
            assert_eq!(sent_one(&mut rx).input_mode, voice(false, -61.0).input_mode);
            // Not a push-to-talk setting: the delay slider changes nothing.
            let _ = app.update(audio(AudioMessage::Step(Slider::ReleaseDelay, true)));
            assert!(drain(&mut rx).is_empty());
        }

        #[test]
        fn the_release_delay_follows_its_slider_and_steps() {
            let (mut app, mut rx) = card();
            let _ = app.update(audio(AudioMessage::Step(Slider::ReleaseDelay, true)));
            let s = sent_one(&mut rx);
            assert_eq!(
                s.input_mode,
                InputMode::PushToTalk {
                    release_delay_ms: 350,
                    toggle: false
                }
            );
            let _ = app.update(audio(AudioMessage::Slide(Slider::ReleaseDelay, 1999.6)));
            let _ = app.update(audio(AudioMessage::SlideReleased));
            assert_eq!(
                sent_one(&mut rx).input_mode,
                InputMode::PushToTalk {
                    release_delay_ms: 2000,
                    toggle: false
                }
            );
            let _ = app.update(core(CoreEvent::AudioSettings(AudioSettings {
                input_mode: InputMode::PushToTalk {
                    release_delay_ms: 0,
                    toggle: false,
                },
                ..stored()
            })));
            let _ = app.update(audio(AudioMessage::Step(Slider::ReleaseDelay, false)));
            assert!(drain(&mut rx).is_empty(), "clamped at 0: unchanged");
            assert_eq!(Slider::ReleaseDelay.range(), (0.0, 2000.0));
            assert_eq!(Slider::Threshold.range(), (-100.0, 0.0));
        }

        #[test]
        fn a_captured_key_becomes_the_binding() {
            let (mut app, mut rx) = card();
            let _ = app.update(key(Code::KeyY, Modifiers::CTRL));
            assert!(drain(&mut rx).is_empty(), "not waiting: nothing");
            let _ = app.update(audio(AudioMessage::CaptureKey));
            assert!(app.audio_ui.capturing);
            let _ = app.update(key(Code::ShiftLeft, Modifiers::SHIFT));
            let _ = app.update(key(Code::ControlRight, Modifiers::CTRL));
            assert!(
                app.audio_ui.capturing && drain(&mut rx).is_empty(),
                "modifiers wait"
            );
            let _ = app.update(key(Code::ContextMenu, Modifiers::empty()));
            assert_eq!(app.audio_ui.capture_note, Some(UNBINDABLE_KEY));
            assert!(app.audio_ui.capturing && drain(&mut rx).is_empty());
            let _ = app.update(key(Code::KeyY, Modifiers::CTRL | Modifiers::ALT));
            assert!(!app.audio_ui.capturing && app.audio_ui.capture_note.is_none());
            let s = sent_one(&mut rx);
            let saved = s.push_to_talk.key.as_deref().unwrap();
            assert_eq!(binding(saved), binding("ctrl+alt+KeyY"));
            assert_eq!(s.input_mode, stored().input_mode);
            assert_eq!(
                s.push_to_talk.gamepad_button,
                stored().push_to_talk.gamepad_button
            );

            let _ = app.update(core(CoreEvent::AudioSettings(s)));
            let _ = app.update(audio(AudioMessage::ClearKey));
            assert_eq!(sent_one(&mut rx).push_to_talk.key, None);
        }

        #[test]
        fn esc_focus_loss_cancel_and_close_stop_waiting() {
            let (mut app, mut rx) = card();
            let wait = |app: &mut App| {
                let _ = app.update(audio(AudioMessage::CaptureKey));
                assert!(app.audio_ui.capturing);
            };
            wait(&mut app);
            // The app's Esc shortcut is off while waiting (it would close
            // the card); the captured Esc cancels.
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Escape,
                captured: false,
            });
            assert!(app.audio_ui.open && app.audio_ui.capturing);
            let _ = app.update(key(Code::Escape, Modifiers::empty()));
            assert!(app.audio_ui.open && !app.audio_ui.capturing);
            wait(&mut app);
            let _ = app.update(Message::WindowFocus(false));
            assert!(!app.audio_ui.capturing);
            wait(&mut app);
            let _ = app.update(audio(AudioMessage::CancelCapture));
            assert!(!app.audio_ui.capturing);
            wait(&mut app);
            let _ = app.update(audio(AudioMessage::Close));
            assert!(!app.audio_ui.capturing);
            assert!(
                drain(&mut rx)
                    .iter()
                    .all(|c| !matches!(c, Command::UpdateAudioSettings(_))),
                "nothing saved"
            );
        }

        #[test]
        fn capture_messages_come_from_fresh_presses_only() {
            use iced::keyboard::{Event, Key, Location};
            let press = |repeat| {
                iced::Event::Keyboard(Event::KeyPressed {
                    key: Key::Unidentified,
                    modified_key: Key::Unidentified,
                    physical_key: iced::keyboard::key::Physical::Code(Code::F13),
                    location: Location::Standard,
                    modifiers: Modifiers::SHIFT,
                    text: None,
                    repeat,
                })
            };
            assert!(matches!(
                capture_message(&press(false)),
                Some(Message::Audio(AudioMessage::Captured(CapturedKey {
                    code: Code::F13,
                    modifiers: Modifiers::SHIFT,
                })))
            ));
            assert!(capture_message(&press(true)).is_none());
            let release = iced::Event::Keyboard(Event::KeyReleased {
                key: Key::Unidentified,
                modified_key: Key::Unidentified,
                physical_key: iced::keyboard::key::Physical::Code(Code::F13),
                location: Location::Standard,
                modifiers: Modifiers::empty(),
            });
            assert!(capture_message(&release).is_none());
            assert_eq!(
                capture(CapturedKey {
                    code: Code::F13,
                    modifiers: Modifiers::SHIFT
                }),
                Capture::Bound("shift+F13".into())
            );
        }

        #[test]
        fn the_gamepad_menu_offers_every_button_and_keeps_an_unknown_one() {
            let (menu, chosen) = ButtonChoice::menu(Some("south"));
            assert_eq!(menu.len(), 1 + GamepadButton::names().count());
            assert_eq!(chosen, ButtonChoice::Button("south".parse().unwrap()));
            assert_eq!(chosen.to_string(), "Bottom face button (A, \u{2715})");
            let labels: std::collections::HashSet<String> =
                menu.iter().map(ToString::to_string).collect();
            assert_eq!(labels.len(), menu.len(), "every label differs");
            for name in GamepadButton::names() {
                let label = ButtonChoice::Button(name.parse().unwrap()).to_string();
                assert_ne!(label, name, "{name} has words");
            }
            let (menu, chosen) = ButtonChoice::menu(Some("paddle9"));
            assert_eq!(chosen, ButtonChoice::Unknown);
            assert_eq!(menu.last(), Some(&ButtonChoice::Unknown));
            assert_eq!(ButtonChoice::menu(None).1, ButtonChoice::None);

            let (mut app, mut rx) = card();
            let east = ButtonChoice::Button("east".parse().unwrap());
            let _ = app.update(audio(AudioMessage::Button(east)));
            let s = sent_one(&mut rx);
            assert_eq!(s.push_to_talk.gamepad_button.as_deref(), Some("east"));
            assert_eq!(s.push_to_talk.key, stored().push_to_talk.key);
            let _ = app.update(core(CoreEvent::AudioSettings(s)));
            let _ = app.update(audio(AudioMessage::Button(ButtonChoice::None)));
            assert_eq!(sent_one(&mut rx).push_to_talk.gamepad_button, None);
            let _ = app.update(audio(AudioMessage::Button(ButtonChoice::Unknown)));
            assert!(drain(&mut rx).is_empty(), "not offered: nothing changes");
        }

        #[test]
        fn keys_read_as_people_write_them() {
            assert_eq!(key_label("shift+control+KeyT"), "Ctrl+Shift+T");
            assert_eq!(key_label("ctrl+shift+KeyT"), "Ctrl+Shift+T");
            assert_eq!(key_label("F13"), "F13");
            assert_eq!(key_label("alt+Digit5"), "Alt+5");
            assert_eq!(key_label("Numpad0"), "Numpad0");
            let logo = if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Super"
            };
            assert_eq!(key_label("super+Space"), format!("{logo}+Space"));
            assert_eq!(key_label("Banana"), "Banana", "shown as stored");
        }

        #[test]
        fn sources_and_the_keyboard_read_well() {
            use crate::screens::audio_settings::{keyboard_note, source_line};
            assert_eq!(
                source_line(Source::Portal, &SourceState::Ready("Ctrl+Shift+T".into())),
                "Desktop shortcut: ready, press Ctrl+Shift+T."
            );
            assert_eq!(
                source_line(Source::Portal, &SourceState::Ready(String::new())),
                "Desktop shortcut: ready."
            );
            assert_eq!(
                source_line(
                    Source::Gamepad,
                    &SourceState::Unavailable("no pad\nat all".into())
                ),
                "Gamepad: not available (no pad\u{23ce}at all)."
            );
            assert!(keyboard_note(KeyboardBackend::WindowOnly).contains("has focus"));
            assert!(keyboard_note(KeyboardBackend::Portal).contains("Wayland"));
            assert!(keyboard_note(KeyboardBackend::Hotkey).contains("every app"));
        }

        #[test]
        fn the_call_bar_says_whether_you_are_talking() {
            use crate::screens::call::talk_line;
            let (mut app, _rx) = card();
            assert_eq!(
                talk_line(&app),
                Some(("Not talking: hold Ctrl+Shift+T".into(), false))
            );
            app.ptt.talking = true;
            assert_eq!(talk_line(&app), Some(("Talking".into(), true)));
            app.ptt.talking = false;
            let mut toggle = stored();
            toggle.input_mode = InputMode::PushToTalk {
                release_delay_ms: 0,
                toggle: true,
            };
            toggle.push_to_talk.key = None;
            let _ = app.update(core(CoreEvent::AudioSettings(toggle.clone())));
            assert_eq!(
                talk_line(&app),
                Some(("Not talking: press the gamepad button".into(), false))
            );
            toggle.push_to_talk.gamepad_button = None;
            let _ = app.update(core(CoreEvent::AudioSettings(toggle)));
            assert!(talk_line(&app).unwrap().0.contains("no key or button"));
            let _ = app.update(core(CoreEvent::AudioSettings(voice(true, -50.0))));
            assert_eq!(talk_line(&app), None);
        }

        #[test]
        fn the_view_builds_in_each_mode() {
            let (mut app, _rx) = card();
            app.ptt.sources = vec![
                (Source::Portal, SourceState::Ready("Ctrl+Shift+T".into())),
                (Source::Gamepad, SourceState::Unavailable("no pad".into())),
            ];
            let _ = app.view();
            let _ = app.update(audio(AudioMessage::CaptureKey));
            let _ = app.update(key(Code::ContextMenu, Modifiers::empty()));
            let _ = app.view();
            let _ = app.update(audio(AudioMessage::CancelCapture));
            let mut bad = stored();
            bad.push_to_talk.key = Some("ctrl+Banana".into());
            bad.push_to_talk.gamepad_button = Some("paddle9".into());
            let _ = app.update(core(CoreEvent::AudioSettings(bad)));
            let _ = app.view();
            for settings in [voice(true, -50.0), voice(false, -30.0)] {
                let _ = app.update(core(CoreEvent::AudioSettings(settings)));
                let _ = app.view();
            }
        }

        /// A started service that records the window keys it gets.
        #[derive(Clone, Default)]
        struct Keys(std::sync::Arc<std::sync::Mutex<Vec<bool>>>);

        impl crate::ptt::PttHandle for Keys {
            fn window_key(&self, pressed: bool) {
                self.0.lock().unwrap().push(pressed);
            }
        }

        impl crate::ptt::PttStarter for Keys {
            fn start(
                &self,
                _config: twicecat_ptt::PttConfig,
                _sink: twicecat_ptt::Sink,
            ) -> Box<dyn crate::ptt::PttHandle> {
                Box::new(self.clone())
            }
        }

        #[test]
        fn the_old_key_is_off_while_a_new_one_is_awaited() {
            let (mut app, _rx) = card();
            let keys = Keys::default();
            app.set_ptt_starter(Box::new(keys.clone()), KeyboardBackend::Hotkey);
            let _ = app.update(audio(AudioMessage::Close));
            let _ = app.update(audio(AudioMessage::Open));
            assert!(app.ptt.is_running());
            let window_key = |pressed| {
                Message::Ptt(PttMessage::WindowKey(crate::ptt::WindowKey {
                    code: Code::KeyT,
                    modifiers: Modifiers::CTRL | Modifiers::SHIFT,
                    pressed,
                }))
            };
            let _ = app.update(audio(AudioMessage::CaptureKey));
            let _ = app.update(window_key(true));
            assert!(keys.0.lock().unwrap().is_empty(), "no press while waiting");
            let _ = app.update(audio(AudioMessage::CancelCapture));
            let _ = app.update(window_key(true));
            assert_eq!(*keys.0.lock().unwrap(), [true]);
            // Held when the card starts waiting: the release still lets go.
            let _ = app.update(audio(AudioMessage::CaptureKey));
            let _ = app.update(window_key(false));
            let _ = app.update(audio(AudioMessage::CancelCapture));
            assert_eq!(*keys.0.lock().unwrap(), [true, false]);
            // What the sources report is shown for the running service.
            let generation = 1;
            let _ = app.update(Message::Ptt(PttMessage::Update(PttUpdate::Ready {
                generation,
                source: Source::Hotkey,
                trigger: "shift+control+KeyT".into(),
            })));
            assert_eq!(app.ptt.sources.len(), 1);
        }
    }
}
