//! DeepFilterNet3 as one stateful graph on ONNX Runtime (feature
//! `onnxruntime`, plan Task 13b): `scripts/export-deepfilternet.sh` exports
//! the three networks as one graph that runs one frame, with the GRU states
//! and the time history of its convolutions packed into one state vector,
//! carried from call to call by [`OnnxLoop`] (on the GPU itself with CUDA).
//!
//! The graph selects the next decoder states as upstream's runtime advances
//! them (a decoder whose output a frame does not use keeps its state), with
//! the same local SNR thresholds as the CPU graphs, passed in once.

use std::path::Path;

use super::config::Config;
use super::model::coefficients;
use super::{Inference, MAX_DB_DF, MAX_DB_ERB, MIN_DB, stages};
use crate::AudioError;
use crate::onnx::{Accelerator, LoopSpec, OnnxLoop, OnnxRuntime};

/// The stateful graph and where it runs.
pub(crate) struct StatefulGraph {
    model: OnnxLoop,
    /// The ERB features, then the complex ones (real parts, imaginary ones).
    features: Vec<f32>,
}

impl StatefulGraph {
    /// Opens `model`, trying `prefer` in order, then the CPU. Returns the
    /// graph and the accelerator it runs on (`None`: the CPU).
    pub(crate) fn open(
        runtime: &OnnxRuntime,
        model: &Path,
        cfg: &Config,
        prefer: &[Accelerator],
    ) -> Result<(Self, Option<Accelerator>), AudioError> {
        let inputs = cfg.nb_erb + 2 * cfg.nb_df;
        let outputs = cfg.nb_erb + cfg.nb_df * cfg.df_order * 2 + 1;
        let spec = LoopSpec {
            input: ("features", inputs),
            output: ("out", outputs),
            constants: &[("thresholds", &[MIN_DB, MAX_DB_ERB, MAX_DB_DF])],
            state: ("state", "state_out"),
            dimensions: &[("batch", 1)],
        };
        let (model, accelerator) = runtime.open_loop(model, prefer, &spec)?;
        tracing::info!(
            "DeepFilterNet's stateful graph on {accelerator:?} (CUDA graphs: {})",
            model.uses_cuda_graphs()
        );
        Ok((
            Self {
                model,
                features: vec![0.0; inputs],
            },
            accelerator,
        ))
    }

    /// Back to the start (silence before the first frame).
    pub(crate) fn reset(&mut self) -> Result<(), AudioError> {
        self.model.reset()
    }

    /// Runs one frame (`spec`: the real parts, then the imaginary ones).
    pub(crate) fn infer(
        &mut self,
        cfg: &Config,
        erb: &[f32],
        spec: &[f32],
    ) -> Result<Inference, AudioError> {
        let (head, tail) = self.features.split_at_mut(cfg.nb_erb);
        head.copy_from_slice(erb);
        tail.copy_from_slice(spec);
        let out = self.model.run(&self.features)?;
        let (gains, rest) = out.split_at(cfg.nb_erb);
        let (coefs, lsnr) = rest.split_at(rest.len() - 1);
        let lsnr = lsnr[0];
        let stage = stages(lsnr);
        Ok(Inference {
            lsnr,
            gains: if stage.gains {
                Some(gains.to_vec())
            } else if stage.zero_gains {
                Some(vec![0.0; cfg.nb_erb])
            } else {
                None
            },
            coefs: if stage.deep_filter {
                Some(coefficients(cfg, coefs)?)
            } else {
                None
            },
        })
    }
}
