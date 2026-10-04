//! DeepFilterNet3's signal processing around the network, written after
//! studying upstream libDF (Rikorose/DeepFilterNet v0.5.6, MIT OR
//! Apache-2.0): a Vorbis-windowed STFT (960-point FFT, 480 hop at 48 kHz),
//! the ERB band features with exponential mean normalisation, the
//! unit-normalised complex features of the lowest bins, the ERB gains, and
//! the deep filter.

use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// Initial value of the ERB mean normalisation, from the lowest band to the
/// highest (dB).
const MEAN_NORM_INIT: (f32, f32) = (-60.0, -90.0);
/// Initial value of the complex unit normalisation, from the lowest bin.
const UNIT_NORM_INIT: (f32, f32) = (0.001, 0.0001);

fn freq_to_erb(hz: f32) -> f32 {
    9.265 * (hz / (24.7 * 9.265)).ln_1p()
}

fn erb_to_freq(erb: f32) -> f32 {
    24.7 * 9.265 * ((erb / 9.265).exp() - 1.0)
}

/// How many FFT bins each ERB band covers (lowest band first); the widths
/// add up to `fft_size / 2 + 1`.
pub(crate) fn erb_widths(
    rate: usize,
    fft_size: usize,
    bands: usize,
    min_bins: usize,
) -> Vec<usize> {
    let bin_hz = rate as f32 / fft_size as f32;
    let high = freq_to_erb((rate / 2) as f32);
    let step = high / bands as f32;
    let mut widths = vec![0_usize; bands];
    let mut previous = 0_i64;
    let mut borrowed = 0_i64;
    for (i, width) in widths.iter_mut().enumerate() {
        let edge = (erb_to_freq((i + 1) as f32 * step) / bin_hz).round() as i64;
        let mut bins = edge - previous - borrowed;
        // Low bands are narrower than a bin: give them `min_bins` and take
        // the extra from the next bands.
        if bins < min_bins as i64 {
            borrowed = min_bins as i64 - bins;
            bins = min_bins as i64;
        } else {
            borrowed = 0;
        }
        *width = bins as usize;
        previous = edge;
    }
    let total = fft_size / 2 + 1;
    let last = bands - 1;
    widths[last] += 1;
    let sum: usize = widths.iter().sum();
    widths[last] -= sum.saturating_sub(total);
    widths
}

/// The normalisation smoothing factor for a time constant `tau` (s), rounded
/// as upstream rounds it (0.99 for 10 ms hops and 1 s).
pub(crate) fn norm_alpha(rate: usize, hop: usize, tau: f32) -> f32 {
    let alpha = (-(hop as f32 / rate as f32) / tau).exp();
    let mut precision = 3;
    loop {
        let scale = 10_f32.powi(precision);
        let rounded = (alpha * scale).round() / scale;
        if rounded < 1.0 {
            return rounded;
        }
        precision += 1;
    }
}

/// Short-time Fourier transform with a Vorbis window and 50 % overlap:
/// analysis then synthesis reconstructs the input, `fft_size - hop` samples
/// late.
pub(crate) struct Stft {
    hop: usize,
    window: Vec<f32>,
    /// Applied in analysis only, so that synthesis is the plain inverse.
    norm: f32,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    analysis_mem: Vec<f32>,
    synthesis_mem: Vec<f32>,
    time: Vec<f32>,
    spec: Vec<Complex32>,
    scratch_fwd: Vec<Complex32>,
    scratch_inv: Vec<Complex32>,
}

