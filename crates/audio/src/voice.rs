//! The voice engine: one [`FrameProcessor`] that mixes the remote
//! participants for playback, feeds exactly what is played to the echo
//! canceller, processes the microphone, and queues the result for sending.
//!
//! Everything here runs on the processing thread. The owner talks to it
//! through [`VoiceControl`]: commands go through a lock-free queue, and
//! levels, the transmit flag and counters come back through atomics.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use crate::chain::{CaptureChain, ChainSettings, ChainStats, LEVEL_FLOOR_DB};
use crate::frame::{FRAME_LEN, Frame, SILENCE};
use crate::pipeline::FrameProcessor;

/// Per remote source: 200 ms of 48 kHz audio.
pub const SOURCE_RING_LEN: usize = 9600;
/// A source more than this far ahead (100 ms) is trimmed to keep latency
/// bounded when its sender's clock runs faster than ours.
const SOURCE_MAX_QUEUED: usize = 4800;
/// Outgoing (microphone) queue: 200 ms.
pub const OUTGOING_RING_LEN: usize = 9600;
/// Commands waiting for the processing thread.
const CONTROL_QUEUE_LEN: usize = 64;
/// Enhancers waiting to be dropped by the control side (the outgoing one
/// and the remote voices' cleaners).
const RETIRED_LEN: usize = 32;

/// Identifies a remote source (the core maps it to a participant).
pub type SourceId = u64;

/// A remote voice quieter than this (RMS, about -60 dBFS)...
const QUIET_ENERGY: f32 = 1e-6;
/// ...for this many frames in a row (300 ms) stops running its cleaner,
/// until it is louder again: most voices in a call are silent most of the
/// time, and the network is the cost.
const QUIET_FRAMES: u32 = 30;
/// Cleaners that run at once by default (voices talking at the same time).
pub const CLEANING_SLOTS: usize = 4;

/// What a remote voice's cleaner does, frame by frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum SourceCleaning {
    /// No cleaner (none set, or it failed).
    None = 0,
    /// The cleaner runs on this voice.
    Running = 1,
    /// The voice is quiet: its cleaner rests and the voice plays at the
    /// cleaner's floor (its attenuation limit).
    Quiet = 2,
    /// More voices talk at once than cleaners may run
    /// ([`VoiceControl::set_cleaning_slots`]): this one plays as it came.
    OverLimit = 3,
}

impl SourceCleaning {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Running,
            2 => Self::Quiet,
            3 => Self::OverLimit,
            _ => Self::None,
        }
    }
}

/// An `f32` in an atomic.
#[derive(Debug, Default)]
struct AtomicF32(AtomicU32);

impl AtomicF32 {
    fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

fn gain_from_percent(percent: u16) -> f32 {
    f32::from(percent) / 100.0
}

fn rms_db(frame: &Frame) -> f32 {
    let mean = frame.iter().map(|s| s * s).sum::<f32>() / FRAME_LEN as f32;
    if mean <= 1e-10 {
        LEVEL_FLOOR_DB
    } else {
        (10.0 * mean.log10()).max(LEVEL_FLOOR_DB)
    }
}

/// Feeds one remote participant's decoded audio (48 kHz mono, -1..=1) into
/// the mix. Held by whatever receives that participant's track.
pub struct SourceInput {
    ring: rtrb::Producer<f32>,
    level: Arc<AtomicF32>,
    dropped: Arc<AtomicU64>,
    cleaning: Arc<AtomicU8>,
}

impl SourceInput {
    /// Queues decoded samples; samples that do not fit are dropped (and
    /// counted).
    pub fn push(&mut self, samples: &[f32]) {
        let (_, rest) = self.ring.push_partial_slice(samples);
        if !rest.is_empty() {
            self.dropped.fetch_add(rest.len() as u64, Ordering::Relaxed);
        }
    }

    /// Playback level of this source in dBFS after its volume (for a
    /// "speaking" indicator).
    pub fn level_db(&self) -> f32 {
        self.level.load()
    }

    /// Samples dropped because the source was too far ahead.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// What this voice's cleaner did in the last frame (set with
    /// [`VoiceControl::set_source_cleaner`]; `None` again if it failed).
    pub fn cleaning(&self) -> SourceCleaning {
        SourceCleaning::from_u8(self.cleaning.load(Ordering::Relaxed))
    }

    /// [`cleaning`](Self::cleaning), readable by whoever manages the
    /// cleaners.
    pub fn cleaning_flag(&self) -> CleaningFlag {
        CleaningFlag(self.cleaning.clone())
    }
}

/// What one remote voice's cleaner does; see [`SourceInput::cleaning_flag`].
#[derive(Clone, Debug)]
pub struct CleaningFlag(Arc<AtomicU8>);

impl CleaningFlag {
    /// As of the last frame (`None` before a cleaner is plugged in and after
    /// one failed).
    pub fn get(&self) -> SourceCleaning {
        SourceCleaning::from_u8(self.0.load(Ordering::Relaxed))
    }
}

/// The microphone side, held by whatever sends the track.
pub struct Outgoing {
    ring: rtrb::Consumer<f32>,
    transmitting: Arc<AtomicBool>,
}

impl Outgoing {
    /// Takes one frame of processed microphone audio when one is ready.
    /// Closed-gate and muted frames are silence.
    pub fn pop_frame(&mut self, out: &mut Frame) -> bool {
        self.ring.pop_entire_slice(&mut out[..]).is_ok()
    }

