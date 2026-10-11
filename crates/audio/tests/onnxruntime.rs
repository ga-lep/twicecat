//! ONNX Runtime end to end (feature `onnxruntime`): the pinned library from
//! `scripts/fetch-onnxruntime.sh`, DeepFilterNet3's encoder from
//! `scripts/fetch-models.sh`, one frame through it; and the stateful graph
//! from `scripts/export-deepfilternet.sh` on the GPU against the CPU graphs.
//! The CUDA build finds NVIDIA's libraries installed system-wide or fetched
//! by `scripts/fetch-cuda-runtime.sh`.
//!
//! Run together, these tests also check that the process exits cleanly: a
//! WebGPU device still alive at exit crashed it inside NVIDIA's Vulkan driver
//! (`crate::onnx` now releases the device with the last WebGPU session).

#![cfg(feature = "onnxruntime")]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use twicecat_audio::onnx::{Accelerator, OnnxRuntime, default_runtime_dir};
use twicecat_audio::{
    BatchCleaner, DeepFilter, DeepFilterBackend, DeepFilterBank, FRAME_LEN, Frame,
    default_model_dir, default_stateful_model, flush_denormals_on_this_thread,
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
    voice_in_noise_at(frames, 140.0, 0x9e37_79b9)
}

/// As [`voice_in_noise`], at another pitch and with other noise.
fn voice_in_noise_at(frames: usize, pitch: f32, mut seed: u32) -> Vec<f32> {
    (0..frames * FRAME_LEN)
        .map(|n| {
            let t = n as f32 / 48_000.0;
            let syllable = (std::f32::consts::TAU * 4.0 * t).sin().max(0.0);
            let voice: f32 = (1..=12)
                .map(|h| (std::f32::consts::TAU * pitch * h as f32 * t).sin() / h as f32)
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
    // its first engine build takes about 20 s, cached after; WebGPU needs
    // scripts/fetch-webgpu-ep.sh).
    let mut compared = 0;
    for candidate in [
        Accelerator::Cuda,
        Accelerator::TensorRt,
        Accelerator::WebGpu,
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
        // little), WebGPU 133 dB, DirectML 132 dB. The bar for them sits well
        // above the 26 to 40 dB that a race between our copies and TensorRT's
        // replay gave; providers not measured yet get a looser one, still far
        // below anything audible.
        let bar = if matches!(
            candidate,
            Accelerator::Cuda | Accelerator::TensorRt | Accelerator::WebGpu | Accelerator::DirectMl
        ) {
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

/// How close `b` is to `a`, dB (signal over difference).
fn agreement_db(a: &[f32], b: &[f32]) -> f32 {
    let energy = |x: &mut dyn Iterator<Item = f32>| x.map(|s| s * s).sum::<f32>();
    let signal = energy(&mut a.iter().copied());
    let difference = energy(&mut a.iter().zip(b).map(|(x, y)| x - y));
    10.0 * (signal / difference.max(1e-20)).log10()
}

/// Several voices in one batched GPU call per frame (plan Task 16): each
/// comes out as its own CPU filter makes it, in any row, and a row given to
/// a new voice halfway starts afresh while the others go on. Also prints
/// what a call costs for 1, 4 and 16 rows.
#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh cuda13, fetch-models.sh, export-deepfilternet.sh and a GPU"]
fn a_bank_cleans_each_voice_as_its_own_filter_would() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    flush_denormals_on_this_thread();
    let runtime =
        OnnxRuntime::load(&default_runtime_dir().expect("run scripts/fetch-onnxruntime.sh"))
            .expect("load ONNX Runtime");
    let dir = default_model_dir().unwrap();
    let model = default_stateful_model().unwrap();
    assert!(model.exists(), "run scripts/export-deepfilternet.sh");
    const FRAMES: usize = 600;
    const SWAP: usize = 300;
    let voices: Vec<Vec<f32>> = [(110.0, 1), (140.0, 2), (180.0, 3), (220.0, 4), (95.0, 5)]
        .iter()
        .map(|&(pitch, seed)| voice_in_noise_at(FRAMES, pitch, 0x9e37_79b9 ^ seed))
        .collect();
    // Voices 0 to 3 in rows 0, 5, 9, 15; voice 4 takes row 9 at `SWAP`.
    let rows = [0, 5, 9, 15];
    let reference = |voice: usize, from: usize| {
        let mut cpu = DeepFilter::load(&dir, 100.0).expect("load the CPU graphs");
        filter(&voices[voice][from * FRAME_LEN..], &mut cpu).0
    };
    let expected: Vec<Vec<f32>> = (0..4).map(|v| reference(v, 0)).collect();
    let newcomer = reference(4, SWAP);

    let mut compared = 0;
    for candidate in [
        Accelerator::Cuda,
        Accelerator::TensorRt,
        Accelerator::WebGpu,
        Accelerator::DirectMl,
    ] {
        if !runtime.accelerators().contains(&candidate) {
            continue;
        }
        let mut costs = Vec::new();
        for size in [1, 4] {
            if let Some(small) =
                DeepFilterBank::load(&dir, &model, size, 100.0, &runtime, &[candidate]).unwrap()
            {
                costs.push((size, small.call_time()));
            }
        }
        let Some(mut bank) =
            DeepFilterBank::load(&dir, &model, 16, 100.0, &runtime, &[candidate]).unwrap()
        else {
            eprintln!("{candidate:?} did not start here, or is too slow for 16 rows");
            continue;
        };
        costs.push((16, bank.call_time()));
        for &row in &rows {
            bank.reset_row(row).unwrap();
        }
        let mut outputs = vec![Vec::new(); 5];
        let mut times = Vec::new();
        for frame in 0..FRAMES {
            if frame == SWAP {
                bank.reset_row(9).unwrap();
            }
            let playing = if frame < SWAP {
                [0, 1, 2, 3]
            } else {
                [0, 1, 4, 3]
            };
            let mut frames: Vec<Frame> = playing
                .iter()
                .map(|&v| {
                    voices[v][frame * FRAME_LEN..][..FRAME_LEN]
                        .try_into()
                        .unwrap()
                })
                .collect();
            let start = Instant::now();
            bank.clean(&mut frames, &rows).expect("clean");
            times.push(start.elapsed());
            for (&v, out) in playing.iter().zip(&frames) {
                outputs[v].extend_from_slice(out);
            }
        }
        times.sort();
        let agreements: Vec<f32> = (0..4)
            .map(|v| {
                let len = outputs[v].len();
                agreement_db(&expected[v][..len], &outputs[v])
            })
            .chain([agreement_db(&newcomer, &outputs[4])])
            .collect();
        eprintln!(
            "{candidate:?}, 16 rows, 4 voices: agreement with each voice's CPU filter {agreements:.1?} dB; \
             per frame (4 voices' signal side and the call) median {:?}, p99 {:?}; \
             a call at warm-up: {costs:?}",
            times[times.len() / 2],
            times[times.len() * 99 / 100],
        );
        // As the single-voice GPU filter against the CPU (60 dB there).
        for (v, a) in agreements.iter().enumerate() {
            assert!(*a > 60.0, "{candidate:?}, voice {v}: {a:.1} dB");
        }
        compared += 1;
    }
    if compared == 0 {
        eprintln!("no GPU backend started here: nothing to compare");
    }
}

/// A bank loads (and captures its graph) while the microphone's GPU filter
/// runs on another thread, as in a call: neither may fail or fall back.
#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh cuda13, fetch-models.sh, export-deepfilternet.sh and a GPU"]
fn a_bank_loads_while_a_gpu_filter_runs() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let runtime =
        OnnxRuntime::load(&default_runtime_dir().expect("run scripts/fetch-onnxruntime.sh"))
            .expect("load ONNX Runtime");
    let dir = default_model_dir().unwrap();
    let model = default_stateful_model().unwrap();
    for candidate in [
        Accelerator::Cuda,
        Accelerator::TensorRt,
        Accelerator::WebGpu,
        Accelerator::DirectMl,
    ] {
        if !runtime.accelerators().contains(&candidate) {
            continue;
        }
        let (mut mic, accelerator) =
            DeepFilter::load_gpu(&dir, &model, 100.0, &runtime, &[candidate]).unwrap();
        if accelerator != Some(candidate) {
            continue;
        }
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let running = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let input = voice_in_noise(100);
                let mut frames = 0_usize;
                let mut slowest = Duration::ZERO;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for chunk in input.chunks_exact(FRAME_LEN) {
                        let mut frame: Frame = chunk.try_into().unwrap();
                        let start = Instant::now();
                        mic.process(&mut frame).expect("the microphone's filter");
                        slowest = slowest.max(start.elapsed());
                        frames += 1;
                        // About real time: a frame every 10 ms at most.
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                (mic.status().get(), frames, slowest)
            })
        };
        std::thread::sleep(Duration::from_millis(200));
        let mut banks = 0;
        for rows in [16, 4, 16] {
            let bank = DeepFilterBank::load(&dir, &model, rows, 100.0, &runtime, &[candidate])
                .expect("load a bank");
            assert!(bank.is_some(), "{candidate:?}: the bank did not start");
            banks += 1;
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let (status, frames, slowest) = running.join().unwrap();
        eprintln!(
            "{candidate:?}: {banks} banks loaded beside the microphone's filter ({frames} frames, slowest {slowest:?}); it ends {status:?}"
        );
        assert_eq!(
            status,
            DeepFilterBackend::Gpu,
            "{candidate:?}: the microphone's filter fell back"
        );
    }
}

#[test]
#[ignore = "needs scripts/fetch-onnxruntime.sh cuda13, export-deepfilternet.sh and a GPU"]
fn the_gpu_loops_match_the_host_loop_frame_by_frame() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    use twicecat_audio::onnx::LoopSpec;
    let runtime = OnnxRuntime::load(&default_runtime_dir().expect("ONNX Runtime")).unwrap();
    let model = default_stateful_model().unwrap();
    let spec = LoopSpec {
        input: ("features", 32 + 2 * 96),
        output: ("out", 32 + 960 + 1),
        constants: &[("thresholds", &[-15.0, 35.0, 35.0])],
        state: ("state", "state_out"),
        dimensions: &[("batch", 1)],
        rows: 1,
    };
    for candidate in [
        Accelerator::Cuda,
        Accelerator::TensorRt,
        Accelerator::WebGpu,
        Accelerator::DirectMl,
    ] {
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
            "{candidate:?} (replayed graph: {}): largest difference over 300 frames: {worst}",
            gpu.replays_a_graph()
        );
        assert!(worst < 0.05, "{candidate:?}: {worst}");
    }
}
