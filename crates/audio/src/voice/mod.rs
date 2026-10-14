//! The voice engine: one [`FrameProcessor`] that mixes the remote
//! participants for playback ([`sources`]), feeds exactly what is played to
//! the echo canceller, processes the microphone, and queues the result for
//! sending.
//!
//! Everything here runs on the processing thread. The owner talks to it
//! through [`VoiceControl`] ([`control`]): commands go through a lock-free
//! queue, and levels, the transmit flag and counters come back through
//! atomics.

mod control;
mod sources;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use control::{
    CleaningFlag, Meters, Outgoing, SourceCleaning, SourceInput, VoiceControl, VoiceStats,
};
use control::{Command, Shared};
use sources::{Source, floor_gain};

use crate::chain::{CaptureChain, ChainSettings};
use crate::enhancer::{BatchCleaner, Enhancer};
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
/// Enhancers waiting to be dropped by the control side (the outgoing one,
/// the remote voices' cleaners and banks).
const RETIRED_LEN: usize = 32;
/// Voices a bank cleans at most without the engine allocating.
const BATCH_CAPACITY: usize = 32;
/// Cleaners that run at once by default (voices talking at the same time).
pub const CLEANING_SLOTS: usize = 4;

/// Identifies a remote source (the core maps it to a participant).
pub type SourceId = u64;

/// What the engine hands back to the control side to drop.
enum Retired {
    Enhancer(Box<dyn Enhancer>),
    Bank(Box<dyn BatchCleaner>),
}

impl From<Box<dyn Enhancer>> for Retired {
    fn from(enhancer: Box<dyn Enhancer>) -> Self {
        Self::Enhancer(enhancer)
    }
}

impl From<Box<dyn BatchCleaner>> for Retired {
    fn from(bank: Box<dyn BatchCleaner>) -> Self {
        Self::Bank(bank)
    }
}

/// Hands an enhancer or a bank back to the control side to drop (freeing a
/// model on this thread would stall the audio); dropped here only if the
/// control side has not collected [`RETIRED_LEN`] of them.
fn retire(retired: &mut rtrb::Producer<Retired>, item: impl Into<Retired>) {
    let _ = retired.push(item.into());
}

fn gain_from_percent(percent: u16) -> f32 {
    f32::from(percent) / 100.0
}

