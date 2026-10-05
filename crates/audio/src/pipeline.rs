//! The processing-thread side of audio I/O, independent of cpal so tests can
//! drive it with plain rings.
//!
//! Device callbacks only move samples between the device buffers and two
//! lock-free SPSC rings (mono, at the device rate). This thread pulls capture
//! samples, resamples them to 48 kHz frames, runs the [`FrameProcessor`] once
//! per 10 ms frame, and pushes the playback frame back towards the device.
//! The capture clock paces processing; without a microphone a 10 ms timer
//! does.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;
use std::time::Instant;

use crate::AudioError;
use crate::frame::{FRAME_LEN, Frame, SILENCE};
use crate::resample::{FromInternal, ToInternal};

/// Work done every 10 ms on the processing thread. Implementations must not
/// block; allocation should be avoided in the steady state.
pub trait FrameProcessor: Send + 'static {
    /// `capture` is the microphone frame (silence without a microphone);
    /// `playback` starts silent and is what the speakers will play.
    fn process(&mut self, capture: &Frame, playback: &mut Frame);

    /// What the output device played in this frame, all applications
    /// included, when an echo reference stream runs
    /// ([`crate::IoConfig::echo_reference`]); called right before
    /// [`process`](Self::process). Silence when the stream had nothing.
    fn echo_reference(&mut self, _played: &Frame) {}
}

impl<F> FrameProcessor for F
where
    F: FnMut(&Frame, &mut Frame) + Send + 'static,
{
    fn process(&mut self, capture: &Frame, playback: &mut Frame) {
        self(capture, playback)
    }
}

/// Counters shared between device callbacks, the processing thread and the
/// owner. All relaxed: they are diagnostics.
#[derive(Debug, Default)]
pub struct IoCounters {
    /// 10 ms frames processed.
    pub frames: AtomicU64,
    /// Capture samples dropped because the capture ring was full.
    pub capture_overruns: AtomicU64,
    /// Playback samples the device wanted but the ring did not have.
    pub playback_underruns: AtomicU64,
    /// Playback frames dropped: ring full, or ahead of the device clock.
    pub playback_drops: AtomicU64,
    /// Times the processing thread woke to captured audio and ran frames.
    pub wakes: AtomicU64,
    /// Of those, the ones that started more than 1 ms after the audio came.
    pub late_wakes: AtomicU64,
    /// The longest time from captured audio to processing it, nanoseconds.
    pub max_wake_ns: AtomicU64,
    /// The same since the owner last read it
    /// ([`take_recent_max_wake_us`](Self::take_recent_max_wake_us)).
    recent_max_wake_ns: AtomicU64,
    /// When the capture callback completed the oldest frame not yet
    /// processed ([`now_ns`]; 0: none waiting).
    captured_at_ns: AtomicU64,
    /// Capture samples (at the device rate) the next frame needs, set by
    /// the processing thread: the callback stamps the time a frame is
    /// complete, not when part of one comes (device quanta are often not
    /// 10 ms: 256 samples in the test daemon).
    frame_need: AtomicUsize,
    /// The processing thread, woken by the capture callback.
    processing: OnceLock<Thread>,
}

/// Nanoseconds on a clock shared by the callbacks and the processing thread.
fn now_ns() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let nanos = EPOCH.get_or_init(Instant::now).elapsed().as_nanos();
    u64::try_from(nanos).unwrap_or(u64::MAX)
}

/// A wake that starts this long after the audio came counts as late.
const LATE_WAKE_NS: u64 = 1_000_000;

/// A copy of [`IoCounters`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IoStats {
    /// See [`IoCounters::frames`].
    pub frames: u64,
    /// See [`IoCounters::capture_overruns`].
    pub capture_overruns: u64,
    /// See [`IoCounters::playback_underruns`].
    pub playback_underruns: u64,
    /// See [`IoCounters::playback_drops`].
    pub playback_drops: u64,
    /// See [`IoCounters::wakes`].
    pub wakes: u64,
    /// See [`IoCounters::late_wakes`].
    pub late_wakes: u64,
    /// See [`IoCounters::max_wake_ns`], in microseconds.
    pub max_wake_us: u64,
}

impl IoCounters {
    /// Reads every counter.
    pub fn snapshot(&self) -> IoStats {
        IoStats {
            frames: self.frames.load(Ordering::Relaxed),
            capture_overruns: self.capture_overruns.load(Ordering::Relaxed),
            playback_underruns: self.playback_underruns.load(Ordering::Relaxed),
            playback_drops: self.playback_drops.load(Ordering::Relaxed),
            wakes: self.wakes.load(Ordering::Relaxed),
            late_wakes: self.late_wakes.load(Ordering::Relaxed),
            max_wake_us: self.max_wake_ns.load(Ordering::Relaxed) / 1000,
        }
    }

