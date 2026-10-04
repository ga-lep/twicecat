//! The three DeepFilterNet3 graphs (encoder, ERB decoder, deep-filter
//! decoder) loaded with tract and pulsed to one 10 ms frame, so each call
//! advances the networks' convolution buffers and GRU states by one frame.

use std::path::Path;

use realfft::num_complex::Complex32;
use tract_onnx::prelude::*;
use tract_onnx::tract_hir::infer::ShapeFactoid;
use tract_onnx::tract_hir::shapefactoid;
use tract_pulse::internal::{PulsedModel, PulsedModelExt, ToDim};

use super::config::Config;
use super::{Inference, stages};
use crate::AudioError;

type State = TypedSimpleState;
/// A compiled network, shared by every filter made from it.
type Plan = Arc<TypedRunnableModel>;

/// A graph that runs one frame per call, keeping its state.
pub(crate) struct Graph {
    state: State,
}

impl Graph {
    pub(crate) fn run(&mut self, inputs: TVec<TValue>) -> Result<TVec<TValue>, AudioError> {
        self.state.run(inputs).map_err(|e| {
            tracing::warn!("DeepFilterNet inference failed: {e}");
            AudioError::Internal
        })
    }
}

fn load_err(what: &str, e: impl std::fmt::Display) -> AudioError {
    tracing::warn!("DeepFilterNet {what}: {e}");
    AudioError::Model
}

