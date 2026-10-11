//! Running a stateful model frame after frame: one input vector in, one
//! output vector out, and a state vector whose output becomes the next
//! call's input.
//!
//! On CUDA the state stays on the GPU and each call replays one captured
//! CUDA graph: one launch instead of one per kernel (profiling showed a
//! launch-bound workload, about 115 small kernels for 130 µs of GPU work).
//! A call is a copy of the input to the device, the replay, a copy of the
//! new state over the old one on the device, and a copy of the output back,
//! then one wait. Everything goes on one CUDA stream of ours, which the
//! provider runs on too, so the copies and the replay are ordered by the
//! stream. (Synchronous `cudaMemcpy` on the default stream was not enough:
//! TensorRT replays on a stream of its own, and the input copy or the state
//! copy could overlap the replay; its output then drifted from the CPU's by
//! up to 3.5 per frame, depending on timing.) (Two captured graphs swapping
//! two state buffers would save the device copy, but replays of a session's
//! second graph read wrong data with ONNX Runtime 1.28; one graph is exact.)
//!
//! ONNX Runtime captures a graph in CUDA's global capture mode, which forbids
//! other threads' "potentially unsafe" CUDA calls meanwhile: a session
//! opened (a bank of voices, a probe) while another loop ran on the audio
//! thread made that loop fail (measured; it fell back to the CPU). A thread
//! that runs a loop therefore switches itself to the thread-local mode, where
//! only its own captures restrict it; its calls only touch its loops' own
//! streams and buffers.
//!
//! On WebGPU likewise: the state stays in GPU buffers and each call replays
//! the graph captured on the first one, the copies made with the providers'
//! own data transfer (ONNX Runtime's `CopyTensors`). WebGPU sessions share
//! one device, and using it from two threads at once crashed the process
//! (opening a session while another ran on the audio thread; measured with
//! the plugin 0.4.0): opening and closing a WebGPU session waits for a run to
//! end, and a run during either is skipped, never waiting
//! ([`AudioError::GpuBusy`]).
//!
//! Elsewhere (other accelerators, the CPU, or when capture fails) the state
//! goes through the host on each call.
//!
//! A loop may run several independent rows at once (the model's batch,
//! [`LoopSpec::rows`]): one call for every row, each with its own state, which
//! [`OnnxLoop::reset_row`] starts afresh (plan Task 16: the voices of a call
//! cleaned in one GPU call per frame).

mod cuda;
mod host;
mod webgpu;

use std::ops::Range;
use std::path::Path;

use ort::session::Session;

use self::cuda::CudaLoop;
use self::host::HostLoop;
use self::webgpu::WebGpuLoop;
use super::webgpu as device;
use super::{Accelerator, OnnxRuntime, SessionConfig};
use crate::AudioError;

/// What a stateful model takes and gives; the batch dimension is fixed to
/// `rows` and every tensor is `[rows, len]` but the constants (`[len]`).
#[derive(Clone, Debug)]
pub struct LoopSpec<'a> {
    /// The per-call input and its length, per row.
    pub input: (&'a str, usize),
    /// The per-call output and its length, per row.
    pub output: (&'a str, usize),
    /// Inputs that never change, set once.
    pub constants: &'a [(&'a str, &'a [f32])],
    /// The state's input and output names (its length comes from the model).
    pub state: (&'a str, &'a str),
    /// Free dimensions to fix (such as `("batch", 1)`); the batch one must
    /// be `rows`.
    pub dimensions: &'a [(&'a str, i64)],
    /// Rows run in each call (the batch), at least 1.
    pub rows: usize,
}

/// A stateful model, ready to run one call after another.
pub struct OnnxLoop {
    backend: Backend,
    /// All rows'.
    input_len: usize,
    output: Vec<f32>,
    rows: usize,
}

enum Backend {
    Host(HostLoop),
    /// On WebGPU without graph capture.
    HostWebGpu(OnWebGpu<HostLoop>),
    Cuda(Box<CudaLoop>),
    WebGpu(OnWebGpu<Box<WebGpuLoop>>),
}

impl OnnxRuntime {
    /// Opens `model` on the first of `prefer` that starts (on CUDA and
    /// WebGPU, with its state on the GPU and a captured graph when
    /// possible), else the CPU. `TWICECAT_ONNX_CUDA_GRAPHS=0` keeps the state
    /// on the host (a way around a driver problem, and for comparisons).
    pub fn open_loop(
        &self,
        model: &Path,
        prefer: &[Accelerator],
        spec: &LoopSpec<'_>,
    ) -> Result<(OnnxLoop, Option<Accelerator>), AudioError> {
        if spec.rows == 0 {
            return Err(AudioError::Internal);
        }
        let make = |backend| OnnxLoop {
            backend,
            input_len: spec.rows * spec.input.1,
            output: vec![0.0; spec.rows * spec.output.1],
            rows: spec.rows,
        };
        let graphs = std::env::var_os("TWICECAT_ONNX_CUDA_GRAPHS").is_none_or(|v| v != "0");
        for &accelerator in prefer {
            if !self.accelerators().contains(&accelerator) {
                continue;
            }
            let opened = if accelerator == Accelerator::WebGpu {
                open_on_webgpu(model, spec, graphs)
            } else {
                open_elsewhere(model, spec, accelerator, graphs)
            };
            match opened {
                Ok(backend) => return Ok((make(backend), Some(accelerator))),
                Err(e) => tracing::info!("{accelerator:?} unavailable, trying the next: {e}"),
            }
        }
        let host = HostLoop::open(model, None, spec).map_err(|e| {
            tracing::warn!("ONNX model could not be opened: {e}");
            AudioError::Model
        })?;
        Ok((make(Backend::Host(host)), None))
    }
}

