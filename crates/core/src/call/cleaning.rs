//! Cleaning other people's voices (plan Task 16): which remote voices get a
//! DeepFilterNet3 filter of their own on this device (at most
//! `limits::CLEANING_FILTERS`), and each voice's status for the participant
//! list. One per call, shared by its SFU connections (they feed one voice
//! engine). The model is compiled once per call and each voice gets its own
//! state; both are made off the audio thread. The engine runs the filters
//! of voices that talk, at most `limits::CLEANED_VOICES` at once (voices set
//! to always first, then the loudest), and rests those of quiet voices.
//!
//! Voices their sender's device already cleans are left alone
//! ([`SkipCleaned`]): Tessera announces its microphone's suppressor as a
//! LiveKit participant attribute ([`ANNOUNCE_KEY`]), and LiveKit's web and
//! mobile clients flag their tracks (the browser's noise suppression, Krisp).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use livekit::prelude::AudioTrackFeature;
use tessera_audio::{CleaningFlag, DeepFilter, DeepFilterModel, SourceCleaning, SourceId};

use super::media::{SharedFlags, user_of};
use crate::api::{
    AudioSettings, CleaningStatus, NoiseSuppression, RemoteCleaning, SkipCleaned, SkipReason,
    UserId, VoiceCleaning, limits,
};
use crate::audio_session::Voice;

/// The LiveKit participant attribute where Tessera says which suppressor
/// cleans its microphone (other clients ignore it).
pub(crate) const ANNOUNCE_KEY: &str = "tessera.voice_cleaning";

