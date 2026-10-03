//! Runs DeepFilterNet3 frame by frame for profiling (`samply`, Nsight
//! Systems) and prints the frame times.
//!
//! `cargo run --release -p tessera-audio --features onnxruntime --example
//! dfn_profile -- <cpu|gpu|trt> [frames]`; the models from
//! `scripts/fetch-models.sh` and, for `gpu`, `scripts/export-deepfilternet.sh`,
//! `scripts/fetch-onnxruntime.sh cuda13` (and NVIDIA's libraries).

use std::time::{Duration, Instant};

use tessera_audio::{
    DeepFilter, FRAME_LEN, Frame, default_model_dir, flush_denormals_on_this_thread,
};

/// A voiced-like sound in white noise (as the onnxruntime test).
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

fn load(mode: &str) -> DeepFilter {
    let dir = default_model_dir().expect("model dir");
    match mode {
        "cpu" => DeepFilter::load(&dir, 100.0).expect("run scripts/fetch-models.sh"),
        #[cfg(feature = "onnxruntime")]
        "gpu" | "trt" => {
            use tessera_audio::onnx::{Accelerator, OnnxRuntime, default_runtime_dir};
            let runtime = OnnxRuntime::load(&default_runtime_dir().expect("ONNX Runtime"))
                .expect("load ONNX Runtime");
            let model = tessera_audio::default_stateful_model().expect("stateful model");
            let prefer = if mode == "trt" {
                Accelerator::TensorRt
            } else {
                Accelerator::Cuda
            };
            let (filter, accelerator) =
                DeepFilter::load_gpu(&dir, &model, 100.0, &runtime, &[prefer]).expect("load");
            eprintln!("running on {accelerator:?}");
            filter
        }
        other => panic!("unknown mode {other} (cpu; gpu, trt with --features onnxruntime)"),
    }
}

/// CPU time of every thread of this process so far (Linux: the kernel's
/// per-thread run time, nanoseconds), `None` elsewhere.
fn process_cpu_ns() -> Option<u64> {
    let tasks = std::fs::read_dir("/proc/self/task").ok()?;
    let mut total = 0;
    for task in tasks.flatten() {
        let stat = std::fs::read_to_string(task.path().join("schedstat")).ok()?;
        total += stat.split_whitespace().next()?.parse::<u64>().ok()?;
    }
    Some(total)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "cpu".into());
    let frames: usize = args.next().map_or(3000, |n| n.parse().expect("frames"));
    flush_denormals_on_this_thread();
    let started = Instant::now();
    let mut filter = load(&mode);
    eprintln!("loaded in {:?}", started.elapsed());

    let input = voice_in_noise(frames);
    let mut times = Vec::with_capacity(frames);
    let cpu_before = process_cpu_ns();
    let mut sink = 0.0_f32;
    for chunk in input.chunks_exact(FRAME_LEN) {
        let mut frame: Frame = chunk.try_into().unwrap();
        let start = Instant::now();
        filter.process(&mut frame).expect("process");
        times.push(start.elapsed());
        sink += frame[0];
    }
    let cpu = process_cpu_ns()
        .zip(cpu_before)
        .map(|(after, before)| Duration::from_nanos((after - before) / frames as u64));
    times.sort();
    let pct = |p: usize| times[(times.len() - 1) * p / 100];
    let mean = times.iter().sum::<Duration>() / times.len() as u32;
    println!(
        "{mode}: {frames} frames, mean {mean:?}, median {:?}, p99 {:?}, worst {:?}, \
         CPU per frame {cpu:?} ({:?}, {sink:.1e})",
        pct(50),
        pct(99),
        times.last().unwrap(),
        filter.status().get(),
    );
}
