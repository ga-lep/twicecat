//! Audio types of the core API (Phase 6): devices, settings, levels and
//! statistics. None of them names an SDK, LiveKit or audio-backend type.
//!
//! [`AudioSettings`] is also the on-disk format of `call_settings.json`
//! (inside a versioned wrapper), so its serde shape is part of that format:
//! renaming a field or variant needs a settings version bump.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::types::{UserId, string_newtype};

/// An attenuation limit that limits nothing (DeepFilterNet removes all it
/// can): the default, and what an invalid value becomes.
const NO_ATTENUATION_LIMIT_DB: f32 = 100.0;

string_newtype!(
    /// An audio device, as named by the platform's audio system. Opaque and
    /// only meaningful on this machine.
    AudioDeviceId
);

/// Samples in one 10 ms audio frame (48 kHz mono) for [`VirtualAudio`].
pub const AUDIO_FRAME_LEN: usize = 480;

/// A software microphone and speakers, used instead of the system's audio
/// devices when [`AudioBackend::Virtual`] is configured (tests, bots,
/// headless use). Called every 10 ms from the audio thread, in real time:
/// do not block.
pub trait VirtualAudio: Send + Sync + 'static {
    /// Fills one 48 kHz mono microphone frame, samples in -1..=1.
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]);
    /// Receives one frame that would be played.
    fn playback(&self, frame: &[f32; AUDIO_FRAME_LEN]);
}

/// Where call and mic-test audio comes from and goes to.
#[derive(Clone, Default)]
pub enum AudioBackend {
    /// The system's devices (`AudioSettings::{input,output}_device`).
    #[default]
    System,
    /// A [`VirtualAudio`] implementation.
    Virtual(Arc<dyn VirtualAudio>),
}

impl std::fmt::Debug for AudioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => f.write_str("System"),
            Self::Virtual(_) => f.write_str("Virtual"),
        }
    }
}

/// Cleaning other people's voices: DeepFilterNet3 on this device, on each
/// voice before it is mixed (off by default). Costs CPU per voice and about
/// 30 ms of delay on each cleaned voice; music or game sound someone shares
/// through their microphone is treated as noise.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteCleaning {
    /// Clean the voices you hear.
    pub enabled: bool,
    /// Most attenuation applied, dB (0..=100, as
    /// `NoiseSuppression::DeepFilter`).
    pub attenuation_limit_db: f32,
    /// Voices their sender's device already cleans, left as they come.
    pub skip_cleaned: SkipCleaned,
    /// Clean on a GPU (opt-in; offer it when [`GpuEnhancement::available`]
    /// lists a backend): every voice given a filter is cleaned while it
    /// talks, all in one GPU call per frame, for less CPU than one voice on
    /// the CPU. The CPU cleans instead (at most [`limits::CLEANED_VOICES`]
    /// talking at once) when no GPU starts, and takes over if the GPU fails
    /// or falls behind ([`AudioStats::remote_cleaning`] says where it runs).
    pub gpu: bool,
}

impl Default for RemoteCleaning {
    fn default() -> Self {
        Self {
            enabled: false,
            attenuation_limit_db: NO_ATTENUATION_LIMIT_DB,
            skip_cleaned: SkipCleaned::default(),
            gpu: false,
        }
    }
}

/// Which voices [`RemoteCleaning`] leaves alone because their sender's
/// device says it cleans them already: Twicecat announces its suppressor
/// ([`CleaningAnnouncement`]), and LiveKit's web and mobile clients flag
/// their tracks (the browser's noise suppression, Krisp). Your per-person
/// choice wins over this.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipCleaned {
    /// Voices cleaned by a neural suppressor at their source (RNNoise,
    /// DeepFilterNet, Krisp, or cleaned before their client, as they say):
    /// cleaning them again gains little (the default).
    #[default]
    Neural,
    /// Also voices with a classic suppressor (WebRTC's, a browser's), which
    /// leaves more noise.
    AnySuppressor,
    /// Clean every voice, whatever their sender says.
    Never,
}

/// What others in a call learn about your microphone's cleaning, so their
/// Twicecat need not clean your voice again ([`SkipCleaned`]). Sent as a
/// LiveKit participant attribute, seen by everyone in the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleaningAnnouncement {
    /// Tell others which suppressor runs on your microphone (default on).
    /// Off, others may clean your voice a second time.
    pub enabled: bool,
    /// Your microphone is cleaned before Twicecat (NVIDIA Broadcast, a
    /// Krisp app, a headset's own processing): announced as cleaned by a
    /// neural suppressor, whatever [`AudioProcessing::noise_suppression`]
    /// says.
    pub cleaned_outside: bool,
}

