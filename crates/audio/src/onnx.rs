//! ONNX Runtime for the opt-in GPU speech enhancement (plan Task 13b;
//! feature `onnxruntime`). The library is Microsoft's prebuilt release,
//! fetched and checked by `scripts/fetch-onnxruntime.sh` and loaded at run
//! time from that directory (or `TESSERA_ONNXRUNTIME_DIR`); nothing is
//! downloaded or linked at build time.
//!
//! Sessions try the requested accelerators in order (CUDA, TensorRT,
//! DirectML, CoreML) and fall back to the CPU, reporting what runs: an
//! accelerator can be compiled in and still fail at run time (missing CUDA
//! or cuDNN libraries, an unsupported operator).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::ep::{self, ExecutionProvider, ExecutionProviderDispatch};
use ort::session::Session;
use ort::value::Tensor;

use crate::AudioError;

/// The ONNX Runtime release `scripts/fetch-onnxruntime.sh` pins.
pub const ONNXRUNTIME_VERSION: &str = "1.28.3";

/// A hardware backend of ONNX Runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Accelerator {
    /// NVIDIA GPUs (CUDA and cuDNN libraries needed).
    Cuda,
    /// NVIDIA GPUs through TensorRT (its libraries needed).
    TensorRt,
    /// Any DirectX 12 GPU on Windows.
    DirectMl,
    /// Apple's Neural Engine and GPU on macOS.
    CoreMl,
}

impl Accelerator {
    /// Every accelerator, preferred first.
    pub const ALL: [Self; 4] = [Self::TensorRt, Self::Cuda, Self::DirectMl, Self::CoreMl];

    fn dispatch(self) -> ExecutionProviderDispatch {
        match self {
            Self::Cuda => ep::CUDA::default().build().error_on_failure(),
            Self::TensorRt => ep::TensorRT::default().build().error_on_failure(),
            Self::DirectMl => ep::DirectML::default().build().error_on_failure(),
            Self::CoreMl => ep::CoreML::default().build().error_on_failure(),
        }
    }

    fn compiled_in(self) -> bool {
        let available = match self {
            Self::Cuda => ep::CUDA::default().is_available(),
            Self::TensorRt => ep::TensorRT::default().is_available(),
            Self::DirectMl => ep::DirectML::default().is_available(),
            Self::CoreMl => ep::CoreML::default().is_available(),
        };
        available.unwrap_or(false)
    }
}

/// The platform part of the cache directory's name.
fn platform() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "aarch64") => "osx-arm64",
        ("windows", "x86_64") => "win-x64",
        ("windows", "aarch64") => "win-arm64",
        _ => return None,
    })
}

fn library_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "onnxruntime.dll",
        "macos" => "libonnxruntime.dylib",
        _ => "libonnxruntime.so",
    }
}

/// Where the runtime's library is: `TESSERA_ONNXRUNTIME_DIR`, else the
/// fetched GPU build (CUDA 13, then 12) if present, else the CPU build.
pub fn default_runtime_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TESSERA_ONNXRUNTIME_DIR") {
        return Some(PathBuf::from(dir));
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let base = cache.join("tessera/onnxruntime").join(ONNXRUNTIME_VERSION);
    let platform = platform()?;
    ["-gpu_cuda13", "-gpu_cuda12", ""]
        .iter()
        .map(|flavour| base.join(format!("{platform}{flavour}")).join("lib"))
        .find(|dir| dir.join(library_name()).exists())
}

/// The loaded runtime (once per process).
pub struct OnnxRuntime {
    accelerators: Vec<Accelerator>,
}

static RUNTIME: OnceLock<Result<Vec<Accelerator>, String>> = OnceLock::new();

impl OnnxRuntime {
    /// Loads the runtime library from `dir`. Only the first call loads;
    /// later calls get the same result whatever `dir` they pass.
    pub fn load(dir: &Path) -> Result<Self, AudioError> {
        let loaded = RUNTIME.get_or_init(|| {
            let builder = ort::init_from(dir.join(library_name())).map_err(|e| e.to_string())?;
            if !builder.with_name("tessera").commit() {
                tracing::debug!("ONNX Runtime environment was already set up");
            }
            Ok(Accelerator::ALL
                .into_iter()
                .filter(|a| a.compiled_in())
                .collect())
        });
        match loaded {
            Ok(accelerators) => Ok(Self {
                accelerators: accelerators.clone(),
            }),
            Err(e) => {
                tracing::warn!("ONNX Runtime could not be loaded: {e}");
                Err(AudioError::Model)
            }
        }
    }

    /// The accelerators this build of the runtime has (compiled in; each
    /// can still fail when a session starts).
    pub fn accelerators(&self) -> &[Accelerator] {
        &self.accelerators
    }

    /// Opens `model`, trying `prefer` in order, then the CPU. Returns the
    /// session and the accelerator it runs on (`None`: the CPU).
    pub fn session(
        &self,
        model: &Path,
        prefer: &[Accelerator],
    ) -> Result<(OnnxSession, Option<Accelerator>), AudioError> {
        for &accelerator in prefer {
            if !self.accelerators.contains(&accelerator) {
                continue;
            }
            let opened = Session::builder()
                .map_err(|e| e.to_string())
                .and_then(|b| {
                    b.with_execution_providers([accelerator.dispatch()])
                        .map_err(|e| e.to_string())
                })
                .and_then(|mut b| b.commit_from_file(model).map_err(|e| e.to_string()));
            match opened {
                Ok(session) => return Ok((OnnxSession { session }, Some(accelerator))),
                Err(e) => tracing::info!("{accelerator:?} unavailable, trying the next: {e}"),
            }
        }
        let session = Session::builder()
            .and_then(|mut b| b.commit_from_file(model))
            .map_err(|e| {
                tracing::warn!("ONNX model could not be opened: {e}");
                AudioError::Model
            })?;
        Ok((OnnxSession { session }, None))
    }
}

/// An open model.
pub struct OnnxSession {
    session: Session,
}

/// A tensor in and out: its shape and its values.
pub type TensorData = (Vec<usize>, Vec<f32>);

impl OnnxSession {
    /// The model's input names, in order.
    pub fn input_names(&self) -> Vec<String> {
        self.session
            .inputs()
            .iter()
            .map(|i| i.name().to_owned())
            .collect()
    }

    /// Runs the model on `inputs` (by name); returns every output, in the
    /// model's order.
    pub fn run(&mut self, inputs: Vec<(&str, TensorData)>) -> Result<Vec<TensorData>, AudioError> {
        let fail = |e: ort::Error| {
            tracing::warn!("ONNX inference failed: {e}");
            AudioError::Internal
        };
        let mut values = Vec::with_capacity(inputs.len());
        for (name, (shape, data)) in inputs {
            let tensor = Tensor::from_array((shape, data)).map_err(fail)?;
            values.push((name.to_owned(), tensor.into_dyn()));
        }
        let outputs = self.session.run(values).map_err(fail)?;
        let mut out = Vec::with_capacity(outputs.len());
        for (_, value) in outputs.iter() {
            let (shape, data) = value.try_extract_tensor::<f32>().map_err(fail)?;
            let shape = shape
                .iter()
                .map(|d| usize::try_from(*d).unwrap_or(0))
                .collect();
            out.push((shape, data.to_vec()));
        }
        Ok(out)
    }
}