    /// The longest time from captured audio to processing it since the
    /// previous call, microseconds (0: no wake since).
    pub fn take_recent_max_wake_us(&self) -> u64 {
        self.recent_max_wake_ns.swap(0, Ordering::Relaxed) / 1000
    }

    /// Called by the capture callback after it delivered audio, with the
    /// ring's fill: notes the time when that completes a frame (unless an
    /// older one is still waiting) and wakes the processing thread.
    /// Real-time safe: atomics and, if the thread sleeps, one futex wake (no
    /// allocation, no lock).
    pub(crate) fn captured(&self, filled: usize) {
        if filled < self.frame_need.load(Ordering::Relaxed) {
            return;
        }
        let _ = self.captured_at_ns.compare_exchange(
            0,
            now_ns().max(1),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        if let Some(thread) = self.processing.get() {
            thread.unpark();
        }
    }

    /// Wakes the processing thread (to see a stop).
    pub(crate) fn wake(&self) {
        if let Some(thread) = self.processing.get() {
            thread.unpark();
        }
    }

    /// Makes the calling thread the one [`captured`](Self::captured) wakes.
    pub(crate) fn set_processing_thread(&self) {
        let _ = self.processing.set(std::thread::current());
    }

    /// The processing thread woke at `woke_ns` and ran every frame waiting:
    /// how long after the oldest of that audio came. (Audio delivered while
    /// it ran may lose its stamp here: that wake then goes uncounted, never
    /// counted wrong.)
    fn note_wake(&self, woke_ns: u64) {
        let at = self.captured_at_ns.swap(0, Ordering::Relaxed);
        if at == 0 {
            return;
        }
        let late = woke_ns.saturating_sub(at);
        self.wakes.fetch_add(1, Ordering::Relaxed);
        if late > LATE_WAKE_NS {
            self.late_wakes.fetch_add(1, Ordering::Relaxed);
        }
        self.max_wake_ns.fetch_max(late, Ordering::Relaxed);
        self.recent_max_wake_ns.fetch_max(late, Ordering::Relaxed);
    }
}

/// Capture ring capacity: 200 ms at the device rate.
pub(crate) fn capture_ring_len(device_rate: u32) -> usize {
    device_rate as usize / 5
}

/// Playback ring capacity: 200 ms at the device rate.
pub(crate) fn playback_ring_len(device_rate: u32) -> usize {
    device_rate as usize / 5
}

/// Above this fill (60 ms) the playback side is ahead of the device clock
/// (the capture and playback devices drift apart), so a frame is dropped to
/// keep latency bounded.
fn playback_high_water(device_rate: u32) -> usize {
    device_rate as usize * 6 / 100
}

pub(crate) struct CaptureSide {
    pub ring: rtrb::Consumer<f32>,
    pub converter: ToInternal,
}

pub(crate) struct PlaybackSide {
    pub ring: rtrb::Producer<f32>,
    pub converter: FromInternal,
    pub high_water: usize,
}

impl CaptureSide {
    pub(crate) fn new(ring: rtrb::Consumer<f32>, device_rate: u32) -> Result<Self, AudioError> {
        Ok(Self {
            ring,
            converter: ToInternal::new(device_rate)?,
        })
    }
}

impl PlaybackSide {
    pub(crate) fn new(ring: rtrb::Producer<f32>, device_rate: u32) -> Result<Self, AudioError> {
        Ok(Self {
            ring,
            converter: FromInternal::new(device_rate)?,
            high_water: playback_high_water(device_rate),
        })
    }
}

/// The most echo reference left queued after a frame, in frames (30 ms):
/// a loopback stream's bursts fit, drift beyond it is dropped.
const REFERENCE_BACKLOG_FRAMES: usize = 3;

pub(crate) struct Pipeline<P> {
    capture: Option<CaptureSide>,
    reference: Option<CaptureSide>,
    frame_ref: Frame,
    playback: Option<PlaybackSide>,
    processor: P,
    scratch: Vec<f32>,
    frame_in: Frame,
    frame_out: Frame,
    counters: Arc<IoCounters>,
}

impl<P: FrameProcessor> Pipeline<P> {
    pub(crate) fn new(
        capture: Option<CaptureSide>,
        playback: Option<PlaybackSide>,
        processor: P,
        counters: Arc<IoCounters>,
    ) -> Self {
        let scratch_len = capture
            .as_ref()
            .map_or(FRAME_LEN, |c| c.converter.input_frames_max());
        Self {
            capture,
            reference: None,
            frame_ref: SILENCE,
            playback,
            processor,
            scratch: vec![0.0; scratch_len],
            frame_in: SILENCE,
            frame_out: SILENCE,
            counters,
        }
    }

