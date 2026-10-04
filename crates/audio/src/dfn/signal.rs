//! One voice's signal side of DeepFilterNet3: the STFT, the features the
//! networks take, and the filters their outputs drive. The networks run
//! between [`Dsp::analyse`] and [`Dsp::synthesise`], reading and writing
//! [`NetIo`]; nothing here allocates once made.

use std::collections::VecDeque;

use realfft::num_complex::Complex32;

use super::config::Config;
use super::dsp::{self, Features, Stft};
use super::{MAX_DB_DF, MAX_DB_ERB, MIN_DB};
use crate::AudioError;
use crate::frame::{FRAME_LEN, Frame};

/// What the network's local SNR estimate asks for in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stages {
    /// Apply the ERB gains.
    pub gains: bool,
    /// The frame is noise only: silence it.
    pub zero_gains: bool,
    /// Apply the deep filter.
    pub deep_filter: bool,
}

impl Stages {
    pub(crate) fn of(lsnr: f32) -> Self {
        if lsnr < MIN_DB {
            Self {
                gains: false,
                zero_gains: true,
                deep_filter: false,
            }
        } else if lsnr > MAX_DB_ERB {
            Self {
                gains: false,
                zero_gains: false,
                deep_filter: false,
            }
        } else {
            Self {
                gains: true,
                zero_gains: false,
                deep_filter: lsnr <= MAX_DB_DF,
            }
        }
    }
}

/// The networks' input and output for one frame of one voice, kept from
/// frame to frame.
pub(crate) struct NetIo {
    /// In: the ERB features.
    pub erb: Vec<f32>,
    /// In: the complex features, real parts then imaginary ones.
    pub feat_spec: Vec<f32>,
    /// Out: the ERB gains, when the stages ask for them.
    pub gains: Vec<f32>,
    /// Out: the deep filter's coefficients, `[bin][tap]` as interleaved
    /// real and imaginary parts, when the stages ask for them.
    pub coefs: Vec<f32>,
    /// Out: the local SNR estimate, dB.
    pub lsnr: f32,
}

impl NetIo {
    pub(crate) fn new(cfg: &Config) -> Self {
        Self {
            erb: vec![0.0; cfg.nb_erb],
            feat_spec: vec![0.0; 2 * cfg.nb_df],
            gains: vec![0.0; cfg.nb_erb],
            coefs: vec![0.0; coefficient_len(cfg)],
            lsnr: 0.0,
        }
    }

    /// The stages this frame's local SNR asks for.
    pub(crate) fn stages(&self) -> Stages {
        Stages::of(self.lsnr)
    }
}

/// Values in the deep filter's coefficients (interleaved complex).
pub(crate) fn coefficient_len(cfg: &Config) -> usize {
    cfg.nb_df * cfg.df_order * 2
}

/// Copies `values` into `out`, which must be as long.
pub(crate) fn fill(
    out: &mut [f32],
    values: impl ExactSizeIterator<Item = f32>,
) -> Result<(), AudioError> {
    if values.len() != out.len() {
        return Err(AudioError::Internal);
    }
    for (o, v) in out.iter_mut().zip(values) {
        *o = v;
    }
    Ok(())
}

/// The share of the noisy signal mixed back for an attenuation limit in
/// dB; `None` for no limit (100 dB and up).
pub(crate) fn mix_back(limit_db: f32) -> Option<f32> {
    let limit = limit_db.abs();
    if limit >= 100.0 {
        None
    } else {
        Some(10_f32.powf(-limit.max(0.01) / 20.0))
    }
}

/// See the module docs.
pub(crate) struct Dsp {
    stft: Stft,
    features: Features,
    spec: Vec<Complex32>,
    cplx: Vec<Complex32>,
    pub(crate) net: NetIo,
    /// Spectra for the ERB gains: the target frame is `df_order - 1`.
    enhanced: VecDeque<Vec<Complex32>>,
    /// Noisy spectra for the deep filter, oldest first.
    noisy: VecDeque<Vec<Complex32>>,
    out: Vec<Complex32>,
    lookahead: usize,
    mix_back: Option<f32>,
}

impl Dsp {
    pub(crate) fn new(cfg: &Config, attenuation_limit_db: f32) -> Self {
        let stft = Stft::new(960, FRAME_LEN);
        let bins = stft.bins();
        let widths = dsp::erb_widths(48_000, 960, cfg.nb_erb, cfg.min_nb_erb_freqs);
        let alpha = dsp::norm_alpha(48_000, FRAME_LEN, cfg.norm_tau);
        let lookahead = cfg.conv_lookahead.max(cfg.df_lookahead);
        let zeros = || vec![Complex32::default(); bins];
        Self {
            features: Features::new(widths, cfg.nb_df, alpha),
            spec: zeros(),
            cplx: vec![Complex32::default(); cfg.nb_df],
            net: NetIo::new(cfg),
            enhanced: (0..cfg.df_order + cfg.conv_lookahead)
                .map(|_| zeros())
                .collect(),
            noisy: (0..cfg.df_order.max(lookahead)).map(|_| zeros()).collect(),
            out: zeros(),
            lookahead,
            mix_back: mix_back(attenuation_limit_db),
            stft,
        }
    }

