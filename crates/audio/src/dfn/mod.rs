//! DeepFilterNet3 noise suppression: our STFT and features ([`dsp`],
//! [`signal`]) around the three pulsed networks ([`model`]), run one 10 ms
//! frame at a time ([`filter`]). With the `onnxruntime` feature the
//! networks can run on a GPU instead, as one stateful graph ([`stateful`]);
//! the CPU graphs stay loaded and take over if the GPU fails or falls behind
//! ([`gpu`]). [`DeepFilterBank`] runs several voices through one batched
//! graph, one GPU call per frame for all ([`bank`]).
//!
//! The models are fetched by `scripts/fetch-models.sh` (pinned SHA-256)
//! into the cache dir, or found in `TESSERA_MODEL_DIR`. Loading takes a
//! while (the graphs are optimised for this CPU), so it happens off the
//! audio thread; the loaded filter is then handed over.

#[cfg(feature = "onnxruntime")]
mod bank;
mod config;
mod dsp;
mod filter;
#[cfg(feature = "onnxruntime")]
mod gpu;
mod model;
mod signal;
#[cfg(feature = "onnxruntime")]
mod stateful;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

#[cfg(feature = "onnxruntime")]
pub use bank::DeepFilterBank;
pub use filter::{DeepFilter, DeepFilterModel};

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
    crate::paths::from_env_or_cache("TESSERA_MODEL_DIR", "models/deepfilternet3")
}

/// The stateful graph for the GPU (`scripts/export-deepfilternet.sh`):
/// `TESSERA_STATEFUL_MODEL`, else where that script installs it.
pub fn default_stateful_model() -> Option<PathBuf> {
    crate::paths::from_env_or_cache(
        "TESSERA_STATEFUL_MODEL",
        "models/deepfilternet3-stateful/deepfilternet3_stateful.onnx",
    )
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

/// Where a running [`DeepFilter`] (or [`DeepFilterBank`]) is, readable from
/// other threads.
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