impl Stft {
    pub(crate) fn new(fft_size: usize, hop: usize) -> Self {
        assert_eq!(fft_size, hop * 2, "50 % overlap");
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(fft_size);
        let inverse = planner.plan_fft_inverse(fft_size);
        let half = fft_size as f64 / 2.0;
        let window = (0..fft_size)
            .map(|i| {
                let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / half).sin();
                (0.5 * std::f64::consts::PI * s * s).sin() as f32
            })
            .collect();
        Self {
            hop,
            window,
            norm: (2 * hop) as f32 / (fft_size * fft_size) as f32,
            scratch_fwd: forward.make_scratch_vec(),
            scratch_inv: inverse.make_scratch_vec(),
            time: forward.make_input_vec(),
            spec: forward.make_output_vec(),
            forward,
            inverse,
            analysis_mem: vec![0.0; fft_size - hop],
            synthesis_mem: vec![0.0; fft_size - hop],
        }
    }

    /// Back to silence before the first frame.
    #[cfg(feature = "onnxruntime")]
    pub(crate) fn reset(&mut self) {
        self.analysis_mem.fill(0.0);
        self.synthesis_mem.fill(0.0);
    }

    /// Number of frequency bins (`fft_size / 2 + 1`).
    pub(crate) fn bins(&self) -> usize {
        self.spec.len()
    }

    /// One hop of input into its spectrum.
    pub(crate) fn analysis(&mut self, input: &[f32], out: &mut [Complex32]) {
        debug_assert_eq!(input.len(), self.hop);
        let (old, new) = self.time.split_at_mut(self.window.len() - self.hop);
        let (w_old, w_new) = self.window.split_at(self.window.len() - self.hop);
        for ((t, m), w) in old.iter_mut().zip(&self.analysis_mem).zip(w_old) {
            *t = m * w;
        }
        for ((t, x), w) in new.iter_mut().zip(input).zip(w_new) {
            *t = x * w;
        }
        self.analysis_mem.copy_from_slice(input);
        // Cannot fail: the buffers have the planned lengths.
        let _ = self
            .forward
            .process_with_scratch(&mut self.time, out, &mut self.scratch_fwd);
        for x in out.iter_mut() {
            *x *= self.norm;
        }
    }

    /// One spectrum back into a hop of output (overlap-add).
    pub(crate) fn synthesis(&mut self, spec: &[Complex32], out: &mut [f32]) {
        debug_assert_eq!(out.len(), self.hop);
        self.spec.copy_from_slice(spec);
        // A real signal's DC and Nyquist bins have no imaginary part.
        if let Some(first) = self.spec.first_mut() {
            first.im = 0.0;
        }
        if let Some(last) = self.spec.last_mut() {
            last.im = 0.0;
        }
        let _ = self.inverse.process_with_scratch(
            &mut self.spec,
            &mut self.time,
            &mut self.scratch_inv,
        );
        for (t, w) in self.time.iter_mut().zip(&self.window) {
            *t *= w;
        }
        let (first, second) = self.time.split_at(self.hop);
        for ((o, t), m) in out.iter_mut().zip(first).zip(&self.synthesis_mem) {
            *o = t + m;
        }
        self.synthesis_mem.copy_from_slice(second);
    }
}

/// The network's input features, with their running normalisation.
pub(crate) struct Features {
    widths: Vec<usize>,
    alpha: f32,
    mean_norm: Vec<f32>,
    unit_norm: Vec<f32>,
}

fn ramp(from: f32, to: f32, n: usize) -> Vec<f32> {
    let mut out = vec![0.0; n];
    ramp_into((from, to), &mut out);
    out
}

fn ramp_into((from, to): (f32, f32), out: &mut [f32]) {
    let step = (to - from) / (out.len() - 1) as f32;
    for (i, o) in out.iter_mut().enumerate() {
        *o = from + i as f32 * step;
    }
}

impl Features {
    pub(crate) fn new(widths: Vec<usize>, df_bins: usize, alpha: f32) -> Self {
        let bands = widths.len();
        Self {
            widths,
            alpha,
            mean_norm: ramp(MEAN_NORM_INIT.0, MEAN_NORM_INIT.1, bands),
            unit_norm: ramp(UNIT_NORM_INIT.0, UNIT_NORM_INIT.1, df_bins),
        }
    }

    /// Back to the initial normalisation, without allocating.
    #[cfg(feature = "onnxruntime")]
    pub(crate) fn reset(&mut self) {
        ramp_into(MEAN_NORM_INIT, &mut self.mean_norm);
        ramp_into(UNIT_NORM_INIT, &mut self.unit_norm);
    }

    pub(crate) fn widths(&self) -> &[usize] {
        &self.widths
    }

    /// Band energies in dB, mean-normalised and scaled by 1/40.
    pub(crate) fn erb(&mut self, spec: &[Complex32], out: &mut [f32]) {
        let mut start = 0;
        for ((o, width), mean) in out.iter_mut().zip(&self.widths).zip(&mut self.mean_norm) {
            let band = &spec[start..start + width];
            let power = band.iter().map(Complex32::norm_sqr).sum::<f32>() / *width as f32;
            let db = 10.0 * (power + 1e-10).log10();
            *mean = db * (1.0 - self.alpha) + *mean * self.alpha;
            *o = (db - *mean) / 40.0;
            start += width;
        }
    }

    /// The lowest bins divided by the square root of their smoothed
    /// magnitude. `out` holds `df_bins` values.
    pub(crate) fn complex(&mut self, spec: &[Complex32], out: &mut [Complex32]) {
        for ((o, x), state) in out.iter_mut().zip(spec).zip(&mut self.unit_norm) {
            *state = x.norm() * (1.0 - self.alpha) + *state * self.alpha;
            *o = x / state.sqrt();
        }
    }
}