/// CUDA or TensorRT with a CUDA graph when it can, else the host loop.
fn open_elsewhere(
    model: &Path,
    spec: &LoopSpec<'_>,
    accelerator: Accelerator,
    graphs: bool,
) -> Result<Backend, String> {
    if graphs && matches!(accelerator, Accelerator::Cuda | Accelerator::TensorRt) {
        match CudaLoop::open(model, spec, accelerator) {
            Ok(cuda) => return Ok(Backend::Cuda(Box::new(cuda))),
            Err(e) => {
                tracing::info!("{accelerator:?} with CUDA graphs unavailable ({e}); trying without")
            }
        }
    }
    HostLoop::open(model, Some(accelerator), spec).map(Backend::Host)
}

/// WebGPU with graph capture when it can, else the host loop: opened,
/// warmed up and (on failure) dropped holding the device, with the plugin
/// registered meanwhile.
fn open_on_webgpu(model: &Path, spec: &LoopSpec<'_>, graphs: bool) -> Result<Backend, String> {
    let exclusive = device::Exclusive::take();
    let lease = device::Lease::take(&exclusive).ok_or("its provider did not register")?;
    if graphs {
        match WebGpuLoop::open(model, spec) {
            Ok(webgpu) => return Ok(Backend::WebGpu(OnWebGpu::new(Box::new(webgpu), lease))),
            Err(e) => tracing::info!("WebGPU with graph capture unavailable ({e}); trying without"),
        }
    }
    let host = HostLoop::open(model, Some(Accelerator::WebGpu), spec)?;
    Ok(Backend::HostWebGpu(OnWebGpu::new(host, lease)))
}

impl OnnxLoop {
    /// Runs one call on `input` (every row's, one after the other); returns
    /// the output (likewise). [`AudioError::GpuBusy`]: not run this time
    /// (see the module docs), the state unchanged.
    pub fn run(&mut self, input: &[f32]) -> Result<&[f32], AudioError> {
        if input.len() != self.input_len {
            return Err(AudioError::Internal);
        }
        let output = &mut self.output;
        let result = match &mut self.backend {
            Backend::Host(host) => Some(host.run(input, output)),
            Backend::HostWebGpu(host) => device::run(|| host.run(input, output)),
            Backend::Cuda(cuda) => Some(cuda.run(input, output)),
            Backend::WebGpu(webgpu) => device::run(|| webgpu.run(input, output)),
        };
        match result {
            None => Err(AudioError::GpuBusy),
            Some(Err(e)) => {
                tracing::warn!("ONNX inference failed: {e}");
                Err(AudioError::Internal)
            }
            Some(Ok(())) => Ok(&self.output),
        }
    }

    /// Back to the initial (zero) state, every row.
    pub fn reset(&mut self) -> Result<(), AudioError> {
        self.reset_rows(0..self.rows)
    }

    /// Back to the initial (zero) state for one row; the others go on.
    pub fn reset_row(&mut self, row: usize) -> Result<(), AudioError> {
        if row >= self.rows {
            return Err(AudioError::Internal);
        }
        self.reset_rows(row..row + 1)
    }

    fn reset_rows(&mut self, rows: Range<usize>) -> Result<(), AudioError> {
        let result = match &mut self.backend {
            Backend::Host(host) => Some(host.reset(rows)),
            Backend::HostWebGpu(host) => Some(host.reset(rows)),
            Backend::Cuda(cuda) => Some(cuda.reset(rows)),
            Backend::WebGpu(webgpu) => device::run(|| webgpu.reset(rows)),
        };
        match result {
            None => Err(AudioError::GpuBusy),
            Some(Err(e)) => {
                tracing::warn!("ONNX state reset failed: {e}");
                Err(AudioError::Internal)
            }
            Some(Ok(())) => Ok(()),
        }
    }

    /// Whether the state stays on the device and calls replay a captured
    /// graph (a CUDA graph, or WebGPU's graph capture).
    pub fn replays_a_graph(&self) -> bool {
        matches!(self.backend, Backend::Cuda(_) | Backend::WebGpu(_))
    }
}

/// The state's length: its input's last dimension.
fn state_len(session: &Session, spec: &LoopSpec<'_>) -> Result<usize, String> {
    let name = spec.state.0;
    let outlet = session
        .inputs()
        .iter()
        .find(|i| i.name() == name)
        .ok_or_else(|| format!("no input {name}"))?;
    outlet
        .dtype()
        .tensor_shape()
        .and_then(|shape| shape.last().copied())
        .and_then(|len| usize::try_from(len).ok())
        .ok_or_else(|| format!("{name} has no fixed length"))
}

fn session_config<'a>(spec: &LoopSpec<'a>, graph_capture: bool) -> SessionConfig<'a> {
    SessionConfig {
        dimensions: spec.dimensions,
        graph_capture,
    }
}

/// A loop on WebGPU and its lease on the plugin: dropped (its session
/// closed, then the lease) while holding [`device::Exclusive`].
struct OnWebGpu<T> {
    /// Both in an `Option` only to be dropped in that order, in `drop`.
    inner: Option<T>,
    lease: Option<device::Lease>,
}

impl<T> OnWebGpu<T> {
    fn new(inner: T, lease: device::Lease) -> Self {
        Self {
            inner: Some(inner),
            lease: Some(lease),
        }
    }
}

impl<T> std::ops::Deref for OnWebGpu<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.inner.as_ref().expect("present until dropped")
    }
}

impl<T> std::ops::DerefMut for OnWebGpu<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.inner.as_mut().expect("present until dropped")
    }
}

impl<T> Drop for OnWebGpu<T> {
    fn drop(&mut self) {
        let _exclusive = device::Exclusive::take();
        drop(self.inner.take());
        drop(self.lease.take());
    }
}
