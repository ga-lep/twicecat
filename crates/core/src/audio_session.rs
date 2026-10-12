//! A running audio session: devices open, the voice engine processing, and
//! a task that reports levels and statistics. Used by the mic test now and
//! by calls (Phase 6 Task 8).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use twicecat_audio::{
    AudioIo, ChainSettings, DeepFilter, DeepFilterStatus, Frame, GateMode, IoConfig, IoEvent,
    NoiseMode, NsLevel as AudioNsLevel, Outgoing, Realtime, VirtualDevice, VoiceControl,
    VoiceEngine,
};

use crate::api::{
    AudioBackend, AudioSettings, AudioStats, CoreError, CoreEvent, EnhancerBackend, GpuBackend,
    GpuEnhancement, InputLevel, InputMode, NoiseSuppression, NsLevel, VirtualAudio,
};
use crate::gpu_enhancement;

/// `InputLevel` events while a session runs (20 Hz).
const LEVEL_INTERVAL: Duration = Duration::from_millis(50);
/// `AudioStats` every this many level ticks (1 Hz).
const STATS_EVERY: u32 = 20;

/// The processing settings for `settings`. DeepFilterNet runs as the
/// chain's plugged enhancer, RNNoise until its model is loaded (or when it
/// is missing).
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
            NoiseSuppression::RnNoise => NoiseMode::RnNoise,
            NoiseSuppression::DeepFilter { .. } => NoiseMode::Enhancer,
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

