//! WebGPU with graph capture: the state stays in GPU buffers and each call
//! replays the graph captured on the first one, the copies made with the
//! providers' own data transfer. Runs only through `crate::onnx::webgpu`'s
//! device guard (one thread at a time).

use std::ops::Range;
use std::path::Path;

use ort::memory::{AllocationDevice, Allocator, AllocatorType, MemoryInfo, MemoryType};
use ort::session::{IoBinding, RunOptions, Session};
use ort::value::Tensor;

use super::{LoopSpec, session_config, state_len};
use crate::onnx::{Accelerator, open_session};

/// A value's raw pointer, for [`copy_tensors`].
type OrtValuePointer = dyn ort::AsPointer<Sys = ort::sys::OrtValue>;

/// Copies each of `pairs` (source, target) with the providers' own data
/// transfer (ONNX Runtime's `CopyTensors`), and returns when they are done.
fn copy_tensors(pairs: &[(&OrtValuePointer, &OrtValuePointer)]) -> Result<(), String> {
    let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
    let env_ptr = ort::AsPointer::ptr(&*env).cast_mut();
    let sources: Vec<*const ort::sys::OrtValue> = pairs.iter().map(|(s, _)| s.ptr()).collect();
    let mut targets: Vec<*const ort::sys::OrtValue> = pairs.iter().map(|(_, t)| t.ptr()).collect();
    // SAFETY: live tensors of the same shapes (made from one spec) and type;
    // no stream, so the copies are done when the call returns, and nothing
    // else touches the targets meanwhile (this loop owns them).
    let status = unsafe {
        (ort::api().CopyTensors)(
            env_ptr,
            sources.as_ptr(),
            targets.as_mut_ptr(),
            std::ptr::null_mut(),
            pairs.len(),
        )
    };
    // SAFETY: a status from the API, owned from here on.
    unsafe { ort::Error::result_from_status(status) }.map_err(|e| e.to_string())
}

/// WebGPU: the state stays in GPU buffers and each call replays the graph
/// captured on the first one (`enableGraphCapture`): recording ~240 small
/// dispatches cost about 6 µs of CPU each per call, 1.4 ms in all.
pub(super) struct WebGpuLoop {
    binding: IoBinding,
    no_wait: RunOptions,
    /// Host staging tensors: the input, the output, zeros for a reset, the
    /// state for a reset of some rows.
    host_input: Tensor<f32>,
    host_output: Tensor<f32>,
    zeros: Tensor<f32>,
    host_state: Tensor<f32>,
    rows: usize,
    /// Device buffers: the input and the state; the binding owns the output
    /// and the next state, which each run returns.
    input: Tensor<f32>,
    state: Tensor<f32>,
    /// The output's and the next state's names.
    outputs: [String; 2],
    _constants: Vec<Tensor<f32>>,
    _allocator: Allocator,
    session: Session,
}

impl WebGpuLoop {
    pub(super) fn open(model: &Path, spec: &LoopSpec<'_>) -> Result<Self, String> {
        let text = |e: ort::Error| e.to_string();
        let session = open_session(
            model,
            Some(Accelerator::WebGpu),
            &session_config(spec, true),
            None,
        )?;
        let state_len = state_len(&session, spec)?;
        let info = MemoryInfo::new(
            AllocationDevice::WEBGPU_BUFFER,
            0,
            AllocatorType::Device,
            MemoryType::Default,
        )
        .map_err(text)?;
        let allocator = Allocator::new(&session, info).map_err(text)?;
        let device = |shape: Vec<usize>| Tensor::<f32>::new(&allocator, shape).map_err(text);
        let rows = spec.rows;
        let host =
            |len: usize| Tensor::from_array(([rows, len], vec![0.0; rows * len])).map_err(text);

        let mut binding = session.create_binding().map_err(text)?;
        let input = device(vec![rows, spec.input.1])?;
        let state = device(vec![rows, state_len])?;
        binding.bind_input(spec.input.0, &input).map_err(text)?;
        binding.bind_input(spec.state.0, &state).map_err(text)?;
        let mut constants = Vec::new();
        for (name, value) in spec.constants {
            let tensor = device(vec![value.len()])?;
            let staged = Tensor::from_array(([value.len()], value.to_vec())).map_err(text)?;
            copy_tensors(&[(&staged, &tensor)])?;
            binding.bind_input(*name, &tensor).map_err(text)?;
            constants.push(tensor);
        }
        binding
            .bind_output(spec.output.0, device(vec![rows, spec.output.1])?)
            .map_err(text)?;
        binding
            .bind_output(spec.state.1, device(vec![rows, state_len])?)
            .map_err(text)?;
        let mut no_wait = RunOptions::new().map_err(text)?;
        no_wait.disable_device_sync().map_err(text)?;
        let mut this = Self {
            binding,
            no_wait,
            host_input: host(spec.input.1)?,
            host_output: host(spec.output.1)?,
            zeros: host(state_len)?,
            host_state: host(state_len)?,
            rows,
            input,
            state,
            outputs: [spec.output.0.to_owned(), spec.state.1.to_owned()],
            _constants: constants,
            _allocator: allocator,
            session,
        };
        // The first call captures the graph: here, not on the audio thread.
        this.reset(0..rows)?;
        let mut scratch = vec![0.0; rows * spec.output.1];
        let zeros = vec![0.0; rows * spec.input.1];
        this.run(&zeros, &mut scratch)?;
        this.reset(0..rows)?;
        Ok(this)
    }

    pub(super) fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        let text = |e: ort::Error| e.to_string();
        self.host_input
            .extract_tensor_mut()
            .1
            .copy_from_slice(input);
        copy_tensors(&[(&self.host_input, &self.input)])?;
        let outputs = self
            .session
            .run_binding_with_options(&self.binding, &self.no_wait)
            .map_err(text)?;
        let (Some(out), Some(next)) = (
            outputs.get(self.outputs[0].as_str()),
            outputs.get(self.outputs[1].as_str()),
        ) else {
            return Err("bound outputs missing".into());
        };
        copy_tensors(&[(next, &self.state)])?;
        copy_tensors(&[(out, &self.host_output)])?;
        drop(outputs);
        output.copy_from_slice(self.host_output.extract_tensor().1);
        Ok(())
    }

    /// The providers' copies take whole tensors: resetting some rows reads
    /// the state back, zeroes them and writes it again (rare: a new voice).
    pub(super) fn reset(&mut self, rows: Range<usize>) -> Result<(), String> {
        if rows.len() == self.rows {
            return copy_tensors(&[(&self.zeros, &self.state)]);
        }
        copy_tensors(&[(&self.state, &self.host_state)])?;
        let state = self.host_state.extract_tensor_mut().1;
        let len = state.len() / self.rows;
        state[rows.start * len..rows.end * len].fill(0.0);
        copy_tensors(&[(&self.host_state, &self.state)])
    }
}
