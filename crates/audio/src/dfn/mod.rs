//! DeepFilterNet3 noise suppression: our STFT and features ([`dsp`]) around
//! the three pulsed networks ([`model`]), run one 10 ms frame at a time.
//! With the `onnxruntime` feature the networks can run on a GPU instead, as
//! one stateful graph ([`stateful`]); the CPU graphs stay loaded and take
//! over if the GPU fails or falls behind.
//!
//! The models are fetched by `scripts/fetch-models.sh` (pinned SHA-256)
//! into the cache dir, or found in `TESSERA_MODEL_DIR`. Loading takes a
//! while (the graphs are optimised for this CPU), so it happens off the
//! audio thread; the loaded filter is then handed over.

mod config;
mod dsp;
mod model;
#[cfg(feature = "onnxruntime")]
mod stateful;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use realfft::num_complex::Complex32;

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
pub(crate) const MIN_DB: f32 = -15.0;
/// Above this the frame is clean speech: nothing is applied.
pub(crate) const MAX_DB_ERB: f32 = 35.0;
/// Above this only the ERB gains are applied, not the deep filter.
pub(crate) const MAX_DB_DF: f32 = 35.0;

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

/// The stateful graph for the GPU (`scripts/export-deepfilternet.sh`):
/// `TESSERA_STATEFUL_MODEL`, else where that script installs it.
pub fn default_stateful_model() -> Option<PathBuf> {
    if let Some(file) = std::env::var_os("TESSERA_STATEFUL_MODEL") {
        return Some(PathBuf::from(file));
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(cache.join("tessera/models/deepfilternet3-stateful/deepfilternet3_stateful.onnx"))
}

/// Where DeepFilterNet's networks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeepFilterBackend {
    /// On the CPU (tract), as asked.
    Cpu,
    /// On the accelerator [`DeepFilter::load_gpu`] reported.
    Gpu,
    /// On the CPU after the GPU failed (a driver error, a lost device).
    CpuAfterGpuError,
    /// On the CPU because the GPU missed its deadline (at warm-up, or
    /// frames in a row, such as when a game takes the GPU).
    CpuAfterGpuTooSlow,
}

/// Where a running [`DeepFilter`] is, readable from other threads.
#[derive(Clone, Debug)]
pub struct DeepFilterStatus(Arc<AtomicU8>);

impl DeepFilterStatus {
    fn new(backend: DeepFilterBackend) -> Self {
        let status = Self(Arc::new(AtomicU8::new(0)));
        status.set(backend);
        status
    }

    fn set(&self, backend: DeepFilterBackend) {
        let code = match backend {
            DeepFilterBackend::Cpu => 0,
            DeepFilterBackend::Gpu => 1,
            DeepFilterBackend::CpuAfterGpuError => 2,
            DeepFilterBackend::CpuAfterGpuTooSlow => 3,
        };
        self.0.store(code, Ordering::Relaxed);
    }

    /// The backend now.
    pub fn get(&self) -> DeepFilterBackend {
        match self.0.load(Ordering::Relaxed) {
            1 => DeepFilterBackend::Gpu,
            2 => DeepFilterBackend::CpuAfterGpuError,
            3 => DeepFilterBackend::CpuAfterGpuTooSlow,
            _ => DeepFilterBackend::Cpu,
        }
    }
}

/// A GPU frame slower than this is a missed deadline: the whole chain has
/// 10 ms, and the GPU call must leave room for the rest.
#[cfg(feature = "onnxruntime")]
const GPU_DEADLINE: std::time::Duration = std::time::Duration::from_millis(4);
/// Missed deadlines in a row that hand the frames to the CPU for good.
#[cfg(feature = "onnxruntime")]
const GPU_MISSES: u32 = 3;
/// Frames run at load time, before the deadline counts (the first ones pay
/// for kernel compilation and memory allocation on the device).
#[cfg(feature = "onnxruntime")]
const GPU_WARM_UP_FRAMES: usize = 50;

