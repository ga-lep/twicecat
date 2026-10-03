//! ONNX Runtime end to end (feature `onnxruntime`): the pinned library from
//! `scripts/fetch-onnxruntime.sh`, DeepFilterNet3's encoder from
//! `scripts/fetch-models.sh`, one frame through it; and the stateful graph
//! from `scripts/export-deepfilternet.sh` on the GPU against the CPU graphs.
//! The CUDA build finds NVIDIA's libraries installed system-wide or fetched
//! by `scripts/fetch-cuda-runtime.sh`.

#![cfg(feature = "onnxruntime")]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tessera_audio::onnx::{Accelerator, OnnxRuntime, default_runtime_dir};
use tessera_audio::{
    DeepFilter, DeepFilterBackend, FRAME_LEN, Frame, default_model_dir, default_stateful_model,
    flush_denormals_on_this_thread,
};

#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh and scripts/fetch-models.sh"]
fn the_pinned_runtime_runs_deepfilternets_encoder() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
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

/// One test on the GPU at a time: a CUDA graph capture in one forbids other
/// threads' CUDA calls (the app never runs two GPU filters at once).
static GPU: Mutex<()> = Mutex::new(());

/// Two seconds of a voiced-like sound (harmonics of 140 Hz, syllables at
/// 4 Hz) in white noise, repeated: deterministic, and it makes every stage
/// of the filter run (noise-only, mixed and clean-ish frames).
fn voice_in_noise(frames: usize) -> Vec<f32> {
    let mut seed = 0x9e37_79b9_u32;
    (0..frames * FRAME_LEN)
        .map(|n| {
            let t = n as f32 / 48_000.0;
            let syllable = (std::f32::consts::TAU * 4.0 * t).sin().max(0.0);
            let voice: f32 = (1..=12)
                .map(|h| (std::f32::consts::TAU * 140.0 * h as f32 * t).sin() / h as f32)
                .sum();
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            0.1 * syllable * voice + 0.03 * noise
        })
        .collect()
}

/// Runs `filter` over `input`; returns the output and the sorted frame times.
fn filter(input: &[f32], filter: &mut DeepFilter) -> (Vec<f32>, Vec<Duration>) {
    let mut out = Vec::with_capacity(input.len());
    let mut times = Vec::new();
    for chunk in input.chunks_exact(FRAME_LEN) {
        let mut frame: Frame = chunk.try_into().unwrap();
        let start = Instant::now();
        filter.process(&mut frame).expect("process");
        times.push(start.elapsed());
        out.extend_from_slice(&frame);
    }
    times.sort();
    (out, times)
}

