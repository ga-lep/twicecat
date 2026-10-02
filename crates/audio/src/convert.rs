//! Sample-format and channel conversion between device buffers and the
//! internal mono `f32` signal. Used inside device callbacks, so nothing here
//! allocates or locks.

use cpal::{FromSample, Sample};

/// Averages each interleaved frame of `data` (with `channels` channels) into
/// one mono sample and hands it to `push`.
pub(crate) fn downmix<T>(data: &[T], channels: usize, mut push: impl FnMut(f32))
where
    T: Sample,
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    let scale = 1.0 / channels as f32;
    for frame in data.chunks_exact(channels) {
        let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
        push(sum * scale);
    }
}

/// Fills each interleaved frame of `out` (with `channels` channels) with the
/// next mono sample from `next`, the same value on every channel.
pub(crate) fn upmix<T>(out: &mut [T], channels: usize, mut next: impl FnMut() -> f32)
where
    T: Sample + FromSample<f32>,
{
    let channels = channels.max(1);
    for frame in out.chunks_exact_mut(channels) {
        let value = T::from_sample(next().clamp(-1.0, 1.0));
        frame.fill(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        let stereo = [1.0_f32, 0.0, -0.5, -0.5, 0.25, 0.75];
        let mut mono = Vec::new();
        downmix(&stereo, 2, |s| mono.push(s));
        assert_eq!(mono, [0.5, -0.5, 0.5]);
    }

    #[test]
    fn downmix_converts_integer_formats() {
        let mut mono = Vec::new();
        downmix(&[i16::MAX, i16::MIN, 0], 1, |s| mono.push(s));
        assert!((mono[0] - 1.0).abs() < 1e-4, "{mono:?}");
        assert!((mono[1] + 1.0).abs() < 1e-4, "{mono:?}");
        assert_eq!(mono[2], 0.0);
        let mut unsigned = Vec::new();
        downmix(&[u16::MAX / 2 + 1], 1, |s| unsigned.push(s));
        assert!(unsigned[0].abs() < 1e-4, "{unsigned:?}");
    }

    #[test]
    fn downmix_ignores_a_trailing_partial_frame() {
        let mut mono = Vec::new();
        downmix(&[0.5_f32, 0.5, 0.9], 2, |s| mono.push(s));
        assert_eq!(mono, [0.5]);
    }

    #[test]
    fn upmix_copies_to_every_channel_and_clamps() {
        let mut out = [0.0_f32; 6];
        let mut values = [0.25_f32, 2.0, -3.0].into_iter();
        upmix(&mut out, 2, || values.next().unwrap());
        assert_eq!(out, [0.25, 0.25, 1.0, 1.0, -1.0, -1.0]);
        let mut ints = [0_i16; 2];
        upmix(&mut ints, 1, || 1.0);
        assert_eq!(ints[0], i16::MAX);
    }
}
