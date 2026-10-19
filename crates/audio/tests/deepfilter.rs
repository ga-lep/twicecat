//! DeepFilterNet3 with the fetched models (`scripts/fetch-models.sh`, or
//! `TWICECAT_MODEL_DIR`): latency, cost per frame, and noise reduction
//! against RNNoise on the same input.
//!
//! On real speech: set `TWICECAT_SPEECH_WAV` and `TWICECAT_NOISE_WAV` to
//! 48 kHz mono 16-bit WAV files (not shipped: recordings carry their own
//! licences).

use std::time::{Duration, Instant};

use twicecat_audio::{
    CaptureChain, ChainSettings, DeepFilter, DeepFilterModel, FRAME_LEN, Frame, GateMode,
    LEVEL_FLOOR_DB, NoiseMode, default_model_dir, flush_denormals_on_this_thread,
};

fn load() -> DeepFilter {
    let dir = default_model_dir().expect("a model directory");
    DeepFilter::load(&dir, 100.0)
        .unwrap_or_else(|e| panic!("{e}: run scripts/fetch-models.sh ({})", dir.display()))
}

fn rnnoise() -> CaptureChain {
    CaptureChain::new(ChainSettings {
        echo_cancellation: false,
        high_pass: false,
        auto_gain: false,
        auto_gain_max_db: 30.0,
        noise: NoiseMode::RnNoise,
        gate: GateMode::VoiceActivity {
            automatic: false,
            threshold_db: LEVEL_FLOOR_DB,
        },
        input_gain_db: 0.0,
    })
}

/// Deterministic white noise in -1..1 (xorshift).
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn energy(x: &[f32]) -> f32 {
    x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32
}

fn db(ratio: f32) -> f32 {
    10.0 * ratio.max(1e-12).log10()
}

/// Runs `f` over `input` frame by frame; returns the output and the
/// slowest and mean frame time.
fn run(input: &[f32], mut f: impl FnMut(&mut Frame)) -> (Vec<f32>, Duration, Duration) {
    let mut out = Vec::with_capacity(input.len());
    let (mut worst, mut total) = (Duration::ZERO, Duration::ZERO);
    let mut n = 0_u32;
    for chunk in input.chunks_exact(FRAME_LEN) {
        let mut frame: Frame = chunk.try_into().unwrap();
        let start = Instant::now();
        f(&mut frame);
        let took = start.elapsed();
        worst = worst.max(took);
        total += took;
        n += 1;
        out.extend_from_slice(&frame);
    }
    (out, worst, total / n.max(1))
}

#[test]
#[ignore = "needs scripts/fetch-models.sh"]
fn deep_filter_removes_more_stationary_noise_than_rnnoise_in_real_time() {
    flush_denormals_on_this_thread();
    let started = Instant::now();
    let mut df = load();
    eprintln!("models loaded in {:?}", started.elapsed());
    assert_eq!(df.latency_samples(), 3 * FRAME_LEN, "30 ms");

    let mut source = Noise(0x9e37_79b9);
    let input: Vec<f32> = (0..FRAME_LEN * 300).map(|_| 0.05 * source.next()).collect();
    let tail = FRAME_LEN * 200..;
    let (out, worst, mean) = run(&input, |f| df.process(f).unwrap());
    let df_db = db(energy(&input[tail.clone()]) / energy(&out[tail.clone()]));
    let mut chain = rnnoise();
    let (rn_out, _, rn_mean) = run(&input, |f| {
        chain.capture(f);
    });
    let rn_db = db(energy(&input[tail.clone()]) / energy(&rn_out[tail]));
    eprintln!(
        "white noise: DeepFilterNet3 {df_db:.1} dB less (mean {mean:?}, worst {worst:?} per \
         10 ms frame), RNNoise {rn_db:.1} dB less (mean {rn_mean:?})"
    );
    assert!(
        df_db >= rn_db,
        "DeepFilterNet3 {df_db:.1} dB, RNNoise {rn_db:.1} dB"
    );
    // The real-time claim is a number, not a gate (docs/testing.md): a busy
    // machine is slower, and the filter is not wrong.
    twicecat_perf::record_duration(
        "audio.deepfilter.cpu_mean_frame",
        mean,
        Duration::from_millis(5),
    );
    twicecat_perf::record_duration(
        "audio.deepfilter.cpu_worst_frame",
        worst,
        Duration::from_millis(10),
    );
}

fn read_wav(path: &str) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(&bytes[..4], b"RIFF", "{path}: not a WAV file");
    let mut at = 12;
    let (mut channels, mut bits, mut rate) = (0_u16, 0_u16, 0_u32);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = &bytes[at + 8..(at + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            assert_eq!((rate, bits), (48_000, 16), "{path}: need 48 kHz 16-bit");
            return body
                .chunks_exact(2 * usize::from(channels))
                .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32_768.0)
                .collect();
        }
        at += 8 + len + (len & 1);
    }
    panic!("{path}: no data chunk");
}

