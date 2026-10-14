//! The owner's side of a running [`VoiceEngine`](super::VoiceEngine):
//! commands go to the processing thread through a lock-free queue; levels,
//! the transmit flag and counters come back through atomics; enhancers the
//! engine let go of come back to be dropped here.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};

use super::sources::Source;
use super::{Retired, SOURCE_RING_LEN, SourceId, gain_from_percent};
use crate::chain::{ChainSettings, ChainStats, LEVEL_FLOOR_DB};
use crate::enhancer::{BatchCleaner, Enhancer};
use crate::frame::Frame;

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
pub(super) struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub(super) fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub(super) fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub(super) fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
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
        self.cleaning_flag().get()
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
    pub(super) ring: rtrb::Consumer<f32>,
    pub(super) transmitting: Arc<AtomicBool>,
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

/// What the owner asks of the processing thread.
pub(super) enum Command {
    Configure(ChainSettings),
    PushToTalk(bool),
    Muted(bool),
    Deafened(bool),
    Loopback(bool),
    MasterVolume(u16),
    AddSource(Box<Source>),
    Enhancer(Option<Box<dyn Enhancer>>),
    EnhancerAttenuation(f32),
    SourceVolume(SourceId, u16),
    SourceCleaner(SourceId, Option<Box<dyn Enhancer>>),
    CleanerBank(Option<Box<dyn BatchCleaner>>),
    SourceRow(SourceId, Option<usize>),
    SourcePriority(SourceId, bool),
    CleanerAttenuation(f32),
    CleaningSlots(usize),
    RemoveSource(SourceId),
    Cue(&'static [f32], u16),
}

/// Levels and counters shared with [`VoiceControl`].
#[derive(Debug)]
pub(super) struct Shared {
    pub(super) input_rms_db: AtomicF32,
    pub(super) input_peak_db: AtomicF32,
    pub(super) transmitting: Arc<AtomicBool>,
    pub(super) erle_db: AtomicF32,
    pub(super) residual_echo: AtomicF32,
    pub(super) delay_ms: AtomicU32,
    pub(super) processing_ns: AtomicU64,
    pub(super) frames: AtomicU64,
    pub(super) source_underruns: AtomicU64,
    pub(super) outgoing_drops: AtomicU64,
}

impl Shared {
    pub(super) fn new(transmitting: Arc<AtomicBool>) -> Self {
        Self {
            input_rms_db: AtomicF32::new(LEVEL_FLOOR_DB),
            input_peak_db: AtomicF32::new(LEVEL_FLOOR_DB),
            transmitting,
            erle_db: AtomicF32::new(f32::NAN),
            residual_echo: AtomicF32::new(f32::NAN),
            delay_ms: AtomicU32::new(u32::MAX),
            processing_ns: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            source_underruns: AtomicU64::new(0),
            outgoing_drops: AtomicU64::new(0),
        }
    }
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

/// The owner's handle on a running [`VoiceEngine`](super::VoiceEngine).
pub struct VoiceControl {
    pub(super) commands: rtrb::Producer<Command>,
    /// Enhancers and banks the engine let go of, dropped here.
    pub(super) retired: rtrb::Consumer<Retired>,
    pub(super) shared: Arc<Shared>,
    pub(super) next_source: SourceId,
    pub(super) last_frames: u64,
    pub(super) last_ns: u64,
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
    pub fn set_enhancer(&mut self, enhancer: Option<Box<dyn Enhancer>>) -> bool {
        self.drop_retired();
        self.send(Command::Enhancer(enhancer))
    }

    /// Sets the plugged enhancer's attenuation limit, dB, without reloading
    /// it.
    pub fn set_enhancer_attenuation(&mut self, db: f32) -> bool {
        self.send(Command::EnhancerAttenuation(db))
    }

    /// Drops the enhancers and banks the engine replaced or that failed
    /// (also done by [`set_enhancer`](Self::set_enhancer) and
    /// [`stats`](Self::stats)). Returns how many.
    pub fn drop_retired(&mut self) -> usize {
        let mut n = 0;
        while let Ok(retired) = self.retired.pop() {
            match retired {
                Retired::Enhancer(enhancer) => drop(enhancer),
                Retired::Bank(bank) => drop(bank),
            }
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
        let cleaning = Arc::new(AtomicU8::new(SourceCleaning::None as u8));
        let source = Source::new(
            id,
            consumer,
            gain_from_percent(volume_percent),
            (level.clone(), dropped.clone(), cleaning.clone()),
        );
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
    pub fn set_source_cleaner(&mut self, id: SourceId, cleaner: Option<Box<dyn Enhancer>>) -> bool {
        self.drop_retired();
        self.send(Command::SourceCleaner(id, cleaner))
    }

    /// Plugs a bank (made off this thread) that cleans the voices given a
    /// row ([`set_source_row`](Self::set_source_row)) all in one call per
    /// frame, or removes it. Either way every voice loses its row; a
    /// replaced or failing bank comes back here to be dropped, and its
    /// voices play as they came.
    pub fn set_cleaner_bank(&mut self, bank: Option<Box<dyn BatchCleaner>>) -> bool {
        self.drop_retired();
        self.send(Command::CleanerBank(bank))
    }

    /// Cleans one remote voice in a row of the bank (started afresh), or
    /// takes its row away. The bank's voices run every frame, quiet or not,
    /// and do not count against the cleaning slots.
    pub fn set_source_row(&mut self, id: SourceId, row: Option<usize>) -> bool {
        self.send(Command::SourceRow(id, row))
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
    /// default [`CLEANING_SLOTS`](super::CLEANING_SLOTS)). Quiet voices' cleaners rest and do not
    /// count.
    pub fn set_cleaning_slots(&mut self, slots: usize) -> bool {
        self.send(Command::CleaningSlots(slots))
    }

    /// Plays `samples` (48 kHz mono) once, at `volume_percent`, in the
    /// call's playback: added to the remote voices, scaled by the master
    /// volume, and so part of the echo canceller's reference. It is never
    /// part of [`Outgoing`], and it plays muted and deafened too. A cue
    /// already playing is replaced. Returns false if the queue is full.
    ///
    /// The samples are `'static` so the processing thread never frees them.
    pub fn play_cue(&mut self, samples: &'static [f32], volume_percent: u16) -> bool {
        self.send(Command::Cue(samples, volume_percent))
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
