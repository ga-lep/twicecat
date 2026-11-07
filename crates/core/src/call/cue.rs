//! Playing a call cue from the call's tasks.

use tokio::sync::watch;

use crate::api::CallSounds;
use crate::audio_session::{Voice, voice_control};
use crate::call_sounds::{self, Cue};

/// What a call's tasks need to play a cue: the call's voice engine and the
/// live call-sound settings, so a change applies without a restart. Playing
/// is local only: it queues the cue in the voice engine's playback and
/// nothing is sent.
#[derive(Clone)]
pub(crate) struct CueHandle {
    voice: Option<Voice>,
    sounds: watch::Receiver<CallSounds>,
}

impl CueHandle {
    pub(super) fn new(voice: Option<Voice>, sounds: watch::Receiver<CallSounds>) -> Self {
        Self { voice, sounds }
    }

    /// Plays `cue` if the current settings allow it (and 0 % is silent).
    /// Returns whether it was queued. Never blocks: it only queues a command.
    pub(crate) fn play(&self, cue: Cue) -> bool {
        let Some(voice) = &self.voice else {
            return false;
        };
        let volume = call_sounds::playable_volume(&self.sounds.borrow(), cue);
        let Some(volume) = volume else {
            return false;
        };
        let queued = voice_control(voice).play_cue(call_sounds::samples(cue), volume);
        if queued {
            tracing::debug!(cue = cue.name(), "call sound");
        }
        queued
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::api::{AUDIO_FRAME_LEN, AudioBackend, AudioSettings, VirtualAudio};
    use crate::audio_session::{AudioSession, Mode};

    #[derive(Default)]
    struct Speaker {
        played: Mutex<Vec<f32>>,
    }

    impl VirtualAudio for Speaker {
        fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
            frame.fill(0.0);
        }
        fn playback(&self, frame: &[f32; AUDIO_FRAME_LEN]) {
            self.played.lock().unwrap().extend_from_slice(frame);
        }
    }

    fn energy(speaker: &Speaker) -> f32 {
        speaker.played.lock().unwrap().iter().map(|s| s * s).sum()
    }

    /// A call's voice engine on a recording speaker.
    async fn call_voice() -> (AudioSession, Arc<Speaker>) {
        let speaker = Arc::new(Speaker::default());
        let (events, mut rx) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let (session, _outgoing) = AudioSession::start(
            &AudioSettings::default(),
            Mode::Call,
            &AudioBackend::Virtual(speaker.clone()),
            events,
        )
        .await
        .expect("a virtual device always opens");
        (session, speaker)
    }

    async fn played_out() {
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    #[tokio::test]
    async fn the_live_setting_decides_whether_the_blip_plays() {
        let (session, speaker) = call_voice().await;
        let (tx, rx) = watch::channel(CallSounds::default());
        let cues = CueHandle::new(Some(session.voice()), rx);

        // Switched off while in the call: nothing plays.
        tx.send_replace(CallSounds {
            others_joining: false,
            ..CallSounds::default()
        });
        assert!(!cues.play(Cue::OtherJoined));
        played_out().await;
        assert!(energy(&speaker) < 1e-9, "played with others_joining off");

        // Back on, without a restart: it plays, at the live volume.
        tx.send_replace(CallSounds::default());
        assert!(cues.play(Cue::OtherJoined));
        played_out().await;
        let full: f32 = call_sounds::samples(Cue::OtherJoined)
            .iter()
            .map(|s| s * s)
            .sum();
        let ratio = energy(&speaker) / (full * 0.36);
        assert!((0.98..=1.02).contains(&ratio), "60 % blip, ratio {ratio}");

        // Disabled, and 0 %: silent.
        tx.send_replace(CallSounds {
            enabled: false,
            ..CallSounds::default()
        });
        assert!(!cues.play(Cue::OtherJoined));
        tx.send_replace(CallSounds {
            volume_percent: 0,
            ..CallSounds::default()
        });
        assert!(!cues.play(Cue::OtherJoined));
        session.stop().await;
    }

    #[tokio::test]
    async fn without_a_voice_engine_there_is_no_cue() {
        let (_tx, rx) = watch::channel(CallSounds::default());
        assert!(!CueHandle::new(None, rx).play(Cue::OtherJoined));
    }

    #[tokio::test]
    async fn the_blip_plays_while_deafened() {
        let (session, speaker) = call_voice().await;
        let (_tx, rx) = watch::channel(CallSounds::default());
        let cues = CueHandle::new(Some(session.voice()), rx);
        session.set_deafened(true);
        assert!(cues.play(Cue::OtherJoined));
        played_out().await;
        assert!(energy(&speaker) > 1e-6, "silent while deafened");
        session.stop().await;
    }
}