impl Default for CleaningAnnouncement {
    fn default() -> Self {
        Self {
            enabled: true,
            cleaned_outside: false,
        }
    }
}

/// Per-person choice for [`RemoteCleaning`], kept across calls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceCleaning {
    /// As the global switch says (the default; not stored).
    #[default]
    Automatic,
    /// Clean this person's voice whenever cleaning is on, before others
    /// when too many talk at once.
    Always,
    /// Never clean this person's voice.
    Never,
}

/// An input or output device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    /// Identifier for `AudioSettings::{input,output}_device`.
    pub id: AudioDeviceId,
    /// Human-readable name.
    pub name: String,
    /// The system default device.
    pub is_default: bool,
}

/// Every audio device found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioDevices {
    /// Microphones and other capture devices.
    pub inputs: Vec<AudioDevice>,
    /// Speakers, headsets and other playback devices.
    pub outputs: Vec<AudioDevice>,
}

/// Strength of the classic (WebRTC) noise suppressor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NsLevel {
    /// About 6 dB.
    Low,
    /// About 12 dB.
    Moderate,
    /// About 18 dB.
    High,
    /// About 21 dB.
    VeryHigh,
}

/// Which noise suppressor runs on the microphone: exactly one, or none.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NoiseSuppression {
    /// None.
    Off,
    /// WebRTC's spectral suppressor (cheap).
    WebRtc {
        /// Strength.
        level: NsLevel,
    },
    /// RNNoise, a small neural network (default; light on battery).
    RnNoise,
    /// DeepFilterNet3 (best quality, more CPU). Falls back to RNNoise when
    /// its model is not installed.
    DeepFilter {
        /// Most attenuation applied, in dB (higher removes more, 0..=100).
        attenuation_limit_db: f32,
        /// Run the networks on a GPU (opt-in; offer it only when
        /// [`GpuEnhancement::available`] lists a backend). The same model
        /// and output; the CPU runs instead when no GPU starts, and takes
        /// over if the GPU fails or falls behind (reported as
        /// [`GpuEnhancement::running`]).
        #[serde(default)]
        gpu: bool,
    },
}

/// A GPU backend for speech enhancement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuBackend {
    /// NVIDIA, through CUDA.
    Cuda,
    /// NVIDIA, through TensorRT.
    TensorRt,
    /// Any vendor's GPU through WebGPU (Vulkan on Linux, Direct3D 12 on
    /// Windows, Metal on macOS).
    WebGpu,
    /// Any DirectX 12 GPU (Windows).
    DirectMl,
    /// Apple's GPU and Neural Engine (macOS).
    CoreMl,
}

/// Where DeepFilterNet3 runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EnhancerBackend {
    /// On the CPU, as asked (or no GPU backend started).
    Cpu,
    /// On a GPU.
    Gpu {
        /// Which.
        backend: GpuBackend,
    },
    /// On the CPU after the GPU failed (a driver error, a lost device).
    CpuAfterGpuError,
    /// On the CPU because the GPU kept missing its deadline (a game may be
    /// using it).
    CpuAfterGpuTooSlow,
}

/// GPU speech enhancement on this machine: answers `ProbeGpuEnhancement`,
/// and is sent again when where DeepFilterNet3 runs changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpuEnhancement {
    /// Backends that started here (empty without a usable GPU, its runtime
    /// or the GPU model, or in a build without GPU support). Offer the
    /// `gpu` setting only when this is not empty.
    pub available: Vec<GpuBackend>,
    /// Where DeepFilterNet3 runs now, when it runs.
    pub running: Option<EnhancerBackend>,
}

