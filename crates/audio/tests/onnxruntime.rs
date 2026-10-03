//! ONNX Runtime end to end (feature `onnxruntime`): the pinned library from
//! `scripts/fetch-onnxruntime.sh`, DeepFilterNet3's encoder from
//! `scripts/fetch-models.sh`, one frame through it.

#![cfg(feature = "onnxruntime")]

use std::time::{Duration, Instant};

use tessera_audio::default_model_dir;
use tessera_audio::onnx::{Accelerator, OnnxRuntime, default_runtime_dir};

#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh and scripts/fetch-models.sh"]
fn the_pinned_runtime_runs_deepfilternets_encoder() {
    let dir = default_runtime_dir().expect("run scripts/fetch-onnxruntime.sh");
    let runtime = OnnxRuntime::load(&dir).expect("load ONNX Runtime");
    eprintln!(
        "runtime {}: accelerators {:?}",
        dir.display(),
        runtime.accelerators()
    );

    let model = default_model_dir().unwrap().join("enc.onnx");
    // Ask for the GPU; without one (or with the CPU build) the CPU runs.
    let (mut session, accelerator) = runtime
        .session(&model, &[Accelerator::Cuda, Accelerator::TensorRt])
        .expect("open the encoder");
    eprintln!(
        "running on {}",
        accelerator.map_or("the CPU".into(), |a| format!("{a:?}"))
    );
    assert_eq!(session.input_names(), ["feat_erb", "feat_spec"]);

    // One frame of features (time axis 1).
    let erb: Vec<f32> = (0..32).map(|i| -0.5 + i as f32 / 64.0).collect();
    let spec: Vec<f32> = (0..2 * 96)
        .map(|i| ((i as f32) * 0.37).sin() * 0.1)
        .collect();
    let mut times = Vec::new();
    let mut outputs = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        outputs = session
            .run(vec![
                ("feat_erb", (vec![1, 1, 1, 32], erb.clone())),
                ("feat_spec", (vec![1, 2, 1, 96], spec.clone())),
            ])
            .expect("inference");
        times.push(start.elapsed());
    }
    let shapes: Vec<Vec<usize>> = outputs.iter().map(|(s, _)| s.clone()).collect();
    assert_eq!(
        shapes,
        [
            vec![1, 64, 1, 32],
            vec![1, 64, 1, 16],
            vec![1, 64, 1, 8],
            vec![1, 64, 1, 8],
            vec![1, 1, 512],
            vec![1, 64, 1, 96],
            vec![1, 1, 1],
        ],
        "e0, e1, e2, e3, emb, c0, lsnr"
    );
    assert!(outputs.iter().all(|(_, v)| v.iter().all(|x| x.is_finite())));
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "encoder, one frame: median {median:?}, worst {:?}",
        times.last().unwrap()
    );
    assert!(median < Duration::from_millis(10));
}