    /// Adds the echo reference stream (what the output device plays): our
    /// PipeWire client's sink monitor, or WASAPI's loopback on Windows.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub(crate) fn set_echo_reference(&mut self, reference: Option<CaptureSide>) {
        if let Some(r) = &reference {
            let need = r.converter.input_frames_max();
            if need > self.scratch.len() {
                self.scratch.resize(need, 0.0);
            }
        }
        self.reference = reference;
    }

    pub(crate) fn has_capture(&self) -> bool {
        self.capture.is_some()
    }

    /// Runs one frame for every 10 ms of captured audio available; returns
    /// how many ran. Without a capture side it runs nothing (see
    /// [`tick`](Self::tick)).
    pub(crate) fn pump(&mut self) -> Result<usize, AudioError> {
        let woke = now_ns();
        let mut ran = 0;
        loop {
            let Some(capture) = self.capture.as_mut() else {
                return Ok(ran);
            };
            let need = capture.converter.input_frames_next();
            if capture.ring.slots() < need {
                self.counters.frame_need.store(need, Ordering::Relaxed);
                if ran > 0 {
                    self.counters.note_wake(woke);
                }
                return Ok(ran);
            }
            let input = &mut self.scratch[..need];
            capture
                .ring
                .pop_entire_slice(input)
                .map_err(|_| AudioError::Internal)?;
            capture.converter.process(input, &mut self.frame_in)?;
            self.pull_reference()?;
            self.run_frame()?;
            ran += 1;
        }
    }

    /// Hands the processor one frame of the echo reference, silence when the
    /// stream has none yet. On PipeWire it runs on the same graph clock as
    /// capture; WASAPI's loopback runs on the output device's clock, which
    /// drifts from the microphone's, so a backlog beyond
    /// [`REFERENCE_BACKLOG_FRAMES`] is dropped: the reference stays within
    /// echo cancellation's delay range instead of falling ever further
    /// behind.
    fn pull_reference(&mut self) -> Result<(), AudioError> {
        let Some(reference) = self.reference.as_mut() else {
            return Ok(());
        };
        let need = reference.converter.input_frames_next();
        if reference.ring.slots() >= need {
            let input = &mut self.scratch[..need];
            reference
                .ring
                .pop_entire_slice(input)
                .map_err(|_| AudioError::Internal)?;
            reference.converter.process(input, &mut self.frame_ref)?;
            let backlog = reference.ring.slots();
            if backlog > need * REFERENCE_BACKLOG_FRAMES {
                // Keep one frame's worth: the next frame's.
                if let Ok(chunk) = reference.ring.read_chunk(backlog - need) {
                    chunk.commit_all();
                }
            }
        } else {
            self.frame_ref = SILENCE;
        }
        self.processor.echo_reference(&self.frame_ref);
        Ok(())
    }

    /// Runs one frame of silence through the processor (no microphone; the
    /// caller paces this every 10 ms), with the echo reference if one runs.
    pub(crate) fn tick(&mut self) -> Result<(), AudioError> {
        self.frame_in = SILENCE;
        self.pull_reference()?;
        self.run_frame()
    }

