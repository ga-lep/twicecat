//! Running a stateful model frame after frame: one input vector in, one
//! output vector out, and a state vector whose output becomes the next
//! call's input.
//!
//! On CUDA the state stays on the GPU and each call replays one captured
//! CUDA graph: one launch instead of one per kernel (profiling showed a
//! launch-bound workload, about 115 small kernels for 130 µs of GPU work).
//! A call is a copy of the input to the device, the replay, a copy of the
//! new state over the old one on the device, and a copy of the output back.
//! The copies are synchronous `cudaMemcpy` calls through the CUDA runtime
//! the provider has loaded, ordered with the replay by ONNX Runtime's
//! synchronisation at the end of each run. (Two captured graphs swapping
//! two state buffers would save the device copy, but replays of a session's
//! second graph read wrong data with ONNX Runtime 1.28; one graph is exact.)
//!
//! Elsewhere (other accelerators, the CPU, or when capture fails) the state
//! goes through the host on each call.

use std::ffi::{c_int, c_uint, c_void};
use std::path::Path;

use ort::memory::{AllocationDevice, Allocator, AllocatorType, MemoryInfo, MemoryType};
use ort::session::{IoBinding, Session};
use ort::value::Tensor;

use super::{Accelerator, OnnxRuntime, SessionConfig, open_session};
use crate::AudioError;

/// What a stateful model takes and gives; the batch dimension is fixed to 1
/// and every tensor is `[1, len]` but the constants (`[len]`).
#[derive(Clone, Debug)]
pub struct LoopSpec<'a> {
    /// The per-call input and its length.
    pub input: (&'a str, usize),
    /// The per-call output and its length.
    pub output: (&'a str, usize),
    /// Inputs that never change, set once.
    pub constants: &'a [(&'a str, &'a [f32])],
    /// The state's input and output names (its length comes from the model).
    pub state: (&'a str, &'a str),
    /// Free dimensions to fix (such as `("batch", 1)`).
    pub dimensions: &'a [(&'a str, i64)],
}

/// A stateful model, ready to run one call after another.
pub struct OnnxLoop {
    backend: Backend,
    input_len: usize,
    output: Vec<f32>,
}

enum Backend {
    Host(HostLoop),
    Cuda(Box<CudaLoop>),
}

