//! Rate conversion between a device's rate and the internal 48 kHz, on the
//! processing thread (never in a device callback). A device that already runs
//! at 48 kHz, the usual case with PipeWire, passes through untouched.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::AudioError;
use crate::frame::{FRAME_LEN, Frame, SAMPLE_RATE};

/// Room for the ratio to be adjusted later (clock drift), relative.
const MAX_RATIO_RELATIVE: f64 = 1.1;

fn sinc(ratio: f64, fixed: FixedAsync) -> Result<Async<f32>, AudioError> {
    // 128 taps: transparent for voice, about 1.3 ms of delay at 48 kHz.
    let mut params = SincInterpolationParameters::new(128, WindowFunction::BlackmanHarris2);
    params.interpolation = SincInterpolationType::Cubic;
    Async::new_sinc(ratio, MAX_RATIO_RELATIVE, &params, FRAME_LEN, 1, fixed)
        .map_err(|_| AudioError::Resampler)
}

/// Device rate → 48 kHz frames (capture side).
pub(crate) struct ToInternal {
    inner: Option<Async<f32>>,
}

impl ToInternal {
    pub(crate) fn new(device_rate: u32) -> Result<Self, AudioError> {
        let inner = (device_rate != SAMPLE_RATE)
            .then(|| {
                sinc(
                    f64::from(SAMPLE_RATE) / f64::from(device_rate),
                    FixedAsync::Output,
                )
            })
            .transpose()?;
        Ok(Self { inner })
    }

    /// Device-rate samples the next [`process`](Self::process) needs.
    pub(crate) fn input_frames_next(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(FRAME_LEN, Resampler::input_frames_next)
    }

    /// Most device-rate samples any call can need.
    pub(crate) fn input_frames_max(&self) -> usize {
        self.inner
            .as_ref()
            .map_or(FRAME_LEN, Resampler::input_frames_max)
    }

    /// Converts exactly [`input_frames_next`](Self::input_frames_next)
    /// samples into one frame.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Frame) -> Result<(), AudioError> {
        let Some(rs) = self.inner.as_mut() else {
            out.copy_from_slice(input);
            return Ok(());
        };
        let src =
            InterleavedSlice::new(input, 1, input.len()).map_err(|_| AudioError::Resampler)?;
        let mut dst = InterleavedSlice::new_mut(&mut out[..], 1, FRAME_LEN)
            .map_err(|_| AudioError::Resampler)?;
        rs.process_into_buffer(&src, &mut dst, None)
            .map_err(|_| AudioError::Resampler)?;
        Ok(())
    }
}

/// 48 kHz frames → device rate (playback side).
pub(crate) struct FromInternal {
    inner: Option<Async<f32>>,
    out: Vec<f32>,
}

impl FromInternal {
    pub(crate) fn new(device_rate: u32) -> Result<Self, AudioError> {
        let inner = (device_rate != SAMPLE_RATE)
            .then(|| {
                sinc(
                    f64::from(device_rate) / f64::from(SAMPLE_RATE),
                    FixedAsync::Input,
                )
            })
            .transpose()?;
        let capacity = inner
            .as_ref()
            .map_or(FRAME_LEN, Resampler::output_frames_max);
        Ok(Self {
            inner,
            out: vec![0.0; capacity],
        })
    }

    /// Converts one frame; returns the device-rate samples.
    pub(crate) fn process(&mut self, frame: &Frame) -> Result<&[f32], AudioError> {
        let Some(rs) = self.inner.as_mut() else {
            self.out.copy_from_slice(frame);
            return Ok(&self.out);
        };
        let frames = rs.output_frames_next();
        let src =
            InterleavedSlice::new(&frame[..], 1, FRAME_LEN).map_err(|_| AudioError::Resampler)?;
        let mut dst = InterleavedSlice::new_mut(&mut self.out[..frames], 1, frames)
            .map_err(|_| AudioError::Resampler)?;
        let (_, written) = rs
            .process_into_buffer(&src, &mut dst, None)
            .map_err(|_| AudioError::Resampler)?;
        Ok(&self.out[..written])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Amplitude of `freq` in `signal` (sampled at `rate`), by correlation.
    pub(crate) fn tone_amplitude(signal: &[f32], rate: u32, freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / rate as f32;
        let (mut re, mut im) = (0.0_f32, 0.0_f32);
        for (n, s) in signal.iter().enumerate() {
            re += s * (w * n as f32).cos();
            im += s * (w * n as f32).sin();
        }
        2.0 * (re * re + im * im).sqrt() / signal.len() as f32
    }

    pub(crate) fn sine(rate: u32, freq: f32, amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| amplitude * (2.0 * std::f32::consts::PI * freq * n as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn same_rate_passes_through() {
        let mut to = ToInternal::new(SAMPLE_RATE).unwrap();
        assert_eq!(to.input_frames_next(), FRAME_LEN);
        let input = sine(SAMPLE_RATE, 1000.0, 0.5, FRAME_LEN);
        let mut frame = [0.0; FRAME_LEN];
        to.process(&input, &mut frame).unwrap();
        assert_eq!(&frame[..], &input[..]);
        let mut from = FromInternal::new(SAMPLE_RATE).unwrap();
        assert_eq!(from.process(&frame).unwrap(), &input[..]);
    }

    #[test]
    fn capture_at_44100_keeps_a_tone() {
        let mut to = ToInternal::new(44_100).unwrap();
        let input = sine(44_100, 1000.0, 0.5, 44_100);
        let mut pos = 0;
        let mut out = Vec::new();
        let mut frame = [0.0; FRAME_LEN];
        while pos + to.input_frames_next() <= input.len() {
            let n = to.input_frames_next();
            to.process(&input[pos..pos + n], &mut frame).unwrap();
            out.extend_from_slice(&frame);
            pos += n;
        }
        // About one second in, minus the filter warm-up.
        assert!(out.len() >= 47_000, "{}", out.len());
        let steady = &out[4800..];
        let a = tone_amplitude(steady, SAMPLE_RATE, 1000.0);
        assert!((a - 0.5).abs() < 0.02, "amplitude {a}");
        let leak = tone_amplitude(steady, SAMPLE_RATE, 1088.4);
        assert!(leak < 0.02, "1 kHz must not be shifted: {leak}");
    }

    #[test]
    fn playback_to_96000_keeps_a_tone_and_rate() {
        let mut from = FromInternal::new(96_000).unwrap();
        let input = sine(SAMPLE_RATE, 1000.0, 0.5, SAMPLE_RATE as usize);
        let mut out = Vec::new();
        for chunk in input.chunks_exact(FRAME_LEN) {
            let frame: &Frame = chunk.try_into().unwrap();
            out.extend_from_slice(from.process(frame).unwrap());
        }
        let len = out.len() as f32;
        assert!((len - 96_000.0).abs() < 400.0, "{len}");
        let a = tone_amplitude(&out[9600..], 96_000, 1000.0);
        assert!((a - 0.5).abs() < 0.02, "amplitude {a}");
    }
}
