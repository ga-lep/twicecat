//! Which remote voices are cleaned (pure decisions, unit-tested), and the
//! announcement that tells others how ours is cleaned.

use std::collections::{BTreeMap, HashMap};

use livekit::prelude::AudioTrackFeature;

use super::super::media::user_of;
use crate::api::{
    AudioSettings, NoiseSuppression, RemoteCleaning, SkipCleaned, SkipReason, UserId, VoiceCleaning,
};

/// The LiveKit participant attribute where Tessera says which suppressor
/// cleans its microphone (other clients ignore it).
pub(crate) const ANNOUNCE_KEY: &str = "tessera.voice_cleaning";

// Its values: the wire format other Tessera clients read (approved with
// Task 16 part 2; a change needs every client to follow).
/// WebRTC's classic suppressor.
const WEBRTC: &str = "webrtc";
/// RNNoise.
const RNNOISE: &str = "rnnoise";
/// DeepFilterNet3 (on the CPU or a GPU, whatever its attenuation limit).
const DEEPFILTERNET3: &str = "deepfilternet3";
/// Cleaned before Tessera (NVIDIA Broadcast, Krisp), by the user's word.
const EXTERNAL: &str = "external";

/// What we announce under [`ANNOUNCE_KEY`] (`None`: nothing, the attribute
/// removed). DeepFilterNet counts whatever its attenuation limit, and falls
/// back to RNNoise (also neural) when its model is missing.
pub(crate) fn announcement(settings: &AudioSettings) -> Option<&'static str> {
    let announce = settings.announce_cleaning;
    if !announce.enabled {
        return None;
    }
    if announce.cleaned_outside {
        return Some(EXTERNAL);
    }
    match settings.processing.noise_suppression {
        NoiseSuppression::Off => None,
        NoiseSuppression::WebRtc { .. } => Some(WEBRTC),
        NoiseSuppression::RnNoise => Some(RNNOISE),
        NoiseSuppression::DeepFilter { .. } => Some(DEEPFILTERNET3),
    }
}

/// How clean a voice's sender says it is, weakest first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Claim {
    #[default]
    None,
    /// A classic suppressor: WebRTC's, a browser's.
    Classic,
    /// A neural one: RNNoise, DeepFilterNet, Krisp, or one before the
    /// client.
    Neural,
}

impl Claim {
    /// From the sender's announcement (Tessera; an unknown value counts
    /// for nothing) and its microphone track's features (LiveKit's clients:
    /// `TF_NOISE_SUPPRESSION` is the browser's suppressor,
    /// `TF_ENHANCED_NOISE_CANCELLATION` Krisp); the strongest wins.
    pub(crate) fn of(announced: Option<&str>, features: &[AudioTrackFeature]) -> Self {
        let announced = match announced {
            Some(RNNOISE | DEEPFILTERNET3 | EXTERNAL) => Self::Neural,
            Some(WEBRTC) => Self::Classic,
            _ => Self::None,
        };
        let flagged = features
            .iter()
            .map(|f| match f {
                AudioTrackFeature::TfEnhancedNoiseCancellation => Self::Neural,
                AudioTrackFeature::TfNoiseSuppression => Self::Classic,
                _ => Self::None,
            })
            .max()
            .unwrap_or_default();
        announced.max(flagged)
    }
}

/// What the settings say about one voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Off,
    Clean,
    Skip(SkipReason),
}

