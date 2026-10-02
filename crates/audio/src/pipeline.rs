//! The processing-thread side of audio I/O, independent of cpal so tests can
//! drive it with plain rings.
//!
//! Device callbacks only move samples between the device buffers and two
//! lock-free SPSC rings (mono, at the device rate). This thread pulls capture
//! samples, resamples them to 48 kHz frames, runs the [`FrameProcessor`] once
//! per 10 ms frame, and pushes the playback frame back towards the device.
//! The capture clock paces processing; without a microphone a 10 ms timer
//! does.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::AudioError;
use crate::frame::{FRAME_LEN, Frame, SILENCE};
use crate::resample::{FromInternal, ToInternal};

/// Work done every 10 ms on the processing thread. Implementations must not
/// block; allocation should be avoided in the steady state.
pub trait FrameProcessor: Send + 'static {
    /// `capture` is the microphone frame (silence without a microphone);
    /// `playback` starts silent and is what the speakers will play.
    fn process(&mut self, capture: &Frame, playback: &mut Frame);
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
}

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
}

impl IoCounters {
    /// Reads every counter.
    pub fn snapshot(&self) -> IoStats {
        IoStats {
            frames: self.frames.load(Ordering::Relaxed),
            capture_overruns: self.capture_overruns.load(Ordering::Relaxed),
            playback_underruns: self.playback_underruns.load(Ordering::Relaxed),
            playback_drops: self.playback_drops.load(Ordering::Relaxed),
        }
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

pub(crate) struct Pipeline<P> {
    capture: Option<CaptureSide>,
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
            playback,
            processor,
            scratch: vec![0.0; scratch_len],
            frame_in: SILENCE,
            frame_out: SILENCE,
            counters,
        }
    }

    pub(crate) fn has_capture(&self) -> bool {
        self.capture.is_some()
    }

    /// Runs one frame for every 10 ms of captured audio available; returns
    /// how many ran. Without a capture side it runs nothing (see
    /// [`tick`](Self::tick)).
    pub(crate) fn pump(&mut self) -> Result<usize, AudioError> {
        let mut ran = 0;
        loop {
            let Some(capture) = self.capture.as_mut() else {
                return Ok(ran);
            };
            let need = capture.converter.input_frames_next();
            if capture.ring.slots() < need {
                return Ok(ran);
            }
            let input = &mut self.scratch[..need];
            capture
                .ring
                .pop_entire_slice(input)
                .map_err(|_| AudioError::Internal)?;
            capture.converter.process(input, &mut self.frame_in)?;
            self.run_frame()?;
            ran += 1;
        }
    }

    /// Runs one frame of silence through the processor (no microphone; the
    /// caller paces this every 10 ms).
    pub(crate) fn tick(&mut self) -> Result<(), AudioError> {
        self.frame_in = SILENCE;
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