/// Multiplies every bin of each ERB band by the band's gain.
pub(crate) fn apply_gains(spec: &mut [Complex32], gains: &[f32], widths: &[usize]) {
    let mut start = 0;
    for (gain, width) in gains.iter().zip(widths) {
        for x in &mut spec[start..start + width] {
            *x *= *gain;
        }
        start += width;
    }
}

/// The deep filter: each of the lowest `coefs.len() / (2 * order)` bins of
/// `out` becomes the sum over the frames of `history` (oldest first, `order`
/// of them) of that bin times its complex coefficient. `coefs` is
/// `[bin][frame]`, each an interleaved real and imaginary part.
pub(crate) fn deep_filter<'a>(
    history: impl Iterator<Item = &'a [Complex32]> + Clone,
    coefs: &[f32],
    order: usize,
    out: &mut [Complex32],
) {
    let bins = out.iter_mut().zip(coefs.chunks_exact(2 * order));
    for (bin, (o, taps)) in bins.enumerate() {
        *o = history
            .clone()
            .zip(taps.chunks_exact(2))
            .map(|(frame, c)| frame[bin] * Complex32::new(c[0], c[1]))
            .sum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erb_bands_cover_every_bin_with_at_least_two() {
        let widths = erb_widths(48_000, 960, 32, 2);
        assert_eq!(widths.len(), 32);
        assert_eq!(widths.iter().sum::<usize>(), 481);
        assert!(widths.iter().all(|w| *w >= 2), "{widths:?}");
        // Wider at the top, as the ear's bands are.
        assert!(widths[31] > widths[8], "{widths:?}");
    }

    #[test]
    fn normalisation_alpha_matches_upstream_rounding() {
        assert_eq!(norm_alpha(48_000, 480, 1.0), 0.99);
    }

    #[test]
    fn analysis_then_synthesis_gives_the_input_back_a_hop_late() {
        let mut stft = Stft::new(960, 480);
        let input: Vec<f32> = (0..480 * 20)
            .map(|n| 0.3 * (n as f32 * 0.031).sin() + 0.1 * (n as f32 * 0.17).cos())
            .collect();
        let mut spec = vec![Complex32::default(); stft.bins()];
        let mut output = Vec::new();
        let mut hop = vec![0.0; 480];
        for chunk in input.chunks(480) {
            stft.analysis(chunk, &mut spec);
            stft.synthesis(&spec, &mut hop);
            output.extend_from_slice(&hop);
        }
        let worst = input[..input.len() - 480]
            .iter()
            .zip(&output[480..])
            .skip(480)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 1e-4, "reconstruction error {worst}");
    }

    #[test]
    fn features_are_normalised_and_gains_follow_the_bands() {
        let widths = erb_widths(48_000, 960, 32, 2);
        let mut features = Features::new(widths.clone(), 96, 0.99);
        let spec = vec![Complex32::new(0.01, 0.0); 481];
        let mut erb = vec![0.0; 32];
        // A steady input converges towards zero after normalisation.
        for _ in 0..2_000 {
            features.erb(&spec, &mut erb);
        }
        assert!(erb.iter().all(|e| e.abs() < 0.01), "{erb:?}");
        let mut cplx = vec![Complex32::default(); 96];
        for _ in 0..2_000 {
            features.complex(&spec[..96], &mut cplx);
        }
        // x / sqrt(|x|) for a steady x.
        assert!((cplx[0].re - 0.1).abs() < 1e-3, "{:?}", cplx[0]);

        let mut gained = spec.clone();
        let mut gains = vec![1.0; 32];
        gains[0] = 0.0;
        apply_gains(&mut gained, &gains, &widths);
        assert!(gained[..widths[0]].iter().all(|x| x.norm() == 0.0));
        assert_eq!(gained[widths[0]], spec[widths[0]]);
    }

    #[test]
    fn the_deep_filter_sums_frames_times_their_coefficients() {
        let a = [Complex32::new(1.0, 0.0); 4];
        let b = [Complex32::new(0.0, 1.0); 4];
        let history: [&[Complex32]; 2] = [&a, &b];
        // Two bins, order 2: bin 0 takes 2·a + 1·b, bin 1 takes i·a.
        let coefs = [2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let mut out = [Complex32::new(9.0, 9.0); 4];
        deep_filter(history.into_iter(), &coefs, 2, &mut out);
        assert_eq!(out[0], Complex32::new(2.0, 1.0));
        assert_eq!(out[1], Complex32::new(0.0, 1.0));
        assert_eq!(out[2], Complex32::new(9.0, 9.0), "higher bins untouched");
    }
}
