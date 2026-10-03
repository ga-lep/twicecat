//! DeepFilterNet3 noise suppression: our STFT and features ([`dsp`]) around
//! the three pulsed networks ([`model`]), run one 10 ms frame at a time.
//!
//! The models are fetched by `scripts/fetch-models.sh` (pinned SHA-256)
//! into the cache dir, or found in `TESSERA_MODEL_DIR`. Loading takes a
//! while (the graphs are optimised for this CPU), so it happens off the
//! audio thread; the loaded filter is then handed over.

mod config;
mod dsp;
mod model;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use realfft::num_complex::Complex32;
use tract_onnx::prelude::*;

use crate::AudioError;
use crate::frame::{FRAME_LEN, Frame};
use config::Config;
use dsp::{Features, Stft};
use model::Graphs;

// Local SNR thresholds, as upstream's `deep-filter` binary sets them. The
// library's own defaults (-10, 30, 20) apply the ERB gains alone to fairly
// clean speech, which measured much worse here (SI-SDR 4.9 dB against 18.6
// dB on the same mix with upstream's binary; Task 13 notes).
/// Below this local SNR (dB) a frame is noise only: its gains are zero.
const MIN_DB: f32 = -15.0;
/// Above this the frame is clean speech: nothing is applied.
const MAX_DB_ERB: f32 = 35.0;
/// Above this only the ERB gains are applied, not the deep filter.
const MAX_DB_DF: f32 = 35.0;

/// The model directory: `TESSERA_MODEL_DIR`, else where
/// `scripts/fetch-models.sh` puts it.
pub fn default_model_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TESSERA_MODEL_DIR") {
        return Some(PathBuf::from(dir));
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(cache.join("tessera/models/deepfilternet3"))
}

/// What the network decided for a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stages {
    gains: bool,
    zero_gains: bool,
    deep_filter: bool,
}

fn stages(lsnr: f32) -> Stages {
    if lsnr < MIN_DB {
        Stages {
            gains: false,
            zero_gains: true,
            deep_filter: false,
        }
    } else if lsnr > MAX_DB_ERB {
        Stages {
            gains: false,
            zero_gains: false,
            deep_filter: false,
        }
    } else {
        Stages {
            gains: true,
            zero_gains: false,
            deep_filter: lsnr <= MAX_DB_DF,
        }
    }
}

/// The share of the noisy signal mixed back for an attenuation limit in
/// dB; `None` for no limit (100 dB and up).
fn mix_back(limit_db: f32) -> Option<f32> {
    let limit = limit_db.abs();
    if limit >= 100.0 {
        None
    } else {
        Some(10_f32.powf(-limit.max(0.01) / 20.0))
    }
}

/// One frame's ERB gains and deep-filter coefficients, each when needed.
type Decision = (Option<Vec<f32>>, Option<Vec<Complex32>>);

/// DeepFilterNet3 on 48 kHz mono 10 ms frames.
pub struct DeepFilter {
    cfg: Config,
    stft: Stft,
    features: Features,
    graphs: Graphs,
    spec: Vec<Complex32>,
    erb: Vec<f32>,
    cplx: Vec<Complex32>,
    /// Spectra for the ERB gains: the target frame is `df_order - 1`.
    enhanced: VecDeque<Vec<Complex32>>,
    /// Noisy spectra for the deep filter, oldest first.
    noisy: VecDeque<Vec<Complex32>>,
    out: Vec<Complex32>,
    lookahead: usize,
    mix_back: Option<f32>,
    lsnr: f32,
}

impl DeepFilter {
    /// Loads the models from `dir`. Slow (seconds in a debug build): call
    /// it off the audio thread.
    pub fn load(dir: &Path, attenuation_limit_db: f32) -> Result<Self, AudioError> {
        let text =
            std::fs::read_to_string(dir.join("config.ini")).map_err(|_| AudioError::Model)?;
        let cfg = Config::parse(&text)?;
        let graphs = model::load(dir, &cfg)?;
        let stft = Stft::new(960, FRAME_LEN);
        let bins = stft.bins();
        let widths = dsp::erb_widths(48_000, 960, cfg.nb_erb, cfg.min_nb_erb_freqs);
        let alpha = dsp::norm_alpha(48_000, FRAME_LEN, cfg.norm_tau);
        let lookahead = cfg.conv_lookahead.max(cfg.df_lookahead);
        let zeros = || vec![Complex32::default(); bins];
        Ok(Self {
            features: Features::new(widths, cfg.nb_df, alpha),
            graphs,
            spec: zeros(),
            erb: vec![0.0; cfg.nb_erb],
            cplx: vec![Complex32::default(); cfg.nb_df],
            enhanced: (0..cfg.df_order + cfg.conv_lookahead)
                .map(|_| zeros())
                .collect(),
            noisy: (0..cfg.df_order.max(lookahead)).map(|_| zeros()).collect(),
            out: zeros(),
            lookahead,
            mix_back: mix_back(attenuation_limit_db),
            lsnr: 0.0,
            stft,
            cfg,
        })
    }