    /// Back to silence before the first frame, without allocating (a new
    /// voice in a `DeepFilterBank` row).
    #[cfg(feature = "onnxruntime")]
    pub(crate) fn reset(&mut self) {
        self.stft.reset();
        self.features.reset();
        for ring in [&mut self.enhanced, &mut self.noisy] {
            for spec in ring.iter_mut() {
                spec.fill(Complex32::default());
            }
        }
        self.net.lsnr = 0.0;
    }

    /// Most attenuation applied, dB (100 or more: no limit).
    pub(crate) fn set_attenuation_limit(&mut self, db: f32) {
        self.mix_back = mix_back(db);
    }

    /// Takes one frame in: its spectrum, and the networks' input in
    /// [`NetIo`].
    pub(crate) fn analyse(&mut self, cfg: &Config, frame: &Frame) {
        self.stft.analysis(&frame[..], &mut self.spec);
        for ring in [&mut self.enhanced, &mut self.noisy] {
            if let Some(mut oldest) = ring.pop_front() {
                oldest.copy_from_slice(&self.spec);
                ring.push_back(oldest);
            }
        }
        self.features.erb(&self.spec, &mut self.net.erb);
        self.features
            .complex(&self.spec[..cfg.nb_df], &mut self.cplx);
        let n = cfg.nb_df;
        for (i, c) in self.cplx.iter().enumerate() {
            self.net.feat_spec[i] = c.re;
            self.net.feat_spec[n + i] = c.im;
        }
    }

    /// Applies the networks' outputs in [`NetIo`] and writes the cleaned
    /// frame (it comes out a hop and the lookahead late).
    pub(crate) fn synthesise(&mut self, cfg: &Config, frame: &mut Frame) {
        let stages = self.net.stages();
        let order = cfg.df_order;
        let target = &mut self.enhanced[order - 1];
        if stages.zero_gains {
            target.fill(Complex32::default());
        } else if stages.gains {
            dsp::apply_gains(target, &self.net.gains, self.features.widths());
        }
        self.out.copy_from_slice(target);
        if stages.deep_filter {
            let history = self.noisy.range(..order).map(Vec::as_slice);
            dsp::deep_filter(history, &self.net.coefs, order, &mut self.out);
        }
        if let Some(mix) = self.mix_back {
            let noisy = &self.noisy[self.noisy.len() - self.lookahead - 1];
            for (o, n) in self.out.iter_mut().zip(noisy) {
                *o = *o * (1.0 - mix) + n * mix;
            }
        }
        self.stft.synthesis(&self.out, &mut frame[..]);
    }

    /// How late the output is, in samples: one hop for the STFT, plus the
    /// networks' lookahead.
    pub(crate) fn latency_samples(&self) -> usize {
        FRAME_LEN * (1 + self.lookahead)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_snr_picks_the_stages() {
        assert_eq!(
            Stages::of(-16.0),
            Stages {
                gains: false,
                zero_gains: true,
                deep_filter: false
            }
        );
        assert_eq!(
            Stages::of(36.0),
            Stages {
                gains: false,
                zero_gains: false,
                deep_filter: false
            }
        );
        assert!(Stages::of(-12.0).gains && Stages::of(-12.0).deep_filter);
        assert!(Stages::of(30.0).gains && Stages::of(30.0).deep_filter);
    }

    #[test]
    fn the_attenuation_limit_mixes_some_noisy_signal_back() {
        assert_eq!(mix_back(100.0), None);
        assert_eq!(mix_back(120.0), None);
        assert!((mix_back(20.0).unwrap() - 0.1).abs() < 1e-6);
        assert!(
            (mix_back(-20.0).unwrap() - 0.1).abs() < 1e-6,
            "sign ignored"
        );
        assert!(mix_back(0.0).unwrap() > 0.99, "no reduction at all");
    }

    #[test]
    fn outputs_are_copied_only_at_their_length() {
        let mut out = [0.0; 3];
        assert!(fill(&mut out, [1.0, 2.0, 3.0].into_iter()).is_ok());
        assert_eq!(out, [1.0, 2.0, 3.0]);
        assert!(fill(&mut out, [1.0].into_iter()).is_err());
    }
}
