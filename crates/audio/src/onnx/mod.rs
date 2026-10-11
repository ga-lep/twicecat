//! ONNX Runtime for the opt-in GPU speech enhancement (plan Task 13b;
//! feature `onnxruntime`). The library is Microsoft's prebuilt release,
//! fetched and checked by `scripts/fetch-onnxruntime.sh` (on Windows, the
//! same release with DirectML is built by
//! `scripts/build-onnxruntime-directml.sh`: Microsoft no longer publishes
//! it), and loaded at run time from that directory (or
//! `TWICECAT_ONNXRUNTIME_DIR`); nothing is downloaded or linked at build
//! time.
//!
//! Sessions try the requested accelerators in order (TensorRT, CUDA,
//! DirectML, WebGPU, CoreML) and fall back to the CPU, reporting what runs:
//! an accelerator can be compiled in and still fail at run time (missing
//! CUDA or cuDNN libraries, an unsupported operator).
//!
//! WebGPU is a plugin provider ([`webgpu`]), available when it lists a
//! device (a Vulkan, Direct3D 12 or Metal adapter).
//!
//! Stateful models (one frame per call, their state carried from call to
//! call) run through [`OnnxLoop`]: on CUDA and WebGPU the state never leaves
//! the GPU and each call replays one captured graph ([`stateful_loop`]).
//!
//! On Linux the CUDA build's libraries (CUDA 13, cuDNN 9, TensorRT) are
//! found as [`nvidia`] says.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::ep::{self, ExecutionProvider, ExecutionProviderDispatch};
use ort::session::Session;
use ort::value::Tensor;

use crate::AudioError;

mod nvidia;
mod stateful_loop;
mod webgpu;

pub use nvidia::{default_cuda_dir, default_tensorrt_dir};
pub use stateful_loop::{LoopSpec, OnnxLoop};
pub(crate) use webgpu::Exclusive as WebGpuExclusive;
pub use webgpu::{WEBGPU_EP_VERSION, default_webgpu_dir};

/// The ONNX Runtime release `scripts/fetch-onnxruntime.sh` pins.
pub const ONNXRUNTIME_VERSION: &str = "1.28.3";

/// A hardware backend of ONNX Runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Accelerator {
    /// NVIDIA GPUs (CUDA and cuDNN libraries needed).
    Cuda,
    /// NVIDIA GPUs through TensorRT (its libraries needed).
    TensorRt,
    /// Any GPU with Vulkan (Linux), Direct3D 12 (Windows) or Metal (macOS),
    /// through ONNX Runtime's WebGPU plugin provider (Dawn inside), fetched
    /// by `scripts/fetch-webgpu-ep.sh`.
    WebGpu,
    /// Any DirectX 12 GPU on Windows.
    DirectMl,
    /// Apple's Neural Engine and GPU on macOS.
    CoreMl,
}

impl Accelerator {
    /// Every accelerator, preferred first: the fastest for the least CPU
    /// first, as measured (their outputs are all far closer to the CPU's
    /// than anything audible). DirectML before WebGPU: on an RTX 5090 it
    /// ran a frame in 0.30 ms for 0.11 ms of CPU, against 0.93 ms and
    /// 0.85 ms (Direct3D 12); a 16-row bank costs the same on both. One
    /// runtime has either CUDA or DirectML, never both.
    pub const ALL: [Self; 5] = [
        Self::TensorRt,
        Self::Cuda,
        Self::DirectMl,
        Self::WebGpu,
        Self::CoreMl,
    ];

