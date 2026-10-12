//! One voice's DeepFilterNet3: the networks on the CPU (tract), or, with
//! the `onnxruntime` feature, as one stateful graph on a GPU, the CPU
//! graphs staying loaded to take over if the GPU fails or falls behind. A
//! GPU that fell behind is tried again later (see `super::gpu::Retry`); one
//! that failed is not.

use std::path::Path;
use std::sync::Arc;

use super::config::Config;
use super::model::{self, Graphs};
use super::signal::Dsp;
use super::{DeepFilterBackend, DeepFilterStatus};
use crate::AudioError;
use crate::frame::Frame;

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
        let cfg = Config::load(dir)?;
        let plans = Arc::new(model::load(dir, &cfg)?);
        Ok(Self { cfg, plans })
    }

    /// A filter with its own state, on the CPU. Cheap next to [`load`]
    /// (no compilation), but it allocates: not on the audio thread.
    ///
    /// [`load`]: Self::load
    pub fn filter(&self, attenuation_limit_db: f32) -> Result<DeepFilter, AudioError> {
        Ok(DeepFilter {
            dsp: Dsp::new(&self.cfg, attenuation_limit_db),
            graphs: self.plans.spawn()?,
            #[cfg(feature = "onnxruntime")]
            gpu: None,
            status: DeepFilterStatus::new_internal(DeepFilterBackend::Cpu),
            cfg: self.cfg.clone(),
        })
    }
}

/// The GPU side of a [`DeepFilter`].
#[cfg(feature = "onnxruntime")]
struct OnGpu {
    graph: super::stateful::StatefulGraph,
    deadline: super::gpu::Deadline,
    /// Given up on: kept until the filter is dropped (off the processing
    /// thread, see `crate::Enhancer`), as tearing down a GPU session there
    /// would stall the audio.
    abandoned: bool,
    /// When to try it again after it fell behind.
    retry: super::gpu::Retry,
}

/// DeepFilterNet3 on 48 kHz mono 10 ms frames.
pub struct DeepFilter {
    cfg: Config,
    dsp: Dsp,
    graphs: Graphs,
    #[cfg(feature = "onnxruntime")]
    gpu: Option<OnGpu>,
    status: DeepFilterStatus,
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
        use super::gpu::{Deadline, hold_webgpu, warm_up};
        use super::stateful::StatefulGraph;

        let _webgpu = hold_webgpu(prefer);
        let mut filter = Self::load(dir, attenuation_limit_db)?;
        let (mut graph, accelerator) =
            StatefulGraph::open(runtime, stateful_model, &filter.cfg, prefer, 1)?;
        let Some(accelerator) = accelerator else {
            tracing::info!("no GPU backend started; DeepFilterNet runs on the CPU");
            return Ok((filter, None));
        };
        let cfg = &filter.cfg;
        let net = &mut filter.dsp.net;
        let warm = warm_up(|| graph.infer(cfg, net))?;
        graph.reset()?;
        filter.dsp.net.lsnr = 0.0;
        tracing::info!(
            "DeepFilterNet on {accelerator:?}: first frame {:?}, then a median of {:?}",
            warm.first,
            warm.median
        );
        if !warm.keeps_up() {
            filter
                .status
                .set_internal(DeepFilterBackend::CpuAfterGpuTooSlow);
            return Ok((filter, None));
        }
        filter.gpu = Some(OnGpu {
            graph,
            deadline: Deadline::default(),
            abandoned: false,
            retry: Default::default(),
        });
        filter.status.set_internal(DeepFilterBackend::Gpu);
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
        use super::gpu::hold_webgpu;
        use super::signal::NetIo;
        use super::stateful::StatefulGraph;

