//! Cleaning other people's voices (plan Task 16): which remote voices get a
//! DeepFilterNet3 filter of their own on this device (at most
//! `limits::CLEANING_FILTERS`), and each voice's status for the participant
//! list. One per call, shared by its SFU connections (they feed one voice
//! engine). The model is compiled once per call and each voice gets its own
//! state; both are made off the audio thread. The engine runs the filters
//! of voices that talk, at most `limits::CLEANED_VOICES` at once (voices set
//! to always first, then the loudest), and rests those of quiet voices.
//!
//! With the GPU setting the call loads one bank (a batched graph, one row
//! per voice, `limits::CLEANING_FILTERS` rows) instead: every voice given a
//! row is cleaned in one GPU call per frame while it talks, and the CPU
//! filters take over for the rest of the call when no GPU starts or the bank
//! gives up.
//!
//! Voices their sender's device already cleans are left alone
//! ([`SkipCleaned`]): Tessera announces its microphone's suppressor as a
//! LiveKit participant attribute ([`ANNOUNCE_KEY`]), and LiveKit's web and
//! mobile clients flag their tracks (the browser's noise suppression, Krisp).

mod policy;

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use tessera_audio::{
    CleaningFlag, DeepFilter, DeepFilterBackend, DeepFilterModel, DeepFilterStatus, SourceCleaning,
    SourceId,
};

pub(crate) use self::policy::{ANNOUNCE_KEY, Claim, announcement};
use self::policy::{Decision, decide};
use super::media::{SharedFlags, user_of};
use crate::api::{
    CleaningStatus, EnhancerBackend, GpuBackend, RemoteCleaning, SkipReason, UserId, VoiceCleaning,
    limits,
};
use crate::audio_session::{CleaningBackend, Voice, voice_control};
use crate::gpu_enhancement;

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
    /// Cleaned in this row of the GPU bank.
    Row(usize),
    /// Its filter failed (or could not be handed over); not retried in this
    /// call.
    Failed,
}

