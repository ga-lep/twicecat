//! CUDA and TensorRT: the state stays on the GPU and each call replays one
//! captured CUDA graph, on a CUDA stream of ours (see the parent module's
//! docs for why one stream, and the thread-local capture mode).

use std::ffi::{c_int, c_uint, c_void};
use std::ops::Range;
use std::path::Path;

use ort::memory::{AllocationDevice, Allocator, AllocatorType, MemoryInfo, MemoryType};
use ort::session::{IoBinding, RunOptions, Session};
use ort::value::Tensor;

use super::{LoopSpec, session_config, state_len};
use crate::onnx::{Accelerator, ComputeStream, open_session};

type MemcpyAsync =
    unsafe extern "C" fn(*mut c_void, *const c_void, usize, c_int, *mut c_void) -> c_int;
type MemsetAsync = unsafe extern "C" fn(*mut c_void, c_int, usize, *mut c_void) -> c_int;
type StreamCall = unsafe extern "C" fn(*mut c_void) -> c_int;

/// The CUDA runtime's stream, copy and fill functions, from the library the
/// CUDA provider loaded.
struct Cudart {
    memcpy_async: MemcpyAsync,
    memset_async: MemsetAsync,
    set_device_flags: unsafe extern "C" fn(c_uint) -> c_int,
    stream_create: unsafe extern "C" fn(*mut *mut c_void) -> c_int,
    stream_destroy: StreamCall,
    stream_synchronize: StreamCall,
    exchange_capture_mode: unsafe extern "C" fn(*mut c_int) -> c_int,
    _library: libloading::Library,
}

/// A CUDA stream, destroyed on drop.
struct Stream {
    raw: *mut c_void,
    destroy: StreamCall,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: the stream was created by `Cudart::stream` and nothing
        // runs on it any more (its users drop first).
        unsafe { (self.destroy)(self.raw) };
    }
}

/// `cudaDeviceScheduleBlockingSync`: a thread waiting for the GPU sleeps
/// instead of spinning.
const SCHEDULE_BLOCKING_SYNC: c_uint = 0x04;

/// `cudaStreamCaptureModeThreadLocal`.
const CAPTURE_MODE_THREAD_LOCAL: c_int = 1;

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
            // SAFETY: the signatures of these functions, unchanged in every
            // CUDA runtime (`cudaStream_t` is a pointer).
            unsafe {
                let text = |e: libloading::Error| e.to_string();
                return Ok(Self {
                    memcpy_async: *library.get(b"cudaMemcpyAsync\0").map_err(text)?,
                    memset_async: *library.get(b"cudaMemsetAsync\0").map_err(text)?,
                    set_device_flags: *library.get(b"cudaSetDeviceFlags\0").map_err(text)?,
                    stream_create: *library.get(b"cudaStreamCreate\0").map_err(text)?,
                    stream_destroy: *library.get(b"cudaStreamDestroy\0").map_err(text)?,
                    stream_synchronize: *library.get(b"cudaStreamSynchronize\0").map_err(text)?,
                    exchange_capture_mode: *library
                        .get(b"cudaThreadExchangeStreamCaptureMode\0")
                        .map_err(text)?,
                    _library: library,
                });
            }
        }
        Err("no CUDA runtime library".into())
    }

    /// A new stream (a blocking one: it also waits for work on the default
    /// stream, should anything put some there).
    fn stream(&self) -> Result<Stream, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: `raw` is a valid place for the new stream.
        match unsafe { (self.stream_create)(&mut raw) } {
            0 => Ok(Stream {
                raw,
                destroy: self.stream_destroy,
            }),
            code => Err(format!("cudaStreamCreate failed ({code})")),
        }
    }

    /// Queues a copy of `bytes` from `src` to `dst` (`kind`: direction) on
    /// `stream`. A host source may be reused when this returns (pageable
    /// memory is staged at once); a host destination is filled when the
    /// stream has been waited for.
    ///
    /// # Safety
    /// Both pointers must be valid for `bytes` on their side of the copy,
    /// a host destination until the stream has been waited for.
    unsafe fn copy(
        &self,
        stream: &Stream,
        dst: *mut c_void,
        src: *const c_void,
        bytes: usize,
        kind: c_int,
    ) -> Result<(), String> {
        // SAFETY: as the caller guarantees.
        match unsafe { (self.memcpy_async)(dst, src, bytes, kind, stream.raw) } {
            0 => Ok(()),
            code => Err(format!("cudaMemcpyAsync failed ({code})")),
        }
    }

    /// Lets this thread call CUDA while another thread captures a graph (see
    /// the module docs); once per thread.
    fn allow_beside_captures(&self) -> Result<(), String> {
        thread_local! {
            static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        if DONE.get() {
            return Ok(());
        }
        let mut mode = CAPTURE_MODE_THREAD_LOCAL;
        // SAFETY: `mode` is a valid place for the mode in and the old one
        // out; the call changes only this thread's capture interaction.
        match unsafe { (self.exchange_capture_mode)(&mut mode) } {
            0 => {
                DONE.set(true);
                Ok(())
            }
            code => Err(format!(
                "cudaThreadExchangeStreamCaptureMode failed ({code})"
            )),
        }
    }

    /// Waits until everything queued on `stream` has run.
    fn wait(&self, stream: &Stream) -> Result<(), String> {
        // SAFETY: a valid stream.
        match unsafe { (self.stream_synchronize)(stream.raw) } {
            0 => Ok(()),
            code => Err(format!("cudaStreamSynchronize failed ({code})")),
        }
    }
}