/// Counts GPU frames over [`GPU_DEADLINE`] in a row.
#[cfg(feature = "onnxruntime")]
#[derive(Debug, Default)]
struct Deadline {
    misses: u32,
}

#[cfg(feature = "onnxruntime")]
impl Deadline {
    /// Records a frame's time; true once [`GPU_MISSES`] in a row missed.
    fn gives_up(&mut self, took: std::time::Duration) -> bool {
        self.misses = if took > GPU_DEADLINE {
            self.misses + 1
        } else {
            0
        };
        self.misses >= GPU_MISSES
    }
}

/// What the network decided for a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stages {
    gains: bool,
    zero_gains: bool,
    deep_filter: bool,
}

pub(crate) fn stages(lsnr: f32) -> Stages {
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

/// One frame's network outputs: the local SNR estimate, and the ERB gains
/// and deep-filter coefficients (`[bin][tap]`) when it asks for them.
pub(crate) struct Inference {
    lsnr: f32,
    gains: Option<Vec<f32>>,
    coefs: Option<Vec<Complex32>>,
}

/// DeepFilterNet3 on 48 kHz mono 10 ms frames.
pub struct DeepFilter {
    cfg: Config,
    stft: Stft,
    features: Features,
    graphs: Graphs,
    #[cfg(feature = "onnxruntime")]
    gpu: Option<stateful::StatefulGraph>,
    /// A GPU graph given up on, kept until the filter is dropped (off the
    /// processing thread, see `crate::Enhancer`): tearing down a GPU
    /// session there would stall the audio.
    #[cfg(feature = "onnxruntime")]
    _abandoned: Option<stateful::StatefulGraph>,
    #[cfg(feature = "onnxruntime")]
    deadline: Deadline,
    status: DeepFilterStatus,
    spec: Vec<Complex32>,
    erb: Vec<f32>,
    cplx: Vec<Complex32>,
    /// `cplx` as the networks take it: real parts, then imaginary ones.
    feat_spec: Vec<f32>,
    /// Spectra for the ERB gains: the target frame is `df_order - 1`.
    enhanced: VecDeque<Vec<Complex32>>,
    /// Noisy spectra for the deep filter, oldest first.
    noisy: VecDeque<Vec<Complex32>>,
    out: Vec<Complex32>,
    lookahead: usize,
    mix_back: Option<f32>,
    lsnr: f32,
}

/// DeepFilterNet3's networks, compiled once: every [`DeepFilter`] made from
/// it (one per voice) shares them and keeps only its own state.
#[derive(Clone)]
pub struct DeepFilterModel {
    cfg: Config,
    plans: Arc<model::Plans>,
}

impl DeepFilterModel {
    /// Loads and compiles the models from `dir`. Slow (seconds in a debug
    /// build): call it off the audio thread.
    pub fn load(dir: &Path) -> Result<Self, AudioError> {
        let text =
            std::fs::read_to_string(dir.join("config.ini")).map_err(|_| AudioError::Model)?;
        let cfg = Config::parse(&text)?;
        let plans = Arc::new(model::load(dir, &cfg)?);
        Ok(Self { cfg, plans })
    }

    /// A filter with its own state, on the CPU. Cheap next to [`load`]
    /// (no compilation), but it allocates: not on the audio thread.
    ///
    /// [`load`]: Self::load
    pub fn filter(&self, attenuation_limit_db: f32) -> Result<DeepFilter, AudioError> {
        let cfg = self.cfg.clone();
        let graphs = self.plans.spawn()?;
        let stft = Stft::new(960, FRAME_LEN);
        let bins = stft.bins();
        let widths = dsp::erb_widths(48_000, 960, cfg.nb_erb, cfg.min_nb_erb_freqs);
        let alpha = dsp::norm_alpha(48_000, FRAME_LEN, cfg.norm_tau);
        let lookahead = cfg.conv_lookahead.max(cfg.df_lookahead);
        let zeros = || vec![Complex32::default(); bins];
        Ok(DeepFilter {
            features: Features::new(widths, cfg.nb_df, alpha),
            graphs,
            #[cfg(feature = "onnxruntime")]
            gpu: None,
            #[cfg(feature = "onnxruntime")]
            _abandoned: None,
            #[cfg(feature = "onnxruntime")]
            deadline: Deadline::default(),
            status: DeepFilterStatus::new(DeepFilterBackend::Cpu),
            spec: zeros(),
            erb: vec![0.0; cfg.nb_erb],
            cplx: vec![Complex32::default(); cfg.nb_df],
            feat_spec: vec![0.0; 2 * cfg.nb_df],
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
}

impl DeepFilter {
    /// Loads the models from `dir` for one filter (see [`DeepFilterModel`]
    /// to make several). Slow (seconds in a debug build): call it off the
    /// audio thread.
    pub fn load(dir: &Path, attenuation_limit_db: f32) -> Result<Self, AudioError> {
        DeepFilterModel::load(dir)?.filter(attenuation_limit_db)
    }

    /// Loads the models from `dir` (as [`load`]) and the stateful graph
    /// `stateful_model` on the first of `prefer` that starts, then warms it
    /// up. Returns the filter and the accelerator it runs on; `None` when
    /// none started or the warmed-up GPU is still too slow (the
    /// [`status`] says which), and the CPU graphs run. Slower than [`load`]
    /// (seconds the first time on a machine): call it off the audio thread.
    ///
    /// [`load`]: Self::load
    /// [`status`]: Self::status
    #[cfg(feature = "onnxruntime")]
    pub fn load_gpu(
        dir: &Path,
        stateful_model: &Path,
        attenuation_limit_db: f32,
        runtime: &crate::onnx::OnnxRuntime,
        prefer: &[crate::onnx::Accelerator],
    ) -> Result<(Self, Option<crate::onnx::Accelerator>), AudioError> {
        let mut filter = Self::load(dir, attenuation_limit_db)?;
        let (mut graph, accelerator) =
            stateful::StatefulGraph::open(runtime, stateful_model, &filter.cfg, prefer)?;
        let Some(accelerator) = accelerator else {
            tracing::info!("no GPU backend started; DeepFilterNet runs on the CPU");
            return Ok((filter, None));
        };
        let (erb, spec) = (
            vec![0.0; filter.cfg.nb_erb],
            vec![0.0; 2 * filter.cfg.nb_df],
        );
        let mut times = Vec::with_capacity(GPU_WARM_UP_FRAMES);
        for _ in 0..GPU_WARM_UP_FRAMES {
            let start = std::time::Instant::now();
            graph.infer(&filter.cfg, &erb, &spec)?;
            times.push(start.elapsed());
        }
        graph.reset()?;
        let first = times[0];
        let mut settled = times[GPU_WARM_UP_FRAMES / 2..].to_vec();
        settled.sort();
        let median = settled[settled.len() / 2];
        tracing::info!(
            "DeepFilterNet on {accelerator:?}: first frame {first:?}, then a median of {median:?}"
        );
        if median > GPU_DEADLINE {
            filter.status.set(DeepFilterBackend::CpuAfterGpuTooSlow);
            return Ok((filter, None));
        }
        filter.gpu = Some(graph);
        filter.status.set(DeepFilterBackend::Gpu);
        Ok((filter, Some(accelerator)))
    }

    /// Which of `candidates` run the stateful graph here: each is opened
    /// and runs one frame. Slow (a second or so with CUDA); call it off the
    /// audio thread, and not while a GPU filter runs (a CUDA graph capture
    /// forbids other threads' CUDA calls).
    #[cfg(feature = "onnxruntime")]
    pub fn gpu_backends(
        dir: &Path,
        stateful_model: &Path,
        runtime: &crate::onnx::OnnxRuntime,
        candidates: &[crate::onnx::Accelerator],
    ) -> Vec<crate::onnx::Accelerator> {
        let Ok(text) = std::fs::read_to_string(dir.join("config.ini")) else {
            return Vec::new();
        };
        let Ok(cfg) = Config::parse(&text) else {
            return Vec::new();
        };
        let (erb, spec) = (vec![0.0; cfg.nb_erb], vec![0.0; 2 * cfg.nb_df]);
        candidates
            .iter()
            .copied()
            .filter(|&candidate| {
                match stateful::StatefulGraph::open(runtime, stateful_model, &cfg, &[candidate]) {
                    Ok((mut graph, Some(started))) if started == candidate => {
                        graph.infer(&cfg, &erb, &spec).is_ok()
                    }
                    _ => false,
                }
            })
            .collect()
    }

    /// Where the networks run; the handle follows a fallback to the CPU
    /// while the filter runs.
    pub fn status(&self) -> DeepFilterStatus {
        self.status.clone()
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
        let Inference { lsnr, gains, coefs } = self.infer()?;
        self.lsnr = lsnr;

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

    /// Runs the networks on the current features, on the GPU while it
    /// keeps up, else on the CPU.
    fn infer(&mut self) -> Result<Inference, AudioError> {
        let n = self.cfg.nb_df;
        for (i, c) in self.cplx.iter().enumerate() {
            self.feat_spec[i] = c.re;
            self.feat_spec[n + i] = c.im;
        }
        #[cfg(feature = "onnxruntime")]
        if let Some(gpu) = self.gpu.as_mut() {
            let start = std::time::Instant::now();
            match gpu.infer(&self.cfg, &self.erb, &self.feat_spec) {
                Ok(inference) => {
                    let took = start.elapsed();
                    if self.deadline.gives_up(took) {
                        tracing::warn!(
                            "DeepFilterNet's GPU missed {GPU_MISSES} deadlines in a row \
                             (last frame {took:?}); the CPU takes over"
                        );
                        self._abandoned = self.gpu.take();
                        self.status.set(DeepFilterBackend::CpuAfterGpuTooSlow);
                    }
                    return Ok(inference);
                }
                Err(_) => {
                    tracing::warn!("DeepFilterNet's GPU failed; the CPU takes over");
                    self._abandoned = self.gpu.take();
                    self.status.set(DeepFilterBackend::CpuAfterGpuError);
                }
            }
        }
        self.graphs.infer(&self.cfg, &self.erb, &self.feat_spec)
    }
}

impl crate::Enhancer for DeepFilter {
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        self.process(frame)
    }

    fn latency_samples(&self) -> usize {
        DeepFilter::latency_samples(self)
    }

    fn set_attenuation_limit(&mut self, db: f32) {
        DeepFilter::set_attenuation_limit(self, db);
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

    #[cfg(feature = "onnxruntime")]
    #[test]
    fn the_gpu_is_given_up_after_missed_deadlines_in_a_row_only() {
        use std::time::Duration;
        let (late, on_time) = (GPU_DEADLINE + Duration::from_micros(1), GPU_DEADLINE);
        let mut deadline = Deadline::default();
        assert!(!deadline.gives_up(late));
        assert!(!deadline.gives_up(late));
        assert!(
            !deadline.gives_up(on_time),
            "a frame in time resets the count"
        );
        assert!(!deadline.gives_up(late));
        assert!(!deadline.gives_up(late));
        assert!(deadline.gives_up(late), "{GPU_MISSES} in a row");
    }

    #[test]
    fn the_status_reads_back_what_was_set() {
        let status = DeepFilterStatus::new(DeepFilterBackend::Gpu);
        let reader = status.clone();
        for backend in [
            DeepFilterBackend::Cpu,
            DeepFilterBackend::CpuAfterGpuError,
            DeepFilterBackend::CpuAfterGpuTooSlow,
            DeepFilterBackend::Gpu,
        ] {
            status.set(backend);
            assert_eq!(reader.get(), backend);
        }
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
