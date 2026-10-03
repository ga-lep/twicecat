//! GPU speech enhancement (plan Task 13b; cargo feature `gpu-enhancement`):
//! which GPU backends run DeepFilterNet3 here, and loading it on one. Off by
//! default; without the feature nothing is available and a `gpu` setting
//! runs on the CPU.

use std::path::Path;
use std::sync::Mutex;

use tessera_audio::{DeepFilter, DeepFilterBackend};

use crate::api::{EnhancerBackend, GpuBackend};

/// What the probe found, once per process (also filled by a GPU load that
/// started, so a probe never runs next to a running GPU filter).
static AVAILABLE: Mutex<Option<Vec<GpuBackend>>> = Mutex::new(None);

/// The backends found so far, if probed.
pub(crate) fn probed() -> Option<Vec<GpuBackend>> {
    AVAILABLE.lock().ok().and_then(|a| a.clone())
}

/// Where a running filter is, for the API.
pub(crate) fn running(backend: DeepFilterBackend, gpu: Option<GpuBackend>) -> EnhancerBackend {
    match (backend, gpu) {
        (DeepFilterBackend::Gpu, Some(backend)) => EnhancerBackend::Gpu { backend },
        (DeepFilterBackend::CpuAfterGpuError, _) => EnhancerBackend::CpuAfterGpuError,
        (DeepFilterBackend::CpuAfterGpuTooSlow, _) => EnhancerBackend::CpuAfterGpuTooSlow,
        _ => EnhancerBackend::Cpu,
    }
}

#[cfg(feature = "gpu-enhancement")]
mod imp {
    use super::*;
    use tessera_audio::onnx::{Accelerator, OnnxRuntime, default_runtime_dir};

    /// Tried in this order. TensorRT first: on an RTX 5090 it runs a frame in
    /// 0.13 ms for 16 µs of CPU, against 0.36 ms and 76 µs with CUDA. Its
    /// first use on a machine builds an engine for the GPU (about 20 s,
    /// cached after); without its libraries it fails at once and CUDA runs.
    const PREFER: [Accelerator; 4] = [
        Accelerator::TensorRt,
        Accelerator::Cuda,
        Accelerator::DirectMl,
        Accelerator::CoreMl,
    ];

    fn backend(accelerator: Accelerator) -> GpuBackend {
        match accelerator {
            Accelerator::Cuda => GpuBackend::Cuda,
            Accelerator::TensorRt => GpuBackend::TensorRt,
            Accelerator::DirectMl => GpuBackend::DirectMl,
            Accelerator::CoreMl => GpuBackend::CoreMl,
        }
    }

    fn runtime() -> Option<OnnxRuntime> {
        OnnxRuntime::load(&default_runtime_dir()?).ok()
    }

    fn remember(backends: &[GpuBackend]) {
        if let Ok(mut available) = AVAILABLE.lock() {
            let list = available.get_or_insert_with(Vec::new);
            for backend in backends {
                if !list.contains(backend) {
                    list.push(*backend);
                }
            }
        }
    }

    /// Loads and probes one at a time: a CUDA graph capture in one forbids
    /// CUDA calls in the other.
    static GPU_WORK: Mutex<()> = Mutex::new(());

    /// Opens each candidate backend once (blocking, about a second).
    pub(crate) fn probe(models: &Path) -> Vec<GpuBackend> {
        let _work = GPU_WORK.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = probed() {
            return found;
        }
        let found: Vec<GpuBackend> = match (runtime(), tessera_audio::default_stateful_model()) {
            (Some(runtime), Some(model)) if model.exists() => {
                DeepFilter::gpu_backends(models, &model, &runtime, &PREFER)
                    .into_iter()
                    .map(backend)
                    .collect()
            }
            _ => Vec::new(),
        };
        if let Ok(mut available) = AVAILABLE.lock() {
            *available = Some(found.clone());
        }
        found
    }

    /// Loads DeepFilterNet3 on the first backend that starts (blocking).
    /// `Ok` with no backend: the CPU runs (no GPU started, or it was too
    /// slow); `Err(reason)` when the GPU parts are missing.
    pub(crate) fn load(
        models: &Path,
        attenuation_limit_db: f32,
    ) -> Result<(DeepFilter, Option<GpuBackend>), String> {
        let _work = GPU_WORK.lock().unwrap_or_else(|e| e.into_inner());
        let runtime = runtime()
            .ok_or("ONNX Runtime is not installed (scripts/fetch-onnxruntime.sh cuda13)")?;
        let model = tessera_audio::default_stateful_model()
            .filter(|m| m.exists())
            .ok_or("the GPU model is not installed (scripts/export-deepfilternet.sh)")?;
        let (filter, accelerator) =
            DeepFilter::load_gpu(models, &model, attenuation_limit_db, &runtime, &PREFER)
                .map_err(|e: tessera_audio::AudioError| e.to_string())?;
        let gpu = accelerator.map(backend);
        if let Some(gpu) = gpu {
            remember(&[gpu]);
        }
        Ok((filter, gpu))
    }
}

#[cfg(not(feature = "gpu-enhancement"))]
mod imp {
    use super::*;

    pub(crate) fn probe(_models: &Path) -> Vec<GpuBackend> {
        Vec::new()
    }

    pub(crate) fn load(
        _models: &Path,
        _attenuation_limit_db: f32,
    ) -> Result<(DeepFilter, Option<GpuBackend>), String> {
        Err("this build has no GPU support (cargo feature gpu-enhancement)".into())
    }
}

pub(crate) use imp::{load, probe};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_backend_maps_onto_the_api() {
        assert_eq!(
            running(DeepFilterBackend::Gpu, Some(GpuBackend::Cuda)),
            EnhancerBackend::Gpu {
                backend: GpuBackend::Cuda
            }
        );
        assert_eq!(running(DeepFilterBackend::Cpu, None), EnhancerBackend::Cpu);
        assert_eq!(
            running(
                DeepFilterBackend::CpuAfterGpuTooSlow,
                Some(GpuBackend::Cuda)
            ),
            EnhancerBackend::CpuAfterGpuTooSlow
        );
        assert_eq!(
            running(DeepFilterBackend::CpuAfterGpuError, Some(GpuBackend::Cuda)),
            EnhancerBackend::CpuAfterGpuError
        );
    }
}