/// What we announce under [`ANNOUNCE_KEY`] (`None`: nothing, the attribute
/// removed). DeepFilterNet counts whatever its attenuation limit, and falls
/// back to RNNoise (also neural) when its model is missing.
pub(crate) fn announcement(settings: &AudioSettings) -> Option<&'static str> {
    let announce = settings.announce_cleaning;
    if !announce.enabled {
        return None;
    }
    if announce.cleaned_outside {
        return Some("external");
    }
    match settings.processing.noise_suppression {
        NoiseSuppression::Off => None,
        NoiseSuppression::WebRtc { .. } => Some("webrtc"),
        NoiseSuppression::RnNoise => Some("rnnoise"),
        NoiseSuppression::DeepFilter { .. } => Some("deepfilternet3"),
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
            Some("rnnoise" | "deepfilternet3" | "external") => Self::Neural,
            Some("webrtc") => Self::Classic,
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
enum Decision {
    Off,
    Clean,
    Skip(SkipReason),
}

/// The voices to clean (`(identity, join order)`): none while cleaning is
/// off; voices set to never are skipped, and so are automatic ones whose
/// sender cleans them already (`claims`, by identity) as the policy says;
/// voices set to always come first, then the earliest to join; at most
/// `limit`.
fn decide(
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// No filter.
    Plain,
    /// Waits for the model.
    AwaitingModel,
    /// Its filter is being made.
    Creating,
    /// Its filter runs.
    Cleaned,
    /// Its filter failed (or could not be handed over); not retried in this
    /// call.
    Failed,
}

struct Entry {
    source: SourceId,
    flag: CleaningFlag,
    joined: u64,
    state: State,
    /// Told to the engine: cleaned first when too many talk at once.
    priority: bool,
    /// When its filter was handed to the engine, which plugs it in on its
    /// next frame: before that the flag is still down.
    handed_over: Option<Instant>,
}

/// A handed-over filter whose flag is still down after this has failed.
const START_GRACE: Duration = Duration::from_secs(1);

enum Model {
    Absent,
    Loading,
    Ready(DeepFilterModel),
    /// Not installed (or failed to load): nothing is cleaned in this call.
    Missing,
}

struct Inner {
    settings: RemoteCleaning,
    choices: BTreeMap<UserId, VoiceCleaning>,
    voice: Option<Voice>,
    model: Model,
    voices: BTreeMap<String, Entry>,
    /// What senders say about their voice, by identity (absent: nothing),
    /// kept while they are in the call, playing or not.
    claims: HashMap<String, Claim>,
    next_joined: u64,
    flags: SharedFlags,
    watching: bool,
}

/// See the module docs. Cheap to clone.
#[derive(Clone)]
pub(crate) struct Cleaning(Arc<Mutex<Inner>>);

impl Cleaning {
    pub(crate) fn new(
        voice: Option<Voice>,
        settings: RemoteCleaning,
        choices: BTreeMap<UserId, VoiceCleaning>,
        flags: SharedFlags,
    ) -> Self {
        if let Some(voice) = &voice {
            let _ = voice
                .lock()
                .expect("voice control lock")
                .set_cleaning_slots(limits::CLEANED_VOICES);
        }
        Self(Arc::new(Mutex::new(Inner {
            settings,
            choices,
            voice,
            model: Model::Absent,
            voices: BTreeMap::new(),
            claims: HashMap::new(),
            next_joined: 0,
            flags,
            watching: false,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().expect("cleaning lock")
    }

    /// A remote voice started playing as `source`.
    pub(crate) fn add(&self, identity: &str, source: SourceId, flag: CleaningFlag) {
        {
            let mut inner = self.lock();
            let joined = inner.next_joined;
            inner.next_joined += 1;
            inner.voices.insert(
                identity.to_owned(),
                Entry {
                    source,
                    flag,
                    joined,
                    state: State::Plain,
                    priority: false,
                    handed_over: None,
                },
            );
        }
        self.reconcile();
    }

    /// A remote voice stopped (its source is removed, and with it its
    /// filter); another may now fit under the limit.
    pub(crate) fn remove(&self, identity: &str) {
        let removed = self.lock().voices.remove(identity).is_some();
        if removed {
            self.reconcile();
        }
    }

    /// What `identity`'s sender says about its voice now (`Claim::None` as
    /// it leaves).
    pub(crate) fn set_claim(&self, identity: &str, claim: Claim) {
        let changed = {
            let mut inner = self.lock();
            let old = if claim == Claim::None {
                inner.claims.remove(identity)
            } else {
                inner.claims.insert(identity.to_owned(), claim)
            };
            old.unwrap_or_default() != claim
        };
        if changed {
            self.reconcile();
        }
    }

    /// New settings: a new attenuation limit goes to the running filters,
    /// the rest decides again which voices are cleaned.
    pub(crate) fn set_settings(
        &self,
        settings: RemoteCleaning,
        choices: BTreeMap<UserId, VoiceCleaning>,
    ) {
        {
            let mut inner = self.lock();
            if settings.attenuation_limit_db != inner.settings.attenuation_limit_db
                && let Some(voice) = &inner.voice
            {
                let _ = voice
                    .lock()
                    .expect("voice control lock")
                    .set_cleaner_attenuation(settings.attenuation_limit_db);
            }
            inner.settings = settings;
            inner.choices = choices;
        }
        self.reconcile();
    }

    /// Starts and stops filters as the decisions say, and publishes every
    /// voice's status.
    fn reconcile(&self) {
        let mut inner = self.lock();
        let Some(voice) = inner.voice.clone() else {
            return;
        };
        let voices: Vec<(&str, u64)> = inner
            .voices
            .iter()
            .map(|(identity, e)| (identity.as_str(), e.joined))
            .collect();
        let decisions = decide(
            &inner.settings,
            &inner.choices,
            &voices,
            &inner.claims,
            limits::CLEANING_FILTERS,
        );
        let ready = match &inner.model {
            Model::Ready(model) => Some(model.clone()),
            _ => None,
        };
        let limit = inner.settings.attenuation_limit_db;
        let mut load_model = false;
        let mut create = Vec::new();
        let always: Vec<bool> = inner
            .voices
            .keys()
            .map(|identity| {
                inner.choices.get(&UserId::from(user_of(identity))) == Some(&VoiceCleaning::Always)
            })
            .collect();
        for ((identity, entry), always) in inner.voices.iter_mut().zip(always) {
            if entry.priority != always {
                entry.priority = always;
                let _ = voice
                    .lock()
                    .expect("voice control lock")
                    .set_source_priority(entry.source, always);
            }
            let clean = decisions.get(identity) == Some(&Decision::Clean);
            entry.state = match (clean, entry.state) {
                (true, State::Plain | State::AwaitingModel) => match &ready {
                    Some(model) => {
                        create.push((identity.clone(), entry.source, model.clone()));
                        State::Creating
                    }
                    None => {
                        load_model = true;
                        State::AwaitingModel
                    }
                },
                (false, State::Cleaned) => {
                    let _ = voice
                        .lock()
                        .expect("voice control lock")
                        .set_source_cleaner(entry.source, None);
                    State::Plain
                }
                // A filter on its way is dropped when it arrives.
                (false, State::AwaitingModel | State::Creating) => State::Plain,
                (_, state) => state,
            };
        }
        if load_model && matches!(inner.model, Model::Absent) {
            inner.model = Model::Loading;
            self.load_model();
        }
        for (identity, source, model) in create {
            self.create_filter(identity, source, model, limit);
        }
        if !inner.watching && inner.voices.values().any(|e| e.state == State::Cleaned) {
            inner.watching = true;
            self.watch_failures();
        }
        publish(&inner, &decisions);
    }

    fn load_model(&self) {
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let model = tokio::task::spawn_blocking(|| {
                let dir = tessera_audio::default_model_dir()?;
                DeepFilterModel::load(&dir).ok()
            })
            .await
            .ok()
            .flatten();
            let Some(this) = upgrade(&weak) else {
                return;
            };
            if model.is_none() {
                tracing::warn!(
                    "DeepFilterNet's model could not be loaded; other voices are not cleaned"
                );
            }
            this.lock().model = model.map_or(Model::Missing, Model::Ready);
            this.reconcile();
        });
    }

    fn create_filter(
        &self,
        identity: String,
        source: SourceId,
        model: DeepFilterModel,
        limit: f32,
    ) {
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let filter = tokio::task::spawn_blocking(move || model.filter(limit).ok())
                .await
                .ok()
                .flatten();
            let unused = match upgrade(&weak) {
                Some(this) => {
                    let unused = this.hand_over(&identity, source, filter, limit);
                    this.reconcile();
                    unused
                }
                None => filter,
            };
            if let Some(filter) = unused {
                let _ = tokio::task::spawn_blocking(move || drop(filter)).await;
            }
        });
    }

    /// Gives a new filter to its voice if it still wants one; returns it
    /// otherwise.
    fn hand_over(
        &self,
        identity: &str,
        source: SourceId,
        filter: Option<DeepFilter>,
        limit: f32,
    ) -> Option<DeepFilter> {
        let mut inner = self.lock();
        let current_limit = inner.settings.attenuation_limit_db;
        let Some(voice) = inner.voice.clone() else {
            return filter;
        };
        let Some(entry) = inner
            .voices
            .get_mut(identity)
            .filter(|e| e.source == source && e.state == State::Creating)
        else {
            return filter;
        };
        let Some(filter) = filter else {
            entry.state = State::Failed;
            return None;
        };
        let mut control = voice.lock().expect("voice control lock");
        entry.state = if control.set_source_cleaner(source, Some(Box::new(filter))) {
            entry.handed_over = Some(Instant::now());
            if current_limit != limit {
                let _ = control.set_cleaner_attenuation(current_limit);
            }
            State::Cleaned
        } else {
            // The engine's queue is full (stuck): the filter went with the
            // rejected command.
            State::Failed
        };
        None
    }

    /// About once a second while any filter is plugged in: notices filters
    /// that failed on the audio thread (it plays the voice as it came and
    /// clears its flag), and republishes the statuses (a voice over the
    /// limit of talkers shows it).
    fn watch_failures(&self) {
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            // Not at once: a filter just handed over is not plugged in yet.
            let mut ticker =
                tokio::time::interval_at(tokio::time::Instant::now() + START_GRACE, START_GRACE);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let Some(this) = upgrade(&weak) else {
                    return;
                };
                let (failed, running) = {
                    let mut inner = this.lock();
                    let mut failed = false;
                    for entry in inner.voices.values_mut() {
                        let started = entry.handed_over.is_none_or(|t| t.elapsed() >= START_GRACE);
                        if entry.state == State::Cleaned
                            && started
                            && entry.flag.get() == SourceCleaning::None
                        {
                            entry.state = State::Failed;
                            failed = true;
                        }
                    }
                    let running = inner.voices.values().any(|e| e.state == State::Cleaned);
                    inner.watching = running;
                    (failed, running)
                };
                if failed {
                    tracing::warn!("a voice's DeepFilterNet filter failed; it plays as it came");
                }
                this.reconcile();
                if !running {
                    return;
                }
            }
        });
    }
}