/// Microphone processing before sending, in this order: high-pass, echo
/// cancellation, noise suppression, automatic gain.
///
/// What combines and what excludes:
/// - `high_pass`, `echo_cancellation`, `noise_suppression` and `auto_gain`
///   are independent stages and run together.
/// - `noise_suppression` picks exactly one suppressor (or none): WebRTC,
///   RNNoise and DeepFilterNet3 never run on top of each other.
/// - `cancel_system_audio` extends echo cancellation and does nothing
///   without it.
/// - Processing done outside Twicecat (NVIDIA Broadcast, a virtual
///   PipeWire source, Krisp) is the user's choice: it is not detected, and
///   ours runs on top of it unless turned off here.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioProcessing {
    /// Remove the far end's voice picked up from the speakers (AEC3).
    pub echo_cancellation: bool,
    /// Noise suppressor.
    pub noise_suppression: NoiseSuppression,
    /// Automatic gain control (AGC2).
    pub auto_gain: bool,
    /// Remove low-frequency rumble.
    pub high_pass: bool,
    /// Also cancel other applications' sound (a game, a video) picked up by
    /// the microphone: echo cancellation then uses everything the output
    /// device plays as its reference, not only the call. Linux with
    /// PipeWire; needs `echo_cancellation`.
    pub cancel_system_audio: bool,
}

impl Default for AudioProcessing {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            noise_suppression: NoiseSuppression::RnNoise,
            auto_gain: true,
            high_pass: true,
            cancel_system_audio: false,
        }
    }
}

/// When the microphone transmits.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InputMode {
    /// Transmit while voice is detected.
    VoiceActivity {
        /// Pick the threshold automatically from the noise floor.
        automatic: bool,
        /// Gate threshold in dBFS when not automatic (-100..=0).
        threshold_db: f32,
    },
    /// Transmit only while the push-to-talk key is held.
    PushToTalk {
        /// Keep transmitting this long after release, in ms (0..=2000).
        release_delay_ms: u32,
    },
}

/// What push-to-talk listens to. The strings are parsed by `twicecat-ptt`
/// (the core only stores them); kept when the input mode changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PushToTalkBindings {
    /// Global keyboard shortcut in global-hotkey syntax: modifiers, then one
    /// key (`ctrl+shift+KeyT`, `F13`).
    pub key: Option<String>,
    /// Gamepad button by position (`south`, `left_thumb`, `right_trigger2`).
    pub gamepad_button: Option<String>,
}

/// All persistent audio settings. Send the whole value with
/// `Command::UpdateAudioSettings`; the core validates it (see
/// [`AudioSettings::validated`]) and echoes the effective value back as
/// `CoreEvent::AudioSettings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Capture device; `None` follows the system default.
    pub input_device: Option<AudioDeviceId>,
    /// Playback device; `None` follows the system default.
    pub output_device: Option<AudioDeviceId>,
    /// Microphone processing.
    pub processing: AudioProcessing,
    /// Voice activity or push-to-talk.
    pub input_mode: InputMode,
    /// Push-to-talk key and gamepad button.
    pub push_to_talk: PushToTalkBindings,
    /// Gain after processing, in dB (-20..=20).
    pub input_gain_db: f32,
    /// Master playback volume, percent (0..=200).
    pub output_volume_percent: u16,
    /// Opus bitrate for the microphone track, kbit/s (16..=510).
    pub voice_bitrate_kbps: u16,
    /// Per-user playback volume, percent (0..=200), kept across calls.
    pub user_volumes: BTreeMap<UserId, u16>,
    /// Cleaning other people's voices on this device.
    pub remote_cleaning: RemoteCleaning,
    /// Per-user cleaning choice, kept across calls (absent: automatic).
    pub user_cleaning: BTreeMap<UserId, VoiceCleaning>,
    /// What others learn about your microphone's cleaning.
    pub announce_cleaning: CleaningAnnouncement,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input_device: None,
            output_device: None,
            processing: AudioProcessing::default(),
            input_mode: InputMode::VoiceActivity {
                automatic: true,
                threshold_db: -50.0,
            },
            push_to_talk: PushToTalkBindings::default(),
            input_gain_db: 0.0,
            output_volume_percent: 100,
            // Opus voice is near-transparent here; Discord's default is 64.
            voice_bitrate_kbps: 64,
            user_volumes: BTreeMap::new(),
            remote_cleaning: RemoteCleaning::default(),
            user_cleaning: BTreeMap::new(),
            announce_cleaning: CleaningAnnouncement::default(),
        }
    }
}

