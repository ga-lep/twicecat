//! ONNX Runtime for the opt-in GPU speech enhancement (plan Task 13b;
//! feature `onnxruntime`). The library is Microsoft's prebuilt release,
//! fetched and checked by `scripts/fetch-onnxruntime.sh` and loaded at run
//! time from that directory (or `TESSERA_ONNXRUNTIME_DIR`); nothing is
//! downloaded or linked at build time.
//!
//! Sessions try the requested accelerators in order (TensorRT, CUDA,
//! WebGPU, DirectML, CoreML) and fall back to the CPU, reporting what runs:
//! an accelerator can be compiled in and still fail at run time (missing
//! CUDA or cuDNN libraries, an unsupported operator).
//!
//! WebGPU is a plugin provider (Microsoft's `onnxruntime-ep-webgpu`, fetched
//! by `scripts/fetch-webgpu-ep.sh`): registered with the environment when the
//! runtime loads, and available when it lists a device (a Vulkan, Direct3D
//! 12 or Metal adapter).
//!
//! Stateful models (one frame per call, their state carried from call to
//! call) run through [`OnnxLoop`]: on CUDA and WebGPU the state never leaves
//! the GPU and each call replays one captured graph ([`stateful_loop`]).
//!
//! On Linux the CUDA build's libraries (CUDA 13, cuDNN 9) come from the
//! system, or from `scripts/fetch-cuda-runtime.sh` (or `TESSERA_CUDA_DIR`):
//! those are preloaded before the runtime starts, so its CUDA provider finds
//! them without `LD_LIBRARY_PATH`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ort::ep::{self, ExecutionProvider, ExecutionProviderDispatch};
use ort::session::Session;
use ort::value::Tensor;

use crate::AudioError;

mod stateful_loop;
pub use stateful_loop::{LoopSpec, OnnxLoop};

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
    /// Every accelerator, preferred first.
    pub const ALL: [Self; 5] = [
        Self::TensorRt,
        Self::Cuda,
        Self::WebGpu,
        Self::DirectMl,
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
                    .with_cuda_graph(config.cuda_graph);
                if let Some(stream) = stream {
                    // SAFETY: `ComputeStream::new`'s caller keeps the stream
                    // alive as long as the session.
                    cuda = unsafe { cuda.with_compute_stream(stream.0.cast()) };
                }
                cuda.build().error_on_failure()
            }
            // Building an engine takes seconds to minutes: it is cached (with
            // its timing cache) for the next session on this GPU and driver.
            Self::TensorRt => {
                let mut trt = ep::TensorRT::default().with_cuda_graph(config.cuda_graph);
                if let Some(stream) = stream {
                    // SAFETY: as for CUDA above.
                    trt = unsafe { trt.with_compute_stream(stream.0.cast()) };
                }
                if let Some(dir) = engine_cache_dir() {
                    let dir = dir.to_string_lossy();
                    trt = trt
                        .with_engine_cache(true)
                        .with_engine_cache_path(dir.as_ref())
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
            Self::WebGpu => return webgpu_device_listed(),
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

/// The WebGPU plugin provider release `scripts/fetch-webgpu-ep.sh` pins
/// (PyPI `onnxruntime-ep-webgpu`, MIT; needs ONNX Runtime 1.24.4 or later).
pub const WEBGPU_EP_VERSION: &str = "0.4.0";

/// The provider's name, as the plugin registers it.
const WEBGPU_EP_NAME: &str = "WebGpuExecutionProvider";

fn webgpu_library_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "onnxruntime_providers_webgpu.dll",
        "macos" => "libonnxruntime_providers_webgpu.dylib",
        _ => "libonnxruntime_providers_webgpu.so",
    }
}

/// Where `scripts/fetch-webgpu-ep.sh` put the WebGPU plugin provider, or
/// `TESSERA_WEBGPU_EP_DIR`; `None` when neither exists.
pub fn default_webgpu_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TESSERA_WEBGPU_EP_DIR") {
        return Some(PathBuf::from(dir));
    }
    let dir = cache_root()?
        .join("tessera/onnxruntime")
        .join(format!("webgpu-ep-{WEBGPU_EP_VERSION}"));
    dir.is_dir().then_some(dir)
}

/// Registers the WebGPU plugin provider from `dir` with the environment;
/// its device is listed if it found a GPU it can use.
fn register_webgpu(dir: &Path) {
    let library = dir.join(webgpu_library_name());
    if !library.exists() {
        return;
    }
    let registered = ort::environment::Environment::current()
        .and_then(|env| env.register_ep_library("webgpu", &library));
    if let Err(e) = registered {
        tracing::info!("WebGPU provider not registered: {e}");
    }
}