fn audio_error(e: twicecat_audio::AudioError) -> CoreError {
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
pub(crate) struct Virtual(pub(crate) Arc<dyn VirtualAudio>);

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

/// Locks the voice engine's control handle. Its methods only queue commands
/// and never panic, so a poisoned lock is used as it is.
pub(crate) fn voice_control(voice: &Voice) -> MutexGuard<'_, VoiceControl> {
    voice.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where other people's voices are cleaned: set by the call's cleaning
/// manager, reported in `AudioStats::remote_cleaning`.
pub(crate) type CleaningBackend = Arc<Mutex<Option<EnhancerBackend>>>;

/// The DeepFilterNet3 a session asked for, and where it runs.
#[derive(Default)]
struct EnhancerState {
    /// `(gpu, attenuation limit)` of the filter loaded or loading.
    wanted: Option<(bool, f32)>,
    /// Where the loaded filter runs.
    status: Option<(DeepFilterStatus, Option<GpuBackend>)>,
}

impl EnhancerState {
    fn running(&self) -> Option<EnhancerBackend> {
        self.status
            .as_ref()
            .map(|(status, gpu)| gpu_enhancement::running(status.get(), *gpu))
    }

    fn wants_gpu(&self) -> bool {
        matches!(self.wanted, Some((true, _)))
    }
}

/// A running session. Stop it with [`AudioSession::stop`]; dropping it also
/// ends the reporting task, which closes the devices.
pub(crate) struct AudioSession {
    control: Voice,
    mode: Mode,
    io_config: IoConfig,
    /// The DeepFilterNet loaded or loading.
    enhancer: Arc<Mutex<EnhancerState>>,
    remote_cleaning: CleaningBackend,
    events: mpsc::Sender<CoreEvent>,
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
        match &io.info().realtime {
            Realtime::Granted { priority } => {
                tracing::info!("audio processing runs at real-time priority {priority}")
            }
            Realtime::GrantedBy(class) => {
                tracing::info!("audio processing runs at real-time priority ({class})")
            }
            Realtime::Refused(why) => {
                tracing::warn!("audio processing runs at normal priority: {why}")
            }
            Realtime::NotRequested => {}
        }
        let control = Arc::new(Mutex::new(control));
        let (drain, handed_out) = match mode {
            Mode::MicTest { .. } => (Some(outgoing), None),
            Mode::Call => (None, Some(outgoing)),
        };
        let (stop_tx, stop_rx) = oneshot::channel();
        let enhancer = Arc::new(Mutex::new(EnhancerState::default()));
        let remote_cleaning = CleaningBackend::default();
        let task = tokio::spawn(report(
            io,
            drain,
            control.clone(),
            (enhancer.clone(), remote_cleaning.clone()),
            events.clone(),
            stop_rx,
        ));
        let session = Self {
            control,
            mode,
            io_config,
            enhancer,
            remote_cleaning,
            events,
            stop: Some(stop_tx),
            task: Some(task),
        };
        session.load_enhancer(settings);
        Ok((session, handed_out))
    }

    /// Loads DeepFilterNet3 off the async threads when the settings ask for
    /// it, on the GPU when they say so, then plugs it in; RNNoise runs
    /// meanwhile, and stays if the model is missing. A new attenuation limit
    /// goes to the running filter (no reload); only the GPU choice reloads.
    fn load_enhancer(&self, settings: &AudioSettings) {
        let NoiseSuppression::DeepFilter {
            attenuation_limit_db,
            gpu,
        } = settings.processing.noise_suppression
        else {
            return;
        };
        {
            let mut state = self.enhancer.lock().expect("enhancer lock");
            if let Some((loaded_gpu, limit)) = state.wanted
                && loaded_gpu == gpu
            {
                if limit != attenuation_limit_db {
                    state.wanted = Some((gpu, attenuation_limit_db));
                    let _ =
                        voice_control(&self.control).set_enhancer_attenuation(attenuation_limit_db);
                }
                return;
            }
            state.wanted = Some((gpu, attenuation_limit_db));
            state.status = None;
        }
        let control = self.control.clone();
        let state = self.enhancer.clone();
        let events = self.events.clone();
        tokio::spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || {
                let dir = twicecat_audio::default_model_dir().ok_or(None::<String>)?;
                if gpu {
                    match gpu_enhancement::load(&dir, attenuation_limit_db) {
                        Ok((filter, backend)) => return Ok((filter, backend, None)),
                        Err(reason) => {
                            let filter = DeepFilter::load(&dir, attenuation_limit_db)
                                .map_err(|_| Some(reason.clone()))?;
                            return Ok((filter, None, Some(reason)));
                        }
                    }
                }
                DeepFilter::load(&dir, attenuation_limit_db)
                    .map(|filter| (filter, None, None))
                    .map_err(|_| None::<String>)
            })
            .await
            .unwrap_or(Err(None));
            match loaded {
                Ok((filter, backend, note)) => {
                    let status = filter.status();
                    let current_limit = {
                        let mut state = state.lock().expect("enhancer lock");
                        match state.wanted {
                            // Still what the settings ask for (the limit may
                            // have changed meanwhile).
                            Some((wanted_gpu, limit)) if wanted_gpu == gpu => {
                                state.status = Some((status.clone(), backend));
                                Some(limit)
                            }
                            _ => None,
                        }
                    };
                    let Some(limit) = current_limit else {
                        // Superseded: drop it off the async threads.
                        let _ = tokio::task::spawn_blocking(move || drop(filter)).await;
                        return;
                    };
                    {
                        let mut control = voice_control(&control);
                        let _ = control.set_enhancer(Some(Box::new(filter)));
                        if limit != attenuation_limit_db {
                            let _ = control.set_enhancer_attenuation(limit);
                        }
                    }
                    if let Some(note) = note {
                        let message = format!("{note}; DeepFilterNet runs on the CPU");
                        let _ = events
                            .send(CoreEvent::Error(CoreError::Audio(message)))
                            .await;
                    }
                    if gpu {
                        let event = GpuEnhancement {
                            available: gpu_enhancement::probed().unwrap_or_default(),
                            running: Some(gpu_enhancement::running(status.get(), backend)),
                        };
                        let _ = events.send(CoreEvent::GpuEnhancement(event)).await;
                    }
                }
                Err(note) => {
                    *state.lock().expect("enhancer lock") = EnhancerState::default();
                    let mut message = "DeepFilterNet's model could not be loaded \
                                       (scripts/fetch-models.sh installs it); RNNoise is used instead"
                        .to_owned();
                    if let Some(note) = note {
                        message = format!("{message} ({note})");
                    }
                    let _ = events
                        .send(CoreEvent::Error(CoreError::Audio(message)))
                        .await;
                }
            }
        });
    }

    /// Where DeepFilterNet3 runs in this session, when it does.
    pub(crate) fn enhancer_running(&self) -> Option<EnhancerBackend> {
        self.enhancer.lock().expect("enhancer lock").running()
    }

    /// Applies new settings. Returns false when the devices changed, so the
    /// caller must restart the session.
    pub(crate) fn apply(&self, settings: &AudioSettings) -> bool {
        if io_config(settings, self.io_config.playback) != self.io_config {
            return false;
        }
        {
            let mut control = voice_control(&self.control);
            control.configure(chain_settings(settings));
            control.set_master_volume(settings.output_volume_percent);
        }
        self.load_enhancer(settings);
        true
    }

    /// Push-to-talk key state.
    pub(crate) fn push_to_talk(&self, pressed: bool) {
        voice_control(&self.control).push_to_talk(pressed);
    }

    /// Mute the microphone (the meter keeps working).
    pub(crate) fn set_muted(&self, muted: bool) {
        voice_control(&self.control).set_muted(muted);
    }

    /// Play nothing.
    pub(crate) fn set_deafened(&self, deafened: bool) {
        voice_control(&self.control).set_deafened(deafened);
    }

    /// The control handle, for the media layer's sources.
    pub(crate) fn voice(&self) -> Voice {
        self.control.clone()
    }

    /// Where the call says other voices are cleaned.
    pub(crate) fn remote_cleaning(&self) -> CleaningBackend {
        self.remote_cleaning.clone()
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
    control: Voice,
    (enhancer, remote_cleaning): (Arc<Mutex<EnhancerState>>, CleaningBackend),
    events: mpsc::Sender<CoreEvent>,
    mut stop: oneshot::Receiver<()>,
) {
    let mut last_running = None;
    let mut ticker = tokio::time::interval(LEVEL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks = 0_u32;
    let mut scratch = twicecat_audio::SILENCE;
    let mut last_io = io.stats();
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
                // Followed to another device; nothing failed.
                IoEvent::Switched { input, device } => {
                    tracing::info!(
                        "audio {} moved to {}",
                        if input { "input" } else { "output" },
                        device.device_name
                    );
                    continue;
                }
                IoEvent::Stopped(e) => audio_error(e),
            };
            if events.send(CoreEvent::Error(error)).await.is_err() {
                break;
            }
        }
        let (meters, stats) = {
            let mut control = voice_control(&control);
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
            let now = io.stats();
            let since =
                |now: u64, then: u64| u32::try_from(now.saturating_sub(then)).unwrap_or(u32::MAX);
            let stats = CoreEvent::AudioStats(AudioStats {
                echo_return_loss_enhancement_db: s.chain.erle_db,
                residual_echo_likelihood: s.chain.residual_echo_likelihood,
                echo_delay_ms: s.chain.delay_ms,
                processing_us: s.processing_us,
                late_wakes: since(now.late_wakes, last_io.late_wakes),
                max_wake_us: u32::try_from(io.take_recent_max_wake_us()).unwrap_or(u32::MAX),
                capture_overruns: since(now.capture_overruns, last_io.capture_overruns),
                playback_underruns: since(now.playback_underruns, last_io.playback_underruns),
                remote_cleaning: remote_cleaning.lock().ok().and_then(|r| *r),
            });
            last_io = now;
            if events.send(stats).await.is_err() {
                break;
            }
            // A GPU filter that fell back to the CPU (checked once a second;
            // the load itself is announced where it completes).
            let (running, wants_gpu) = {
                let state = enhancer.lock().expect("enhancer lock");
                (state.running(), state.wants_gpu())
            };
            if wants_gpu && last_running.is_some() && running.is_some() && running != last_running {
                let event = GpuEnhancement {
                    available: gpu_enhancement::probed().unwrap_or_default(),
                    running,
                };
                if events.send(CoreEvent::GpuEnhancement(event)).await.is_err() {
                    break;
                }
            }
            last_running = running;
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
    fn deep_filter_runs_as_the_enhancer() {
        let mut settings = AudioSettings::default();
        settings.processing.noise_suppression = NoiseSuppression::DeepFilter {
            attenuation_limit_db: 40.0,
            gpu: false,
        };
        assert_eq!(chain_settings(&settings).noise, NoiseMode::Enhancer);
        settings.processing.noise_suppression = NoiseSuppression::RnNoise;
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
