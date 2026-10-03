//! The microphone processing chain, run once per 10 ms frame on the
//! processing thread:
//!
//! high-pass → AEC3 → noise suppression (WebRTC NS, RNNoise, or a plugged
//! [`Enhancer`] such as DeepFilterNet3) → AGC2 → input gain → transmit gate
//! → limiter → level meter.
//!
//! Echo cancellation, high-pass and WebRTC NS run in one sonora instance
//! ("front"); AGC2 runs in a second one ("back") so that RNNoise, between
//! the two, sees the signal before gain is added (denoise first, then level).
//! The front instance also receives every frame that is played (the render
//! reference AEC3 subtracts).

use nnnoiseless::DenoiseState;
use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression,
    NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

use crate::AudioError;
use crate::frame::{FRAME_LEN, Frame, SAMPLE_RATE};
use crate::gate::{Gate, GateMode};

/// A speech enhancer plugged into the chain in place of the built-in noise
/// suppressors ([`NoiseMode::Enhancer`]): DeepFilterNet3, on the CPU or a
/// GPU (plan Tasks 13, 13b). Loaded off the processing thread, then handed
/// over with [`crate::VoiceControl::set_enhancer`]; a replaced or failed
/// enhancer goes back to the control side to be dropped there (freeing a
/// model, or a GPU context, has no place on the processing thread).
pub trait Enhancer: Send + 'static {
    /// Cleans one frame in place. An error removes the enhancer and the
    /// chain falls back to RNNoise.
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError>;
    /// How late its output is, in samples.
    fn latency_samples(&self) -> usize;
    /// Most attenuation applied, dB (100 or more: no limit). Enhancers
    /// without one ignore it.
    fn set_attenuation_limit(&mut self, _db: f32) {}
}

/// Strength of the WebRTC noise suppressor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NsLevel {
    /// About 6 dB.
    Low,
    /// About 12 dB.
    Moderate,
    /// About 18 dB.
    High,
    /// About 21 dB.
    VeryHigh,
}

/// Which noise suppressor runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseMode {
    /// None.
    Off,
    /// WebRTC's spectral suppressor.
    WebRtc(NsLevel),
    /// RNNoise.
    RnNoise,
    /// The plugged [`Enhancer`]; RNNoise until one is plugged in.
    Enhancer,
}

/// Settings of the chain; every field can change while it runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChainSettings {
    /// AEC3.
    pub echo_cancellation: bool,
    /// High-pass filter.
    pub high_pass: bool,
    /// AGC2 (adaptive digital gain with its limiter).
    pub auto_gain: bool,
    /// Noise suppressor.
    pub noise: NoiseMode,
    /// Transmit gate.
    pub gate: GateMode,
    /// Gain after processing, dB.
    pub input_gain_db: f32,
}

impl Default for ChainSettings {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            high_pass: true,
            auto_gain: true,
            noise: NoiseMode::RnNoise,
            gate: GateMode::VoiceActivity {
                automatic: true,
                threshold_db: -50.0,
            },
            input_gain_db: 0.0,
        }
    }
}

/// What one processed frame did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureResult {
    /// The gate is open: the frame should be sent.
    pub transmit: bool,
    /// RMS level of the processed frame before the gate, dBFS.
    pub rms_db: f32,
    /// Peak level of the processed frame before the gate, dBFS.
    pub peak_db: f32,
}

/// Echo-cancellation diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChainStats {
    /// Echo return loss enhancement, dB.
    pub erle_db: Option<f32>,
    /// Residual echo likelihood, 0..=1.
    pub residual_echo_likelihood: Option<f32>,
    /// Estimated echo delay, ms.
    pub delay_ms: Option<u32>,
}

/// Lowest level reported, dBFS (digital silence).
pub const LEVEL_FLOOR_DB: f32 = -100.0;

fn db(linear: f32) -> f32 {
    if linear <= 1e-5 {
        LEVEL_FLOOR_DB
    } else {
        (20.0 * linear.log10()).max(LEVEL_FLOOR_DB)
    }
}

fn levels(frame: &Frame) -> (f32, f32) {
    let mut sum = 0.0_f32;
    let mut peak = 0.0_f32;
    for s in frame {
        sum += s * s;
        peak = peak.max(s.abs());
    }
    (db((sum / FRAME_LEN as f32).sqrt()), db(peak))
}