/// Whether the environment lists a WebGPU device (the plugin registered and
/// found an adapter).
fn webgpu_device_listed() -> bool {
    ort::environment::Environment::current().is_ok_and(|env| {
        env.devices()
            .any(|d| d.ep().is_ok_and(|n| n == WEBGPU_EP_NAME))
    })
}

/// Where `scripts/fetch-cuda-runtime.sh` put NVIDIA's libraries, or
/// `TESSERA_CUDA_DIR`; `None` when neither exists (a system install is then
/// used, if any).
pub fn default_cuda_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TESSERA_CUDA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let dir = cache.join("tessera/cuda/13.0/lib");
    dir.is_dir().then_some(dir)
}

/// Where `scripts/fetch-tensorrt.sh` put TensorRT, or
/// `TESSERA_TENSORRT_DIR`; `None` when neither exists (a system install is
/// then used, if any).
pub fn default_tensorrt_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TESSERA_TENSORRT_DIR") {
        return Some(PathBuf::from(dir));
    }
    let dir = cache_root()?.join("tessera/cuda/tensorrt-10.16.1/lib");
    dir.is_dir().then_some(dir)
}

/// Where TensorRT engines built for this machine are kept.
fn engine_cache_dir() -> Option<PathBuf> {
    let dir = cache_root()?.join("tessera/tensorrt-engines");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn cache_root() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
}

/// TensorRT's libraries, dependencies first; the engine builder's resources
/// (one per GPU architecture, found by name) go between the runtime and its
/// plugins, as NVIDIA's own loader loads everything in the directory.
#[cfg(target_os = "linux")]
fn preload_tensorrt(dir: &Path) {
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

/// Loads NVIDIA's libraries from `dir`, so that the CUDA provider's own
/// loads find them already in the process. Missing files are skipped (the
/// provider then fails to start and the CPU runs).
#[cfg(target_os = "linux")]
fn preload_cuda(dir: &Path) {
    for name in CUDA_LIBRARIES {
        let path = dir.join(name);
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
            #[cfg(target_os = "linux")]
            if dir.join("libonnxruntime_providers_cuda.so").exists()
                && let Some(cuda) = default_cuda_dir()
            {
                preload_cuda(&cuda);
            }
            #[cfg(target_os = "linux")]
            if dir.join("libonnxruntime_providers_tensorrt.so").exists()
                && let Some(tensorrt) = default_tensorrt_dir()
            {
                preload_tensorrt(&tensorrt);
            }
            let builder = ort::init_from(dir.join(library_name())).map_err(|e| e.to_string())?;
            if !builder.with_name("tessera").commit() {
                tracing::debug!("ONNX Runtime environment was already set up");
            }
            if let Some(webgpu) = default_webgpu_dir() {
                register_webgpu(&webgpu);
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
            if !self.accelerators.contains(&accelerator) {
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

/// Opens `model` on `accelerator` (`None`: the CPU) as `config` says.
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
    if let Some(prefix) = std::env::var_os("TESSERA_ONNX_PROFILE") {
        builder = builder.with_profiling(prefix).map_err(text)?;
    }
    for (name, size) in config.dimensions {
        builder = builder
            .with_dimension_override(*name, *size)
            .map_err(|e| e.to_string())?;
    }
    if accelerator == Some(Accelerator::WebGpu) {
        // The plugin takes one device and picks the adapter itself.
        let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
        let device = env
            .devices()
            .find(|d| d.ep().is_ok_and(|n| n == WEBGPU_EP_NAME))
            .ok_or("no WebGPU device")?;
        let capture = if config.cuda_graph { "1" } else { "0" };
        let mut options = vec![(
            format!("{WEBGPU_EP_NAME}.enableGraphCapture"),
            capture.to_owned(),
        )];
        // More provider options, for measuring on other GPUs:
        // `TESSERA_WEBGPU_EP_OPTIONS="maxNumPendingDispatches=512;..."`.
        for (key, value) in std::env::var("TESSERA_WEBGPU_EP_OPTIONS")
            .unwrap_or_default()
            .split(';')
            .filter_map(|kv| kv.split_once('='))
        {
            options.push((format!("{WEBGPU_EP_NAME}.{key}"), value.to_owned()));
        }
        builder = builder
            .with_devices([device], Some(&options))
            .map_err(text)?;
    } else if let Some(accelerator) = accelerator {
        // TensorRT runs what it supports; CUDA takes any node it does not.
        let fallback = (accelerator == Accelerator::TensorRt).then(|| {
            Accelerator::Cuda.dispatch(
                &SessionConfig {
                    cuda_graph: false,
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
    /// CUDA only: capture the graph on the first calls and replay it after
    /// (one launch instead of one per kernel). Needs every input and output
    /// bound to device memory at fixed addresses ([`OnnxLoop`] does it).
    pub cuda_graph: bool,
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