/// Bounds enforced by [`AudioSettings::validated`].
pub mod limits {
    /// `input_gain_db`.
    pub const INPUT_GAIN_DB: (f32, f32) = (-20.0, 20.0);
    /// `InputMode::VoiceActivity::threshold_db`.
    pub const VAD_THRESHOLD_DB: (f32, f32) = (-100.0, 0.0);
    /// `InputMode::PushToTalk::release_delay_ms`.
    pub const PTT_RELEASE_DELAY_MS: u32 = 2000;
    /// Every volume percentage.
    pub const VOLUME_PERCENT: u16 = 200;
    /// `voice_bitrate_kbps` (Opus range for 48 kHz voice).
    pub const VOICE_BITRATE_KBPS: (u16, u16) = (16, 510);
    /// `NoiseSuppression::DeepFilter::attenuation_limit_db`.
    pub const DF_ATTENUATION_DB: (f32, f32) = (0.0, 100.0);
    /// Length of a push-to-talk binding string, bytes.
    pub const PTT_BINDING_LEN: usize = 64;
    /// Remote voices cleaned at the same time on the CPU (`RemoteCleaning`):
    /// those talking at once; quiet voices' filters rest. Each running
    /// filter costs about 0.3 ms per 10 ms frame on a desktop performance
    /// core and 0.55 ms on an efficiency core, on the audio thread (Task 16).
    pub const CLEANED_VOICES: usize = 4;
    /// Remote voices given a filter in one call (memory, and the time to
    /// make each); beyond them, voices are not cleaned.
    pub const CLEANING_FILTERS: usize = 16;
}

/// Clamps `v` into `range`; NaN becomes `fallback`.
fn clamp_f32(v: f32, (lo, hi): (f32, f32), fallback: f32) -> f32 {
    if v.is_nan() {
        fallback
    } else {
        v.clamp(lo, hi)
    }
}

impl AudioSettings {
    /// Returns these settings with every value clamped into its range (see
    /// [`limits`]). NaN values fall back to the default.
    ///
    /// ```
    /// use twicecat_core::api::AudioSettings;
    /// let s = AudioSettings { input_gain_db: 99.0, ..AudioSettings::default() };
    /// assert_eq!(s.validated().input_gain_db, 20.0);
    /// ```
    pub fn validated(mut self) -> Self {
        let defaults = Self::default();
        self.input_gain_db = clamp_f32(
            self.input_gain_db,
            limits::INPUT_GAIN_DB,
            defaults.input_gain_db,
        );
        self.output_volume_percent = self.output_volume_percent.min(limits::VOLUME_PERCENT);
        let (lo, hi) = limits::VOICE_BITRATE_KBPS;
        self.voice_bitrate_kbps = self.voice_bitrate_kbps.clamp(lo, hi);
        for volume in self.user_volumes.values_mut() {
            *volume = (*volume).min(limits::VOLUME_PERCENT);
        }
        // 100 % is the default; storing it would only grow the file.
        self.user_volumes.retain(|_, v| *v != 100);
        self.remote_cleaning.attenuation_limit_db = clamp_f32(
            self.remote_cleaning.attenuation_limit_db,
            limits::DF_ATTENUATION_DB,
            NO_ATTENUATION_LIMIT_DB,
        );
        self.user_cleaning
            .retain(|_, c| *c != VoiceCleaning::Automatic);
        match &mut self.input_mode {
            InputMode::VoiceActivity { threshold_db, .. } => {
                *threshold_db = clamp_f32(*threshold_db, limits::VAD_THRESHOLD_DB, -50.0);
            }
            InputMode::PushToTalk { release_delay_ms } => {
                *release_delay_ms = (*release_delay_ms).min(limits::PTT_RELEASE_DELAY_MS);
            }
        }
        // Bindings are short ASCII names; anything else is dropped.
        for binding in [
            &mut self.push_to_talk.key,
            &mut self.push_to_talk.gamepad_button,
        ] {
            if binding.as_ref().is_some_and(|b| {
                b.is_empty()
                    || b.len() > limits::PTT_BINDING_LEN
                    || !b.bytes().all(|c| c.is_ascii_graphic())
            }) {
                *binding = None;
            }
        }
        if let NoiseSuppression::DeepFilter {
            attenuation_limit_db,
            ..
        } = &mut self.processing.noise_suppression
        {
            *attenuation_limit_db = clamp_f32(
                *attenuation_limit_db,
                limits::DF_ATTENUATION_DB,
                NO_ATTENUATION_LIMIT_DB,
            );
        }
        self
    }