    /// The gate is open (and not muted): the user is talking.
    pub fn transmitting(&self) -> bool {
        self.transmitting.load(Ordering::Relaxed)
    }
}

enum Command {
    Configure(ChainSettings),
    PushToTalk(bool),
    Muted(bool),
    Deafened(bool),
    Loopback(bool),
    MasterVolume(u16),
    AddSource(Box<Source>),
    Enhancer(Option<Box<dyn crate::chain::Enhancer>>),
    EnhancerAttenuation(f32),
    SourceVolume(SourceId, u16),
    SourceCleaner(SourceId, Option<Box<dyn crate::chain::Enhancer>>),
    SourcePriority(SourceId, bool),
    CleanerAttenuation(f32),
    CleaningSlots(usize),
    RemoveSource(SourceId),
}

struct Source {
    id: SourceId,
    ring: rtrb::Consumer<f32>,
    gain: f32,
    level: Arc<AtomicF32>,
    dropped: Arc<AtomicU64>,
    /// Cleans this voice before it is mixed (DeepFilterNet3, Task 16).
    cleaner: Option<Box<dyn crate::chain::Enhancer>>,
    cleaning: Arc<AtomicU8>,
    /// Cleaned before others when too many talk at once.
    priority: bool,
    /// This frame's audio.
    chunk: Frame,
    /// Frames in a row below [`QUIET_ENERGY`].
    quiet_frames: u32,
    /// Smoothed energy (about 100 ms), to pick the loudest.
    loudness: f32,
    /// Its cleaner ran in the last frame (kept running while it talks).
    running: bool,
    /// Picked to run this frame.
    selected: bool,
}

/// Levels and counters shared with [`VoiceControl`].
#[derive(Debug)]
struct Shared {
    input_rms_db: AtomicF32,
    input_peak_db: AtomicF32,
    transmitting: Arc<AtomicBool>,
    erle_db: AtomicF32,
    residual_echo: AtomicF32,
    delay_ms: AtomicU32,
    processing_ns: AtomicU64,
    frames: AtomicU64,
    source_underruns: AtomicU64,
    outgoing_drops: AtomicU64,
}

/// What [`VoiceControl::meters`] reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meters {
    /// Processed microphone RMS level, dBFS.
    pub input_rms_db: f32,
    /// Processed microphone peak level, dBFS.
    pub input_peak_db: f32,
    /// Gate open and not muted.
    pub transmitting: bool,
}

/// Diagnostics read by [`VoiceControl::stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoiceStats {
    /// Echo cancellation statistics (from the last frame).
    pub chain: ChainStats,
    /// Mean processing time per frame since the last call, microseconds.
    pub processing_us: u32,
    /// Frames processed in total.
    pub frames: u64,
    /// Remote samples that were missing when mixing (network jitter).
    pub source_underruns: u64,
    /// Outgoing samples dropped because nobody took them.
    pub outgoing_drops: u64,
}

/// The owner's handle on a running [`VoiceEngine`].
pub struct VoiceControl {
    commands: rtrb::Producer<Command>,
    /// Enhancers the engine let go of, dropped here.
    retired: rtrb::Consumer<Box<dyn crate::chain::Enhancer>>,
    shared: Arc<Shared>,
    next_source: SourceId,
    last_frames: u64,
    last_ns: u64,
}

impl VoiceControl {
    fn send(&mut self, command: Command) -> bool {
        self.commands.push(command).is_ok()
    }

    /// Changes the processing settings. Returns false if the queue is full
    /// (the processing thread is stuck); retry later.
    pub fn configure(&mut self, settings: ChainSettings) -> bool {
        self.send(Command::Configure(settings))
    }

    /// Plugs a speech enhancer (loaded off this thread) into the chain, or
    /// removes it; it runs while the noise mode is `NoiseMode::Enhancer`.
    pub fn set_enhancer(&mut self, enhancer: Option<Box<dyn crate::Enhancer>>) -> bool {
        self.drop_retired();
        self.send(Command::Enhancer(enhancer))
    }

    /// Sets the plugged enhancer's attenuation limit, dB, without reloading
    /// it.
    pub fn set_enhancer_attenuation(&mut self, db: f32) -> bool {
        self.send(Command::EnhancerAttenuation(db))
    }

    /// Drops the enhancers the engine replaced or that failed (also done by
    /// [`set_enhancer`](Self::set_enhancer) and [`stats`](Self::stats)).
    /// Returns how many.
    pub fn drop_retired(&mut self) -> usize {
        let mut n = 0;
        while let Ok(enhancer) = self.retired.pop() {
            drop(enhancer);
            n += 1;
        }
        n
    }

