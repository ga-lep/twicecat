//! The CPU voice path, stage by stage: each processing configuration on a
//! realistic signal (a voice and noise on the microphone, a far-end voice
//! played back and heard again as echo), then the whole voice engine with
//! remote voices mixed in. Prints per-frame cost (mean, median, p99,
//! worst) and heap allocations per frame, and can run one configuration
//! alone for `perf`.
//!
//! `cargo run --release -p twicecat-audio --example voice_profile -- [name]
//! [frames]` (no name: every configuration). DeepFilterNet rows need
//! `scripts/fetch-models.sh`. On a hybrid CPU, run it on an efficiency core
//! too (`taskset -c <cpu>`): the processing thread often runs there. With
//! `--features onnxruntime` (and the GPU parts of Task 13b installed), the
//! "GPU bank" rows clean the remote voices in one batched GPU call per
//! frame (Task 16).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use twicecat_audio::{
    CaptureChain, ChainSettings, DeepFilter, DeepFilterModel, FRAME_LEN, Frame, FrameProcessor,
    GateMode, NoiseMode, NsLevel, SILENCE, VoiceEngine, default_model_dir,
    flush_denormals_on_this_thread,
};

/// Counts heap allocations (and reallocations), process-wide; the harness
/// is single-threaded.
struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