impl State {
    /// Handed to the engine, which cleans it from its next frame.
    fn handed_over(self) -> bool {
        matches!(self, Self::Cleaned | Self::Row(_))
    }
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

/// The call's GPU bank, with the GPU setting.
enum Gpu {
    /// Not loaded (the setting is off, or no voice needed it yet).
    Absent,
    Loading,
    /// In the engine; `rows[r]`: row `r` is taken.
    Ready {
        backend: GpuBackend,
        status: DeepFilterStatus,
        rows: Vec<bool>,
    },
    /// No GPU started (or it was too slow at warm-up, or the GPU parts are
    /// missing): the CPU cleans.
    Unavailable,
    /// It gave up in this call (why): the CPU cleans.
    GaveUp(EnhancerBackend),
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
    gpu: Gpu,
    /// Where the cleaning runs, for `AudioStats`.
    backend: Option<CleaningBackend>,
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
        backend: Option<CleaningBackend>,
    ) -> Self {
        if let Some(voice) = &voice {
            let _ = voice_control(voice).set_cleaning_slots(limits::CLEANED_VOICES);
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
            gpu: Gpu::Absent,
            backend,
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
        let removed = {
            let mut inner = self.lock();
            let removed = inner.voices.remove(identity);
            if let (
                Some(Entry {
                    state: State::Row(row),
                    ..
                }),
                Gpu::Ready { rows, .. },
            ) = (&removed, &mut inner.gpu)
            {
                rows[*row] = false;
            }
            removed.is_some()
        };
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
                let _ = voice_control(voice).set_cleaner_attenuation(settings.attenuation_limit_db);
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
        // The GPU setting turned off: the bank goes (with every row), and a
        // later turn on tries again.
        if !inner.settings.gpu && !matches!(inner.gpu, Gpu::Absent) {
            if matches!(inner.gpu, Gpu::Ready { .. }) {
                let _ = voice_control(&voice).set_cleaner_bank(None);
            }
            for entry in inner.voices.values_mut() {
                if let State::Row(_) = entry.state {
                    entry.state = State::Plain;
                }
            }
            inner.gpu = Gpu::Absent;
        }
        let use_gpu = inner.settings.gpu && !matches!(inner.gpu, Gpu::Unavailable | Gpu::GaveUp(_));
        let mut load_bank = false;
        let always: Vec<bool> = inner
            .voices
            .keys()
            .map(|identity| {
                inner.choices.get(&UserId::from(user_of(identity))) == Some(&VoiceCleaning::Always)
            })
            .collect();
        let Inner { voices, gpu, .. } = &mut *inner;
        for ((identity, entry), always) in voices.iter_mut().zip(always) {
            if entry.priority != always {
                entry.priority = always;
                let _ = voice_control(&voice).set_source_priority(entry.source, always);
            }
            let clean = decisions.get(identity) == Some(&Decision::Clean);
            if let (false, State::Row(row)) = (clean, entry.state) {
                let _ = voice_control(&voice).set_source_row(entry.source, None);
                if let Gpu::Ready { rows, .. } = gpu {
                    rows[row] = false;
                }
                entry.state = State::Plain;
                continue;
            }
            if clean && use_gpu {
                entry.state = match (entry.state, &mut *gpu) {
                    (State::Row(row), _) => State::Row(row),
                    (State::Failed, _) => State::Failed,
                    (state, Gpu::Ready { rows, .. }) => {
                        match rows.iter().position(|taken| !taken) {
                            Some(row) => {
                                let mut control = voice_control(&voice);
                                if state == State::Cleaned {
                                    let _ = control.set_source_cleaner(entry.source, None);
                                }
                                if control.set_source_row(entry.source, Some(row)) {
                                    rows[row] = true;
                                    entry.handed_over = Some(Instant::now());
                                    State::Row(row)
                                } else {
                                    State::Failed
                                }
                            }
                            // More voices than rows: `decide` keeps them under.
                            None => State::Failed,
                        }
                    }
                    // A CPU filter keeps cleaning until the bank is in.
                    (State::Cleaned, _) => {
                        load_bank = true;
                        State::Cleaned
                    }
                    (_, _) => {
                        load_bank = true;
                        State::AwaitingModel
                    }
                };
                continue;
            }
            entry.state = match (clean, entry.state) {
                // A row here means the bank gave up (the engine took every
                // row): the CPU now.
                (true, State::Row(_) | State::Plain | State::AwaitingModel) => match &ready {
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
                    let _ = voice_control(&voice).set_source_cleaner(entry.source, None);
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
        if load_bank && matches!(inner.gpu, Gpu::Absent) {
            inner.gpu = Gpu::Loading;
            self.load_bank(limit);
        }
        for (identity, source, model) in create {
            self.create_filter(identity, source, model, limit);
        }
        if !inner.watching && inner.voices.values().any(|e| e.state.handed_over()) {
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

    fn load_bank(&self, limit: f32) {
        let weak = Arc::downgrade(&self.0);
        tokio::spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || {
                let dir = tessera_audio::default_model_dir().ok_or("no model directory")?;
                gpu_enhancement::load_bank(&dir, limits::CLEANING_FILTERS, limit)
            })
            .await
            .unwrap_or_else(|_| Err("the GPU load stopped".to_owned()));
            let unused = match upgrade(&weak) {
                Some(this) => {
                    let unused = this.hand_over_bank(loaded);
                    this.reconcile();
                    unused
                }
                None => loaded.ok().flatten(),
            };
            if let Some(bank) = unused {
                let _ = tokio::task::spawn_blocking(move || drop(bank)).await;
            }
        });
    }

    /// Plugs a loaded bank into the engine if the GPU is still wanted;
    /// returns it otherwise.
    fn hand_over_bank(
        &self,
        loaded: Result<Option<gpu_enhancement::Bank>, String>,
    ) -> Option<gpu_enhancement::Bank> {
        let mut inner = self.lock();
        if !matches!(inner.gpu, Gpu::Loading) {
            return loaded.ok().flatten();
        }
        let (bank, backend, status) = match loaded {
            Ok(Some(bank)) => bank,
            Ok(None) => {
                tracing::info!("no GPU keeps up for cleaning other voices; the CPU cleans them");
                inner.gpu = Gpu::Unavailable;
                return None;
            }
            Err(reason) => {
                tracing::warn!("other voices are cleaned on the CPU: {reason}");
                inner.gpu = Gpu::Unavailable;
                return None;
            }
        };
        let Some(voice) = inner.voice.clone() else {
            return Some((bank, backend, status));
        };
        let current_limit = inner.settings.attenuation_limit_db;
        let mut control = voice_control(&voice);
        inner.gpu = if control.set_cleaner_bank(Some(bank)) {
            let _ = control.set_cleaner_attenuation(current_limit);
            Gpu::Ready {
                backend,
                status,
                rows: vec![false; limits::CLEANING_FILTERS],
            }
        } else {
            // The engine's queue is full (stuck): the bank went with it.
            Gpu::Unavailable
        };
        None
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
        let mut control = voice_control(&voice);
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
                let (failed, gave_up, running) = {
                    let mut inner = this.lock();
                    let mut failed = false;
                    let mut gave_up = false;
                    for entry in inner.voices.values_mut() {
                        let started = entry.handed_over.is_none_or(|t| t.elapsed() >= START_GRACE);
                        if !(started && entry.flag.get() == SourceCleaning::None) {
                            continue;
                        }
                        match entry.state {
                            State::Cleaned => {
                                entry.state = State::Failed;
                                failed = true;
                            }
                            // The engine dropped the bank and every row.
                            State::Row(_) => gave_up = true,
                            _ => {}
                        }
                    }
                    if gave_up && let Gpu::Ready { status, .. } = &inner.gpu {
                        let why = match status.get() {
                            DeepFilterBackend::CpuAfterGpuTooSlow => {
                                EnhancerBackend::CpuAfterGpuTooSlow
                            }
                            _ => EnhancerBackend::CpuAfterGpuError,
                        };
                        inner.gpu = Gpu::GaveUp(why);
                    }
                    let running = inner.voices.values().any(|e| e.state.handed_over());
                    inner.watching = running;
                    (failed, gave_up, running)
                };
                if failed {
                    tracing::warn!("a voice's DeepFilterNet filter failed; it plays as it came");
                }
                if gave_up {
                    tracing::warn!("cleaning other voices on the GPU stopped; the CPU takes over");
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
                    State::Cleaned | State::Row(_) => match entry.flag.get() {
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
    if let Some(slot) = &inner.backend
        && let Ok(mut slot) = slot.lock()
    {
        *slot = where_cleaned(inner);
    }
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

/// Where the voices cleaned now are cleaned (`None`: none is).
fn where_cleaned(inner: &Inner) -> Option<EnhancerBackend> {
    let mut states = inner.voices.values().map(|e| e.state);
    if states.clone().any(|s| matches!(s, State::Row(_))) {
        return match &inner.gpu {
            Gpu::Ready { backend, .. } => Some(EnhancerBackend::Gpu { backend: *backend }),
            _ => None,
        };
    }
    states
        .any(|s| s == State::Cleaned)
        .then_some(match &inner.gpu {
            Gpu::GaveUp(why) => *why,
            _ => EnhancerBackend::Cpu,
        })
}