impl OnnxRuntime {
    /// Opens `model` on the first of `prefer` that starts (on CUDA, with its
    /// state on the GPU and a CUDA graph when possible), else the CPU.
    /// `TESSERA_ONNX_CUDA_GRAPHS=0` keeps CUDA on the host loop (a way
    /// around a driver problem, and for comparisons).
    pub fn open_loop(
        &self,
        model: &Path,
        prefer: &[Accelerator],
        spec: &LoopSpec<'_>,
    ) -> Result<(OnnxLoop, Option<Accelerator>), AudioError> {
        let make = |backend| OnnxLoop {
            backend,
            input_len: spec.input.1,
            output: vec![0.0; spec.output.1],
        };
        let graphs = std::env::var_os("TESSERA_ONNX_CUDA_GRAPHS").is_none_or(|v| v != "0");
        for &accelerator in prefer {
            if !self.accelerators().contains(&accelerator) {
                continue;
            }
            if accelerator == Accelerator::Cuda && graphs {
                match CudaLoop::open(model, spec) {
                    Ok(cuda) => {
                        return Ok((make(Backend::Cuda(Box::new(cuda))), Some(accelerator)));
                    }
                    Err(e) => tracing::info!("CUDA graphs unavailable ({e}); trying plain CUDA"),
                }
            }
            match HostLoop::open(model, Some(accelerator), spec) {
                Ok(host) => return Ok((make(Backend::Host(host)), Some(accelerator))),
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

impl OnnxLoop {
    /// Runs one call on `input`; returns the output.
    pub fn run(&mut self, input: &[f32]) -> Result<&[f32], AudioError> {
        if input.len() != self.input_len {
            return Err(AudioError::Internal);
        }
        let result = match &mut self.backend {
            Backend::Host(host) => host.run(input, &mut self.output),
            Backend::Cuda(cuda) => cuda.run(input, &mut self.output),
        };
        result.map_err(|e| {
            tracing::warn!("ONNX inference failed: {e}");
            AudioError::Internal
        })?;
        Ok(&self.output)
    }

    /// Back to the initial (zero) state.
    pub fn reset(&mut self) -> Result<(), AudioError> {
        match &mut self.backend {
            Backend::Host(host) => {
                host.state.fill(0.0);
                Ok(())
            }
            Backend::Cuda(cuda) => cuda.reset().map_err(|e| {
                tracing::warn!("ONNX state reset failed: {e}");
                AudioError::Internal
            }),
        }
    }

    /// Whether the state stays on the device and calls replay a CUDA graph.
    pub fn uses_cuda_graphs(&self) -> bool {
        matches!(self.backend, Backend::Cuda(_))
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

fn session_config<'a>(spec: &LoopSpec<'a>, cuda_graph: bool) -> SessionConfig<'a> {
    SessionConfig {
        dimensions: spec.dimensions,
        cuda_graph,
    }
}

/// The state on the host, fed and fetched on each call.
struct HostLoop {
    session: Session,
    names: [String; 2],
    constants: Vec<(String, Vec<f32>)>,
    state: Vec<f32>,
    /// The output's and the next state's places in the model's outputs.
    outputs: [usize; 2],
}

impl HostLoop {
    fn open(
        model: &Path,
        accelerator: Option<Accelerator>,
        spec: &LoopSpec<'_>,
    ) -> Result<Self, String> {
        let session = open_session(model, accelerator, &session_config(spec, false))?;
        let state = vec![0.0; state_len(&session, spec)?];
        let index = |name: &str| {
            session
                .outputs()
                .iter()
                .position(|o| o.name() == name)
                .ok_or_else(|| format!("no output {name}"))
        };
        let outputs = [index(spec.output.0)?, index(spec.state.1)?];
        Ok(Self {
            names: [spec.input.0.to_owned(), spec.state.0.to_owned()],
            constants: spec
                .constants
                .iter()
                .map(|(n, v)| ((*n).to_owned(), v.to_vec()))
                .collect(),
            session,
            state,
            outputs,
        })
    }

    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        let text = |e: ort::Error| e.to_string();
        let row = |data: &[f32]| {
            Tensor::from_array(([1, data.len()], data.to_vec())).map(Tensor::into_dyn)
        };
        let mut feeds = vec![
            (self.names[0].clone(), row(input).map_err(text)?),
            (self.names[1].clone(), row(&self.state).map_err(text)?),
        ];
        for (name, value) in &self.constants {
            let tensor = Tensor::from_array(([value.len()], value.clone())).map_err(text)?;
            feeds.push((name.clone(), tensor.into_dyn()));
        }
        let outputs = self.session.run(feeds).map_err(text)?;
        for (index, target) in self.outputs.into_iter().zip([output, &mut self.state[..]]) {
            let (_, value) = outputs[index].try_extract_tensor::<f32>().map_err(text)?;
            if value.len() != target.len() {
                return Err("unexpected output length".into());
            }
            target.copy_from_slice(value);
        }
        Ok(())
    }
}

/// The CUDA runtime's copy and fill functions, from the library the CUDA
/// provider loaded.
struct Cudart {
    memcpy: unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int) -> c_int,
    memset: unsafe extern "C" fn(*mut c_void, c_int, usize) -> c_int,
    set_device_flags: unsafe extern "C" fn(c_uint) -> c_int,
    _library: libloading::Library,
}

/// `cudaDeviceScheduleBlockingSync`: a thread waiting for the GPU sleeps
/// instead of spinning.
const SCHEDULE_BLOCKING_SYNC: c_uint = 0x04;

/// `cudaMemcpyKind`.
const HOST_TO_DEVICE: c_int = 1;
const DEVICE_TO_HOST: c_int = 2;
const DEVICE_TO_DEVICE: c_int = 3;

impl Cudart {
    fn load() -> Result<Self, String> {
        #[cfg(windows)]
        const NAMES: &[&str] = &["cudart64_13.dll", "cudart64_12.dll"];
        #[cfg(not(windows))]
        const NAMES: &[&str] = &["libcudart.so.13", "libcudart.so.12"];
        for name in NAMES {
            // SAFETY: loading the CUDA runtime runs only its initialisers;
            // with the provider started it is already loaded.
            let Ok(library) = (unsafe { libloading::Library::new(*name) }) else {
                continue;
            };
            // SAFETY: the signatures of cudaMemcpy and cudaMemset, unchanged
            // in every CUDA runtime.
            unsafe {
                let memcpy = *library
                    .get::<unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int) -> c_int>(
                        b"cudaMemcpy\0",
                    )
                    .map_err(|e| e.to_string())?;
                let memset = *library
                    .get::<unsafe extern "C" fn(*mut c_void, c_int, usize) -> c_int>(
                        b"cudaMemset\0",
                    )
                    .map_err(|e| e.to_string())?;
                let set_device_flags = *library
                    .get::<unsafe extern "C" fn(c_uint) -> c_int>(b"cudaSetDeviceFlags\0")
                    .map_err(|e| e.to_string())?;
                return Ok(Self {
                    memcpy,
                    memset,
                    set_device_flags,
                    _library: library,
                });
            }
        }
        Err("no CUDA runtime library".into())
    }

    /// Copies `bytes` from `src` to `dst` (`kind`: direction), waiting for it.
    ///
    /// # Safety
    /// Both pointers must be valid for `bytes` on their side of the copy.
    unsafe fn copy(
        &self,
        dst: *mut c_void,
        src: *const c_void,
        bytes: usize,
        kind: c_int,
    ) -> Result<(), String> {
        // SAFETY: as the caller guarantees.
        match unsafe { (self.memcpy)(dst, src, bytes, kind) } {
            0 => Ok(()),
            code => Err(format!("cudaMemcpy failed ({code})")),
        }
    }
}

/// The state on the GPU, each call replaying the captured CUDA graph.
///
/// Fields drop in order: the binding and buffers, then the allocator they
/// were made with (freeing a tensor goes through it), then the session.
struct CudaLoop {
    binding: IoBinding,
    /// Device buffers: the input, the state, the constants; the binding owns
    /// the output and the next state.
    _held: Vec<Tensor<f32>>,
    _allocator: Allocator,
    session: Session,
    cudart: Cudart,
    input: *mut c_void,
    output: *mut c_void,
    state: *mut c_void,
    next_state: *mut c_void,
    state_bytes: usize,
}

// SAFETY: the raw pointers are device addresses of tensors this struct or
// its binding owns; nothing here is tied to a thread.
unsafe impl Send for CudaLoop {}

impl CudaLoop {
    fn open(model: &Path, spec: &LoopSpec<'_>) -> Result<Self, String> {
        let text = |e: ort::Error| e.to_string();
        let cudart = Cudart::load()?;
        // Waiting for the GPU (at the end of each run) spun the calling
        // thread through the whole replay, about 0.3 ms of CPU per frame, as
        // much as running the networks on the CPU. Blocking makes the wait
        // cheap. The flag belongs to the process's context on the device, so
        // it is set before the session creates that context (and left as it
        // is, with a note, if something created it first).
        // SAFETY: a plain CUDA runtime call with a valid flag.
        match unsafe { (cudart.set_device_flags)(SCHEDULE_BLOCKING_SYNC) } {
            0 => {}
            code => tracing::info!("CUDA scheduling left as it was ({code})"),
        }
        let session = open_session(model, Some(Accelerator::Cuda), &session_config(spec, true))?;
        let state_len = state_len(&session, spec)?;
        let info = MemoryInfo::new(
            AllocationDevice::CUDA,
            0,
            AllocatorType::Device,
            MemoryType::Default,
        )
        .map_err(text)?;
        let allocator = Allocator::new(&session, info).map_err(text)?;
        let new = |shape: Vec<usize>| Tensor::<f32>::new(&allocator, shape).map_err(text);

        let mut binding = session.create_binding().map_err(text)?;
        let mut input = new(vec![1, spec.input.1])?;
        let mut state = new(vec![1, state_len])?;
        let mut output = new(vec![1, spec.output.1])?;
        let mut next_state = new(vec![1, state_len])?;
        let pointers = (
            input.data_ptr_mut(),
            state.data_ptr_mut(),
            output.data_ptr_mut(),
            next_state.data_ptr_mut(),
        );
        binding.bind_input(spec.input.0, &input).map_err(text)?;
        binding.bind_input(spec.state.0, &state).map_err(text)?;
        let mut held = vec![input, state];
        for (name, value) in spec.constants {
            let mut tensor = new(vec![value.len()])?;
            // SAFETY: the tensor holds `value.len()` floats on the device.
            unsafe {
                cudart.copy(
                    tensor.data_ptr_mut(),
                    value.as_ptr().cast(),
                    size_of_val(*value),
                    HOST_TO_DEVICE,
                )?;
            }
            binding.bind_input(*name, &tensor).map_err(text)?;
            held.push(tensor);
        }
        binding.bind_output(spec.output.0, output).map_err(text)?;
        binding
            .bind_output(spec.state.1, next_state)
            .map_err(text)?;

        let mut this = Self {
            binding,
            _held: held,
            _allocator: allocator,
            session,
            cudart,
            input: pointers.0,
            state: pointers.1,
            output: pointers.2,
            next_state: pointers.3,
            state_bytes: state_len * size_of::<f32>(),
        };
        // The first call runs, allocates and captures (ONNX Runtime runs the
        // graph more than once inside it): do it here, not on the audio
        // thread, and fail here if capture does.
        this.reset()?;
        let mut scratch = vec![0.0; spec.output.1];
        this.run(&vec![0.0; spec.input.1], &mut scratch)?;
        this.reset()?;
        Ok(this)
    }

    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        // SAFETY: each device buffer holds the floats copied in or out: the
        // input and output lengths come from the spec the buffers were made
        // with (checked by `OnnxLoop::run`), the state from the model.
        unsafe {
            self.cudart.copy(
                self.input,
                input.as_ptr().cast(),
                size_of_val(input),
                HOST_TO_DEVICE,
            )?;
            drop(
                self.session
                    .run_binding(&self.binding)
                    .map_err(|e| e.to_string())?,
            );
            self.cudart.copy(
                self.state,
                self.next_state,
                self.state_bytes,
                DEVICE_TO_DEVICE,
            )?;
            self.cudart.copy(
                output.as_mut_ptr().cast(),
                self.output,
                size_of_val(output),
                DEVICE_TO_HOST,
            )
        }
    }

    fn reset(&mut self) -> Result<(), String> {
        // SAFETY: the state buffer holds `state_bytes` bytes on the device.
        match unsafe { (self.cudart.memset)(self.state, 0, self.state_bytes) } {
            0 => Ok(()),
            code => Err(format!("cudaMemset failed ({code})")),
        }
    }
}