/// Loads `file`, fixes its inputs (time axis symbolic as `S`), and pulses it
/// to one frame.
fn pulsed(
    file: &Path,
    inputs: impl Fn(&Symbol) -> Vec<(&'static str, InferenceFact)>,
    outputs: &[&str],
) -> Result<Plan, AudioError> {
    let what = file.file_name().and_then(|n| n.to_str()).unwrap_or("model");
    let mut model = tract_onnx::onnx()
        .with_ignore_output_shapes(true)
        .model_for_path(file)
        .map_err(|e| load_err(what, e))?;
    let s = model.symbols.sym("S");
    let inputs = inputs(&s);
    for (i, (_, fact)) in inputs.iter().enumerate() {
        model = model
            .with_input_fact(i, fact.clone())
            .map_err(|e| load_err(what, e))?;
    }
    model = model
        .with_input_names(inputs.iter().map(|(name, _)| *name))
        .map_err(|e| load_err(what, e))?;
    if !outputs.is_empty() {
        // By their value names, in this order.
        let outlets = outputs
            .iter()
            .map(|name| {
                model
                    .find_outlet_label(name)
                    .or_else(|| {
                        model
                            .node_by_name(name)
                            .ok()
                            .map(|n| OutletId::new(n.id, 0))
                    })
                    .ok_or_else(|| load_err(what, format!("no output {name}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        model
            .select_output_outlets(&outlets)
            .map_err(|e| load_err(what, e))?;
    }
    model.analyse(true).map_err(|e| load_err(what, e))?;
    let mut typed = model.into_typed().map_err(|e| load_err(what, e))?;
    typed.declutter().map_err(|e| load_err(what, e))?;
    let pulsed = PulsedModel::new(&typed, s, &1.to_dim()).map_err(|e| load_err(what, e))?;
    pulsed
        .into_typed()
        .and_then(TypedModel::into_optimized)
        .and_then(|m| m.into_runnable())
        .map_err(|e| load_err(what, e))
}

/// The three networks, compiled.
pub(crate) struct Plans {
    encoder: Plan,
    erb_decoder: Plan,
    df_decoder: Plan,
}

impl Plans {
    /// Fresh state for one filter (cheap: the networks are shared).
    pub(crate) fn spawn(&self) -> Result<Graphs, AudioError> {
        let graph = |plan: &Plan| {
            plan.spawn()
                .map(|state| Graph { state })
                .map_err(|e| load_err("state", e))
        };
        Ok(Graphs {
            encoder: graph(&self.encoder)?,
            erb_decoder: graph(&self.erb_decoder)?,
            df_decoder: graph(&self.df_decoder)?,
        })
    }
}

/// The three graphs, with one filter's state.
pub(crate) struct Graphs {
    pub encoder: Graph,
    pub erb_decoder: Graph,
    pub df_decoder: Graph,
}

/// Loads and compiles the networks from `dir` (`enc.onnx`, `erb_dec.onnx`,
/// `df_dec.onnx`).
pub(crate) fn load(dir: &Path, cfg: &Config) -> Result<Plans, AudioError> {
    let f32t = f32::datum_type();
    let erb = cfg.nb_erb;
    let df = cfg.nb_df;
    let width = cfg.conv_ch;
    let hidden = width * erb / 4;
    let (erb_2, erb_4) = (erb / 2, erb / 4);
    let fact = move |shape: ShapeFactoid| InferenceFact::dt_shape(f32t, shape);

    let encoder = pulsed(
        &dir.join("enc.onnx"),
        |s| {
            vec![
                ("feat_erb", fact(shapefactoid!(1, 1, s, erb))),
                ("feat_spec", fact(shapefactoid!(1, 2, s, df))),
            ]
        },
        &["e0", "e1", "e2", "e3", "emb", "c0", "lsnr"],
    )?;
    let erb_decoder = pulsed(
        &dir.join("erb_dec.onnx"),
        |s| {
            vec![
                ("emb", fact(shapefactoid!(1, s, hidden))),
                ("e3", fact(shapefactoid!(1, width, s, erb_4))),
                ("e2", fact(shapefactoid!(1, width, s, erb_4))),
                ("e1", fact(shapefactoid!(1, width, s, erb_2))),
                ("e0", fact(shapefactoid!(1, width, s, erb))),
            ]
        },
        &[],
    )?;
    let df_decoder = pulsed(
        &dir.join("df_dec.onnx"),
        |s| {
            vec![
                ("emb", fact(shapefactoid!(1, s, hidden))),
                ("c0", fact(shapefactoid!(1, width, s, df))),
            ]
        },
        &["coefs"],
    )?;
    Ok(Plans {
        encoder,
        erb_decoder,
        df_decoder,
    })
}

impl Graphs {
    /// Runs the networks on one frame's features (`spec`: the real parts,
    /// then the imaginary ones): the encoder always, each decoder only when
    /// the local SNR asks for its output, so a skipped decoder's state stays
    /// where it was (as upstream's runtime does).
    pub(crate) fn infer(
        &mut self,
        cfg: &Config,
        erb: &[f32],
        spec: &[f32],
    ) -> Result<Inference, AudioError> {
        let tensor = |shape: &[usize], data: &[f32]| {
            Tensor::from_shape(shape, data)
                .map(TValue::from)
                .map_err(|_| AudioError::Internal)
        };
        let feat_erb = tensor(&[1, 1, 1, cfg.nb_erb], erb)?;
        let feat_spec = tensor(&[1, 2, 1, cfg.nb_df], spec)?;
        let mut out = self.encoder.run(tvec!(feat_erb, feat_spec))?;
        // e0, e1, e2, e3, emb, c0, lsnr
        if out.len() != 7 {
            return Err(AudioError::Internal);
        }
        let floats = |t: &TValue| -> Result<Vec<f32>, AudioError> {
            t.to_plain_array_view::<f32>()
                .map(|a| a.iter().copied().collect())
                .map_err(|_| AudioError::Internal)
        };
        let lsnr = floats(&out[6])?.first().copied().unwrap_or(0.0);
        let stage = stages(lsnr);
        let c0 = out.remove(5);
        let emb = out.remove(4);
        let gains = if stage.gains {
            let (e3, e2, e1, e0) = (out.remove(3), out.remove(2), out.remove(1), out.remove(0));
            let m = self.erb_decoder.run(tvec!(emb.clone(), e3, e2, e1, e0))?;
            Some(floats(m.first().ok_or(AudioError::Internal)?)?)
        } else if stage.zero_gains {
            Some(vec![0.0; cfg.nb_erb])
        } else {
            None
        };
        let coefs = if stage.deep_filter {
            let raw = self.df_decoder.run(tvec!(emb, c0))?;
            Some(coefficients(
                cfg,
                &floats(raw.first().ok_or(AudioError::Internal)?)?,
            )?)
        } else {
            None
        };
        Ok(Inference { lsnr, gains, coefs })
    }
}

/// The deep filter's coefficients (`[bin][tap]`, complex) from the
/// decoder's interleaved output.
pub(crate) fn coefficients(cfg: &Config, raw: &[f32]) -> Result<Vec<Complex32>, AudioError> {
    if raw.len() != cfg.nb_df * cfg.df_order * 2 {
        return Err(AudioError::Internal);
    }
    Ok(raw
        .chunks_exact(2)
        .map(|c| Complex32::new(c[0], c[1]))
        .collect())
}