#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh cuda13, fetch-models.sh, export-deepfilternet.sh and a GPU"]
fn deepfilternet_on_the_gpu_sounds_like_on_the_cpu() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    flush_denormals_on_this_thread();
    let runtime =
        OnnxRuntime::load(&default_runtime_dir().expect("run scripts/fetch-onnxruntime.sh"))
            .expect("load ONNX Runtime");
    let dir = default_model_dir().unwrap();
    let model = default_stateful_model().unwrap();
    assert!(model.exists(), "run scripts/export-deepfilternet.sh");
    let input = voice_in_noise(1000);
    let mut cpu = DeepFilter::load(&dir, 100.0).expect("load the CPU graphs");
    let (on_cpu, cpu_times) = filter(&input, &mut cpu);
    let median = |t: &[Duration]| t[t.len() / 2];
    eprintln!(
        "CPU graphs per frame: median {:?} (worst {:?})",
        median(&cpu_times),
        cpu_times.last().unwrap()
    );

    // Each GPU backend this runtime has (TensorRT needs scripts/fetch-tensorrt.sh;
    // its first engine build takes about 20 s, cached after).
    let mut compared = 0;
    for candidate in [
        Accelerator::Cuda,
        Accelerator::TensorRt,
        Accelerator::DirectMl,
        Accelerator::CoreMl,
    ] {
        if !runtime.accelerators().contains(&candidate) {
            continue;
        }
        let started = Instant::now();
        let (mut gpu, accelerator) =
            DeepFilter::load_gpu(&dir, &model, 100.0, &runtime, &[candidate])
                .expect("load DeepFilterNet");
        if accelerator != Some(candidate) {
            eprintln!("{candidate:?} did not start here");
            continue;
        }
        let loaded = started.elapsed();
        let (on_gpu, gpu_times) = filter(&input, &mut gpu);
        assert_eq!(gpu.status().get(), DeepFilterBackend::Gpu, "no fallback");
        let energy = |x: &mut dyn Iterator<Item = f32>| x.map(|s| s * s).sum::<f32>();
        let signal = energy(&mut on_cpu.iter().copied());
        let difference = energy(&mut on_cpu.iter().zip(&on_gpu).map(|(c, g)| c - g));
        let agreement_db = 10.0 * (signal / difference.max(1e-20)).log10();
        eprintln!(
            "{candidate:?} (loaded in {loaded:?}) against the CPU graphs: {agreement_db:.1} dB \
             apart; per frame median {:?} (p99 {:?}, worst {:?})",
            median(&gpu_times),
            gpu_times[gpu_times.len() * 99 / 100],
            gpu_times.last().unwrap(),
        );
        // Measured: CUDA 91 dB from the CPU's output, TensorRT 86 to 88 dB
        // (it times kernels when it builds an engine, so builds differ a
        // little). The bar for them sits well above the 26 to 40 dB that a
        // race between our copies and TensorRT's replay gave; providers not
        // measured yet get a looser one, still far below anything audible.
        let bar = if matches!(candidate, Accelerator::Cuda | Accelerator::TensorRt) {
            60.0
        } else {
            40.0
        };
        assert!(
            agreement_db > bar,
            "{candidate:?} output differs: {agreement_db:.1} dB"
        );
        assert!(median(&gpu_times) < Duration::from_millis(4));
        compared += 1;
    }
    if compared == 0 {
        eprintln!("no GPU backend started here: nothing to compare");
    }
}

#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh cuda13, export-deepfilternet.sh and a GPU"]
fn the_gpu_loops_match_the_host_loop_frame_by_frame() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    use tessera_audio::onnx::LoopSpec;
    let runtime = OnnxRuntime::load(&default_runtime_dir().expect("ONNX Runtime")).unwrap();
    let model = default_stateful_model().unwrap();
    let spec = LoopSpec {
        input: ("features", 32 + 2 * 96),
        output: ("out", 32 + 960 + 1),
        constants: &[("thresholds", &[-15.0, 35.0, 35.0])],
        state: ("state", "state_out"),
        dimensions: &[("batch", 1)],
    };
    for candidate in [Accelerator::Cuda, Accelerator::TensorRt] {
        if !runtime.accelerators().contains(&candidate) {
            continue;
        }
        let (mut gpu, accelerator) = runtime.open_loop(&model, &[candidate], &spec).unwrap();
        if accelerator != Some(candidate) {
            eprintln!("{candidate:?} did not start here");
            continue;
        }
        let (mut cpu, none) = runtime.open_loop(&model, &[], &spec).unwrap();
        assert_eq!(none, None);

        // Random features (far outside speech, which makes the recurrent
        // state sensitive): the GPU drifts from the CPU by rounding only.
        let mut seed = 0x1234_5678_u32;
        let mut worst = 0.0_f32;
        for frame in 0..300 {
            if frame == 150 {
                gpu.reset().unwrap();
                cpu.reset().unwrap();
            }
            let input: Vec<f32> = (0..spec.input.1)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
                })
                .collect();
            let on_cpu = cpu.run(&input).unwrap().to_vec();
            let on_gpu = gpu.run(&input).unwrap();
            worst = on_cpu
                .iter()
                .zip(on_gpu)
                .map(|(c, g)| (c - g).abs())
                .fold(worst, f32::max);
        }
        eprintln!(
            "{candidate:?} (CUDA graphs: {}): largest difference over 300 frames: {worst}",
            gpu.uses_cuda_graphs()
        );
        assert!(worst < 0.05, "{candidate:?}: {worst}");
    }
}
