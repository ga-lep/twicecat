//! Call types of the core API (Phase 6): the call's state and who is in
//! it. None of them names an SDK or LiveKit type. The audio settings and
//! statistics are in `audio`.

use serde::{Deserialize, Serialize};

use super::types::{DeviceId, RoomId, UserId, string_newtype};

string_newtype!(
    /// One MatrixRTC participant: a user's device in a call. Opaque; stable
    /// for the length of that device's membership.
    ParticipantId
);
/// What a call is for, as MatrixRTC's `m.call.intent` says it: rings and
/// call lists show it. It does not decide what is sent; cameras and shares
/// are turned on with their own commands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallIntent {
    /// A voice call.
    #[default]
    Audio,
    /// A video call.
    Video,
}

impl CallIntent {
    /// The value of `m.call.intent` (matrix-js-sdk's spelling).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// From `m.call.intent`; anything but `video` is a voice call.
    pub fn from_intent(value: Option<&str>) -> Self {
        match value {
            Some("video") => Self::Video,
            _ => Self::Audio,
        }
    }
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
    /// Whether this device cleans their voice ([`AudioSettings::remote_cleaning`](super::AudioSettings::remote_cleaning)).
    pub cleaning: CleaningStatus,
}

/// Whether this device cleans a participant's voice with DeepFilterNet3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CleaningStatus {
    /// Not cleaned: cleaning other voices is off, or this is you.
    #[default]
    Off,
    /// Getting ready (the model loads once per call).
    Starting,
    /// Cleaned here, about 30 ms later than it would play otherwise.
    Cleaned,
    /// Not cleaned, and why.
    Skipped(SkipReason),
}

/// Why a voice is not cleaned while cleaning is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Your per-person setting says never ([`VoiceCleaning::Never`](super::VoiceCleaning::Never)).
    Never,
    /// More people talk at once than this device cleans
    /// ([`limits::CLEANED_VOICES`](super::limits::CLEANED_VOICES); voices set to always come first, then the
    /// loudest), or more voices are in the call than it keeps filters for
    /// ([`limits::CLEANING_FILTERS`](super::limits::CLEANING_FILTERS); the earliest to join keep theirs).
    Limit,
    /// DeepFilterNet's model is not installed, or the filter failed.
    Unavailable,
    /// Their device says it cleans their voice already
    /// ([`RemoteCleaning::skip_cleaned`](super::RemoteCleaning::skip_cleaned)); set
    /// them to [`VoiceCleaning::Always`](super::VoiceCleaning::Always) to clean
    /// it here too.
    AlreadyClean,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intents_use_matrix_js_sdk_spelling() {
        assert_eq!(CallIntent::Audio.as_str(), "audio");
        assert_eq!(CallIntent::Video.as_str(), "video");
        assert_eq!(CallIntent::from_intent(Some("video")), CallIntent::Video);
        for other in [None, Some("audio"), Some("Video"), Some("")] {
            assert_eq!(
                CallIntent::from_intent(other),
                CallIntent::Audio,
                "{other:?}"
            );
        }
        assert_eq!(
            serde_json::to_string(&CallIntent::Video).unwrap(),
            "\"video\""
        );
    }
}