fn front_config(s: &ChainSettings) -> Config {
    Config {
        high_pass_filter: s.high_pass.then(HighPassFilter::default),
        echo_canceller: s.echo_cancellation.then(EchoCanceller::default),
        noise_suppression: match s.noise {
            NoiseMode::WebRtc(level) => Some(NoiseSuppression {
                level: match level {
                    NsLevel::Low => NoiseSuppressionLevel::Low,
                    NsLevel::Moderate => NoiseSuppressionLevel::Moderate,
                    NsLevel::High => NoiseSuppressionLevel::High,
                    NsLevel::VeryHigh => NoiseSuppressionLevel::VeryHigh,
                },
                ..NoiseSuppression::default()
            }),
            NoiseMode::Off | NoiseMode::RnNoise | NoiseMode::Enhancer => None,
        },
        ..Config::default()
    }
}

/// The settings `front_config` depends on (sonora's `Config` has no
/// `PartialEq`).
fn front_key(s: &ChainSettings) -> (bool, bool, Option<NsLevel>) {
    let ns = match s.noise {
        NoiseMode::WebRtc(level) => Some(level),
        NoiseMode::Off | NoiseMode::RnNoise | NoiseMode::Enhancer => None,
    };
    (s.echo_cancellation, s.high_pass, ns)
}

fn back_config(s: &ChainSettings) -> Config {
    Config {
        gain_controller2: s.auto_gain.then(|| GainController2 {
            adaptive_digital: Some(AdaptiveDigital::default()),
            ..GainController2::default()
        }),
        ..Config::default()
    }
}

fn apm(config: Config) -> AudioProcessing {
    let mono = StreamConfig::new(SAMPLE_RATE, 1);
    AudioProcessing::builder()
        .config(config)
        .capture_config(mono)
        .render_config(mono)
        .build()
}

/// Peak limiter: instant attack, 50 ms release, ceiling -0.5 dBFS.
struct Limiter {
    gain: f32,
}

impl Limiter {
    const CEILING: f32 = 0.944;
    /// Per-frame gain recovery towards 1 (50 ms to recover fully).
    const RELEASE: f32 = 0.2;

    fn process(&mut self, frame: &mut Frame) {
        let peak = frame.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        let needed = if peak > Self::CEILING {
            Self::CEILING / peak
        } else {
            1.0
        };
        let start = self.gain;
        let end = if needed < self.gain {
            needed
        } else {
            (self.gain + Self::RELEASE).min(needed)
        };
        // Interpolate across the frame; a smaller `end` applies at once.
        let attack = end < start;
        for (i, s) in frame.iter_mut().enumerate() {
            let g = if attack {
                end
            } else {
                start + (end - start) * (i as f32 / FRAME_LEN as f32)
            };
            *s = (*s * g).clamp(-1.0, 1.0);
        }
        self.gain = end;
    }
}

/// The microphone chain. Not `Clone`; one per call or mic test.
pub struct CaptureChain {
    settings: ChainSettings,
    front: AudioProcessing,
    back: AudioProcessing,
    rnnoise: Option<Box<DenoiseState<'static>>>,
    enhancer: Option<Box<dyn Enhancer>>,
    /// A replaced or failed enhancer, waiting to leave the processing thread.
    retired: Option<Box<dyn Enhancer>>,
    gate: Gate,
    limiter: Limiter,
    scratch: Frame,
    rn_in: Frame,
}

impl CaptureChain {
    /// A chain with these settings.
    pub fn new(settings: ChainSettings) -> Self {
        Self {
            front: apm(front_config(&settings)),
            back: apm(back_config(&settings)),
            rnnoise: matches!(settings.noise, NoiseMode::RnNoise | NoiseMode::Enhancer)
                .then(DenoiseState::new),
            enhancer: None,
            retired: None,
            gate: Gate::new(settings.gate),
            limiter: Limiter { gain: 1.0 },
            scratch: [0.0; FRAME_LEN],
            rn_in: [0.0; FRAME_LEN],
            settings,
        }
    }

    /// Changes settings in place; processing state is kept where possible.
    pub fn configure(&mut self, settings: ChainSettings) {
        if front_key(&settings) != front_key(&self.settings) {
            self.front.apply_config(front_config(&settings));
        }
        if settings.auto_gain != self.settings.auto_gain {
            self.back.apply_config(back_config(&settings));
        }
        self.settings = settings;
        self.update_rnnoise();
        self.gate.set_mode(settings.gate);
    }