    fn run_frame(&mut self) -> Result<(), AudioError> {
        self.frame_out = SILENCE;
        self.processor.process(&self.frame_in, &mut self.frame_out);
        self.counters.frames.fetch_add(1, Ordering::Relaxed);
        let Some(playback) = self.playback.as_mut() else {
            return Ok(());
        };
        let samples = playback.converter.process(&self.frame_out)?;
        let queued = playback.ring.buffer().capacity() - playback.ring.slots();
        if queued > playback.high_water || playback.ring.push_entire_slice(samples).is_err() {
            self.counters.playback_drops.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::SAMPLE_RATE;
    use crate::resample::tests::{sine, tone_amplitude};

    fn loopback() -> impl FrameProcessor {
        |capture: &Frame, playback: &mut Frame| playback.copy_from_slice(capture)
    }

    fn rings(
        rate: u32,
    ) -> (
        rtrb::Producer<f32>,
        CaptureSide,
        PlaybackSide,
        rtrb::Consumer<f32>,
    ) {
        let (cap_tx, cap_rx) = rtrb::RingBuffer::new(capture_ring_len(rate));
        let (play_tx, play_rx) = rtrb::RingBuffer::new(playback_ring_len(rate));
        (
            cap_tx,
            CaptureSide::new(cap_rx, rate).unwrap(),
            PlaybackSide::new(play_tx, rate).unwrap(),
            play_rx,
        )
    }

    #[test]
    fn loopback_at_48k_is_bit_exact_and_paced_by_capture() {
        let counters = Arc::new(IoCounters::default());
        let (mut mic, capture, playback, mut speaker) = rings(SAMPLE_RATE);
        let mut pipeline =
            Pipeline::new(Some(capture), Some(playback), loopback(), counters.clone());
        let input = sine(SAMPLE_RATE, 440.0, 0.3, FRAME_LEN * 3 + 100);
        mic.push_entire_slice(&input).unwrap();
        assert_eq!(pipeline.pump().unwrap(), 3, "three whole frames");
        assert_eq!(pipeline.pump().unwrap(), 0, "the partial frame waits");
        let mut out = vec![0.0; FRAME_LEN * 3];
        speaker.pop_entire_slice(&mut out).unwrap();
        assert_eq!(out, input[..FRAME_LEN * 3]);
        assert_eq!(counters.snapshot().frames, 3);
    }

    #[test]
    fn loopback_through_a_44100_device_keeps_the_tone() {
        let counters = Arc::new(IoCounters::default());
        let (mut mic, capture, playback, mut speaker) = rings(44_100);
        let mut pipeline =
            Pipeline::new(Some(capture), Some(playback), loopback(), counters.clone());
        let input = sine(44_100, 1000.0, 0.5, 44_100);
        let mut heard = Vec::new();
        for chunk in input.chunks(441) {
            mic.push_entire_slice(chunk).unwrap();
            pipeline.pump().unwrap();
            // The device drains the ring like a real callback would.
            while let Ok(s) = speaker.pop() {
                heard.push(s);
            }
        }
        assert!(heard.len() > 40_000, "{}", heard.len());
        let a = tone_amplitude(&heard[8_820..], 44_100, 1000.0);
        assert!((a - 0.5).abs() < 0.03, "amplitude {a}");
        assert_eq!(counters.snapshot().playback_drops, 0);
    }

    #[test]
    fn playback_ahead_of_the_device_is_dropped_not_queued() {
        let counters = Arc::new(IoCounters::default());
        let (mut mic, capture, playback, speaker) = rings(SAMPLE_RATE);
        let mut pipeline =
            Pipeline::new(Some(capture), Some(playback), loopback(), counters.clone());
        // 150 ms captured while the speaker never drains.
        for _ in 0..15 {
            mic.push_entire_slice(&SILENCE).unwrap();
            pipeline.pump().unwrap();
        }
        let queued = speaker.slots();
        assert!(
            queued <= playback_high_water(SAMPLE_RATE) + FRAME_LEN,
            "{queued}"
        );
        assert!(counters.snapshot().playback_drops > 0);
    }

    #[test]
    fn the_recent_worst_wake_restarts_when_read() {
        let counters = IoCounters::default();
        let wake = |late_ns: u64| {
            counters.captured_at_ns.store(1_000, Ordering::Relaxed);
            counters.note_wake(1_000 + late_ns);
        };
        wake(3_000_000);
        wake(200_000);
        assert_eq!(counters.take_recent_max_wake_us(), 3_000);
        assert_eq!(counters.take_recent_max_wake_us(), 0, "nothing since");
        wake(400_000);
        assert_eq!(counters.take_recent_max_wake_us(), 400);
        let stats = counters.snapshot();
        assert_eq!(
            (stats.wakes, stats.late_wakes, stats.max_wake_us),
            (3, 1, 3_000)
        );
    }

    /// Records the echo reference frames it is given.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    struct References(Vec<f32>);

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    impl FrameProcessor for References {
        fn process(&mut self, _: &Frame, _: &mut Frame) {}
        fn echo_reference(&mut self, played: &Frame) {
            self.0.extend_from_slice(played);
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn an_echo_reference_ahead_of_capture_is_trimmed_not_queued() {
        let counters = Arc::new(IoCounters::default());
        let (mut mic, capture, _, _) = rings(SAMPLE_RATE);
        let (mut loopback, reference_rx) = rtrb::RingBuffer::new(capture_ring_len(SAMPLE_RATE));
        let mut pipeline = Pipeline::new(Some(capture), None, References(Vec::new()), counters);
        pipeline.set_echo_reference(Some(CaptureSide::new(reference_rx, SAMPLE_RATE).unwrap()));
        // The output device's clock runs ahead: 8 frames of reference
        // (numbered by their value) before one frame of microphone.
        for n in 0..8 {
            loopback.push_entire_slice(&[n as f32; FRAME_LEN]).unwrap();
        }
        mic.push_entire_slice(&SILENCE).unwrap();
        assert_eq!(pipeline.pump().unwrap(), 1);
        // Within the bound: frames 1 to 3 would wait. Beyond it, all but the
        // newest frame are dropped.
        let left = pipeline.reference.as_ref().unwrap().ring.slots();
        assert_eq!(left, FRAME_LEN, "one frame kept");
        mic.push_entire_slice(&SILENCE).unwrap();
        mic.push_entire_slice(&SILENCE).unwrap();
        assert_eq!(pipeline.pump().unwrap(), 2);
        let seen = &pipeline.processor.0;
        assert_eq!(seen.len(), FRAME_LEN * 3);
        assert!(seen[..FRAME_LEN].iter().all(|s| *s == 0.0), "frame 0 first");
        assert!(
            seen[FRAME_LEN..FRAME_LEN * 2].iter().all(|s| *s == 7.0),
            "then the newest, frame 7"
        );
        assert!(
            seen[FRAME_LEN * 2..].iter().all(|s| *s == 0.0),
            "then silence: nothing more came"
        );
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn an_echo_reference_within_the_bound_is_kept() {
        let counters = Arc::new(IoCounters::default());
        let (mut mic, capture, _, _) = rings(SAMPLE_RATE);
        let (mut loopback, reference_rx) = rtrb::RingBuffer::new(capture_ring_len(SAMPLE_RATE));
        let mut pipeline = Pipeline::new(Some(capture), None, References(Vec::new()), counters);
        pipeline.set_echo_reference(Some(CaptureSide::new(reference_rx, SAMPLE_RATE).unwrap()));
        // A burst of four frames (one used now, three queued: the bound).
        for n in 0..4 {
            loopback.push_entire_slice(&[n as f32; FRAME_LEN]).unwrap();
        }
        for _ in 0..4 {
            mic.push_entire_slice(&SILENCE).unwrap();
        }
        assert_eq!(pipeline.pump().unwrap(), 4);
        let seen = &pipeline.processor.0;
        for n in 0..4 {
            let frame = &seen[n * FRAME_LEN..(n + 1) * FRAME_LEN];
            assert!(frame.iter().all(|s| *s == n as f32), "frame {n} in order");
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn without_a_microphone_tick_hands_over_the_echo_reference() {
        let counters = Arc::new(IoCounters::default());
        let (mut loopback, reference_rx) = rtrb::RingBuffer::new(capture_ring_len(SAMPLE_RATE));
        let mut pipeline = Pipeline::new(None, None, References(Vec::new()), counters);
        pipeline.set_echo_reference(Some(CaptureSide::new(reference_rx, SAMPLE_RATE).unwrap()));
        loopback.push_entire_slice(&[0.5; FRAME_LEN]).unwrap();
        pipeline.tick().unwrap();
        pipeline.tick().unwrap();
        let seen = &pipeline.processor.0;
        assert_eq!(seen.len(), FRAME_LEN * 2);
        assert!(seen[..FRAME_LEN].iter().all(|s| *s == 0.5));
        assert!(seen[FRAME_LEN..].iter().all(|s| *s == 0.0), "then silence");
    }

    #[test]
    fn without_a_microphone_tick_feeds_silence() {
        let counters = Arc::new(IoCounters::default());
        let (play_tx, mut speaker) = rtrb::RingBuffer::new(playback_ring_len(SAMPLE_RATE));
        let tone = |capture: &Frame, playback: &mut Frame| {
            assert!(capture.iter().all(|s| *s == 0.0));
            playback.fill(0.25);
        };
        let mut pipeline = Pipeline::new(
            None,
            Some(PlaybackSide::new(play_tx, SAMPLE_RATE).unwrap()),
            tone,
            counters,
        );
        assert!(!pipeline.has_capture());
        assert_eq!(pipeline.pump().unwrap(), 0);
        pipeline.tick().unwrap();
        assert_eq!(speaker.slots(), FRAME_LEN);
        assert_eq!(speaker.pop().unwrap(), 0.25);
    }
}