/// Scale-invariant SNR (SI-SDR) of `out` against `clean`, after aligning
/// `out` by the lag (up to 100 ms) that matches best: how much of the
/// output is the speech, whatever its level.
fn snr_aligned(clean: &[f32], out: &[f32], skip: usize) -> (f32, usize) {
    let window = skip..clean.len() - 4_800;
    let dot = |lag: usize| -> f32 { window.clone().map(|i| clean[i] * out[i + lag]).sum() };
    let coarse = (0..4_800)
        .step_by(10)
        .max_by(|a, b| dot(*a).total_cmp(&dot(*b)))
        .unwrap_or(0);
    let lag = (coarse.saturating_sub(10)..coarse + 10)
        .max_by(|a, b| dot(*a).total_cmp(&dot(*b)))
        .unwrap_or(coarse);
    let energy: f32 = window.clone().map(|i| clean[i] * clean[i]).sum();
    let scale = dot(lag) / energy;
    let error: f32 = window
        .clone()
        .map(|i| (out[i + lag] - scale * clean[i]).powi(2))
        .sum();
    (db(scale * scale * energy / error), lag)
}

#[test]
#[ignore = "needs scripts/fetch-models.sh and TWICECAT_SPEECH_WAV / TWICECAT_NOISE_WAV"]
fn on_real_speech_deep_filter_improves_the_snr_more_than_rnnoise() {
    let (Ok(speech), Ok(noise)) = (
        std::env::var("TWICECAT_SPEECH_WAV"),
        std::env::var("TWICECAT_NOISE_WAV"),
    ) else {
        eprintln!("skipped: set TWICECAT_SPEECH_WAV and TWICECAT_NOISE_WAV");
        return;
    };
    flush_denormals_on_this_thread();
    let clean = read_wav(&speech);
    let noise = read_wav(&noise);
    let n = clean.len() / FRAME_LEN * FRAME_LEN;
    let clean = &clean[..n];
    // Noise looped under the speech at 5 dB SNR.
    let noise: Vec<f32> = (0..n).map(|i| noise[i % noise.len()]).collect();
    let gain = (energy(clean) / energy(&noise) / 10_f32.powf(0.5)).sqrt();
    let noisy: Vec<f32> = clean
        .iter()
        .zip(&noise)
        .map(|(s, v)| s + gain * v)
        .collect();
    let skip = 48_000;
    let (input_snr, _) = snr_aligned(clean, &noisy, skip);

    let mut df = load();
    let (df_out, _, df_mean) = run(&noisy, |f| df.process(f).unwrap());
    let (df_snr, df_lag) = snr_aligned(clean, &df_out, skip);
    let mut chain = rnnoise();
    let (rn_out, _, _) = run(&noisy, |f| {
        chain.capture(f);
    });
    let (rn_snr, rn_lag) = snr_aligned(clean, &rn_out, skip);
    eprintln!(
        "speech + noise at {input_snr:.1} dB SNR: DeepFilterNet3 {df_snr:.1} dB (lag {df_lag}, \
         {df_mean:?} per frame), RNNoise {rn_snr:.1} dB (lag {rn_lag})"
    );
    assert!(df_snr > input_snr, "no improvement: {df_snr:.1} dB");
    assert!(
        df_snr > rn_snr,
        "DeepFilterNet3 {df_snr:.1} dB, RNNoise {rn_snr:.1} dB"
    );
}

#[test]
#[ignore = "needs scripts/fetch-models.sh"]
fn plugged_into_the_chain_it_replaces_rnnoise() {
    flush_denormals_on_this_thread();
    let settings = ChainSettings {
        echo_cancellation: false,
        high_pass: false,
        auto_gain: false,
        auto_gain_max_db: 30.0,
        noise: NoiseMode::Enhancer,
        gate: GateMode::VoiceActivity {
            automatic: false,
            threshold_db: LEVEL_FLOOR_DB,
        },
        input_gain_db: 0.0,
    };
    let mut source = Noise(0x9e37_79b9);
    let input: Vec<f32> = (0..FRAME_LEN * 300).map(|_| 0.05 * source.next()).collect();
    let tail = FRAME_LEN * 200..;
    let reduction = |chain: &mut CaptureChain| {
        let (out, _, _) = run(&input, |f| {
            chain.capture(f);
        });
        db(energy(&input[tail.clone()]) / energy(&out[tail.clone()]))
    };
    let mut fallback = CaptureChain::new(settings);
    let rn = reduction(&mut fallback);
    let mut chain = CaptureChain::new(settings);
    chain.set_enhancer(Some(Box::new(load())));
    let df = reduction(&mut chain);
    eprintln!("chain: RNNoise standing in {rn:.1} dB, DeepFilterNet3 plugged in {df:.1} dB");
    assert!((rn - 15.0).abs() < 5.0, "RNNoise fallback {rn:.1} dB");
    assert!(df > rn + 10.0, "DeepFilterNet3 {df:.1} dB");
}

#[test]
#[ignore = "needs scripts/fetch-models.sh"]
fn filters_from_one_model_keep_their_own_state() {
    let dir = default_model_dir().expect("a model directory");
    let model = DeepFilterModel::load(&dir).expect("run scripts/fetch-models.sh");
    let (mut a, mut b) = (model.filter(100.0).unwrap(), model.filter(100.0).unwrap());
    let mut alone = DeepFilter::load(&dir, 100.0).unwrap();
    let (mut x, mut y) = (Noise(1), Noise(7));
    for _ in 0..100 {
        let mut fa: Frame = std::array::from_fn(|_| 0.1 * x.next());
        let mut fb: Frame = std::array::from_fn(|_| 0.3 * y.next());
        let mut fc = fa;
        a.process(&mut fa).unwrap();
        b.process(&mut fb).unwrap();
        alone.process(&mut fc).unwrap();
        // `b` running in between changes nothing in `a`.
        assert_eq!(fa, fc);
    }
}