fn upgrade(weak: &Weak<Mutex<Inner>>) -> Option<Cleaning> {
    weak.upgrade().map(Cleaning)
}

/// Writes every voice's status into the shared flags (read by the
/// participant list), and nudges it when something changed.
fn publish(inner: &Inner, decisions: &HashMap<String, Decision>) {
    let missing = matches!(inner.model, Model::Missing);
    let statuses: HashMap<String, CleaningStatus> = inner
        .voices
        .iter()
        .map(|(identity, entry)| {
            let status = match decisions.get(identity).copied().unwrap_or(Decision::Off) {
                Decision::Off => CleaningStatus::Off,
                Decision::Skip(reason) => CleaningStatus::Skipped(reason),
                Decision::Clean => match entry.state {
                    State::Cleaned => match entry.flag.get() {
                        SourceCleaning::Running | SourceCleaning::Quiet => CleaningStatus::Cleaned,
                        SourceCleaning::OverLimit => CleaningStatus::Skipped(SkipReason::Limit),
                        // Handed over, not plugged in yet.
                        SourceCleaning::None => CleaningStatus::Starting,
                    },
                    State::Failed => CleaningStatus::Skipped(SkipReason::Unavailable),
                    State::AwaitingModel if missing => {
                        CleaningStatus::Skipped(SkipReason::Unavailable)
                    }
                    State::Plain | State::AwaitingModel | State::Creating => {
                        CleaningStatus::Starting
                    }
                },
            };
            (identity.clone(), status)
        })
        .collect();
    let changed = match inner.flags.flags.write() {
        Ok(mut flags) if flags.cleaning != statuses => {
            flags.cleaning = statuses;
            true
        }
        _ => false,
    };
    if changed {
        inner.flags.changed.notify_one();
    }
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
        for value in ["rnnoise", "deepfilternet3", "webrtc", "external"] {
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