/// The voices to clean (`(identity, join order)`): none while cleaning is
/// off; voices set to never are skipped, and so are automatic ones whose
/// sender cleans them already (`claims`, by identity) as the policy says;
/// voices set to always come first, then the earliest to join; at most
/// `limit`.
pub(super) fn decide(
    settings: &RemoteCleaning,
    choices: &BTreeMap<UserId, VoiceCleaning>,
    voices: &[(&str, u64)],
    claims: &HashMap<String, Claim>,
    limit: usize,
) -> HashMap<String, Decision> {
    let choice = |identity: &str| {
        choices
            .get(&UserId::from(user_of(identity)))
            .copied()
            .unwrap_or_default()
    };
    if !settings.enabled {
        return voices
            .iter()
            .map(|(identity, _)| ((*identity).to_owned(), Decision::Off))
            .collect();
    }
    let skip_from = match settings.skip_cleaned {
        SkipCleaned::Neural => Some(Claim::Neural),
        SkipCleaned::AnySuppressor => Some(Claim::Classic),
        SkipCleaned::Never => None,
    };
    let clean_already = |identity: &str| {
        skip_from.is_some_and(|from| claims.get(identity).copied().unwrap_or_default() >= from)
    };
    let mut order: Vec<&(&str, u64)> = voices.iter().collect();
    order.sort_by_key(|(identity, joined)| (choice(identity) != VoiceCleaning::Always, *joined));
    let mut cleaned = 0;
    order
        .into_iter()
        .map(|(identity, _)| {
            let decision = if choice(identity) == VoiceCleaning::Never {
                Decision::Skip(SkipReason::Never)
            } else if choice(identity) == VoiceCleaning::Automatic && clean_already(identity) {
                Decision::Skip(SkipReason::AlreadyClean)
            } else if cleaned < limit {
                cleaned += 1;
                Decision::Clean
            } else {
                Decision::Skip(SkipReason::Limit)
            };
            ((*identity).to_owned(), decision)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> RemoteCleaning {
        RemoteCleaning {
            enabled: true,
            ..RemoteCleaning::default()
        }
    }

    #[test]
    fn nothing_is_cleaned_while_off() {
        let voices = [("@a:x:D1", 0), ("@b:x:D2", 1)];
        let d = decide(
            &RemoteCleaning::default(),
            &BTreeMap::new(),
            &voices,
            &HashMap::new(),
            4,
        );
        assert!(d.values().all(|d| *d == Decision::Off));
    }

    #[test]
    fn the_earliest_voices_are_cleaned_up_to_the_limit() {
        let voices = [("@c:x:D", 2), ("@a:x:D", 0), ("@b:x:D", 1)];
        let d = decide(&on(), &BTreeMap::new(), &voices, &HashMap::new(), 2);
        assert_eq!(d["@a:x:D"], Decision::Clean);
        assert_eq!(d["@b:x:D"], Decision::Clean);
        assert_eq!(d["@c:x:D"], Decision::Skip(SkipReason::Limit));
    }

    #[test]
    fn always_comes_first_and_never_is_skipped_without_using_a_slot() {
        let mut choices = BTreeMap::new();
        choices.insert(UserId::from("@late:x"), VoiceCleaning::Always);
        choices.insert(UserId::from("@quiet:x"), VoiceCleaning::Never);
        let voices = [
            ("@quiet:x:D", 0),
            ("@a:x:D", 1),
            ("@b:x:D", 2),
            ("@late:x:D", 3),
        ];
        let d = decide(&on(), &choices, &voices, &HashMap::new(), 2);
        assert_eq!(d["@late:x:D"], Decision::Clean, "always, though last");
        assert_eq!(d["@quiet:x:D"], Decision::Skip(SkipReason::Never));
        assert_eq!(d["@a:x:D"], Decision::Clean);
        assert_eq!(d["@b:x:D"], Decision::Skip(SkipReason::Limit));
    }

    #[test]
    fn voices_cleaned_at_their_source_are_skipped_as_the_policy_says() {
        let voices = [("@krisp:x:D", 0), ("@browser:x:D", 1), ("@plain:x:D", 2)];
        let mut claims = HashMap::new();
        claims.insert("@krisp:x:D".to_owned(), Claim::Neural);
        claims.insert("@browser:x:D".to_owned(), Claim::Classic);
        let policy = |skip_cleaned| RemoteCleaning {
            skip_cleaned,
            ..on()
        };
        let d = decide(&on(), &BTreeMap::new(), &voices, &claims, 2);
        assert_eq!(d["@krisp:x:D"], Decision::Skip(SkipReason::AlreadyClean));
        assert_eq!(
            d["@browser:x:D"],
            Decision::Clean,
            "classic: cleaned by default"
        );
        assert_eq!(
            d["@plain:x:D"],
            Decision::Clean,
            "a skipped voice uses no slot"
        );
        let d = decide(
            &policy(SkipCleaned::AnySuppressor),
            &BTreeMap::new(),
            &voices,
            &claims,
            4,
        );
        assert_eq!(d["@browser:x:D"], Decision::Skip(SkipReason::AlreadyClean));
        assert_eq!(d["@plain:x:D"], Decision::Clean);
        let d = decide(
            &policy(SkipCleaned::Never),
            &BTreeMap::new(),
            &voices,
            &claims,
            4,
        );
        assert!(d.values().all(|d| *d == Decision::Clean));
    }

    #[test]
    fn a_per_person_choice_wins_over_what_the_sender_says() {
        let mut choices = BTreeMap::new();
        choices.insert(UserId::from("@a:x"), VoiceCleaning::Always);
        choices.insert(UserId::from("@b:x"), VoiceCleaning::Never);
        let voices = [("@a:x:D", 0), ("@b:x:D", 1)];
        let claims = voices
            .iter()
            .map(|(identity, _)| ((*identity).to_owned(), Claim::Neural))
            .collect();
        let d = decide(&on(), &choices, &voices, &claims, 4);
        assert_eq!(d["@a:x:D"], Decision::Clean);
        assert_eq!(d["@b:x:D"], Decision::Skip(SkipReason::Never));
    }

    #[test]
    fn claims_come_from_the_announcement_and_the_track_flags() {
        use AudioTrackFeature as F;
        assert_eq!(Claim::of(None, &[]), Claim::None);
        assert_eq!(Claim::of(Some("rnnoise"), &[]), Claim::Neural);
        assert_eq!(Claim::of(Some("deepfilternet3"), &[]), Claim::Neural);
        assert_eq!(Claim::of(Some("external"), &[]), Claim::Neural);
        assert_eq!(Claim::of(Some("webrtc"), &[]), Claim::Classic);
        assert_eq!(Claim::of(Some("something new"), &[]), Claim::None);
        assert_eq!(
            Claim::of(None, &[F::TfEchoCancellation, F::TfNoiseSuppression]),
            Claim::Classic
        );
        assert_eq!(
            Claim::of(Some("webrtc"), &[F::TfEnhancedNoiseCancellation]),
            Claim::Neural,
            "the strongest wins"
        );
    }

    #[test]
    fn we_announce_our_suppressor_unless_told_not_to() {
        use crate::api::{CleaningAnnouncement, NsLevel};
        let with = |noise_suppression, announce_cleaning| {
            let mut s = AudioSettings {
                announce_cleaning,
                ..AudioSettings::default()
            };
            s.processing.noise_suppression = noise_suppression;
            announcement(&s)
        };
        let on = CleaningAnnouncement::default();
        let outside = CleaningAnnouncement {
            cleaned_outside: true,
            ..on
        };
        let off = CleaningAnnouncement {
            enabled: false,
            cleaned_outside: true,
        };
        assert_eq!(with(NoiseSuppression::RnNoise, on), Some("rnnoise"));
        assert_eq!(
            with(
                NoiseSuppression::DeepFilter {
                    attenuation_limit_db: 100.0,
                    gpu: false
                },
                on
            ),
            Some("deepfilternet3")
        );
        assert_eq!(
            with(
                NoiseSuppression::WebRtc {
                    level: NsLevel::High
                },
                on
            ),
            Some("webrtc")
        );
        assert_eq!(with(NoiseSuppression::Off, on), None);
        assert_eq!(with(NoiseSuppression::Off, outside), Some("external"));
        assert_eq!(with(NoiseSuppression::RnNoise, off), None);
        for value in [RNNOISE, DEEPFILTERNET3, WEBRTC, EXTERNAL] {
            assert_ne!(
                Claim::of(Some(value), &[]),
                Claim::None,
                "{value} is read back"
            );
        }
    }

    #[test]
    fn a_choice_covers_every_device_of_its_user() {
        let mut choices = BTreeMap::new();
        choices.insert(UserId::from("@a:x"), VoiceCleaning::Never);
        let voices = [("@a:x:PHONE", 0), ("@a:x:DESK", 1)];
        let d = decide(&on(), &choices, &voices, &HashMap::new(), 4);
        assert!(d.values().all(|d| *d == Decision::Skip(SkipReason::Never)));
    }
}