    /// The provider to append; `None` for a plugin provider, which is
    /// appended by device instead (`open_session`).
    fn dispatch(
        self,
        config: &SessionConfig<'_>,
        stream: Option<ComputeStream>,
    ) -> Option<ExecutionProviderDispatch> {
        Some(match self {
            // The heuristic picks convolution algorithms at once; the default
            // exhaustive search benchmarks them on the first frame (seconds).
            Self::Cuda => {
                let mut cuda = ep::CUDA::default()
                    .with_conv_algorithm_search(ep::cuda::ConvAlgorithmSearch::Heuristic)
                    .with_cuda_graph(config.graph_capture);
                if let Some(stream) = stream {
                    // SAFETY: `ComputeStream::new`'s caller keeps the stream
                    // alive as long as the session.
                    cuda = unsafe { cuda.with_compute_stream(stream.0.cast()) };
                }
                cuda.build().error_on_failure()
            }
            // Building an engine takes seconds to minutes: it is cached (with
            // its timing cache) for the next session on this GPU and driver.
            // The cache file is named after the graph, not the shapes fixed
            // by `dimensions`: an engine built for one batch size failed to
            // take another (`setInputShape`), so the fixed dimensions go in
            // its name.
            Self::TensorRt => {
                let mut trt = ep::TensorRT::default().with_cuda_graph(config.graph_capture);
                if let Some(stream) = stream {
                    // SAFETY: as for CUDA above.
                    trt = unsafe { trt.with_compute_stream(stream.0.cast()) };
                }
                if let Some(dir) = nvidia::engine_cache_dir() {
                    let dir = dir.to_string_lossy();
                    let shapes: String = config
                        .dimensions
                        .iter()
                        .map(|(name, size)| format!("_{name}{size}"))
                        .collect();
                    trt = trt
                        .with_engine_cache(true)
                        .with_engine_cache_path(dir.as_ref())
                        .with_engine_cache_prefix(format!("TensorrtExecutionProvider{shapes}"))
                        .with_timing_cache(true)
                        .with_timing_cache_path(dir.as_ref());
                }
                trt.build().error_on_failure()
            }
            Self::WebGpu => return None,
            Self::DirectMl => ep::DirectML::default().build().error_on_failure(),
            Self::CoreMl => ep::CoreML::default().build().error_on_failure(),
        })
    }

