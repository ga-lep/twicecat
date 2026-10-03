//! A running audio session: devices open, the voice engine processing, and
//! a task that reports levels and statistics. Used by the mic test now and
//! by calls (Phase 6 Task 8).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tessera_audio::{
    AudioIo, ChainSettings, Frame, GateMode, IoConfig, IoEvent, NoiseMode, NsLevel as AudioNsLevel,
    Outgoing, SourceId, SourceInput, VirtualDevice, VoiceControl, VoiceEngine,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::api::{
    AudioBackend, AudioSettings, AudioStats, CoreError, CoreEvent, InputLevel, InputMode,
    NoiseSuppression, NsLevel, VirtualAudio,
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
        echo_reference: playback
            && settings.processing.echo_cancellation
            && settings.processing.cancel_system_audio,
    }
}

fn audio_error(e: tessera_audio::AudioError) -> CoreError {
    // The audio crate's messages are fixed text (no user data).
    CoreError::Audio(e.to_string())
}

/// What a session is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Mic test: the processed microphone is discarded, or played back
    /// with `loopback`.
    MicTest { loopback: bool },
    /// A call: the processed microphone goes to the caller, remote audio
    /// comes in through sources.
    Call,
}

/// Adapts the API's [`VirtualAudio`] to the audio crate.
struct Virtual(Arc<dyn VirtualAudio>);

impl VirtualDevice for Virtual {
    fn capture(&mut self, frame: &mut Frame) {
        self.0.capture(frame);
    }
    fn playback(&mut self, frame: &Frame) {
        self.0.playback(frame);
    }
}

/// The voice engine's control handle, shared with the media layer.
pub(crate) type Voice = Arc<Mutex<VoiceControl>>;

/// A running session. Stop it with [`AudioSession::stop`]; dropping it also
/// ends the reporting task, which closes the devices.
pub(crate) struct AudioSession {
    control: Voice,
    mode: Mode,
    io_config: IoConfig,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl AudioSession {
    /// Opens the devices (blocking work off the async threads) and starts
    /// the voice engine. For a call, the processed microphone is returned
    /// for sending; for a mic test it is drained here.
    pub(crate) async fn start(
        settings: &AudioSettings,
        mode: Mode,
        backend: &AudioBackend,
        events: mpsc::Sender<CoreEvent>,
    ) -> Result<(Self, Option<Outgoing>), CoreError> {
        let loopback = matches!(mode, Mode::MicTest { loopback: true });
        let io_config = io_config(settings, loopback || mode == Mode::Call);
        let (engine, mut control, outgoing) = VoiceEngine::new(chain_settings(settings));
        control.set_loopback(loopback);
        control.set_master_volume(settings.output_volume_percent);
        let config = io_config.clone();
        let backend = backend.clone();
        let io = tokio::task::spawn_blocking(move || match backend {
            AudioBackend::System => AudioIo::start(config, engine),
            AudioBackend::Virtual(device) => AudioIo::start_virtual(Virtual(device), engine),
        })
        .await
        .map_err(|_| CoreError::Internal("audio start task failed".into()))?
        .map_err(audio_error)?;
        let control = Arc::new(Mutex::new(control));
        let (drain, handed_out) = match mode {
            Mode::MicTest { .. } => (Some(outgoing), None),
            Mode::Call => (None, Some(outgoing)),
        };
        let (stop_tx, stop_rx) = oneshot::channel();
        let task = tokio::spawn(report(io, drain, control.clone(), events, stop_rx));
        Ok((
            Self {
                control,
                mode,
                io_config,
                stop: Some(stop_tx),
                task: Some(task),
            },
            handed_out,
        ))
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

    /// Mute the microphone (the meter keeps working).
    pub(crate) fn set_muted(&self, muted: bool) {
        self.control
            .lock()
            .expect("voice control lock")
            .set_muted(muted);
    }

    /// Play nothing.
    pub(crate) fn set_deafened(&self, deafened: bool) {
        self.control
            .lock()
            .expect("voice control lock")
            .set_deafened(deafened);
    }

    /// The control handle, for the media layer's sources.
    pub(crate) fn voice(&self) -> Voice {
        self.control.clone()
    }

    /// What this session is for.
    pub(crate) fn mode(&self) -> Mode {
        self.mode
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

/// Adds a remote source at the user's volume.
pub(crate) fn add_source(voice: &Voice, volume_percent: u16) -> Option<(SourceId, SourceInput)> {
    voice
        .lock()
        .ok()
        .and_then(|mut c| c.add_source(volume_percent))
}

/// Removes a remote source.
pub(crate) fn remove_source(voice: &Voice, id: SourceId) {
    if let Ok(mut c) = voice.lock() {
        c.remove_source(id);
    }
}

/// Changes a remote source's volume.
pub(crate) fn set_source_volume(voice: &Voice, id: SourceId, percent: u16) {
    if let Ok(mut c) = voice.lock() {
        c.set_source_volume(id, percent);
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
    mut drain: Option<Outgoing>,
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
        if let Some(outgoing) = drain.as_mut() {
            while outgoing.pop_frame(&mut scratch) {}
        }
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
                cancel_system_audio: false,
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

    #[test]
    fn system_audio_is_cancelled_only_with_echo_cancellation_and_playback() {
        let mut s = AudioSettings::default();
        assert!(!io_config(&s, true).echo_reference, "off by default");
        s.processing.cancel_system_audio = true;
        assert!(io_config(&s, true).echo_reference);
        assert!(
            !io_config(&s, false).echo_reference,
            "mic test without playback"
        );
        s.processing.echo_cancellation = false;
        assert!(!io_config(&s, true).echo_reference);
    }
}