/// `VOICE_PROFILE_TRACE=n`: where the first `n` allocations of the measured
/// frames come from (their stacks, counted by caller), to find who
/// allocates on the processing thread.
static TRACE_LEFT: AtomicU64 = AtomicU64::new(0);
static TRACING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    static IN_TRACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TRACES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Records the current stack (re-entrancy guarded: capturing allocates).
fn trace_allocation() {
    if !TRACING.load(Ordering::Relaxed) || TRACE_LEFT.load(Ordering::Relaxed) == 0 {
        return;
    }
    IN_TRACE.with(|guard| {
        if guard.replace(true) {
            return;
        }
        TRACE_LEFT.fetch_sub(1, Ordering::Relaxed);
        let trace = std::backtrace::Backtrace::force_capture().to_string();

        // The first frame naming an audio crate (names carry their module
        // paths only inside generics), and the two that called it.
        let frames: Vec<String> = trace
            .lines()
            .filter(|l| {
                l.trim_start()
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
            .map(|l| l.split_once(": ").map_or(l, |(_, f)| f).trim().to_owned())
            .collect();
        let crates = [
            "sonora",
            "nnnoiseless",
            "tract",
            "twicecat_audio",
            "realfft",
            "rustfft",
            "ort",
        ];
        let caller = frames
            .iter()
            .position(|f| crates.iter().any(|c| f.contains(c)))
            .map_or_else(
                || "?".to_owned(),
                |i| {
                    frames[i..frames.len().min(i + 3)]
                        .iter()
                        .map(|f| f.chars().take(110).collect::<String>())
                        .collect::<Vec<_>>()
                        .join(" < ")
                },
            );
        TRACES.with(|t| t.borrow_mut().push(caller));
        guard.set(false);
    });
}

// SAFETY: forwards to the system allocator unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        trace_allocation();
        // SAFETY: as the caller of `alloc` guarantees.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller of `dealloc` guarantees.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        trace_allocation();
        // SAFETY: as the caller of `realloc` guarantees.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const RATE: f32 = 48_000.0;
/// Echo path: 40 ms late, -10 dB.
const ECHO_DELAY: usize = 1920;
const ECHO_GAIN: f32 = 0.3;
/// Frames run before measuring (filters converging, buffers sized).
const WARM_UP: usize = 300;

/// A voiced-like sound: harmonics of `pitch`, syllables at `rate` Hz.
fn voice(n: usize, pitch: f32, rate: f32, amplitude: f32) -> f32 {
    let t = n as f32 / RATE;
    let syllable = (std::f32::consts::TAU * rate * t).sin().max(0.0);
    let tone: f32 = (1..=10)
        .map(|h| (std::f32::consts::TAU * pitch * h as f32 * t).sin() / h as f32)
        .sum();
    amplitude * syllable * tone
}

/// The signals for `frames` frames: microphone (near voice, echo of the
/// far voice, noise) and what is played (the far voice).
fn signals(frames: usize) -> (Vec<f32>, Vec<f32>) {
    let len = frames * FRAME_LEN;
    // One second of each voice, repeated (generating every sample took more
    // time than the processing under `perf`).
    let second = RATE as usize;
    let far_second: Vec<f32> = (0..second).map(|n| voice(n, 210.0, 3.0, 0.15)).collect();
    let near_second: Vec<f32> = (0..second).map(|n| voice(n, 140.0, 4.0, 0.1)).collect();
    let far: Vec<f32> = (0..len).map(|n| far_second[n % second]).collect();
    let mut seed = 0x9e37_79b9_u32;
    let mic = (0..len)
        .map(|n| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * 0.01;
            let echo = n
                .checked_sub(ECHO_DELAY)
                .map_or(0.0, |m| far[m] * ECHO_GAIN);
            near_second[n % second] + echo + noise
        })
        .collect();
    (mic, far)
}

fn settings(
    echo_cancellation: bool,
    high_pass: bool,
    auto_gain: bool,
    noise: NoiseMode,
) -> ChainSettings {
    ChainSettings {
        echo_cancellation,
        high_pass,
        auto_gain,
        noise,
        gate: GateMode::VoiceActivity {
            automatic: true,
            threshold_db: -50.0,
        },
        input_gain_db: 0.0,
    }
}

/// Per-frame times and allocations of a run.
struct Run {
    times: Vec<Duration>,
    allocations: u64,
}

impl Run {
    fn print(mut self, name: &str) {
        self.times.sort();
        let n = self.times.len();
        let mean = self.times.iter().sum::<Duration>() / n as u32;
        let us = |d: Duration| d.as_secs_f64() * 1e6;
        println!(
            "{name:<34} {:>8.1} {:>8.1} {:>8.1} {:>8.1} {:>8.1}",
            us(mean),
            us(self.times[n / 2]),
            us(self.times[n * 99 / 100]),
            us(self.times[n - 1]),
            self.allocations as f64 / n as f64,
        );
    }
}

/// Runs `step` over every frame; measures after the warm-up.
fn run(frames: usize, mut step: impl FnMut(usize)) -> Run {
    let mut times = Vec::with_capacity(frames);
    let mut allocations = 0;
    for i in 0..WARM_UP + frames {
        TRACING.store(i >= WARM_UP, Ordering::Relaxed);
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        let start = Instant::now();
        step(i);
        let took = start.elapsed();
        let after = ALLOCATIONS.load(Ordering::Relaxed);
        if i >= WARM_UP {
            times.push(took);
            allocations += after - before;
        }
    }
    TRACING.store(false, Ordering::Relaxed);
    Run { times, allocations }
}

/// Prints the traced allocations by caller, and starts a new trace.
fn print_traces(budget: u64) {
    let traces = TRACES.with(|t| std::mem::take(&mut *t.borrow_mut()));
    TRACE_LEFT.store(budget, Ordering::Relaxed);
    if traces.is_empty() {
        return;
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for caller in traces {
        *counts.entry(caller).or_default() += 1;
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    for (caller, n) in counts.iter().take(12) {
        println!("      {n:>5}  {caller}");
    }
}

fn frame_of(signal: &[f32], i: usize) -> Frame {
    signal[i * FRAME_LEN..(i + 1) * FRAME_LEN]
        .try_into()
        .expect("a frame")
}

/// The capture chain alone (render, then capture, each frame).
fn chain(name: &str, s: ChainSettings, mic: &[f32], far: &[f32], frames: usize) {
    let mut chain = CaptureChain::new(s);
    if s.noise == NoiseMode::Enhancer {
        let Some(filter) = default_model_dir().and_then(|d| DeepFilter::load(&d, 100.0).ok())
        else {
            println!("{name:<34} (no models: scripts/fetch-models.sh)");
            return;
        };
        chain.set_enhancer(Some(Box::new(filter)));
    }
    let result = run(frames, |i| {
        let mut frame = frame_of(mic, i);
        chain.render(&frame_of(far, i));
        chain.capture(&mut frame);
        std::hint::black_box(&frame);
    });
    result.print(name);
}

/// The whole engine: commands, the mix of `sources` remote voices (the
/// first `cleaned` of them cleaned by DeepFilterNet3, Task 16; only the
/// first `talking` talk, the others send silence as a muted or gated voice
/// does), the chain on the microphone, the outgoing ring.
fn engine(
    name: &str,
    s: ChainSettings,
    (sources, cleaned, talking): (usize, usize, usize),
    gpu: bool,
    mic: &[f32],
    frames: usize,
) {
    let model = if cleaned > 0 && !gpu {
        let Some(model) = default_model_dir().and_then(|d| DeepFilterModel::load(&d).ok()) else {
            println!("{name:<34} (no DeepFilterNet model)");
            return;
        };
        Some(model)
    } else {
        None
    };
    let (mut engine, mut control, mut outgoing) = VoiceEngine::new(s);
    let mut name = name.to_owned();
    if gpu {
        match gpu_bank() {
            Some((bank, on)) => {
                name = format!("{name} ({on})");
                control.set_cleaner_bank(Some(bank));
            }
            None => {
                println!(
                    "{name:<34} (no GPU bank: needs --features onnxruntime and the GPU parts)"
                );
                return;
            }
        }
    }
    let mut inputs: Vec<_> = (0..sources)
        .map(|k| {
            let (id, input) = control.add_source(100).expect("a source");
            if let Some(model) = model.as_ref().filter(|_| k < cleaned) {
                let filter = model.filter(100.0).expect("a filter");
                control.set_source_cleaner(id, Some(Box::new(filter)));
            }
            if gpu && k < cleaned {
                control.set_source_row(id, Some(k));
            }
            input
        })
        .collect();
    let remote: Vec<Vec<f32>> = (0..sources)
        .map(|k| {
            (0..FRAME_LEN * 50)
                .map(|n| {
                    if k < talking {
                        voice(n, 120.0 + 15.0 * k as f32, 3.0 + k as f32 * 0.3, 0.05)
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
    let mut sent = SILENCE;
    let mut playback = SILENCE;
    let result = run(frames, |i| {
        // What the network delivered for this frame (outside the measured
        // engine work in a call, but cheap ring pushes).
        for (input, voice) in inputs.iter_mut().zip(&remote) {
            input.push(&voice[(i % 50) * FRAME_LEN..(i % 50 + 1) * FRAME_LEN]);
        }
        playback = SILENCE;
        engine.process(&frame_of(mic, i), &mut playback);
        while outgoing.pop_frame(&mut sent) {}
        std::hint::black_box((&playback, &sent));
    });
    result.print(&name);
    control.drop_retired();
}

/// A bank of 16 rows on the first GPU backend that starts, and its name.
#[cfg(feature = "onnxruntime")]
fn gpu_bank() -> Option<(Box<dyn twicecat_audio::BatchCleaner>, String)> {
    use twicecat_audio::onnx::{Accelerator, OnnxRuntime, default_runtime_dir};
    let runtime = OnnxRuntime::load(&default_runtime_dir()?).ok()?;
    let bank = twicecat_audio::DeepFilterBank::load(
        &default_model_dir()?,
        &twicecat_audio::default_stateful_model()?,
        16,
        100.0,
        &runtime,
        &Accelerator::ALL,
    )
    .ok()??;
    let on = format!("{:?}", bank.accelerator());
    Some((Box::new(bank), on))
}

#[cfg(not(feature = "onnxruntime"))]
fn gpu_bank() -> Option<(Box<dyn twicecat_audio::BatchCleaner>, String)> {
    None
}

/// DeepFilterNet3's network alone (one frame of features in, gains and
/// coefficients out), on ONNX Runtime's CPU provider through the stateful
/// graph, to compare with tract (whose row includes the DSP, about 20 µs).
#[cfg(feature = "onnxruntime")]
fn onnx_cpu_network(name: &str, frames: usize) {
    use twicecat_audio::onnx::{LoopSpec, OnnxRuntime, default_runtime_dir};
    let (Some(dir), Some(model)) = (
        default_runtime_dir(),
        twicecat_audio::default_stateful_model(),
    ) else {
        println!("{name:<34} (no ONNX Runtime or stateful model)");
        return;
    };
    let runtime = OnnxRuntime::load(&dir).expect("ONNX Runtime");
    let spec = LoopSpec {
        input: ("features", 32 + 2 * 96),
        output: ("out", 32 + 960 + 1),
        constants: &[("thresholds", &[-15.0, 35.0, 35.0])],
        state: ("state", "state_out"),
        dimensions: &[("batch", 1)],
        rows: 1,
    };
    let (mut network, _) = runtime.open_loop(&model, &[], &spec).expect("open");
    let features: Vec<f32> = (0..spec.input.1)
        .map(|i| (i as f32 * 0.37).sin() * 0.3)
        .collect();
    let result = run(frames, |_| {
        std::hint::black_box(network.run(&features).expect("run"));
    });
    result.print(name);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let only = args.next().filter(|a| a != "all");
    let frames: usize = args.next().map_or(3000, |n| n.parse().expect("frames"));
    flush_denormals_on_this_thread();
    let (mic, far) = signals(WARM_UP + frames);
    let trace: u64 = std::env::var("VOICE_PROFILE_TRACE")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    TRACE_LEFT.store(trace, Ordering::Relaxed);

    let rows: Vec<(&str, ChainSettings)> = vec![
        (
            "chain: nothing (gate, limiter)",
            settings(false, false, false, NoiseMode::Off),
        ),
        (
            "chain: high-pass",
            settings(false, true, false, NoiseMode::Off),
        ),
        ("chain: AEC3", settings(true, false, false, NoiseMode::Off)),
        (
            "chain: WebRTC NS (high)",
            settings(false, false, false, NoiseMode::WebRtc(NsLevel::High)),
        ),
        (
            "chain: RNNoise",
            settings(false, false, false, NoiseMode::RnNoise),
        ),
        ("chain: AGC2", settings(false, false, true, NoiseMode::Off)),
        (
            "chain: DeepFilterNet3 (CPU)",
            settings(false, false, false, NoiseMode::Enhancer),
        ),
        (
            "chain: default (HPF AEC3 RNN AGC2)",
            settings(true, true, true, NoiseMode::RnNoise),
        ),
        (
            "chain: default with DeepFilterNet3",
            settings(true, true, true, NoiseMode::Enhancer),
        ),
    ];
    println!(
        "{:<34} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "per 10 ms frame (µs)", "mean", "median", "p99", "worst", "allocs"
    );
    for (name, s) in &rows {
        if only.as_deref().is_none_or(|o| name.contains(o)) {
            chain(name, *s, &mic, &far, frames);
            print_traces(trace);
        }
    }
    #[cfg(feature = "onnxruntime")]
    if only
        .as_deref()
        .is_none_or(|o| "network: ONNX Runtime CPU".contains(o))
    {
        onnx_cpu_network("network: ONNX Runtime CPU", frames);
    }
    let default = settings(true, true, true, NoiseMode::RnNoise);
    for (sources, cleaned, talking, gpu) in [
        (0, 0, 0, false),
        (4, 0, 4, false),
        (16, 0, 16, false),
        (4, 1, 4, false),
        (4, 4, 4, false),
        (8, 8, 8, false),
        (8, 8, 2, false),
        (16, 16, 1, false),
        (1, 1, 1, true),
        (4, 4, 4, true),
        (16, 16, 16, true),
        (16, 16, 1, true),
    ] {
        let name = match (cleaned, talking, gpu) {
            (0, ..) => format!("engine: default, {sources} voices mixed"),
            (c, t, true) => format!("engine: GPU bank, {sources} voices, {c} cleaned, {t} talk"),
            (c, t, _) if t == sources => format!("engine: default, {sources} voices, {c} cleaned"),
            (c, t, _) => format!("engine: {sources} voices, {c} cleaned, {t} talk"),
        };
        if only.as_deref().is_none_or(|o| name.contains(o)) {
            engine(
                &name,
                default,
                (sources, cleaned, talking),
                gpu,
                &mic,
                frames,
            );
            print_traces(trace);
        }
    }
}