        let Ok(cfg) = Config::load(dir) else {
            return Vec::new();
        };
        let mut net = NetIo::new(&cfg);
        candidates
            .iter()
            .copied()
            .filter(|&candidate| {
                let _webgpu = hold_webgpu(&[candidate]);
                match StatefulGraph::open(runtime, stateful_model, &cfg, &[candidate], 1) {
                    Ok((mut graph, Some(started))) if started == candidate => {
                        graph.infer(&cfg, &mut net).is_ok()
                    }
                    _ => false,
                }
            })
            .collect()
    }

    /// Where the networks run; the handle follows a fallback to the CPU,
    /// and a return to the GPU, while the filter runs.
    pub fn status(&self) -> DeepFilterStatus {
        self.status.clone()
    }

    /// Hands the frames to the CPU as if the GPU had fallen behind, with
    /// its next try due at once. Only with the `test-fakes` feature, for
    /// the GPU tests; not part of the audio crate's API.
    #[cfg(all(feature = "onnxruntime", feature = "test-fakes"))]
    pub fn fall_behind(&mut self) {
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.abandoned = true;
            gpu.retry = super::gpu::Retry::due_at(std::time::Instant::now());
            self.status
                .set_internal(DeepFilterBackend::CpuAfterGpuTooSlow);
        }
    }

    /// How late the output is, in samples: one hop for the STFT, plus the
    /// networks' lookahead.
    pub fn latency_samples(&self) -> usize {
        self.dsp.latency_samples()
    }

    /// Most attenuation applied, dB (100 or more: no limit).
    pub fn set_attenuation_limit(&mut self, db: f32) {
        self.dsp.set_attenuation_limit(db);
    }

    /// Cleans one frame in place (it comes out [`latency_samples`] late).
    ///
    /// [`latency_samples`]: Self::latency_samples
    pub fn process(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        self.dsp.analyse(&self.cfg, frame);
        self.infer()?;
        self.dsp.synthesise(&self.cfg, frame);
        Ok(())
    }

    /// Runs the networks on the current features, on the GPU while it
    /// keeps up, else on the CPU.
    fn infer(&mut self) -> Result<(), AudioError> {
        #[cfg(feature = "onnxruntime")]
        if let Some(gpu) = self.gpu.as_mut() {
            use super::gpu::{GPU_LATE_LIMIT, GPU_WINDOW};
            let now = std::time::Instant::now();
            if gpu.abandoned && gpu.retry.due(now) {
                // From a clean state: the GPU's is as old as when it was
                // given up on. A failed reset waits for the next try.
                if gpu.graph.reset().is_ok() {
                    tracing::info!("DeepFilterNet tries the GPU again");
                    gpu.deadline.reset();
                    gpu.abandoned = false;
                    self.status.set_internal(DeepFilterBackend::Gpu);
                } else {
                    gpu.retry.gave_up(now);
                }
            }
            if !gpu.abandoned {
                let start = std::time::Instant::now();
                match gpu.graph.infer(&self.cfg, &mut self.dsp.net) {
                    // Another thread holds the GPU this frame: the CPU runs it.
                    Err(AudioError::GpuBusy) => {}
                    Ok(()) => {
                        let took = start.elapsed();
                        if gpu.deadline.gives_up(took) {
                            tracing::warn!(
                                "DeepFilterNet's GPU missed its deadline in {GPU_LATE_LIMIT} of \
                                 the last {GPU_WINDOW} frames (last frame {took:?}); the CPU \
                                 takes over and the GPU is tried again later"
                            );
                            gpu.abandoned = true;
                            gpu.retry.gave_up(now);
                            self.status
                                .set_internal(DeepFilterBackend::CpuAfterGpuTooSlow);
                        }
                        return Ok(());
                    }
                    Err(_) => {
                        tracing::warn!("DeepFilterNet's GPU failed; the CPU takes over");
                        gpu.abandoned = true;
                        self.status
                            .set_internal(DeepFilterBackend::CpuAfterGpuError);
                    }
                }
            }
        }
        self.graphs.infer(&self.cfg, &mut self.dsp.net)
    }
}

impl crate::Enhancer for DeepFilter {
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        self.process(frame)
    }

    fn set_attenuation_limit(&mut self, db: f32) {
        DeepFilter::set_attenuation_limit(self, db);
    }
}