    /// How late the output is, in samples: one hop for the STFT, plus the
    /// networks' lookahead.
    pub fn latency_samples(&self) -> usize {
        FRAME_LEN * (1 + self.lookahead)
    }

    /// The last frame's local SNR estimate, dB.
    pub fn local_snr_db(&self) -> f32 {
        self.lsnr
    }

    /// Most attenuation applied, dB (100 or more: no limit).
    pub fn set_attenuation_limit(&mut self, db: f32) {
        self.mix_back = mix_back(db);
    }

    /// Cleans one frame in place (it comes out [`latency_samples`] late).
    ///
    /// [`latency_samples`]: Self::latency_samples
    pub fn process(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        self.stft.analysis(&frame[..], &mut self.spec);
        for ring in [&mut self.enhanced, &mut self.noisy] {
            if let Some(mut oldest) = ring.pop_front() {
                oldest.copy_from_slice(&self.spec);
                ring.push_back(oldest);
            }
        }
        self.features.erb(&self.spec, &mut self.erb);
        self.features
            .complex(&self.spec[..self.cfg.nb_df], &mut self.cplx);
        let (gains, coefs) = self.infer()?;

        let order = self.cfg.df_order;
        let target = &mut self.enhanced[order - 1];
        if let Some(gains) = &gains {
            dsp::apply_gains(target, gains, self.features.widths());
        }
        self.out.copy_from_slice(target);
        if let Some(coefs) = &coefs {
            let history: Vec<&[Complex32]> = self.noisy.iter().map(Vec::as_slice).collect();
            dsp::deep_filter(&history[..order], coefs, order, &mut self.out);
        }
        if let Some(mix) = self.mix_back {
            let noisy = &self.noisy[self.noisy.len() - self.lookahead - 1];
            for (o, n) in self.out.iter_mut().zip(noisy) {
                *o = *o * (1.0 - mix) + n * mix;
            }
        }
        self.stft.synthesis(&self.out, &mut frame[..]);
        Ok(())
    }

    /// Runs the networks on the current features: ERB gains and deep-filter
    /// coefficients (`[bin][tap]`), each when the local SNR asks for it.
    fn infer(&mut self) -> Result<Decision, AudioError> {
        let cfg = &self.cfg;
        let tensor = |shape: &[usize], data: &[f32]| {
            Tensor::from_shape(shape, data)
                .map(TValue::from)
                .map_err(|_| AudioError::Internal)
        };
        let feat_erb = tensor(&[1, 1, 1, cfg.nb_erb], &self.erb)?;
        let mut spec = Vec::with_capacity(2 * cfg.nb_df);
        spec.extend(self.cplx.iter().map(|c| c.re));
        spec.extend(self.cplx.iter().map(|c| c.im));
        let feat_spec = tensor(&[1, 2, 1, cfg.nb_df], &spec)?;
        let mut out = self.graphs.encoder.run(tvec!(feat_erb, feat_spec))?;
        // e0, e1, e2, e3, emb, c0, lsnr
        if out.len() != 7 {
            return Err(AudioError::Internal);
        }
        let floats = |t: &TValue| -> Result<Vec<f32>, AudioError> {
            t.to_plain_array_view::<f32>()
                .map(|a| a.iter().copied().collect())
                .map_err(|_| AudioError::Internal)
        };
        self.lsnr = floats(&out[6])?.first().copied().unwrap_or(0.0);
        let stage = stages(self.lsnr);
        let c0 = out.remove(5);
        let emb = out.remove(4);
        let gains = if stage.gains {
            let (e3, e2, e1, e0) = (out.remove(3), out.remove(2), out.remove(1), out.remove(0));
            let m = self
                .graphs
                .erb_decoder
                .run(tvec!(emb.clone(), e3, e2, e1, e0))?;
            Some(floats(m.first().ok_or(AudioError::Internal)?)?)
        } else if stage.zero_gains {
            Some(vec![0.0; cfg.nb_erb])
        } else {
            None
        };
        let coefs = if stage.deep_filter {
            let raw = self.graphs.df_decoder.run(tvec!(emb, c0))?;
            let raw = floats(raw.first().ok_or(AudioError::Internal)?)?;
            if raw.len() != cfg.nb_df * cfg.df_order * 2 {
                return Err(AudioError::Internal);
            }
            Some(
                raw.chunks_exact(2)
                    .map(|c| Complex32::new(c[0], c[1]))
                    .collect(),
            )
        } else {
            None
        };
        Ok((gains, coefs))
    }
}

impl crate::Enhancer for DeepFilter {
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        self.process(frame)
    }

    fn latency_samples(&self) -> usize {
        DeepFilter::latency_samples(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_snr_picks_the_stages() {
        assert_eq!(
            stages(-16.0),
            Stages {
                gains: false,
                zero_gains: true,
                deep_filter: false
            }
        );
        assert_eq!(
            stages(36.0),
            Stages {
                gains: false,
                zero_gains: false,
                deep_filter: false
            }
        );
        assert!(stages(-12.0).gains && stages(-12.0).deep_filter);
        assert!(stages(30.0).gains && stages(30.0).deep_filter);
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
}
