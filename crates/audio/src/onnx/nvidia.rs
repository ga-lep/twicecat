//! NVIDIA's libraries for the CUDA and TensorRT providers on Linux: from the
//! system, or from `scripts/fetch-cuda-runtime.sh` and
//! `scripts/fetch-tensorrt.sh` (or `TESSERA_CUDA_DIR`,
//! `TESSERA_TENSORRT_DIR`), preloaded before the runtime starts so its
//! providers find them without `LD_LIBRARY_PATH`. Also where TensorRT keeps
//! the engines it builds for this machine.

use std::path::{Path, PathBuf};

/// Where `scripts/fetch-cuda-runtime.sh` put NVIDIA's libraries, or
/// `TESSERA_CUDA_DIR`; `None` when neither exists (a system install is then
/// used, if any).
pub fn default_cuda_dir() -> Option<PathBuf> {
    let dir = crate::paths::from_env_or_cache("TESSERA_CUDA_DIR", "cuda/13.0/lib")?;
    dir.is_dir().then_some(dir)
}

/// Where `scripts/fetch-tensorrt.sh` put TensorRT, or
/// `TESSERA_TENSORRT_DIR`; `None` when neither exists (a system install is
/// then used, if any).
pub fn default_tensorrt_dir() -> Option<PathBuf> {
    let dir = crate::paths::from_env_or_cache("TESSERA_TENSORRT_DIR", "cuda/tensorrt-10.16.1/lib")?;
    dir.is_dir().then_some(dir)
}

/// Where TensorRT engines built for this machine are kept.
pub(super) fn engine_cache_dir() -> Option<PathBuf> {
    let dir = crate::paths::cache_dir()?.join("tensorrt-engines");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// NVIDIA's libraries the CUDA 13 provider loads, dependencies first (as
/// ONNX Runtime's own Python preloader orders them).
#[cfg(target_os = "linux")]
const CUDA_LIBRARIES: &[&str] = &[
    "libcudart.so.13",
    "libcublasLt.so.13",
    "libcublas.so.13",
    "libcurand.so.10",
    "libnvrtc.so.13",
    "libnvrtc-builtins.so.13.0",
    "libcudnn.so.9",
    "libcudnn_graph.so.9",
    "libcudnn_ops.so.9",
    "libcudnn_heuristic.so.9",
    "libcudnn_adv.so.9",
    "libcudnn_cnn.so.9",
    "libcudnn_engines_precompiled.so.9",
    "libcudnn_engines_runtime_compiled.so.9",
];

/// Preloads the CUDA and TensorRT libraries when the runtime in
/// `runtime_dir` has those providers and the libraries were fetched.
#[cfg(target_os = "linux")]
pub(super) fn preload(runtime_dir: &Path) {
    if runtime_dir
        .join("libonnxruntime_providers_cuda.so")
        .exists()
        && let Some(cuda) = default_cuda_dir()
    {
        preload_all(&cuda, CUDA_LIBRARIES.iter().map(|&n| n.to_owned()));
    }
    if runtime_dir
        .join("libonnxruntime_providers_tensorrt.so")
        .exists()
        && let Some(tensorrt) = default_tensorrt_dir()
    {
        preload_all(&tensorrt, tensorrt_libraries(&tensorrt));
    }
}

/// Nothing to preload elsewhere: the libraries are found by the system.
#[cfg(not(target_os = "linux"))]
pub(super) fn preload(_runtime_dir: &Path) {}

/// TensorRT's libraries, dependencies first; the engine builder's resources
/// (one per GPU architecture, found by name) go between the runtime and its
/// plugins, as NVIDIA's own loader loads everything in the directory.
#[cfg(target_os = "linux")]
fn tensorrt_libraries(dir: &Path) -> Vec<String> {
    let mut names = vec!["libnvinfer.so.10".to_owned()];
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut resources: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("libnvinfer_builder_resource_") && !n.contains("_win_"))
            .collect();
        resources.sort();
        names.extend(resources);
    }
    names.extend([
        "libnvinfer_plugin.so.10".into(),
        "libnvonnxparser.so.10".into(),
    ]);
    names
}

/// Loads `names` from `dir` in order, so that the providers' own loads find
/// them already in the process. Missing files are skipped (the provider
/// then fails to start and the next one, or the CPU, runs); a failing load
/// stops there.
#[cfg(target_os = "linux")]
fn preload_all(dir: &Path, names: impl IntoIterator<Item = String>) {
    for name in names {
        let path = dir.join(&name);
        if !path.exists() {
            tracing::debug!("{name} not in {}", dir.display());
            continue;
        }
        if let Err(e) = ort::util::preload_dylib(&path) {
            tracing::warn!("could not preload {name}: {e}");
            return;
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn tensorrt_resources_load_between_the_runtime_and_its_plugins() {
        let dir = std::env::temp_dir().join(format!("tessera-trt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in [
            "libnvinfer_builder_resource_sm90.so.10",
            "libnvinfer_builder_resource_sm120.so.10",
            "libnvinfer_builder_resource_win_sm90.dll",
        ] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        let names = tensorrt_libraries(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            names,
            [
                "libnvinfer.so.10",
                "libnvinfer_builder_resource_sm120.so.10",
                "libnvinfer_builder_resource_sm90.so.10",
                "libnvinfer_plugin.so.10",
                "libnvonnxparser.so.10",
            ]
        );
    }
}