    /// Plugs an enhancer in (or out). It runs while the noise mode is
    /// [`NoiseMode::Enhancer`].
    /// The one it replaces waits in [`take_retired`](Self::take_retired).
    pub fn set_enhancer(&mut self, enhancer: Option<Box<dyn Enhancer>>) {
        let old = std::mem::replace(&mut self.enhancer, enhancer);
        self.retire(old);
        self.update_rnnoise();
    }

    /// Sets the plugged enhancer's attenuation limit, dB.
    pub fn set_enhancer_attenuation(&mut self, db: f32) {
        if let Some(enhancer) = self.enhancer.as_mut() {
            enhancer.set_attenuation_limit(db);
        }
    }

    /// The enhancer last replaced or failed, to be dropped elsewhere.
    pub fn take_retired(&mut self) -> Option<Box<dyn Enhancer>> {
        self.retired.take()
    }

    fn retire(&mut self, enhancer: Option<Box<dyn Enhancer>>) {
        if enhancer.is_some() {
            // An older one not yet collected is dropped here (only when two
            // are replaced within one frame).
            self.retired = enhancer;
        }
    }

    /// Whether an enhancer is plugged in.
    pub fn has_enhancer(&self) -> bool {
        self.enhancer.is_some()
    }

    /// RNNoise runs when chosen, and in place of a missing enhancer.
    fn update_rnnoise(&mut self) {
        let wanted = match self.settings.noise {
            NoiseMode::RnNoise => true,
            NoiseMode::Enhancer => self.enhancer.is_none(),
            NoiseMode::Off | NoiseMode::WebRtc(_) => false,
        };
        match (wanted, self.rnnoise.is_some()) {
            (true, false) => self.rnnoise = Some(DenoiseState::new()),
            (false, true) => self.rnnoise = None,
            _ => {}
        }
    }

    /// The current settings.
    pub fn settings(&self) -> &ChainSettings {
        &self.settings
    }

    /// Push-to-talk key state (any source).
    pub fn set_push_to_talk(&mut self, pressed: bool) {
        self.gate.set_push_to_talk(pressed);
    }

    /// Feeds the frame that is being played (echo reference). Call once per
    /// frame, before [`capture`](Self::capture).
    pub fn render(&mut self, played: &Frame) {
        if !self.settings.echo_cancellation {
            return;
        }
        // The render output is unused; sonora needs somewhere to write it.
        let _ = self
            .front
            .process_render_f32(&[&played[..]], &mut [&mut self.scratch[..]]);
    }

    /// Processes the microphone frame in place: silence when the gate is
    /// closed.
    pub fn capture(&mut self, frame: &mut Frame) -> CaptureResult {
        let src = *frame;
        let _ = self
            .front
            .process_capture_f32(&[&src[..]], &mut [&mut frame[..]]);

        let mut voice = None;
        if self.settings.noise == NoiseMode::Enhancer
            && let Some(enhancer) = self.enhancer.as_mut()
            && enhancer.enhance(frame).is_err()
        {
            tracing::warn!("the speech enhancer failed; RNNoise takes over");
            let failed = self.enhancer.take();
            self.retire(failed);
            self.update_rnnoise();
        }
        if let Some(rn) = self.rnnoise.as_mut() {
            for (dst, s) in self.rn_in.iter_mut().zip(frame.iter()) {
                *dst = s * 32768.0;
            }
            voice = Some(rn.process_frame(&mut frame[..], &self.rn_in[..]));
            for s in frame.iter_mut() {
                *s /= 32768.0;
            }
        }

        if self.settings.auto_gain {
            let src = *frame;
            let _ = self
                .back
                .process_capture_f32(&[&src[..]], &mut [&mut frame[..]]);
        }

        let gain = 10.0_f32.powf(self.settings.input_gain_db / 20.0);
        if gain != 1.0 {
            for s in frame.iter_mut() {
                *s *= gain;
            }
        }
        self.limiter.process(frame);

        let (rms_db, peak_db) = levels(frame);
        let transmit = self.gate.decide(rms_db, voice);
        self.gate.apply(frame, transmit);
        CaptureResult {
            transmit,
            rms_db,
            peak_db,
        }
    }