    /// Push-to-talk key state.
    pub fn push_to_talk(&mut self, pressed: bool) -> bool {
        self.send(Command::PushToTalk(pressed))
    }

    /// Mute the microphone (the chain keeps running for the meter).
    pub fn set_muted(&mut self, muted: bool) -> bool {
        self.send(Command::Muted(muted))
    }

    /// Deafen: play nothing.
    pub fn set_deafened(&mut self, deafened: bool) -> bool {
        self.send(Command::Deafened(deafened))
    }

    /// Mic test: play the processed microphone back.
    pub fn set_loopback(&mut self, loopback: bool) -> bool {
        self.send(Command::Loopback(loopback))
    }

    /// Master playback volume, percent.
    pub fn set_master_volume(&mut self, percent: u16) -> bool {
        self.send(Command::MasterVolume(percent))
    }

    /// Adds a remote source at `volume_percent`; returns its id and the
    /// input to feed it, or `None` if the queue is full.
    pub fn add_source(&mut self, volume_percent: u16) -> Option<(SourceId, SourceInput)> {
        let id = self.next_source;
        let (producer, consumer) = rtrb::RingBuffer::new(SOURCE_RING_LEN);
        let level = Arc::new(AtomicF32::new(LEVEL_FLOOR_DB));
        let dropped = Arc::new(AtomicU64::new(0));
        let cleaning = Arc::new(AtomicU8::new(0));
        let source = Source {
            id,
            ring: consumer,
            gain: gain_from_percent(volume_percent),
            level: level.clone(),
            dropped: dropped.clone(),
            cleaner: None,
            cleaning: cleaning.clone(),
            priority: false,
            chunk: SILENCE,
            quiet_frames: 0,
            loudness: 0.0,
            running: false,
            selected: false,
        };
        if !self.send(Command::AddSource(Box::new(source))) {
            return None;
        }
        self.next_source += 1;
        Some((
            id,
            SourceInput {
                ring: producer,
                level,
                dropped,
                cleaning,
            },
        ))
    }

    /// Changes one source's volume, percent.
    pub fn set_source_volume(&mut self, id: SourceId, percent: u16) -> bool {
        self.send(Command::SourceVolume(id, percent))
    }

    /// Cleans one remote voice with `cleaner` (made off this thread) before
    /// it is mixed, or stops cleaning it. A replaced or failing cleaner comes
    /// back here to be dropped; a failing one leaves the voice as it is.
    pub fn set_source_cleaner(
        &mut self,
        id: SourceId,
        cleaner: Option<Box<dyn crate::Enhancer>>,
    ) -> bool {
        self.drop_retired();
        self.send(Command::SourceCleaner(id, cleaner))
    }

    /// Sets every remote voice cleaner's attenuation limit, dB.
    pub fn set_cleaner_attenuation(&mut self, db: f32) -> bool {
        self.send(Command::CleanerAttenuation(db))
    }

    /// Cleans this voice before the others when more talk at once than
    /// cleaners may run.
    pub fn set_source_priority(&mut self, id: SourceId, priority: bool) -> bool {
        self.send(Command::SourcePriority(id, priority))
    }

    /// How many cleaners run at once (voices talking at the same time;
    /// default [`CLEANING_SLOTS`]). Quiet voices' cleaners rest and do not
    /// count.
    pub fn set_cleaning_slots(&mut self, slots: usize) -> bool {
        self.send(Command::CleaningSlots(slots))
    }

    /// Removes a source (its [`SourceInput`] may be dropped too).
    pub fn remove_source(&mut self, id: SourceId) -> bool {
        self.send(Command::RemoveSource(id))
    }

    /// Current microphone level and transmit state.
    pub fn meters(&self) -> Meters {
        Meters {
            input_rms_db: self.shared.input_rms_db.load(),
            input_peak_db: self.shared.input_peak_db.load(),
            transmitting: self.shared.transmitting.load(Ordering::Relaxed),
        }
    }

    /// Diagnostics; `processing_us` is averaged since the previous call.
    pub fn stats(&mut self) -> VoiceStats {
        self.drop_retired();
        let s = &self.shared;
        let frames = s.frames.load(Ordering::Relaxed);
        let ns = s.processing_ns.load(Ordering::Relaxed);
        let df = frames.saturating_sub(self.last_frames);
        let processing_us = ns
            .saturating_sub(self.last_ns)
            .checked_div(df)
            .map_or(0, |per_frame| {
                u32::try_from(per_frame / 1000).unwrap_or(u32::MAX)
            });
        self.last_frames = frames;
        self.last_ns = ns;
        let delay = s.delay_ms.load(Ordering::Relaxed);
        let erle = s.erle_db.load();
        let residual = s.residual_echo.load();
        VoiceStats {
            chain: ChainStats {
                erle_db: erle.is_finite().then_some(erle),
                residual_echo_likelihood: residual.is_finite().then_some(residual),
                delay_ms: (delay != u32::MAX).then_some(delay),
            },
            processing_us,
            frames,
            source_underruns: s.source_underruns.load(Ordering::Relaxed),
            outgoing_drops: s.outgoing_drops.load(Ordering::Relaxed),
        }
    }
}