/// The state on the GPU, each call replaying the captured CUDA graph.
///
/// Fields drop in order: the binding and buffers, then the allocator they
/// were made with (freeing a tensor goes through it), then the session, then
/// the stream it runs on, then the runtime library.
pub(super) struct CudaLoop {
    binding: IoBinding,
    /// Runs without ONNX Runtime's wait at the end: `run` waits for the
    /// stream once, after the copies (measured: the least CPU per frame of
    /// the orders tried, and no slower).
    no_wait: RunOptions,
    /// Device buffers: the input, the state, the constants; the binding owns
    /// the output and the next state.
    _held: Vec<Tensor<f32>>,
    _allocator: Allocator,
    session: Session,
    stream: Stream,
    cudart: Cudart,
    input: *mut c_void,
    output: *mut c_void,
    state: *mut c_void,
    next_state: *mut c_void,
    /// Every row's state.
    state_bytes: usize,
    rows: usize,
}

// SAFETY: the raw pointers are device addresses of tensors this struct or
// its binding owns, and a stream it owns; nothing here is tied to a thread.
unsafe impl Send for CudaLoop {}

impl CudaLoop {
    pub(super) fn open(
        model: &Path,
        spec: &LoopSpec<'_>,
        accelerator: Accelerator,
    ) -> Result<Self, String> {
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
        let stream = cudart.stream()?;
        // SAFETY: the stream is destroyed after the session (field order).
        let compute = unsafe { ComputeStream::new(stream.raw) };
        let session = open_session(
            model,
            Some(accelerator),
            &session_config(spec, true),
            Some(compute),
        )?;
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
        let rows = spec.rows;
        let mut input = new(vec![rows, spec.input.1])?;
        let mut state = new(vec![rows, state_len])?;
        let mut output = new(vec![rows, spec.output.1])?;
        let mut next_state = new(vec![rows, state_len])?;
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
                    &stream,
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

        let mut no_wait = RunOptions::new().map_err(text)?;
        no_wait.disable_device_sync().map_err(text)?;
        let mut this = Self {
            binding,
            no_wait,
            _held: held,
            _allocator: allocator,
            session,
            stream,
            cudart,
            input: pointers.0,
            state: pointers.1,
            output: pointers.2,
            next_state: pointers.3,
            state_bytes: rows * state_len * size_of::<f32>(),
            rows,
        };
        // The first call captures the graph (ONNX Runtime runs it more than
        // once inside that call): here, not on the audio thread, and failing
        // here if capture does.
        this.reset(0..rows)?;
        let mut scratch = vec![0.0; rows * spec.output.1];
        let zeros = vec![0.0; rows * spec.input.1];
        this.run(&zeros, &mut scratch)?;
        this.reset(0..rows)?;
        Ok(this)
    }

    pub(super) fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        self.cudart.allow_beside_captures()?;
        // SAFETY: each device buffer holds the floats copied in or out: the
        // input and output lengths come from the spec the buffers were made
        // with (checked by `OnnxLoop::run`), the state from the model; the
        // host output is not touched before the wait below.
        unsafe {
            self.cudart.copy(
                &self.stream,
                self.input,
                input.as_ptr().cast(),
                size_of_val(input),
                HOST_TO_DEVICE,
            )?;
            drop(
                self.session
                    .run_binding_with_options(&self.binding, &self.no_wait)
                    .map_err(|e| e.to_string())?,
            );
            self.cudart.copy(
                &self.stream,
                self.state,
                self.next_state,
                self.state_bytes,
                DEVICE_TO_DEVICE,
            )?;
            self.cudart.copy(
                &self.stream,
                output.as_mut_ptr().cast(),
                self.output,
                size_of_val(output),
                DEVICE_TO_HOST,
            )?;
        }
        self.cudart.wait(&self.stream)
    }

    pub(super) fn reset(&mut self, rows: Range<usize>) -> Result<(), String> {
        self.cudart.allow_beside_captures()?;
        let row_bytes = self.state_bytes / self.rows;
        // SAFETY: the rows are within the state buffer, which holds
        // `state_bytes` bytes on the device (`OnnxLoop` checks the range).
        match unsafe {
            (self.cudart.memset_async)(
                self.state.cast::<u8>().add(rows.start * row_bytes).cast(),
                0,
                rows.len() * row_bytes,
                self.stream.raw,
            )
        } {
            0 => self.cudart.wait(&self.stream),
            code => Err(format!("cudaMemsetAsync failed ({code})")),
        }
    }
}
