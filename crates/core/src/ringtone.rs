//! The ringtone (Phase 6 call UI plan, Task 4, decision Q3b): while a
//! direct call rings and this device is not in a call, a tone plays on the
//! output device at the master volume.
//!
//! The call watcher says whether a ring should sound
//! ([`CallWatch::loud`](crate::call::watcher::CallWatch::loud)), the engine says
//! whether it may and where ([`RingGate`]); [`Ringer`] opens a playback-only
//! device while both agree and closes it as soon as they don't. The tone is
//! made here (two sines, 440 and 480 Hz, one second on and two off, the North
//! American ring), so there is no sound file to ship or license.

use std::f32::consts::TAU;

use tokio::sync::{oneshot, watch};
use tokio::task::JoinHandle;
use twicecat_audio::{AudioIo, Frame, FrameProcessor, IoConfig, SAMPLE_RATE};

use crate::api::{AudioBackend, AudioSettings};
use crate::audio_session::Virtual;

/// The two frequencies of the tone, Hz.
const TONES_HZ: [f32; 2] = [440.0, 480.0];
/// How long the tone sounds, then the whole cycle, in samples.
const ON_SAMPLES: u32 = SAMPLE_RATE;
const CYCLE_SAMPLES: u32 = 3 * SAMPLE_RATE;
/// Fade in and out over 10 ms, so the tone starts and stops without a click.
const FADE_SAMPLES: u32 = SAMPLE_RATE / 100;
/// The level of each sine at 100 % volume: both together peak at 0.3
/// (about -10 dBFS), loud enough to hear from across a room without
/// startling someone wearing headphones.
const LEVEL: f32 = 0.15;

/// What the engine allows: no ringtone during a call or when turned off, and
/// the device and volume it plays at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RingGate {
    /// This device is joining or in a call.
    pub(crate) in_call: bool,
    /// [`AudioSettings::ringtone`].
    pub(crate) enabled: bool,
    /// [`AudioSettings::output_device`].
    pub(crate) output_device: Option<String>,
    /// [`AudioSettings::output_volume_percent`].
    pub(crate) volume_percent: u16,
}

impl Default for RingGate {
    fn default() -> Self {
        Self::from_settings(&AudioSettings::default(), false)
    }
}

impl RingGate {
    pub(crate) fn from_settings(settings: &AudioSettings, in_call: bool) -> Self {
        Self {
            in_call,
            enabled: settings.ringtone,
            output_device: settings
                .output_device
                .as_ref()
                .map(|d| d.as_str().to_owned()),
            volume_percent: settings.output_volume_percent,
        }
    }

    /// The device and volume the tone plays with, when it may play.
    fn playable(&self) -> Option<(Option<String>, u16)> {
        (self.enabled && !self.in_call).then(|| (self.output_device.clone(), self.volume_percent))
    }
}

/// The ring tone, one 10 ms frame at a time.
#[derive(Debug)]
pub(crate) struct Tone {
    /// Position in the cycle, in samples.
    at: u32,
    /// The level of each sine.
    gain: f32,
}

impl Tone {
    pub(crate) fn new(volume_percent: u16) -> Self {
        Self {
            at: 0,
            gain: LEVEL * f32::from(volume_percent) / 100.0,
        }
    }

    pub(crate) fn fill(&mut self, out: &mut Frame) {
        for sample in out.iter_mut() {
            *sample = if self.at < ON_SAMPLES {
                let fade = self.at.min(ON_SAMPLES - 1 - self.at).min(FADE_SAMPLES);
                let envelope = fade as f32 / FADE_SAMPLES as f32;
                // Whole hertz over one second: the phase restarts each cycle.
                let t = self.at as f32 / SAMPLE_RATE as f32;
                let tone: f32 = TONES_HZ.iter().map(|hz| (TAU * hz * t).sin()).sum();
                self.gain * envelope * tone
            } else {
                0.0
            };
            self.at = (self.at + 1) % CYCLE_SAMPLES;
        }
    }
}

impl FrameProcessor for Tone {
    fn process(&mut self, _capture: &Frame, playback: &mut Frame) {
        self.fill(playback);
    }
}

/// Plays the ringtone while the watcher's `loud` and the engine's gate
/// agree. Stop it with [`Ringer::stop`].
pub(crate) struct Ringer {
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Ringer {
    pub(crate) fn start(
        loud: watch::Receiver<bool>,
        gate: watch::Receiver<RingGate>,
        backend: AudioBackend,
    ) -> Self {
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(run(loud, gate, backend, stopped));
        Self {
            stop: Some(stop),
            task,
        }
    }

    /// Stops the tone and waits until the device is closed.
    pub(crate) async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
    }
}