/// See the module docs. Create with [`VoiceEngine::new`] and hand it to
/// [`crate::AudioIo::start`].
pub struct VoiceEngine {
    chain: CaptureChain,
    commands: rtrb::Consumer<Command>,
    retired: rtrb::Producer<Box<dyn crate::chain::Enhancer>>,
    shared: Arc<Shared>,
    outgoing: rtrb::Producer<f32>,
    sources: Vec<Source>,
    /// Scratch: the talking voices with a cleaner, this frame.
    talking: Vec<usize>,
    /// Cleaners that may run at once.
    cleaning_slots: usize,
    /// What a resting cleaner leaves of a quiet voice.
    cleaner_floor: f32,
    muted: bool,
    deafened: bool,
    loopback: bool,
    master_gain: f32,
    mix: Frame,
    mic: Frame,
    last_mic: Frame,
    /// What the output device played this frame (all applications), when
    /// the I/O runs an echo reference stream; else the AEC gets our mix.
    played: Option<Frame>,
    /// Frames between stats refreshes (sonora's stats are not free).
    stats_countdown: u32,
}

impl VoiceEngine {
    /// A voice engine, its control handle, and the outgoing microphone
    /// stream.
    pub fn new(settings: ChainSettings) -> (Self, VoiceControl, Outgoing) {
        let (commands_tx, commands_rx) = rtrb::RingBuffer::new(CONTROL_QUEUE_LEN);
        let (out_tx, out_rx) = rtrb::RingBuffer::new(OUTGOING_RING_LEN);
        let (retired_tx, retired_rx) = rtrb::RingBuffer::new(RETIRED_LEN);
        let transmitting = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            input_rms_db: AtomicF32::new(LEVEL_FLOOR_DB),
            input_peak_db: AtomicF32::new(LEVEL_FLOOR_DB),
            transmitting: transmitting.clone(),
            erle_db: AtomicF32::new(f32::NAN),
            residual_echo: AtomicF32::new(f32::NAN),
            delay_ms: AtomicU32::new(u32::MAX),
            processing_ns: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            source_underruns: AtomicU64::new(0),
            outgoing_drops: AtomicU64::new(0),
        });
        let engine = Self {
            chain: CaptureChain::new(settings),
            commands: commands_rx,
            retired: retired_tx,
            shared: shared.clone(),
            outgoing: out_tx,
            sources: Vec::with_capacity(16),
            talking: Vec::with_capacity(16),
            cleaning_slots: CLEANING_SLOTS,
            cleaner_floor: floor_gain(100.0),
            muted: false,
            deafened: false,
            loopback: false,
            master_gain: 1.0,
            mix: SILENCE,
            mic: SILENCE,
            last_mic: SILENCE,
            played: None,
            stats_countdown: 0,
        };
        let control = VoiceControl {
            commands: commands_tx,
            retired: retired_rx,
            shared,
            next_source: 1,
            last_frames: 0,
            last_ns: 0,
        };
        let outgoing = Outgoing {
            ring: out_rx,
            transmitting,
        };
        (engine, control, outgoing)
    }

    fn apply_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            match command {
                Command::Configure(s) => self.chain.configure(s),
                Command::Enhancer(e) => self.chain.set_enhancer(e),
                Command::EnhancerAttenuation(db) => self.chain.set_enhancer_attenuation(db),
                Command::PushToTalk(p) => self.chain.set_push_to_talk(p),
                Command::Muted(m) => self.muted = m,
                Command::Deafened(d) => self.deafened = d,
                Command::Loopback(l) => self.loopback = l,
                Command::MasterVolume(p) => self.master_gain = gain_from_percent(p),
                Command::AddSource(source) => self.sources.push(*source),
                Command::SourceVolume(id, p) => {
                    if let Some(s) = self.sources.iter_mut().find(|s| s.id == id) {
                        s.gain = gain_from_percent(p);
                    }
                }
                Command::SourceCleaner(id, cleaner) => {
                    if let Some(s) = self.sources.iter_mut().find(|s| s.id == id) {
                        if cleaner.is_none() {
                            s.cleaning
                                .store(SourceCleaning::None as u8, Ordering::Relaxed);
                        }
                        s.running = false;
                        if let Some(old) = std::mem::replace(&mut s.cleaner, cleaner) {
                            retire(&mut self.retired, old);
                        }
                    } else if let Some(cleaner) = cleaner {
                        // The voice left meanwhile.
                        retire(&mut self.retired, cleaner);
                    }
                }
                Command::SourcePriority(id, priority) => {
                    if let Some(s) = self.sources.iter_mut().find(|s| s.id == id) {
                        s.priority = priority;
                    }
                }
                Command::CleanerAttenuation(db) => {
                    self.cleaner_floor = floor_gain(db);
                    for cleaner in self.sources.iter_mut().filter_map(|s| s.cleaner.as_mut()) {
                        cleaner.set_attenuation_limit(db);
                    }
                }
                Command::CleaningSlots(slots) => self.cleaning_slots = slots,
                Command::RemoveSource(id) => {
                    if let Some(i) = self.sources.iter().position(|s| s.id == id) {
                        let mut source = self.sources.remove(i);
                        if let Some(cleaner) = source.cleaner.take() {
                            retire(&mut self.retired, cleaner);
                        }
                    }
                }
            }
        }
    }

    fn mix_sources(&mut self) {
        self.mix = SILENCE;
        // This frame of every voice, and how loud it is.
        for source in &mut self.sources {
            // Trim a source that runs ahead of our clock.
            let queued = source.ring.slots();
            if queued > SOURCE_MAX_QUEUED {
                let excess = queued - SOURCE_MAX_QUEUED;
                if let Ok(c) = source.ring.read_chunk(excess) {
                    c.commit_all();
                }
                source.dropped.fetch_add(excess as u64, Ordering::Relaxed);
            }
            let (filled, missing) = source.ring.pop_partial_slice(&mut source.chunk[..]);
            let got = filled.len();
            missing.fill(0.0);
            if got < FRAME_LEN {
                self.shared
                    .source_underruns
                    .fetch_add((FRAME_LEN - got) as u64, Ordering::Relaxed);
            }
            let energy = source.chunk.iter().map(|s| s * s).sum::<f32>() / FRAME_LEN as f32;
            source.loudness = 0.9 * source.loudness + 0.1 * energy;
            source.quiet_frames = if energy < QUIET_ENERGY {
                source.quiet_frames.saturating_add(1)
            } else {
                0
            };
            source.selected = false;
        }

        // Which cleaners run: those of voices that talk, at most
        // `cleaning_slots`; priority first, then those already running (no
        // switching back and forth), then the loudest.
        self.talking.clear();
        self.talking.extend(
            self.sources
                .iter()
                .enumerate()
                .filter(|(_, s)| s.cleaner.is_some() && s.quiet_frames < QUIET_FRAMES)
                .map(|(i, _)| i),
        );
        if self.talking.len() > self.cleaning_slots {
            let sources = &self.sources;
            self.talking.sort_unstable_by(|&a, &b| {
                let (a, b) = (&sources[a], &sources[b]);
                (b.priority, b.running)
                    .cmp(&(a.priority, a.running))
                    .then(b.loudness.total_cmp(&a.loudness))
            });
            self.talking.truncate(self.cleaning_slots);
        }
        for &i in &self.talking {
            self.sources[i].selected = true;
        }

        for source in &mut self.sources {
            let state = match &source.cleaner {
                None => SourceCleaning::None,
                Some(_) if source.selected => SourceCleaning::Running,
                Some(_) if source.quiet_frames >= QUIET_FRAMES => SourceCleaning::Quiet,
                Some(_) => SourceCleaning::OverLimit,
            };
            let state = match (state, source.cleaner.as_mut()) {
                (SourceCleaning::Running, Some(cleaner)) => {
                    if cleaner.enhance(&mut source.chunk).is_ok() {
                        SourceCleaning::Running
                    } else {
                        // The voice plays as it came; the owner sees it.
                        if let Some(cleaner) = source.cleaner.take() {
                            retire(&mut self.retired, cleaner);
                        }
                        SourceCleaning::None
                    }
                }
                (SourceCleaning::Quiet, _) => {
                    // What the cleaner would leave of a quiet voice.
                    for s in &mut source.chunk {
                        *s *= self.cleaner_floor;
                    }
                    SourceCleaning::Quiet
                }
                (state, _) => state,
            };
            source.running = state == SourceCleaning::Running;
            source.cleaning.store(state as u8, Ordering::Relaxed);
            for s in &mut source.chunk {
                *s *= source.gain;
            }
            source.level.store(rms_db(&source.chunk));
            for (m, s) in self.mix.iter_mut().zip(source.chunk.iter()) {
                *m += s;
            }
        }
    }
}

