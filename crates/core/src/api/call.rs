//! Call and audio types of the core API (Phase 6). None of them names an SDK,
//! LiveKit or audio-backend type.
//!
//! [`AudioSettings`] is also the on-disk format of `call_settings.json`
//! (inside a versioned wrapper), so its serde shape is part of that format:
//! renaming a field or variant needs a settings version bump.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::types::{DeviceId, RoomId, UserId, string_newtype};

string_newtype!(
    /// One MatrixRTC participant: a user's device in a call. Opaque; stable
    /// for the length of that device's membership.
    ParticipantId
);
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

/// What a call is for. Only voice for now; video comes later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallIntent {
    /// Voice only.
    Audio,
}

/// The local user's call, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallState {
    /// Not in a call.
    Idle,
    /// Joining `room`: getting the SFU token, connecting, publishing.
    Joining {
        /// Room of the call.
        room_id: RoomId,
    },
    /// In the call.
    Connected {
        /// Room of the call.
        room_id: RoomId,
        /// Media is end-to-end encrypted (an encrypted room, as Element
        /// Call does).
        encrypted: bool,
    },
    /// The SFU connection dropped; reconnecting.
    Reconnecting {
        /// Room of the call.
        room_id: RoomId,
        /// Consecutive failed attempts so far.
        attempt: u32,
    },
    /// Left the call.
    Left {
        /// Room of the call.
        room_id: RoomId,
        /// Why.
        reason: CallEndReason,
    },
}

/// Why the local user left a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallEndReason {
    /// `LeaveCall`, logout or shutdown.
    Hangup,
    /// Joining or reconnecting failed for good (non-secret description).
    Failed(String),
}

/// One person (device) in the current call.
#[derive(Clone, Debug, PartialEq)]
pub struct Participant {
    /// Stable within the call.
    pub id: ParticipantId,
    /// Account.
    pub user_id: UserId,
    /// Device.
    pub device_id: DeviceId,
    /// Display name, if known.
    pub display_name: Option<String>,
    /// This device.
    pub is_local: bool,
    /// Microphone muted (by them, or not publishing).
    pub muted: bool,
    /// Currently speaking (SFU or local voice activity).
    pub speaking: bool,
    /// Local playback volume for this user, percent (100 = unchanged).
    pub volume_percent: u16,
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

/// Which noise suppressor runs on the microphone.
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
    },
}

/// Microphone processing before sending.
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
}

impl Default for AudioProcessing {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            noise_suppression: NoiseSuppression::RnNoise,
            auto_gain: true,
            high_pass: true,
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
    /// Gain after processing, in dB (-20..=20).
    pub input_gain_db: f32,
    /// Master playback volume, percent (0..=200).
    pub output_volume_percent: u16,
    /// Opus bitrate for the microphone track, kbit/s (16..=510).
    pub voice_bitrate_kbps: u16,
    /// Per-user playback volume, percent (0..=200), kept across calls.
    pub user_volumes: BTreeMap<UserId, u16>,
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
            input_gain_db: 0.0,
            output_volume_percent: 100,
            // Opus voice is near-transparent here; Discord's default is 64.
            voice_bitrate_kbps: 64,
            user_volumes: BTreeMap::new(),
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
    /// use tessera_core::api::AudioSettings;
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
        match &mut self.input_mode {
            InputMode::VoiceActivity { threshold_db, .. } => {
                *threshold_db = clamp_f32(*threshold_db, limits::VAD_THRESHOLD_DB, -50.0);
            }
            InputMode::PushToTalk { release_delay_ms } => {
                *release_delay_ms = (*release_delay_ms).min(limits::PTT_RELEASE_DELAY_MS);
            }
        }
        if let NoiseSuppression::DeepFilter {
            attenuation_limit_db,
        } = &mut self.processing.noise_suppression
        {
            *attenuation_limit_db =
                clamp_f32(*attenuation_limit_db, limits::DF_ATTENUATION_DB, 100.0);
        }
        self
    }

    /// Playback volume for `user`, percent.
    pub fn user_volume(&self, user: &UserId) -> u16 {
        self.user_volumes.get(user).copied().unwrap_or(100)
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
        let s = AudioSettings {
            input_gain_db: -99.0,
            output_volume_percent: 500,
            voice_bitrate_kbps: 2,
            user_volumes,
            input_mode: InputMode::PushToTalk {
                release_delay_ms: 60_000,
            },
            processing: AudioProcessing {
                noise_suppression: NoiseSuppression::DeepFilter {
                    attenuation_limit_db: 500.0,
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
        assert_eq!(
            s.input_mode,
            InputMode::PushToTalk {
                release_delay_ms: 2000
            }
        );
        assert_eq!(
            s.processing.noise_suppression,
            NoiseSuppression::DeepFilter {
                attenuation_limit_db: 100.0
            }
        );
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
                },
                "input_mode": { "kind": "voice_activity", "automatic": true, "threshold_db": -50.0 },
                "input_gain_db": 0.0,
                "output_volume_percent": 100,
                "voice_bitrate_kbps": 64,
                "user_volumes": {},
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
    }
}