    fn compiled_in(self) -> bool {
        let available = match self {
            Self::Cuda => ep::CUDA::default().is_available(),
            Self::TensorRt => ep::TensorRT::default().is_available(),
            Self::WebGpu => return webgpu::device_listed(),
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

/// Where the runtime's library is: `TWICECAT_ONNXRUNTIME_DIR`, else the
/// fetched GPU build (CUDA 13, then 12) if present, else the DirectML build
/// (Windows, `scripts/build-onnxruntime-directml.sh`), else the CPU build.
pub fn default_runtime_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TWICECAT_ONNXRUNTIME_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = crate::paths::cache_dir()?
        .join("onnxruntime")
        .join(ONNXRUNTIME_VERSION);
    let platform = platform()?;
    ["-gpu_cuda13", "-gpu_cuda12", "-directml", ""]
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
            nvidia::preload(dir);
            let builder = ort::init_from(dir.join(library_name())).map_err(|e| e.to_string())?;
            if !builder.with_name("twicecat").commit() {
                tracing::debug!("ONNX Runtime environment was already set up");
            }
            // Registered to see whether WebGPU finds a GPU, then released
            // until a session needs it (see `webgpu`).
            let _webgpu = WebGpuExclusive::take();
            webgpu::set_dir(default_webgpu_dir());
            webgpu::register();
            let accelerators = Accelerator::ALL
                .into_iter()
                .filter(|a| a.compiled_in())
                .collect();
            webgpu::unregister();
            Ok(accelerators)
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
        self.session_with(model, prefer, &SessionConfig::default())
    }

    /// [`session`](Self::session) with `config`.
    pub fn session_with(
        &self,
        model: &Path,
        prefer: &[Accelerator],
        config: &SessionConfig<'_>,
    ) -> Result<(OnnxSession, Option<Accelerator>), AudioError> {
        let open = |accelerator| open_session(model, accelerator, config, None);
        for &accelerator in prefer {
            // WebGPU sessions only through `open_loop`, which keeps WebGPU
            // work on one thread and the plugin registered while they live.
            if !self.accelerators.contains(&accelerator) || accelerator == Accelerator::WebGpu {
                continue;
            }
            match open(Some(accelerator)) {
                Ok(session) => return Ok((OnnxSession { session }, Some(accelerator))),
                Err(e) => tracing::info!("{accelerator:?} unavailable, trying the next: {e}"),
            }
        }
        let session = open(None).map_err(|e| {
            tracing::warn!("ONNX model could not be opened: {e}");
            AudioError::Model
        })?;
        Ok((OnnxSession { session }, None))
    }
}

/// A CUDA stream for a session's CUDA and TensorRT providers to run on
/// (`cudaStream_t`).
#[derive(Clone, Copy)]
pub(crate) struct ComputeStream(*mut std::ffi::c_void);

impl ComputeStream {
    /// # Safety
    /// `stream` must be a CUDA stream that outlives every session opened
    /// with it.
    pub(crate) unsafe fn new(stream: *mut std::ffi::c_void) -> Self {
        Self(stream)
    }
}

/// Opens `model` on `accelerator` (`None`: the CPU) as `config` says, on
/// `stream` for CUDA and TensorRT when given.
pub(crate) fn open_session(
    model: &Path,
    accelerator: Option<Accelerator>,
    config: &SessionConfig<'_>,
    stream: Option<ComputeStream>,
) -> Result<Session, String> {
    // One stream of 10 ms frames: the calling thread runs the graph (no
    // pool), and nothing spins between calls. The default pool keeps a
    // thread per core spinning after each run, which burned most of the
    // process's CPU time while the GPU did the work.
    let text = |e: ort::Error<_>| e.to_string();
    let mut builder = Session::builder().map_err(|e| e.to_string())?;
    builder = builder.with_intra_threads(1).map_err(text)?;
    builder = builder.with_inter_threads(1).map_err(text)?;
    builder = builder.with_intra_op_spinning(false).map_err(text)?;
    builder = builder.with_inter_op_spinning(false).map_err(text)?;
    // ONNX Runtime's profiler, for profiling a backend: a trace of every run
    // and node, with the provider each ran on, written to
    // `<prefix>_<date>.json` when the session ends.
    if let Some(prefix) = std::env::var_os("TWICECAT_ONNX_PROFILE") {
        builder = builder.with_profiling(prefix).map_err(text)?;
    }
    for (name, size) in config.dimensions {
        builder = builder
            .with_dimension_override(*name, *size)
            .map_err(|e| e.to_string())?;
    }
    if accelerator == Some(Accelerator::WebGpu) {
        builder = webgpu::add_to_session(builder, config.graph_capture)?;
    } else if let Some(accelerator) = accelerator {
        // TensorRT runs what it supports; CUDA takes any node it does not.
        let fallback = (accelerator == Accelerator::TensorRt).then(|| {
            Accelerator::Cuda.dispatch(
                &SessionConfig {
                    graph_capture: false,
                    ..config.clone()
                },
                stream,
            )
        });
        builder = builder
            .with_execution_providers(
                accelerator
                    .dispatch(config, stream)
                    .into_iter()
                    .chain(fallback.flatten())
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| e.to_string())?;
    }
    builder.commit_from_file(model).map_err(|e| e.to_string())
}

/// How a session is set up.
#[derive(Clone, Debug, Default)]
pub struct SessionConfig<'a> {
    /// Free dimensions fixed by name (such as `("batch", 1)`): shapes known
    /// when the session starts let the runtime fold shape computations away
    /// instead of running them, with their copies, on every call.
    pub dimensions: &'a [(&'a str, i64)],
    /// CUDA and WebGPU: capture the graph on the first calls and replay it
    /// after (one launch instead of one per kernel). Needs every input and
    /// output bound to device memory at fixed addresses ([`OnnxLoop`] does
    /// it).
    pub graph_capture: bool,
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

    /// Each input's name and shape (`-1` for a dimension left free, such as
    /// a batch axis).
    pub fn input_shapes(&self) -> Vec<(String, Vec<i64>)> {
        self.session
            .inputs()
            .iter()
            .map(|i| {
                let shape = i.dtype().tensor_shape().map(|s| s.to_vec());
                (i.name().to_owned(), shape.unwrap_or_default())
            })
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