/// A tone playing, and what it was opened with.
struct Playing {
    io: AudioIo,
    with: (Option<String>, u16),
}

async fn run(
    mut loud: watch::Receiver<bool>,
    mut gate: watch::Receiver<RingGate>,
    backend: AudioBackend,
    mut stopped: oneshot::Receiver<()>,
) {
    let mut playing: Option<Playing> = None;
    loop {
        let sounding = *loud.borrow_and_update();
        let wanted = gate.borrow_and_update().playable().filter(|_| sounding);
        if playing.as_ref().map(|p| &p.with) != wanted.as_ref() {
            if let Some(old) = playing.take() {
                close(old.io).await;
            }
            if let Some(with) = wanted {
                playing = open(&with, &backend).await.map(|io| Playing { io, with });
            }
        }
        tokio::select! {
            changed = loud.changed() => if changed.is_err() { break },
            changed = gate.changed() => if changed.is_err() { break },
            _ = &mut stopped => break,
        }
    }
    if let Some(old) = playing.take() {
        close(old.io).await;
    }
}

/// Opens the output device with the tone; `None` (logged) when it fails.
async fn open((device, volume): &(Option<String>, u16), backend: &AudioBackend) -> Option<AudioIo> {
    let tone = Tone::new(*volume);
    let config = IoConfig {
        capture: false,
        input_device: None,
        playback: true,
        output_device: device.clone(),
        echo_reference: false,
    };
    let backend = backend.clone();
    let opened = tokio::task::spawn_blocking(move || match backend {
        AudioBackend::System => AudioIo::start(config, tone),
        AudioBackend::Virtual(device) => AudioIo::start_virtual(Virtual(device), tone),
    })
    .await;
    match opened {
        Ok(Ok(io)) => {
            tracing::info!("ringtone playing");
            Some(io)
        }
        // The audio crate's messages are fixed text (no user data).
        Ok(Err(e)) => {
            tracing::warn!("the ringtone could not play: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("the ringtone could not play: the start task failed");
            None
        }
    }
}

/// Closes the device off the async threads (dropping it joins its thread).
async fn close(io: AudioIo) {
    let _ = tokio::task::spawn_blocking(move || drop(io)).await;
    tracing::info!("ringtone stopped");
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use twicecat_audio::{FRAME_LEN, SILENCE};

    use super::*;
    use crate::api::{AUDIO_FRAME_LEN, VirtualAudio};

    fn frames(tone: &mut Tone, n: usize) -> Vec<f32> {
        let mut all = Vec::with_capacity(n * FRAME_LEN);
        for _ in 0..n {
            let mut frame = SILENCE;
            tone.fill(&mut frame);
            all.extend_from_slice(&frame);
        }
        all
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn the_tone_sounds_for_a_second_then_rests_for_two() {
        let mut tone = Tone::new(100);
        let cycle = frames(&mut tone, 300);
        let (on, off) = cycle.split_at(ON_SAMPLES as usize);
        assert!(rms(on) > 0.1, "on: {}", rms(on));
        assert!(off.iter().all(|s| *s == 0.0), "off is silent");
        let peak = on.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak <= 2.0 * LEVEL + 1e-6 && peak > LEVEL, "peak {peak}");
        // The next cycle starts again.
        let next = frames(&mut tone, 100);
        assert!(rms(&next) > 0.1);
    }

    #[test]
    fn the_tone_fades_in_and_out_without_a_click() {
        let mut tone = Tone::new(100);
        let on = frames(&mut tone, 100);
        assert_eq!(on[0], 0.0);
        assert!(
            on[1].abs() < 0.01,
            "first samples are near silence: {}",
            on[1]
        );
        assert!(on[ON_SAMPLES as usize - 1].abs() < 0.01);
        let step = on
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        // Two sines at 480 Hz at most change by about 2 * LEVEL * 2π * 480 / 48000.
        assert!(step < 0.03, "largest step {step}");
    }

    #[test]
    fn the_tone_follows_the_master_volume() {
        let loud = rms(&frames(&mut Tone::new(200), 100));
        let normal = rms(&frames(&mut Tone::new(100), 100));
        let off = rms(&frames(&mut Tone::new(0), 100));
        assert!((loud / normal - 2.0).abs() < 0.01, "{loud} / {normal}");
        assert_eq!(off, 0.0);
    }

    #[test]
    fn the_gate_follows_the_settings_and_the_call() {
        let mut settings = AudioSettings::default();
        assert_eq!(
            RingGate::from_settings(&settings, false).playable(),
            Some((None, 100))
        );
        assert_eq!(RingGate::from_settings(&settings, true).playable(), None);
        settings.output_volume_percent = 50;
        settings.output_device = Some(crate::api::AudioDeviceId::from("speakers"));
        assert_eq!(
            RingGate::from_settings(&settings, false).playable(),
            Some((Some("speakers".into()), 50))
        );
        settings.ringtone = false;
        assert_eq!(RingGate::from_settings(&settings, false).playable(), None);
    }

    /// A virtual device counting the frames played, and the loud ones.
    #[derive(Default)]
    struct Speaker {
        frames: AtomicU64,
        loud: AtomicU64,
    }

    impl VirtualAudio for Speaker {
        fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
            frame.fill(0.0);
        }
        fn playback(&self, frame: &[f32; AUDIO_FRAME_LEN]) {
            self.frames.fetch_add(1, Ordering::Relaxed);
            if frame.iter().any(|s| s.abs() > 0.01) {
                self.loud.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    impl Speaker {
        fn frames(&self) -> u64 {
            self.frames.load(Ordering::Relaxed)
        }
        fn loud(&self) -> u64 {
            self.loud.load(Ordering::Relaxed)
        }
    }

    struct Rig {
        speaker: Arc<Speaker>,
        loud: watch::Sender<bool>,
        gate: watch::Sender<RingGate>,
        ringer: Ringer,
    }

    fn rig() -> Rig {
        let speaker = Arc::new(Speaker::default());
        let (loud, loud_rx) = watch::channel(false);
        let (gate, gate_rx) = watch::channel(RingGate::default());
        let ringer = Ringer::start(
            loud_rx,
            gate_rx,
            AudioBackend::Virtual(speaker.clone() as Arc<dyn VirtualAudio>),
        );
        Rig {
            speaker,
            loud,
            gate,
            ringer,
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    /// Waits until `check` holds (the virtual device runs on a 10 ms clock).
    async fn until(what: &str, check: impl Fn() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Waits until the device stopped playing: no frame for 100 ms.
    async fn until_quiet(speaker: &Speaker) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let before = speaker.frames();
            tokio::time::sleep(Duration::from_millis(100)).await;
            if speaker.frames() == before {
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "still playing");
        }
    }

    /// Checks that nothing plays for a while.
    async fn stays_quiet(speaker: &Speaker) {
        let before = speaker.frames();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(speaker.frames(), before, "the device played");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_loud_ring_plays_until_it_stops() {
        let rig = rig();
        stays_quiet(&rig.speaker).await;
        rig.loud.send_replace(true);
        until("the tone plays", || rig.speaker.loud() > 5).await;
        rig.loud.send_replace(false);
        until_quiet(&rig.speaker).await;
        rig.ringer.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_tone_in_a_call_or_when_turned_off() {
        let rig = rig();
        rig.gate.send_modify(|g| g.in_call = true);
        rig.loud.send_replace(true);
        stays_quiet(&rig.speaker).await;
        rig.gate.send_modify(|g| {
            g.in_call = false;
            g.enabled = false;
        });
        stays_quiet(&rig.speaker).await;
        rig.gate.send_modify(|g| g.enabled = true);
        until("the tone plays", || rig.speaker.loud() > 0).await;
        // Joining a call stops it.
        rig.gate.send_modify(|g| g.in_call = true);
        until_quiet(&rig.speaker).await;
        rig.ringer.stop().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_new_volume_reopens_the_tone_and_stopping_closes_it() {
        let rig = rig();
        rig.loud.send_replace(true);
        until("the tone plays", || rig.speaker.loud() > 0).await;
        rig.gate.send_modify(|g| g.volume_percent = 0);
        // At 0 % frames still come, all silent.
        let loud_before = rig.speaker.loud();
        let frames_before = rig.speaker.frames();
        until("silent frames", || {
            rig.speaker.frames() > frames_before + 20
        })
        .await;
        assert!(
            rig.speaker.loud() <= loud_before + 2,
            "the old tone stopped"
        );
        let speaker = rig.speaker.clone();
        rig.ringer.stop().await;
        stays_quiet(&speaker).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_tone_ends_when_the_watch_goes() {
        let rig = rig();
        rig.loud.send_replace(true);
        until("the tone plays", || rig.speaker.loud() > 0).await;
        drop(rig.loud);
        until_quiet(&rig.speaker).await;
        tokio::time::timeout(WAIT, rig.ringer.task)
            .await
            .expect("the ringer ends")
            .expect("no panic");
    }
}
