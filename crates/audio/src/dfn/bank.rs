//! DeepFilterNet3 for several voices at once on a GPU (plan Task 16): one
//! batched stateful graph, one call per frame for every voice, each voice in
//! a row with its own state and its own signal side. Rows without a voice
//! run on silence; the call costs about the same whoever talks.

use std::path::Path;
use std::time::{Duration, Instant};

use super::config::Config;
use super::gpu::{Deadline, GPU_MISSES, hold_webgpu, warm_up};
use super::signal::Dsp;
use super::stateful::{StatefulGraph, read_output};
use super::{DeepFilterBackend, DeepFilterStatus};
use crate::AudioError;
use crate::frame::Frame;
use crate::onnx::{Accelerator, OnnxRuntime};

/// See the module docs.
pub struct DeepFilterBank {
    cfg: Config,
    graph: StatefulGraph,
    rows: Vec<Dsp>,
    /// Rows to start afresh on the GPU before the next call (done there, as
    /// a reset may find the GPU busy).
    pending_resets: Vec<bool>,
    deadline: Deadline,
    accelerator: Accelerator,
    /// The median call at warm-up.
    call: Duration,
    /// `Gpu` until it gives up; then why.
    status: DeepFilterStatus,
}

impl DeepFilterBank {
    /// Opens the stateful graph `stateful_model` for `rows` voices on the
    /// first of `prefer` that starts, and warms it up; the configuration
    /// comes from the CPU models' `dir`. `Ok(None)` when no GPU started, or
    /// the warmed-up GPU is too slow for the deadline: per-voice CPU filters
    /// are then the way. Slow (seconds; TensorRT's first use on a machine
    /// builds an engine): call it off the audio thread.
    pub fn load(
        dir: &Path,
        stateful_model: &Path,
        rows: usize,
        attenuation_limit_db: f32,
        runtime: &OnnxRuntime,
        prefer: &[Accelerator],
    ) -> Result<Option<Self>, AudioError> {
        let _webgpu = hold_webgpu(prefer);
        let cfg = Config::load(dir)?;
        let (mut graph, accelerator) =
            StatefulGraph::open(runtime, stateful_model, &cfg, prefer, rows)?;
        let Some(accelerator) = accelerator else {
            tracing::info!("no GPU backend started for cleaning other voices");
            return Ok(None);
        };
        let warm = warm_up(|| graph.run().map(drop))?;
        graph.reset()?;
        tracing::info!(
            "DeepFilterNet for {rows} voices on {accelerator:?}: first call {:?}, then a median of {:?}",
            warm.first,
            warm.median
        );
        if !warm.keeps_up() {
            return Ok(None);
        }
        Ok(Some(Self {
            rows: (0..rows)
                .map(|_| Dsp::new(&cfg, attenuation_limit_db))
                .collect(),
            pending_resets: vec![false; rows],
            cfg,
            graph,
            deadline: Deadline::default(),
            accelerator,
            call: warm.median,
            status: DeepFilterStatus::new(DeepFilterBackend::Gpu),
        }))
    }

    /// Where it runs.
    pub fn accelerator(&self) -> Accelerator {
        self.accelerator
    }

    /// The median time of a call at warm-up (every row).
    pub fn call_time(&self) -> Duration {
        self.call
    }

    /// `Gpu` while it runs; once it gave up (and the engine dropped it),
    /// `CpuAfterGpuError` or `CpuAfterGpuTooSlow`.
    pub fn status(&self) -> DeepFilterStatus {
        self.status.clone()
    }

    /// Starts the rows waiting for it afresh on the GPU. `Ok(false)`: the
    /// GPU is busy this frame (the rest stay pending).
    fn apply_resets(&mut self) -> Result<bool, AudioError> {
        for (row, pending) in self.pending_resets.iter_mut().enumerate() {
            if *pending {
                match self.graph.reset_row(row) {
                    Ok(()) => *pending = false,
                    Err(AudioError::GpuBusy) => return Ok(false),
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(true)
    }
}

impl crate::BatchCleaner for DeepFilterBank {
    fn reset_row(&mut self, row: usize) -> Result<(), AudioError> {
        self.rows.get_mut(row).ok_or(AudioError::Internal)?.reset();
        self.pending_resets[row] = true;
        Ok(())
    }

    /// While another thread holds the GPU (WebGPU, `crate::onnx`), frames
    /// pass as they came.
    fn clean(&mut self, frames: &mut [Frame], rows: &[usize]) -> Result<(), AudioError> {
        match self.apply_resets() {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(e) => {
                self.status.set(DeepFilterBackend::CpuAfterGpuError);
                return Err(e);
            }
        }
        self.graph.clear_inputs();
        for (frame, &row) in frames.iter().zip(rows) {
            let dsp = self.rows.get_mut(row).ok_or(AudioError::Internal)?;
            dsp.analyse(&self.cfg, frame);
            self.graph.set_input(row, &self.cfg, &dsp.net);
        }
        let start = Instant::now();
        let outputs = match self.graph.run() {
            Ok(outputs) => outputs,
            Err(AudioError::GpuBusy) => return Ok(()),
            Err(e) => {
                tracing::warn!("cleaning other voices on the GPU failed");
                self.status.set(DeepFilterBackend::CpuAfterGpuError);
                return Err(e);
            }
        };
        let took = start.elapsed();
        if self.deadline.gives_up(took) {
            tracing::warn!(
                "cleaning other voices on the GPU missed {GPU_MISSES} deadlines in a row \
                 (last call {took:?})"
            );
            self.status.set(DeepFilterBackend::CpuAfterGpuTooSlow);
            return Err(AudioError::Internal);
        }
        let per_row = outputs.len() / self.rows.len();
        for (frame, &row) in frames.iter_mut().zip(rows) {
            let dsp = &mut self.rows[row];
            let out = &outputs[row * per_row..(row + 1) * per_row];
            if let Err(e) = read_output(&self.cfg, out, &mut dsp.net) {
                self.status.set(DeepFilterBackend::CpuAfterGpuError);
                return Err(e);
            }
            dsp.synthesise(&self.cfg, frame);
        }
        Ok(())
    }

    fn set_attenuation_limit(&mut self, db: f32) {
        for dsp in &mut self.rows {
            dsp.set_attenuation_limit(db);
        }
    }
}