/// The gain that an attenuation limit of `db` leaves (100 dB or more: none).
fn floor_gain(db: f32) -> f32 {
    if db >= 100.0 {
        0.0
    } else {
        10_f32.powf(-db.max(0.0) / 20.0)
    }
}

/// Hands an enhancer back to the control side to drop (freeing a model on
/// this thread would stall the audio); dropped here only if the control side
/// has not collected [`RETIRED_LEN`] of them.
fn retire(
    retired: &mut rtrb::Producer<Box<dyn crate::chain::Enhancer>>,
    enhancer: Box<dyn crate::chain::Enhancer>,
) {
    let _ = retired.push(enhancer);
}

impl FrameProcessor for VoiceEngine {
    fn echo_reference(&mut self, played: &Frame) {
        self.played = Some(*played);
    }

    fn process(&mut self, capture: &Frame, playback: &mut Frame) {
        let started = std::time::Instant::now();
        self.apply_commands();

        // Playback: remote mix (and the mic test's loopback), unless deafened.
        self.mix_sources();
        if self.loopback {
            for (m, s) in self.mix.iter_mut().zip(self.last_mic.iter()) {
                *m += s;
            }
        }
        if self.deafened {
            *playback = SILENCE;
        } else {
            for (p, m) in playback.iter_mut().zip(self.mix.iter()) {
                *p = (m * self.master_gain).clamp(-1.0, 1.0);
            }
        }

        // The echo reference: what the output device played (a game's sound
        // included) when the I/O captures it, else exactly what we are
        // about to play.
        match self.played.take() {
            Some(played) => self.chain.render(&played),
            None => self.chain.render(playback),
        }

        self.mic = *capture;
        let result = self.chain.capture(&mut self.mic);
        if let Some(enhancer) = self.chain.take_retired() {
            retire(&mut self.retired, enhancer);
        }
        let transmit = result.transmit && !self.muted;
        if !transmit {
            self.mic = SILENCE;
        }
        self.last_mic = self.mic;
        if self.outgoing.push_entire_slice(&self.mic[..]).is_err() {
            self.shared
                .outgoing_drops
                .fetch_add(FRAME_LEN as u64, Ordering::Relaxed);
        }

        let shared = &self.shared;
        shared.input_rms_db.store(result.rms_db);
        shared.input_peak_db.store(result.peak_db);
        shared.transmitting.store(transmit, Ordering::Relaxed);
        if self.stats_countdown == 0 {
            // About once a second.
            self.stats_countdown = 100;
            let stats = self.chain.stats();
            shared.erle_db.store(stats.erle_db.unwrap_or(f32::NAN));
            shared
                .residual_echo
                .store(stats.residual_echo_likelihood.unwrap_or(f32::NAN));
            shared
                .delay_ms
                .store(stats.delay_ms.unwrap_or(u32::MAX), Ordering::Relaxed);
        }
        self.stats_countdown -= 1;
        shared.frames.fetch_add(1, Ordering::Relaxed);
        shared
            .processing_ns
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::NoiseMode;
    use crate::gate::GateMode;

    fn plain() -> ChainSettings {
        ChainSettings {
            echo_cancellation: false,
            high_pass: false,
            auto_gain: false,
            noise: NoiseMode::Off,
            gate: GateMode::VoiceActivity {
                automatic: false,
                threshold_db: LEVEL_FLOOR_DB,
            },
            input_gain_db: 0.0,
        }
    }

    fn run(engine: &mut VoiceEngine, capture: &Frame) -> Frame {
        let mut out = SILENCE;
        engine.process(capture, &mut out);
        out
    }

    #[test]
    fn sources_are_mixed_with_their_volumes() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (_, mut a) = control.add_source(100).unwrap();
        let (b_id, mut b) = control.add_source(50).unwrap();
        a.push(&[0.2; FRAME_LEN]);
        b.push(&[0.4; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!(
            played.iter().all(|s| (s - 0.4).abs() < 1e-6),
            "{}",
            played[0]
        );
        assert!((b.level_db() - rms_db(&[0.2; FRAME_LEN])).abs() < 0.01);

        control.set_source_volume(b_id, 0);
        control.set_master_volume(200);
        a.push(&[0.2; FRAME_LEN]);
        b.push(&[0.4; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!(
            played.iter().all(|s| (s - 0.4).abs() < 1e-6),
            "{}",
            played[0]
        );

        control.remove_source(b_id);
        a.push(&[0.6; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!(played.iter().all(|s| *s == 1.0), "clamped at full scale");
    }

    /// Scales by `gain`, or fails; counts drops and records its limit.
    struct Scale {
        gain: f32,
        fail: bool,
        limit: Arc<AtomicU32>,
        dropped: Arc<AtomicU64>,
    }

    impl Scale {
        fn new(gain: f32, fail: bool) -> (Box<Self>, Arc<AtomicU32>, Arc<AtomicU64>) {
            let (limit, dropped) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU64::new(0)));
            let scale = Self {
                gain,
                fail,
                limit: limit.clone(),
                dropped: dropped.clone(),
            };
            (Box::new(scale), limit, dropped)
        }
    }

    impl crate::Enhancer for Scale {
        fn enhance(&mut self, frame: &mut Frame) -> Result<(), crate::AudioError> {
            if self.fail {
                return Err(crate::AudioError::Internal);
            }
            frame.iter_mut().for_each(|s| *s *= self.gain);
            Ok(())
        }
        fn latency_samples(&self) -> usize {
            0
        }
        fn set_attenuation_limit(&mut self, db: f32) {
            self.limit.store(db as u32, Ordering::Relaxed);
        }
    }

    impl Drop for Scale {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn a_cleaner_works_on_its_own_voice_before_the_volume() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (a_id, mut a) = control.add_source(200).unwrap();
        let (_, mut b) = control.add_source(100).unwrap();
        let (half, limit, _) = Scale::new(0.5, false);
        control.set_source_cleaner(a_id, Some(half));
        control.set_cleaner_attenuation(30.0);
        a.push(&[0.1; FRAME_LEN]);
        b.push(&[0.2; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        // a: 0.1 halved, then doubled by its volume; b as it came.
        assert!(
            played.iter().all(|s| (s - 0.3).abs() < 1e-6),
            "{}",
            played[0]
        );
        assert_eq!(a.cleaning(), SourceCleaning::Running);
        assert_eq!(b.cleaning(), SourceCleaning::None);
        assert_eq!(limit.load(Ordering::Relaxed), 30);
    }

    #[test]
    fn cleaners_come_back_to_the_control_side_to_be_dropped() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (a_id, mut a) = control.add_source(100).unwrap();
        let (first, _, first_dropped) = Scale::new(0.5, false);
        control.set_source_cleaner(a_id, Some(first));
        run(&mut engine, &SILENCE);

        // Replaced: the old one is handed back, not dropped on this thread.
        let (second, _, second_dropped) = Scale::new(0.5, false);
        control.set_source_cleaner(a_id, Some(second));
        run(&mut engine, &SILENCE);
        assert_eq!(first_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(control.drop_retired(), 1);
        assert_eq!(first_dropped.load(Ordering::Relaxed), 1);

        // Failing: the voice plays as it came, and the cleaner comes back.
        let (failing, _, failing_dropped) = Scale::new(0.5, true);
        control.set_source_cleaner(a_id, Some(failing));
        a.push(&[0.25; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!(played.iter().all(|s| *s == 0.25));
        assert_eq!(a.cleaning(), SourceCleaning::None);
        assert_eq!(
            control.drop_retired(),
            2,
            "the replaced one and the failed one"
        );
        assert_eq!(second_dropped.load(Ordering::Relaxed), 1);
        assert_eq!(failing_dropped.load(Ordering::Relaxed), 1);

        // Removed with its voice.
        let (last, _, last_dropped) = Scale::new(0.5, false);
        control.set_source_cleaner(a_id, Some(last));
        control.remove_source(a_id);
        run(&mut engine, &SILENCE);
        assert_eq!(control.drop_retired(), 1);
        assert_eq!(last_dropped.load(Ordering::Relaxed), 1);
    }

    /// Halves, and counts its calls.
    struct Counting(Arc<AtomicU64>);

    impl crate::Enhancer for Counting {
        fn enhance(&mut self, frame: &mut Frame) -> Result<(), crate::AudioError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            frame.iter_mut().for_each(|s| *s *= 0.5);
            Ok(())
        }
        fn latency_samples(&self) -> usize {
            0
        }
    }

    fn counting() -> (Box<Counting>, Arc<AtomicU64>) {
        let calls = Arc::new(AtomicU64::new(0));
        (Box::new(Counting(calls.clone())), calls)
    }

    #[test]
    fn a_quiet_voice_rests_its_cleaner_until_it_talks_again() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (id, mut a) = control.add_source(100).unwrap();
        let (cleaner, calls) = counting();
        control.set_source_cleaner(id, Some(cleaner));
        for _ in 0..5 {
            a.push(&[0.1; FRAME_LEN]);
            run(&mut engine, &SILENCE);
        }
        assert_eq!(calls.load(Ordering::Relaxed), 5);
        // Quiet (-80 dBFS): the cleaner keeps running for 300 ms, then rests.
        for _ in 0..40 {
            a.push(&[1e-4; FRAME_LEN]);
            run(&mut engine, &SILENCE);
        }
        assert_eq!(calls.load(Ordering::Relaxed), 5 + 29);
        assert_eq!(a.cleaning(), SourceCleaning::Quiet);
        a.push(&[1e-4; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!(
            played.iter().all(|s| *s == 0.0),
            "the floor at a 100 dB limit"
        );
        control.set_cleaner_attenuation(20.0);
        a.push(&[1e-4; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!((played[0] - 1e-5).abs() < 1e-9, "-20 dB: {}", played[0]);
        // Talking again: cleaned from the first loud frame.
        a.push(&[0.1; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert_eq!(calls.load(Ordering::Relaxed), 5 + 29 + 1);
        assert_eq!(a.cleaning(), SourceCleaning::Running);
        assert!((played[0] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn over_the_limit_the_loudest_and_priority_voices_are_cleaned() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        control.set_cleaning_slots(1);
        let (a_id, mut a) = control.add_source(100).unwrap();
        let (b_id, mut b) = control.add_source(100).unwrap();
        let (ca, a_calls) = counting();
        let (cb, b_calls) = counting();
        control.set_source_cleaner(a_id, Some(ca));
        control.set_source_cleaner(b_id, Some(cb));
        let frame = |engine: &mut VoiceEngine, a: &mut SourceInput, b: &mut SourceInput, la, lb| {
            a.push(&[la; FRAME_LEN]);
            b.push(&[lb; FRAME_LEN]);
            run(engine, &SILENCE);
        };
        // b is louder: it gets the one cleaner slot; a plays as it came.
        frame(&mut engine, &mut a, &mut b, 0.1, 0.3);
        assert_eq!(b.cleaning(), SourceCleaning::Running);
        assert_eq!(a.cleaning(), SourceCleaning::OverLimit);
        assert_eq!(a.level_db(), rms_db(&[0.1; FRAME_LEN]), "not cleaned");
        // a gets louder, but b keeps its slot while it talks.
        for _ in 0..20 {
            frame(&mut engine, &mut a, &mut b, 0.5, 0.3);
        }
        assert_eq!(b.cleaning(), SourceCleaning::Running);
        assert_eq!(a_calls.load(Ordering::Relaxed), 0);
        // Priority wins at once.
        control.set_source_priority(a_id, true);
        frame(&mut engine, &mut a, &mut b, 0.1, 0.3);
        assert_eq!(a.cleaning(), SourceCleaning::Running);
        assert_eq!(b.cleaning(), SourceCleaning::OverLimit);
        // b goes quiet: its rest does not count, and a keeps running.
        for _ in 0..40 {
            frame(&mut engine, &mut a, &mut b, 0.1, 0.0);
        }
        assert_eq!(b.cleaning(), SourceCleaning::Quiet);
        assert_eq!(a.cleaning(), SourceCleaning::Running);
        let b_before = b_calls.load(Ordering::Relaxed);
        frame(&mut engine, &mut a, &mut b, 0.1, 0.0);
        assert_eq!(b_calls.load(Ordering::Relaxed), b_before);
    }

    #[test]
    fn missing_audio_is_silence_and_counted() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (_, mut a) = control.add_source(100).unwrap();
        a.push(&[0.5; 100]);
        let played = run(&mut engine, &SILENCE);
        assert_eq!(played[99], 0.5);
        assert_eq!(played[100], 0.0);
        assert_eq!(control.stats().source_underruns, (FRAME_LEN - 100) as u64);
    }

    #[test]
    fn a_source_far_ahead_is_trimmed_to_bound_latency() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (_, mut a) = control.add_source(100).unwrap();
        a.push(&vec![0.1; SOURCE_RING_LEN]);
        run(&mut engine, &SILENCE);
        assert!(a.dropped() >= (SOURCE_RING_LEN - SOURCE_MAX_QUEUED) as u64);
    }

    #[test]
    fn deafen_plays_nothing() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        let (_, mut a) = control.add_source(100).unwrap();
        control.set_deafened(true);
        a.push(&[0.5; FRAME_LEN]);
        assert_eq!(run(&mut engine, &SILENCE), SILENCE);
    }

    #[test]
    fn microphone_goes_out_and_mute_sends_silence() {
        let (mut engine, mut control, mut out) = VoiceEngine::new(plain());
        let mut frame = SILENCE;
        run(&mut engine, &[0.3; FRAME_LEN]);
        assert!(out.pop_frame(&mut frame));
        assert!(out.transmitting());
        assert!(frame[FRAME_LEN - 1] > 0.29);
        control.set_muted(true);
        run(&mut engine, &[0.3; FRAME_LEN]);
        assert!(out.pop_frame(&mut frame));
        assert_eq!(frame, SILENCE);
        assert!(!out.transmitting());
        let meters = control.meters();
        assert!(
            meters.input_rms_db > -12.0,
            "the meter keeps working: {meters:?}"
        );
    }

    #[test]
    fn loopback_plays_the_previous_microphone_frame() {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        control.set_loopback(true);
        run(&mut engine, &[0.25; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        assert!(played[FRAME_LEN - 1] > 0.24, "{}", played[FRAME_LEN - 1]);
    }

    #[test]
    fn the_echo_reference_is_what_is_played() {
        // A perfect echo of the played mix must be cancelled; that only
        // works if render() gets exactly the playback frame (after volume).
        crate::flush_denormals_on_this_thread();
        let (mut engine, mut control, mut out) = VoiceEngine::new(ChainSettings {
            echo_cancellation: true,
            ..plain()
        });
        let (_, mut far) = control.add_source(70).unwrap();
        control.set_master_volume(80);
        let mut seed = 0x2468_ace1_u32;
        let mut previous = SILENCE;
        let (mut echo_energy, mut out_energy) = (0.0_f32, 0.0_f32);
        for i in 0..500 {
            let mut f = SILENCE;
            for s in &mut f {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *s = 0.3 * ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0);
            }
            far.push(&f);
            // The microphone hears the previous frame played, at -6 dB.
            let mic: Frame = std::array::from_fn(|j| previous[j] * 0.5);
            previous = run(&mut engine, &mic);
            let mut sent = SILENCE;
            out.pop_frame(&mut sent);
            if i >= 350 {
                echo_energy += mic.iter().map(|s| s * s).sum::<f32>();
                out_energy += sent.iter().map(|s| s * s).sum::<f32>();
            }
        }
        let erle = 10.0 * (echo_energy / out_energy.max(1e-12)).log10();
        assert!(erle >= 20.0, "ERLE {erle:.1} dB");
        assert!(control.stats().chain.delay_ms.is_some());
    }
}