    /// Playback volume for `user`, percent.
    pub fn user_volume(&self, user: &UserId) -> u16 {
        self.user_volumes.get(user).copied().unwrap_or(100)
    }

    /// The cleaning choice for one user.
    pub fn user_cleaning(&self, user: &UserId) -> VoiceCleaning {
        self.user_cleaning.get(user).copied().unwrap_or_default()
    }
}

/// Microphone level, sent about 20 times a second while the microphone is
/// open (in a call or a mic test).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputLevel {
    /// RMS level after processing, dBFS.
    pub rms_db: f32,
    /// Peak level after processing, dBFS.
    pub peak_db: f32,
    /// The gate (voice activity or push-to-talk) is open: audio is sent.
    pub transmitting: bool,
}

/// Processing statistics for diagnostics, sent about once a second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioStats {
    /// Echo return loss enhancement, dB, when echo cancellation runs.
    pub echo_return_loss_enhancement_db: Option<f32>,
    /// Likelihood that echo is still audible (0..=1).
    pub residual_echo_likelihood: Option<f32>,
    /// Estimated delay between playback and its echo, ms.
    pub echo_delay_ms: Option<u32>,
    /// Processing time per 10 ms frame, microseconds (mean over the second).
    pub processing_us: u32,
    /// Since the previous stats: times processing started more than 1 ms
    /// after a frame of microphone audio was complete (a busy CPU; real-time
    /// priority keeps this at 0).
    pub late_wakes: u32,
    /// Since the previous stats: the longest wait from a complete microphone
    /// frame to processing it, microseconds (0 without a microphone).
    pub max_wake_us: u32,
    /// Since the previous stats: microphone samples lost because processing
    /// fell behind (heard by others as a gap).
    pub capture_overruns: u32,
    /// Since the previous stats: samples the speakers wanted and did not get
    /// (heard as a click).
    pub playback_underruns: u32,
    /// Where other people's voices are cleaned ([`RemoteCleaning`]); `None`
    /// while none is.
    pub remote_cleaning: Option<EnhancerBackend>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_within_limits() {
        let d = AudioSettings::default();
        assert_eq!(d.clone().validated(), d);
    }

    #[test]
    fn validated_clamps_every_field() {
        let mut user_volumes = BTreeMap::new();
        user_volumes.insert(UserId::from("@loud:x"), 900);
        user_volumes.insert(UserId::from("@same:x"), 100);
        let mut user_cleaning = BTreeMap::new();
        user_cleaning.insert(UserId::from("@noisy:x"), VoiceCleaning::Always);
        user_cleaning.insert(UserId::from("@plain:x"), VoiceCleaning::Automatic);
        let s = AudioSettings {
            input_gain_db: -99.0,
            output_volume_percent: 500,
            voice_bitrate_kbps: 2,
            user_volumes,
            user_cleaning,
            remote_cleaning: RemoteCleaning {
                enabled: true,
                attenuation_limit_db: f32::NAN,
                ..RemoteCleaning::default()
            },
            input_mode: InputMode::PushToTalk {
                release_delay_ms: 60_000,
            },
            processing: AudioProcessing {
                noise_suppression: NoiseSuppression::DeepFilter {
                    attenuation_limit_db: 500.0,
                    gpu: true,
                },
                ..AudioProcessing::default()
            },
            ..AudioSettings::default()
        }
        .validated();
        assert_eq!(s.input_gain_db, -20.0);
        assert_eq!(s.output_volume_percent, 200);
        assert_eq!(s.voice_bitrate_kbps, 16);
        assert_eq!(s.user_volume(&UserId::from("@loud:x")), 200);
        assert!(!s.user_volumes.contains_key(&UserId::from("@same:x")));
        assert_eq!(s.user_volume(&UserId::from("@same:x")), 100);
        assert_eq!(s.remote_cleaning.attenuation_limit_db, 100.0);
        assert_eq!(
            s.user_cleaning(&UserId::from("@noisy:x")),
            VoiceCleaning::Always
        );
        assert_eq!(
            s.user_cleaning.len(),
            1,
            "automatic is the default, not stored"
        );
        assert_eq!(
            s.input_mode,
            InputMode::PushToTalk {
                release_delay_ms: 2000
            }
        );
        assert_eq!(
            s.processing.noise_suppression,
            NoiseSuppression::DeepFilter {
                attenuation_limit_db: 100.0,
                gpu: true,
            }
        );
    }

    #[test]
    fn deep_filter_settings_saved_before_the_gpu_option_load_on_the_cpu() {
        let old = serde_json::json!({ "processing": { "noise_suppression": {
            "kind": "deep_filter", "attenuation_limit_db": 30.0 } } });
        let s: AudioSettings = serde_json::from_value(old).unwrap();
        assert_eq!(
            s.processing.noise_suppression,
            NoiseSuppression::DeepFilter {
                attenuation_limit_db: 30.0,
                gpu: false,
            }
        );
    }

    #[test]
    fn validated_drops_malformed_push_to_talk_bindings() {
        let keep = PushToTalkBindings {
            key: Some("ctrl+shift+KeyT".into()),
            gamepad_button: Some("left_thumb".into()),
        };
        let s = AudioSettings {
            push_to_talk: keep.clone(),
            ..AudioSettings::default()
        };
        assert_eq!(s.validated().push_to_talk, keep, "valid bindings stay");
        for bad in ["", "ctrl+ KeyT", "ctrl+\u{202e}KeyT", &"F".repeat(65)] {
            let s = AudioSettings {
                push_to_talk: PushToTalkBindings {
                    key: Some(bad.to_owned()),
                    gamepad_button: Some(bad.to_owned()),
                },
                ..AudioSettings::default()
            }
            .validated();
            assert_eq!(s.push_to_talk, PushToTalkBindings::default(), "{bad:?}");
        }
    }

    #[test]
    fn settings_without_bindings_still_load() {
        let old = serde_json::json!({ "input_mode": { "kind": "push_to_talk", "release_delay_ms": 200 } });
        let s: AudioSettings = serde_json::from_value(old).unwrap();
        assert_eq!(s.push_to_talk, PushToTalkBindings::default());
    }

    #[test]
    fn validated_replaces_nan() {
        let s = AudioSettings {
            input_gain_db: f32::NAN,
            input_mode: InputMode::VoiceActivity {
                automatic: false,
                threshold_db: f32::NAN,
            },
            ..AudioSettings::default()
        }
        .validated();
        assert_eq!(s.input_gain_db, 0.0);
        assert_eq!(
            s.input_mode,
            InputMode::VoiceActivity {
                automatic: false,
                threshold_db: -50.0
            }
        );
    }

    #[test]
    fn serde_shape_is_stable() {
        let json = serde_json::to_value(AudioSettings::default()).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "input_device": null,
                "output_device": null,
                "processing": {
                    "echo_cancellation": true,
                    "noise_suppression": { "kind": "rn_noise" },
                    "auto_gain": true,
                    "high_pass": true,
                    "cancel_system_audio": false,
                },
                "input_mode": { "kind": "voice_activity", "automatic": true, "threshold_db": -50.0 },
                "push_to_talk": { "key": null, "gamepad_button": null },
                "input_gain_db": 0.0,
                "output_volume_percent": 100,
                "voice_bitrate_kbps": 64,
                "user_volumes": {},
                "remote_cleaning": {
                    "enabled": false,
                    "attenuation_limit_db": 100.0,
                    "skip_cleaned": "neural",
                    "gpu": false,
                },
                "user_cleaning": {},
                "announce_cleaning": { "enabled": true, "cleaned_outside": false },
            })
        );
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: AudioSettings =
            serde_json::from_str(r#"{ "processing": { "auto_gain": false } }"#).unwrap();
        assert!(!s.processing.auto_gain);
        assert!(s.processing.echo_cancellation);
        assert_eq!(s.voice_bitrate_kbps, 64);
        assert_eq!(s.remote_cleaning, RemoteCleaning::default());
        let s: AudioSettings = serde_json::from_str(
            r#"{ "remote_cleaning": { "enabled": true }, "user_cleaning": { "@a:x": "never" } }"#,
        )
        .unwrap();
        assert!(s.remote_cleaning.enabled);
        assert_eq!(s.remote_cleaning.attenuation_limit_db, 100.0);
        assert_eq!(s.user_cleaning(&UserId::from("@a:x")), VoiceCleaning::Never);
        assert_eq!(s.remote_cleaning.skip_cleaned, SkipCleaned::Neural);
        assert_eq!(s.announce_cleaning, CleaningAnnouncement::default());
        assert!(s.announce_cleaning.enabled, "announced unless turned off");
    }
}
