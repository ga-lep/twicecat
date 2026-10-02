//! The voice engine: one [`FrameProcessor`] that mixes the remote
//! participants for playback, feeds exactly what is played to the echo
//! canceller, processes the microphone, and queues the result for sending.
//!
//! Everything here runs on the processing thread. The owner talks to it
//! through [`VoiceControl`]: commands go through a lock-free queue, and
//! levels, the transmit flag and counters come back through atomics.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

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

/// Identifies a remote source (the core maps it to a participant).
pub type SourceId = u64;

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
    SourceVolume(SourceId, u16),
    RemoveSource(SourceId),
}

struct Source {
    id: SourceId,
    ring: rtrb::Consumer<f32>,
    gain: f32,
    level: Arc<AtomicF32>,
    dropped: Arc<AtomicU64>,
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
        let source = Source {
            id,
            ring: consumer,
            gain: gain_from_percent(volume_percent),
            level: level.clone(),
            dropped: dropped.clone(),
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
            },
        ))
    }

    /// Changes one source's volume, percent.
    pub fn set_source_volume(&mut self, id: SourceId, percent: u16) -> bool {
        self.send(Command::SourceVolume(id, percent))
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
    shared: Arc<Shared>,
    outgoing: rtrb::Producer<f32>,
    sources: Vec<Source>,
    muted: bool,
    deafened: bool,
    loopback: bool,
    master_gain: f32,
    mix: Frame,
    mic: Frame,
    last_mic: Frame,
    /// Frames between stats refreshes (sonora's stats are not free).
    stats_countdown: u32,
}

impl VoiceEngine {
    /// A voice engine, its control handle, and the outgoing microphone
    /// stream.
    pub fn new(settings: ChainSettings) -> (Self, VoiceControl, Outgoing) {
        let (commands_tx, commands_rx) = rtrb::RingBuffer::new(CONTROL_QUEUE_LEN);
        let (out_tx, out_rx) = rtrb::RingBuffer::new(OUTGOING_RING_LEN);
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
            shared: shared.clone(),
            outgoing: out_tx,
            sources: Vec::with_capacity(16),
            muted: false,
            deafened: false,
            loopback: false,
            master_gain: 1.0,
            mix: SILENCE,
            mic: SILENCE,
            last_mic: SILENCE,
            stats_countdown: 0,
        };
        let control = VoiceControl {
            commands: commands_tx,
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
                Command::RemoveSource(id) => self.sources.retain(|s| s.id != id),
            }
        }
    }

    fn mix_sources(&mut self) {
        self.mix = SILENCE;
        let mut chunk = SILENCE;
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
            let (filled, missing) = source.ring.pop_partial_slice(&mut chunk[..]);
            let got = filled.len();
            missing.fill(0.0);
            if got < FRAME_LEN {
                self.shared
                    .source_underruns
                    .fetch_add((FRAME_LEN - got) as u64, Ordering::Relaxed);
            }
            for s in &mut chunk {
                *s *= source.gain;
            }
            source.level.store(rms_db(&chunk));
            for (m, s) in self.mix.iter_mut().zip(chunk.iter()) {
                *m += s;
            }
        }
    }
}

impl FrameProcessor for VoiceEngine {
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

        // The echo reference is exactly what is about to be played.
        self.chain.render(playback);

        self.mic = *capture;
        let result = self.chain.capture(&mut self.mic);
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
