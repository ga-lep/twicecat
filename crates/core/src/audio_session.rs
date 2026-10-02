//! A running audio session: devices open, the voice engine processing, and
//! a task that reports levels and statistics. Used by the mic test now and
//! by calls (Phase 6 Task 8).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tessera_audio::{
    AudioIo, ChainSettings, GateMode, IoConfig, IoEvent, NoiseMode, NsLevel as AudioNsLevel,
    Outgoing, VoiceControl, VoiceEngine,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::api::{
    AudioSettings, AudioStats, CoreError, CoreEvent, InputLevel, InputMode, NoiseSuppression,
    NsLevel,
};

/// `InputLevel` events while a session runs (20 Hz).
const LEVEL_INTERVAL: Duration = Duration::from_millis(50);
/// `AudioStats` every this many level ticks (1 Hz).
const STATS_EVERY: u32 = 20;

/// The processing settings for `settings`. DeepFilterNet arrives in Task 13;
/// until then it falls back to RNNoise, as `NoiseSuppression::DeepFilter`
/// documents for a missing model.
pub(crate) fn chain_settings(settings: &AudioSettings) -> ChainSettings {
    let p = &settings.processing;
    ChainSettings {
        echo_cancellation: p.echo_cancellation,
        high_pass: p.high_pass,
        auto_gain: p.auto_gain,
        noise: match p.noise_suppression {
            NoiseSuppression::Off => NoiseMode::Off,
            NoiseSuppression::WebRtc { level } => NoiseMode::WebRtc(match level {
                NsLevel::Low => AudioNsLevel::Low,
                NsLevel::Moderate => AudioNsLevel::Moderate,
                NsLevel::High => AudioNsLevel::High,
                NsLevel::VeryHigh => AudioNsLevel::VeryHigh,
            }),
            NoiseSuppression::RnNoise | NoiseSuppression::DeepFilter { .. } => NoiseMode::RnNoise,
        },
        gate: match settings.input_mode {
            InputMode::VoiceActivity {
                automatic,
                threshold_db,
            } => GateMode::VoiceActivity {
                automatic,
                threshold_db,
            },
            InputMode::PushToTalk { release_delay_ms } => GateMode::PushToTalk {
                release_ms: release_delay_ms,
            },
        },
        input_gain_db: settings.input_gain_db,
    }
}

/// What a session needs from the settings to open devices; a change here
/// means reopening.
pub(crate) fn io_config(settings: &AudioSettings, playback: bool) -> IoConfig {
    IoConfig {
        capture: true,
        input_device: settings
            .input_device
            .as_ref()
            .map(|d| d.as_str().to_owned()),
        playback,
        output_device: settings
            .output_device
            .as_ref()
            .map(|d| d.as_str().to_owned()),
    }
}

fn audio_error(e: tessera_audio::AudioError) -> CoreError {
    // The audio crate's messages are fixed text (no user data).
    CoreError::Audio(e.to_string())
}