    /// Echo-cancellation statistics.
    pub fn stats(&self) -> ChainStats {
        let s = self.front.statistics();
        ChainStats {
            erle_db: s.echo_return_loss_enhancement.map(|v| v as f32),
            residual_echo_likelihood: s.residual_echo_likelihood.map(|v| v as f32),
            delay_ms: s.delay_ms.and_then(|d| u32::try_from(d).ok()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denormals::flush_denormals_on_this_thread;

    /// Deterministic pseudo-random noise in -1..1 (xorshift).
    struct Noise(u32);
    impl Noise {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    fn energy(frames: &[Frame]) -> f32 {
        frames.iter().flatten().map(|s| s * s).sum::<f32>() / (frames.len() * FRAME_LEN) as f32
    }

    fn open_gate() -> GateMode {
        GateMode::VoiceActivity {
            automatic: false,
            threshold_db: LEVEL_FLOOR_DB,
        }
    }

    /// Speech-like far-end signal: noise shaped by a slowly varying envelope.
    fn far_end(noise: &mut Noise, frame_index: usize) -> Frame {
        let mut f = [0.0; FRAME_LEN];
        let envelope = 0.3 * (0.6 + 0.4 * ((frame_index as f32) * 0.31).sin().abs());
        for s in &mut f {
            *s = envelope * noise.next();
        }
        f
    }

    #[test]
    fn aec_removes_at_least_20_db_of_a_synthetic_echo() {
        flush_denormals_on_this_thread();
        let mut chain = CaptureChain::new(ChainSettings {
            echo_cancellation: true,
            high_pass: false,
            auto_gain: false,
            noise: NoiseMode::Off,
            gate: open_gate(),
            input_gain_db: 0.0,
        });
        let mut noise = Noise(0x1234_5678);
        // Echo path: 30 ms delay, -12 dB, a one-pole low-pass (room/speaker).
        let delay = 30 * (SAMPLE_RATE as usize / 1000);
        let mut history = vec![0.0_f32; delay];
        let mut lp = 0.0_f32;
        let mut echoes = Vec::new();
        let mut outputs = Vec::new();
        for i in 0..600 {
            let far = far_end(&mut noise, i);
            chain.render(&far);
            let mut mic = [0.0; FRAME_LEN];
            for (j, s) in mic.iter_mut().enumerate() {
                history.push(far[j]);
                let delayed = history.remove(0);
                lp = 0.7 * lp + 0.3 * delayed;
                *s = 0.25 * lp;
            }
            let echo = mic;
            chain.capture(&mut mic);
            if i >= 400 {
                echoes.push(echo);
                outputs.push(mic);
            }
        }
        let erle = 10.0 * (energy(&echoes) / energy(&outputs).max(1e-12)).log10();
        assert!(erle >= 20.0, "ERLE after 4 s: {erle:.1} dB");
        let stats = chain.stats();
        assert!(stats.delay_ms.is_some(), "{stats:?}");
    }

    fn noise_reduction_db(noise: NoiseMode) -> f32 {
        flush_denormals_on_this_thread();
        let mut chain = CaptureChain::new(ChainSettings {
            echo_cancellation: false,
            high_pass: false,
            auto_gain: false,
            noise,
            gate: open_gate(),
            input_gain_db: 0.0,
        });
        let mut source = Noise(0x9e37_79b9);
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for i in 0..300 {
            let mut f = [0.0; FRAME_LEN];
            for s in &mut f {
                *s = 0.05 * source.next();
            }
            let input = f;
            chain.capture(&mut f);
            if i >= 200 {
                before.push(input);
                after.push(f);
            }
        }
        10.0 * (energy(&before) / energy(&after).max(1e-12)).log10()
    }

    #[test]
    fn rnnoise_suppresses_stationary_noise() {
        let r = noise_reduction_db(NoiseMode::RnNoise);
        assert!(r >= 10.0, "RNNoise reduction {r:.1} dB");
    }

    #[test]
    fn webrtc_ns_suppresses_stationary_noise() {
        let r = noise_reduction_db(NoiseMode::WebRtc(NsLevel::High));
        assert!(r >= 10.0, "WebRTC NS reduction {r:.1} dB");
    }

    /// Halves the signal, or fails after `fail_after` frames.
    struct Half {
        calls: std::sync::Arc<std::sync::atomic::AtomicU32>,
        fail_after: u32,
    }

    impl Enhancer for Half {
        fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
            let n = self
                .calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n >= self.fail_after {
                return Err(AudioError::Internal);
            }
            for s in frame.iter_mut() {
                *s *= 0.5;
            }
            Ok(())
        }
        fn latency_samples(&self) -> usize {
            0
        }
    }

    /// Records being dropped; fails on its first frame when asked to.
    struct Tracked {
        dropped: std::sync::Arc<std::sync::atomic::AtomicBool>,
        fails: bool,
        limit: std::sync::Arc<std::sync::Mutex<Option<f32>>>,
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl Enhancer for Tracked {
        fn enhance(&mut self, _: &mut Frame) -> Result<(), AudioError> {
            if self.fails {
                Err(AudioError::Internal)
            } else {
                Ok(())
            }
        }
        fn latency_samples(&self) -> usize {
            0
        }
        fn set_attenuation_limit(&mut self, db: f32) {
            *self.limit.lock().unwrap() = Some(db);
        }
    }

    #[test]
    fn replaced_and_failed_enhancers_leave_the_chain_alive_for_dropping_elsewhere() {
        let mut chain = CaptureChain::new(ChainSettings {
            echo_cancellation: false,
            high_pass: false,
            auto_gain: false,
            noise: NoiseMode::Enhancer,
            gate: open_gate(),
            input_gain_db: 0.0,
        });
        let tracked = |fails| {
            let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let limit = std::sync::Arc::new(std::sync::Mutex::new(None));
            let enhancer = Tracked {
                dropped: dropped.clone(),
                fails,
                limit: limit.clone(),
            };
            (Box::new(enhancer), dropped, limit)
        };
        let is_dropped = |d: &std::sync::Arc<std::sync::atomic::AtomicBool>| {
            d.load(std::sync::atomic::Ordering::Relaxed)
        };

        let (first, first_dropped, first_limit) = tracked(false);
        chain.set_enhancer(Some(first));
        chain.set_enhancer_attenuation(12.0);
        assert_eq!(*first_limit.lock().unwrap(), Some(12.0), "set in place");
        let (second, second_dropped, _) = tracked(true);
        chain.set_enhancer(Some(second));
        assert!(!is_dropped(&first_dropped), "replaced, not dropped");
        let retired = chain.take_retired().expect("the replaced one");
        drop(retired);
        assert!(is_dropped(&first_dropped));

        chain.capture(&mut [0.1; FRAME_LEN]);
        assert!(!chain.has_enhancer(), "removed after it failed");
        assert!(!is_dropped(&second_dropped), "failed, not dropped");
        assert!(chain.take_retired().is_some());
        assert!(chain.take_retired().is_none());
    }

    #[test]
    fn a_plugged_enhancer_runs_rnnoise_stands_in_and_takes_over_on_failure() {
        let settings = ChainSettings {
            echo_cancellation: false,
            high_pass: false,
            auto_gain: false,
            noise: NoiseMode::Enhancer,
            gate: open_gate(),
            input_gain_db: 0.0,
        };
        let mut chain = CaptureChain::new(settings);
        assert!(
            chain.rnnoise.is_some(),
            "RNNoise until an enhancer is plugged in"
        );
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        chain.set_enhancer(Some(Box::new(Half {
            calls: calls.clone(),
            fail_after: 2,
        })));
        assert!(chain.has_enhancer() && chain.rnnoise.is_none());
        // The gate fades in over the first frame: look at the second.
        let mut f = [0.4; FRAME_LEN];
        chain.capture(&mut [0.4; FRAME_LEN]);
        chain.capture(&mut f);
        assert!((f[100] - 0.2).abs() < 1e-6, "{}", f[100]);
        // Another mode leaves it plugged in but unused.
        chain.configure(ChainSettings {
            noise: NoiseMode::Off,
            ..settings
        });
        let mut f = [0.4; FRAME_LEN];
        chain.capture(&mut f);
        assert!((f[100] - 0.4).abs() < 1e-6, "{}", f[100]);
        chain.configure(settings);
        for _ in 0..3 {
            chain.capture(&mut [0.4; FRAME_LEN]);
        }
        assert!(!chain.has_enhancer(), "removed after it failed");
        assert!(chain.rnnoise.is_some(), "RNNoise took over");
    }

    #[test]
    fn no_suppressor_leaves_noise_alone() {
        let r = noise_reduction_db(NoiseMode::Off);
        assert!(r.abs() < 0.5, "{r:.2} dB");
    }

    #[test]
    fn closed_gate_sends_silence_and_reports_levels() {
        let mut chain = CaptureChain::new(ChainSettings {
            echo_cancellation: false,
            high_pass: false,
            auto_gain: false,
            noise: NoiseMode::Off,
            gate: GateMode::PushToTalk { release_ms: 0 },
            input_gain_db: 0.0,
        });
        let mut f = [0.5; FRAME_LEN];
        let r = chain.capture(&mut f);
        assert!(!r.transmit);
        assert!(f.iter().all(|s| *s == 0.0));
        assert!((r.rms_db - db(0.5)).abs() < 0.1, "{r:?}");
        chain.set_push_to_talk(true);
        let mut f = [0.5; FRAME_LEN];
        assert!(chain.capture(&mut f).transmit);
        assert!(f[FRAME_LEN - 1] > 0.49);
    }

    #[test]
    fn input_gain_is_applied_and_the_limiter_holds_the_ceiling() {
        let mut chain = CaptureChain::new(ChainSettings {
            echo_cancellation: false,
            high_pass: false,
            auto_gain: false,
            noise: NoiseMode::Off,
            gate: open_gate(),
            input_gain_db: 20.0,
        });
        for _ in 0..3 {
            let mut f = [0.5; FRAME_LEN];
            chain.capture(&mut f);
            assert!(f.iter().all(|s| s.abs() <= Limiter::CEILING + 1e-6));
        }
        chain.configure(ChainSettings {
            input_gain_db: -6.0,
            ..*chain.settings()
        });
        let mut f = [0.5; FRAME_LEN];
        chain.capture(&mut f);
        // Released towards 1 over 50 ms, but never above 0.5 * -6 dB.
        assert!(f.iter().all(|s| *s <= 0.252), "{}", f[FRAME_LEN - 1]);
    }

    #[test]
    fn the_chain_can_move_to_the_processing_thread() {
        fn assert_send<T: Send + 'static>() {}
        assert_send::<CaptureChain>();
    }

    #[test]
    fn switching_suppressors_while_running() {
        let mut chain = CaptureChain::new(ChainSettings::default());
        let mut f = [0.0; FRAME_LEN];
        chain.capture(&mut f);
        for noise in [
            NoiseMode::Off,
            NoiseMode::WebRtc(NsLevel::Low),
            NoiseMode::RnNoise,
        ] {
            chain.configure(ChainSettings {
                noise,
                ..*chain.settings()
            });
            assert_eq!(chain.rnnoise.is_some(), noise == NoiseMode::RnNoise);
            chain.render(&f);
            chain.capture(&mut f);
        }
    }

    /// Runs 3 s of far-end speech with echo (so AEC3 adapts), then 10 s of
    /// a silent far end, and returns (mean, worst) processing time per
    /// frame during the silence.
    fn time_silence_after_speech(guard: bool) -> (std::time::Duration, std::time::Duration) {
        if guard {
            flush_denormals_on_this_thread();
        }
        let mut chain = CaptureChain::new(ChainSettings::default());
        let mut noise = Noise(7);
        let delay = 20 * (SAMPLE_RATE as usize / 1000);
        let mut history = std::collections::VecDeque::from(vec![0.0_f32; delay]);
        for i in 0..300 {
            let far = far_end(&mut noise, i);
            chain.render(&far);
            let mut mic = [0.0; FRAME_LEN];
            for (j, s) in mic.iter_mut().enumerate() {
                history.push_back(far[j]);
                *s = 0.25 * history.pop_front().unwrap_or(0.0) + 0.001 * noise.next();
            }
            chain.capture(&mut mic);
        }
        let silent = [0.0; FRAME_LEN];
        let mut worst = std::time::Duration::ZERO;
        let start = std::time::Instant::now();
        for _ in 0..1000 {
            let mut mic = [0.0; FRAME_LEN];
            for s in &mut mic {
                *s = 0.001 * noise.next();
            }
            let t = std::time::Instant::now();
            chain.render(&silent);
            chain.capture(&mut mic);
            worst = worst.max(t.elapsed());
        }
        (start.elapsed() / 1000, worst)
    }

    #[test]
    #[ignore = "timing: cargo test --release -p tessera-audio -- --ignored timing"]
    fn timing_full_chain_with_silent_far_end_stays_real_time() {
        // sonora#34: AEC3 slows 10-50x on a silent render reference unless
        // denormals are flushed. Each case on its own thread: the guard is
        // per thread.
        let with = std::thread::spawn(|| time_silence_after_speech(true))
            .join()
            .unwrap();
        let without = std::thread::spawn(|| time_silence_after_speech(false))
            .join()
            .unwrap();
        eprintln!(
            "silent far end, with guard: mean {:?}, worst {:?}",
            with.0, with.1
        );
        eprintln!(
            "silent far end, without guard: mean {:?}, worst {:?}",
            without.0, without.1
        );
        assert!(
            with.0 < std::time::Duration::from_millis(2),
            "mean {:?}",
            with.0
        );
    }
}
