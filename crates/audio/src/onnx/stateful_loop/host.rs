//! The state on the host: fed to the session and fetched back on each call
//! (the CPU, accelerators without a device loop, or when capture fails).

use std::ops::Range;
use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use super::{LoopSpec, session_config, state_len};
use crate::onnx::{Accelerator, open_session};

/// The state on the host, fed and fetched on each call.
pub(super) struct HostLoop {
    session: Session,
    names: [String; 2],
    constants: Vec<(String, Vec<f32>)>,
    rows: usize,
    /// Every row's, one after the other.
    state: Vec<f32>,
    /// The output's and the next state's places in the model's outputs.
    outputs: [usize; 2],
}

impl HostLoop {
    pub(super) fn open(
        model: &Path,
        accelerator: Option<Accelerator>,
        spec: &LoopSpec<'_>,
    ) -> Result<Self, String> {
        let session = open_session(model, accelerator, &session_config(spec, false), None)?;
        let state = vec![0.0; spec.rows * state_len(&session, spec)?];
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
            rows: spec.rows,
            session,
            state,
            outputs,
        })
    }

    pub(super) fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), String> {
        let text = |e: ort::Error| e.to_string();
        let rows = self.rows;
        let row = |data: &[f32]| {
            Tensor::from_array(([rows, data.len() / rows], data.to_vec())).map(Tensor::into_dyn)
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

    /// Back to the initial (zero) state for `rows`.
    pub(super) fn reset(&mut self, rows: Range<usize>) -> Result<(), String> {
        let len = self.state.len() / self.rows;
        self.state[rows.start * len..rows.end * len].fill(0.0);
        Ok(())
    }
}