/// A running session. Stop it with [`AudioSession::stop`]; dropping it also
/// ends the reporting task, which closes the devices.
pub(crate) struct AudioSession {
    control: Arc<Mutex<VoiceControl>>,
    io_config: IoConfig,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl AudioSession {
    /// Opens the devices (blocking work off the async threads) and starts
    /// the voice engine. `loopback` plays the processed microphone back
    /// (mic test). `outgoing` is left to the caller in a call; for a mic
    /// test it is drained here.
    pub(crate) async fn start(
        settings: &AudioSettings,
        loopback: bool,
        events: mpsc::Sender<CoreEvent>,
    ) -> Result<Self, CoreError> {
        let io_config = io_config(settings, loopback);
        let (engine, mut control, outgoing) = VoiceEngine::new(chain_settings(settings));
        control.set_loopback(loopback);
        control.set_master_volume(settings.output_volume_percent);
        let config = io_config.clone();
        let io = tokio::task::spawn_blocking(move || AudioIo::start(config, engine))
            .await
            .map_err(|_| CoreError::Internal("audio start task failed".into()))?
            .map_err(audio_error)?;
        let control = Arc::new(Mutex::new(control));
        let (stop_tx, stop_rx) = oneshot::channel();
        let task = tokio::spawn(report(io, outgoing, control.clone(), events, stop_rx));
        Ok(Self {
            control,
            io_config,
            stop: Some(stop_tx),
            task: Some(task),
        })
    }

    /// Applies new settings. Returns false when the devices changed, so the
    /// caller must restart the session.
    pub(crate) fn apply(&self, settings: &AudioSettings) -> bool {
        if io_config(settings, self.io_config.playback) != self.io_config {
            return false;
        }
        let mut control = self.control.lock().expect("voice control lock");
        control.configure(chain_settings(settings));
        control.set_master_volume(settings.output_volume_percent);
        true
    }

    /// Push-to-talk key state.
    pub(crate) fn push_to_talk(&self, pressed: bool) {
        self.control
            .lock()
            .expect("voice control lock")
            .push_to_talk(pressed);
    }

    /// Whether the processed microphone is played back (mic test).
    pub(crate) fn loopback(&self) -> bool {
        self.io_config.playback
    }

    /// Stops the session and waits until the devices are closed.
    pub(crate) async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for AudioSession {
    fn drop(&mut self) {
        // Without `stop`, the reporting task still ends: its stop channel
        // closes here.
        self.stop.take();
    }
}

/// Reports levels and stats, forwards device problems, drains the unused
/// outgoing stream (mic test), and closes the devices at the end.
async fn report(
    io: AudioIo,
    mut outgoing: Outgoing,
    control: Arc<Mutex<VoiceControl>>,
    events: mpsc::Sender<CoreEvent>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut ticker = tokio::time::interval(LEVEL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks = 0_u32;
    let mut scratch = tessera_audio::SILENCE;
    loop {
        tokio::select! {
            _ = &mut stop => break,
            _ = ticker.tick() => {}
        }
        while outgoing.pop_frame(&mut scratch) {}
        while let Some(event) = io.try_event() {
            let error = match event {
                IoEvent::FellBackToDefault { .. } => {
                    CoreError::Audio("the chosen audio device is missing; using the default".into())
                }
                IoEvent::StreamError { input: true, .. } => {
                    CoreError::Audio("the microphone stream failed".into())
                }
                IoEvent::StreamError { input: false, .. } => {
                    CoreError::Audio("the playback stream failed".into())
                }
                IoEvent::Stopped(e) => audio_error(e),
            };
            if events.send(CoreEvent::Error(error)).await.is_err() {
                break;
            }
        }
        let (meters, stats) = {
            let mut control = control.lock().expect("voice control lock");
            let meters = control.meters();
            ticks += 1;
            let stats = ticks.is_multiple_of(STATS_EVERY).then(|| control.stats());
            (meters, stats)
        };
        let level = CoreEvent::InputLevel(InputLevel {
            rms_db: meters.input_rms_db,
            peak_db: meters.input_peak_db,
            transmitting: meters.transmitting,
        });
        if events.send(level).await.is_err() {
            break;
        }
        if let Some(s) = stats {
            let stats = CoreEvent::AudioStats(AudioStats {
                echo_return_loss_enhancement_db: s.chain.erle_db,
                residual_echo_likelihood: s.chain.residual_echo_likelihood,
                echo_delay_ms: s.chain.delay_ms,
                processing_us: s.processing_us,
            });
            if events.send(stats).await.is_err() {
                break;
            }
        }
    }
    // Closing joins the audio thread: do it off the async threads.
    let _ = tokio::task::spawn_blocking(move || drop(io)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{AudioDeviceId, AudioProcessing};

    #[test]
    fn settings_map_onto_the_chain() {
        let settings = AudioSettings {
            processing: AudioProcessing {
                echo_cancellation: false,
                noise_suppression: NoiseSuppression::WebRtc {
                    level: NsLevel::High,
                },
                auto_gain: false,
                high_pass: true,
            },
            input_mode: InputMode::PushToTalk {
                release_delay_ms: 300,
            },
            input_gain_db: 4.0,
            ..AudioSettings::default()
        };
        let chain = chain_settings(&settings);
        assert!(!chain.echo_cancellation && chain.high_pass && !chain.auto_gain);
        assert_eq!(chain.noise, NoiseMode::WebRtc(AudioNsLevel::High));
        assert_eq!(chain.gate, GateMode::PushToTalk { release_ms: 300 });
        assert_eq!(chain.input_gain_db, 4.0);
    }

    #[test]
    fn deep_filter_falls_back_to_rnnoise_for_now() {
        let mut settings = AudioSettings::default();
        settings.processing.noise_suppression = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 40.0,
        };
        assert_eq!(chain_settings(&settings).noise, NoiseMode::RnNoise);
    }

    #[test]
    fn device_changes_need_a_restart() {
        let a = AudioSettings::default();
        let b = AudioSettings {
            input_device: Some(AudioDeviceId::from("pipewire:mic")),
            ..AudioSettings::default()
        };
        assert_ne!(io_config(&a, true), io_config(&b, true));
        let c = AudioSettings {
            input_gain_db: 6.0,
            ..AudioSettings::default()
        };
        assert_eq!(io_config(&a, true), io_config(&c, true));
    }
}