/// See the module docs. Create with [`VoiceEngine::new`] and hand it to
/// [`crate::AudioIo::start`].
pub struct VoiceEngine {
    chain: CaptureChain,
    commands: rtrb::Consumer<Command>,
    retired: rtrb::Producer<Retired>,
    shared: Arc<Shared>,
    outgoing: rtrb::Producer<f32>,
    sources: Vec<Source>,
    /// Cleans the voices with a row, all at once.
    bank: Option<Box<dyn BatchCleaner>>,
    /// Scratch for the bank's call: the frames, their rows, their sources.
    batch_frames: Vec<Frame>,
    batch_rows: Vec<usize>,
    batch_sources: Vec<usize>,
    /// Scratch: the talking voices with a cleaner, this frame.
    talking: Vec<usize>,
    /// Cleaners that may run at once.
    cleaning_slots: usize,
    /// What a resting cleaner leaves of a quiet voice.
    cleaner_floor: f32,
    /// The one-shot cue playing (see [`VoiceControl::play_cue`]), the next
    /// sample to play, and its gain. `'static`: nothing is freed here.
    cue: &'static [f32],
    cue_pos: usize,
    cue_gain: f32,
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
    /// The frame handed to the echo canceller last (tests only).
    #[cfg(test)]
    rendered: Frame,
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
        let shared = Arc::new(Shared::new(transmitting.clone()));
        let engine = Self {
            chain: CaptureChain::new(settings),
            commands: commands_rx,
            retired: retired_tx,
            shared: shared.clone(),
            outgoing: out_tx,
            sources: Vec::with_capacity(16),
            bank: None,
            batch_frames: Vec::with_capacity(BATCH_CAPACITY),
            batch_rows: Vec::with_capacity(BATCH_CAPACITY),
            batch_sources: Vec::with_capacity(BATCH_CAPACITY),
            talking: Vec::with_capacity(16),
            cleaning_slots: CLEANING_SLOTS,
            cleaner_floor: floor_gain(100.0),
            cue: &[],
            cue_pos: 0,
            cue_gain: 0.0,
            muted: false,
            deafened: false,
            loopback: false,
            master_gain: 1.0,
            mix: SILENCE,
            mic: SILENCE,
            last_mic: SILENCE,
            played: None,
            #[cfg(test)]
            rendered: SILENCE,
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

    fn source_mut(&mut self, id: SourceId) -> Option<&mut Source> {
        self.sources.iter_mut().find(|s| s.id == id)
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
                    if let Some(s) = self.source_mut(id) {
                        s.gain = gain_from_percent(p);
                    }
                }
                Command::SourceCleaner(id, cleaner) => self.set_source_cleaner(id, cleaner),
                Command::CleanerBank(bank) => {
                    for s in &mut self.sources {
                        if s.row.take().is_some() && s.cleaner.is_none() {
                            s.set_cleaning(SourceCleaning::None);
                        }
                    }
                    if let Some(old) = std::mem::replace(&mut self.bank, bank) {
                        retire(&mut self.retired, old);
                    }
                }
                Command::SourceRow(id, row) => {
                    if let Some(s) = self.source_mut(id) {
                        s.row = row;
                        if row.is_none() && s.cleaner.is_none() {
                            s.set_cleaning(SourceCleaning::None);
                        }
                    }
                    let fresh = match (row, self.bank.as_mut()) {
                        (Some(row), Some(bank)) => bank.reset_row(row).is_ok(),
                        _ => true,
                    };
                    if !fresh {
                        self.give_up_bank();
                    }
                }
                Command::SourcePriority(id, priority) => {
                    if let Some(s) = self.source_mut(id) {
                        s.priority = priority;
                    }
                }
                Command::CleanerAttenuation(db) => {
                    self.cleaner_floor = floor_gain(db);
                    for cleaner in self.sources.iter_mut().filter_map(|s| s.cleaner.as_mut()) {
                        cleaner.set_attenuation_limit(db);
                    }
                    if let Some(bank) = self.bank.as_mut() {
                        bank.set_attenuation_limit(db);
                    }
                }
                Command::CleaningSlots(slots) => self.cleaning_slots = slots,
                Command::Cue(samples, p) => {
                    self.cue = samples;
                    self.cue_pos = 0;
                    // Never above 100 %, whatever the caller passes.
                    self.cue_gain = gain_from_percent(p.min(100));
                }
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

    fn set_source_cleaner(&mut self, id: SourceId, cleaner: Option<Box<dyn Enhancer>>) {
        let Some(i) = self.sources.iter().position(|s| s.id == id) else {
            // The voice left meanwhile.
            if let Some(cleaner) = cleaner {
                retire(&mut self.retired, cleaner);
            }
            return;
        };
        let source = &mut self.sources[i];
        if cleaner.is_none() {
            source.set_cleaning(SourceCleaning::None);
        }
        source.running = false;
        if let Some(old) = std::mem::replace(&mut source.cleaner, cleaner) {
            retire(&mut self.retired, old);
        }
    }

    /// The next 10 ms of the cue, scaled by its gain (zeros when none plays).
    /// No allocation: it reads a `'static` slice into a stack frame.
    fn next_cue_frame(&mut self) -> Frame {
        let mut frame = SILENCE;
        let rest = self.cue.get(self.cue_pos..).unwrap_or(&[]);
        let n = rest.len().min(FRAME_LEN);
        for (f, s) in frame.iter_mut().zip(&rest[..n]) {
            *f = s * self.cue_gain;
        }
        self.cue_pos += n;
        if n < FRAME_LEN {
            self.cue = &[];
            self.cue_pos = 0;
        }
        frame
    }

    /// Publishes the meters and, about once a second, the echo canceller's
    /// statistics; counts the frame and its processing time.
    fn report(&mut self, levels: (f32, f32), transmit: bool, started: std::time::Instant) {
        let shared = &self.shared;
        shared.input_rms_db.store(levels.0);
        shared.input_peak_db.store(levels.1);
        shared.transmitting.store(transmit, Ordering::Relaxed);
        if self.stats_countdown == 0 {
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
        // The cue has its own frame so that deafen can silence the voices
        // and still let it through. Playback is what the echo reference is
        // taken from, so the cue is in the reference either way.
        let cue = self.next_cue_frame();
        // Deafened is exactly the cue: the voices are not multiplied by
        // zero, so a NaN or inf in the mix cannot get through. `clamp`
        // passes NaN, so a non-finite sample is zeroed.
        let deafened = self.deafened;
        for ((p, m), c) in playback.iter_mut().zip(self.mix.iter()).zip(cue.iter()) {
            let v = if deafened { *c } else { m + c };
            let out = (v * self.master_gain).clamp(-1.0, 1.0);
            *p = if out.is_finite() { out } else { 0.0 };
        }

        // The echo reference: what the output device played (a game's sound
        // included) when the I/O captures it, else exactly what we are
        // about to play.
        let reference = self.played.take();
        let reference = reference.as_ref().unwrap_or(playback);
        #[cfg(test)]
        {
            self.rendered = *reference;
        }
        self.chain.render(reference);

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
        self.report((result.rms_db, result.peak_db), transmit, started);
    }
}
