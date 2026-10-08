//! DeepFilterNet3 as one stateful graph on ONNX Runtime (feature
//! `onnxruntime`, plan Task 13b): `scripts/export-deepfilternet.sh` exports
//! the three networks as one graph that runs one frame, with the GRU states
//! and the time history of its convolutions packed into one state vector,
//! carried from call to call by [`OnnxLoop`] (on the GPU itself with CUDA).
//!
//! The graph selects the next decoder states as upstream's runtime advances
//! them (a decoder whose output a frame does not use keeps its state), with
//! the same local SNR thresholds as the CPU graphs, passed in once.
//!
//! Its batch runs several voices at once, each row with its own state (plan
//! Task 16, [`super::DeepFilterBank`]).

use std::path::Path;

use super::config::Config;
use super::signal::{NetIo, coefficient_len};
use super::{MAX_DB_DF, MAX_DB_ERB, MIN_DB};
use crate::AudioError;
use crate::onnx::{Accelerator, LoopSpec, OnnxLoop, OnnxRuntime};

/// The stateful graph and where it runs.
pub(crate) struct StatefulGraph {
    model: OnnxLoop,
    /// Every row's input: the ERB features, then the complex ones (real
    /// parts, imaginary ones).
    features: Vec<f32>,
    /// Per row.
    inputs: usize,
    outputs: usize,
}

impl StatefulGraph {
    /// Opens `model` for `rows` voices at once, trying `prefer` in order,
    /// then the CPU. Returns the graph and the accelerator it runs on
    /// (`None`: the CPU).
    pub(crate) fn open(
        runtime: &OnnxRuntime,
        model: &Path,
        cfg: &Config,
        prefer: &[Accelerator],
        rows: usize,
    ) -> Result<(Self, Option<Accelerator>), AudioError> {
        let inputs = cfg.nb_erb + 2 * cfg.nb_df;
        let outputs = cfg.nb_erb + coefficient_len(cfg) + 1;
        let batch = [(
            "batch",
            i64::try_from(rows).map_err(|_| AudioError::Internal)?,
        )];
        let spec = LoopSpec {
            input: ("features", inputs),
            output: ("out", outputs),
            constants: &[("thresholds", &[MIN_DB, MAX_DB_ERB, MAX_DB_DF])],
            state: ("state", "state_out"),
            dimensions: &batch,
            rows,
        };
        let (model, accelerator) = runtime.open_loop(model, prefer, &spec)?;
        tracing::info!(
            "DeepFilterNet's stateful graph on {accelerator:?}, {rows} rows (replayed graph: {})",
            model.replays_a_graph()
        );
        Ok((
            Self {
                model,
                features: vec![0.0; rows * inputs],
                inputs,
                outputs,
            },
            accelerator,
        ))
    }

    /// Back to the start (silence before the first frame), every row.
    pub(crate) fn reset(&mut self) -> Result<(), AudioError> {
        self.model.reset()
    }

    /// Back to the start for one row.
    pub(crate) fn reset_row(&mut self, row: usize) -> Result<(), AudioError> {
        self.model.reset_row(row)
    }

    /// Sets one row's input from its features in `net`.
    pub(crate) fn set_input(&mut self, row: usize, cfg: &Config, net: &NetIo) {
        let input = &mut self.features[row * self.inputs..(row + 1) * self.inputs];
        let (head, tail) = input.split_at_mut(cfg.nb_erb);
        head.copy_from_slice(&net.erb);
        tail.copy_from_slice(&net.feat_spec);
    }

    /// Zeroes every row's input (rows without a voice run on silence).
    pub(crate) fn clear_inputs(&mut self) {
        self.features.fill(0.0);
    }

    /// Runs every row on its input; returns the outputs, one row's after
    /// the other (see [`read_output`]).
    pub(crate) fn run(&mut self) -> Result<&[f32], AudioError> {
        self.model.run(&self.features)
    }

    /// Runs one frame of row 0 (a single voice's graph) on `net`'s
    /// features, and writes the outputs there.
    pub(crate) fn infer(&mut self, cfg: &Config, net: &mut NetIo) -> Result<(), AudioError> {
        self.set_input(0, cfg, net);
        let outputs = self.outputs;
        let out = self.model.run(&self.features)?;
        read_output(cfg, &out[..outputs], net)
    }
}

/// Takes one row's output into `net`: the gains, the deep filter's
/// coefficients and the local SNR (each output only when the stages it
/// picks need it).
pub(crate) fn read_output(cfg: &Config, out: &[f32], net: &mut NetIo) -> Result<(), AudioError> {
    if out.len() != cfg.nb_erb + coefficient_len(cfg) + 1 {
        return Err(AudioError::Internal);
    }
    let (gains, rest) = out.split_at(cfg.nb_erb);
    let (coefs, lsnr) = rest.split_at(rest.len() - 1);
    net.lsnr = lsnr[0];
    let stages = net.stages();
    if stages.gains {
        net.gains.copy_from_slice(gains);
    }
    if stages.deep_filter {
        net.coefs.copy_from_slice(coefs);
    }
    Ok(())
}
