//! The transmit gate: voice activity (automatic or a fixed threshold) or
//! push-to-talk, with hangover and short gain ramps so it never clicks.

use crate::frame::{FRAME_LEN, Frame};

/// When the microphone transmits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GateMode {
    /// Open on voice.
    VoiceActivity {
        /// Track the noise floor (or use the denoiser's voice probability)
        /// instead of a fixed threshold.
        automatic: bool,
        /// Fixed threshold in dBFS, used when not automatic.
        threshold_db: f32,
    },
    /// Open while the key is held, and `release_ms` after.
    PushToTalk {
        /// Tail after release, ms.
        release_ms: u32,
    },
}

/// Frames the gate stays open after the last voice trigger (200 ms), so word
/// endings and short pauses are not cut.
const HANGOVER_FRAMES: u32 = 20;
/// Automatic mode opens this far above the tracked noise floor.
const AUTO_MARGIN_DB: f32 = 12.0;
/// Automatic mode with a voice probability: open at or above this.
const VOICE_PROBABILITY: f32 = 0.6;
/// Noise floor rise per frame (6 dB/s); it falls at once.
const FLOOR_RISE_DB: f32 = 0.06;
/// Gain change per sample: open in 2.5 ms, close in 10 ms.
const ATTACK_STEP: f32 = 1.0 / 120.0;
const RELEASE_STEP: f32 = 1.0 / 480.0;

pub(crate) struct Gate {
    mode: GateMode,
    hold: u32,
    ptt_pressed: bool,
    ptt_tail: u32,
    noise_floor_db: f32,
    gain: f32,
}

impl Gate {
    pub(crate) fn new(mode: GateMode) -> Self {
        Self {
            mode,
            hold: 0,
            ptt_pressed: false,
            ptt_tail: 0,
            noise_floor_db: -60.0,
            gain: 0.0,
        }
    }

    pub(crate) fn set_mode(&mut self, mode: GateMode) {
        if std::mem::discriminant(&mode) != std::mem::discriminant(&self.mode) {
            self.hold = 0;
            self.ptt_tail = 0;
        }
        self.mode = mode;
    }

    pub(crate) fn set_push_to_talk(&mut self, pressed: bool) {
        if self.ptt_pressed
            && !pressed
            && let GateMode::PushToTalk { release_ms } = self.mode
        {
            self.ptt_tail = release_ms.div_ceil(10);
        }
        self.ptt_pressed = pressed;
    }

    /// Decides for this frame. `level_db` is the frame's RMS level; `voice`
    /// a voice probability when a denoiser provides one.
    pub(crate) fn decide(&mut self, level_db: f32, voice: Option<f32>) -> bool {
        match self.mode {
            GateMode::PushToTalk { .. } => {
                if self.ptt_pressed {
                    true
                } else if self.ptt_tail > 0 {
                    self.ptt_tail -= 1;
                    true
                } else {
                    false
                }
            }
            GateMode::VoiceActivity {
                automatic,
                threshold_db,
            } => {
                let trigger = if automatic {
                    self.track_floor(level_db);
                    match voice {
                        Some(p) => p >= VOICE_PROBABILITY && level_db > self.noise_floor_db,
                        None => level_db >= self.noise_floor_db + AUTO_MARGIN_DB,
                    }
                } else {
                    level_db >= threshold_db
                };
                if trigger {
                    self.hold = HANGOVER_FRAMES;
                    true
                } else if self.hold > 0 {
                    self.hold -= 1;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn track_floor(&mut self, level_db: f32) {
        if level_db < self.noise_floor_db {
            self.noise_floor_db = level_db;
        } else {
            self.noise_floor_db += FLOOR_RISE_DB;
        }
        self.noise_floor_db = self.noise_floor_db.clamp(-90.0, -20.0);
    }

    /// Applies the decision with a ramp, in place.
    pub(crate) fn apply(&mut self, frame: &mut Frame, open: bool) {
        let (target, step) = if open {
            (1.0, ATTACK_STEP)
        } else {
            (0.0, RELEASE_STEP)
        };
        if self.gain == target {
            if target == 0.0 {
                frame.fill(0.0);
            }
            return;
        }
        for s in frame.iter_mut().take(FRAME_LEN) {
            self.gain = if open {
                (self.gain + step).min(target)
            } else {
                (self.gain - step).max(target)
            };
            *s *= self.gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vad(threshold_db: f32) -> Gate {
        Gate::new(GateMode::VoiceActivity {
            automatic: false,
            threshold_db,
        })
    }

    #[test]
    fn fixed_threshold_opens_and_holds_for_the_hangover() {
        let mut g = vad(-40.0);
        assert!(!g.decide(-50.0, None));
        assert!(g.decide(-30.0, None));
        for _ in 0..HANGOVER_FRAMES {
            assert!(g.decide(-60.0, None), "hangover");
        }
        assert!(!g.decide(-60.0, None), "closed after the hangover");
    }

    #[test]
    fn automatic_tracks_the_noise_floor() {
        let mut g = Gate::new(GateMode::VoiceActivity {
            automatic: true,
            threshold_db: 0.0,
        });
        // Two seconds of steady -50 dB noise: the floor settles there.
        for _ in 0..200 {
            g.decide(-50.0, None);
        }
        for _ in 0..=HANGOVER_FRAMES {
            g.decide(-50.0, None);
        }
        assert!(!g.decide(-49.0, None), "noise does not open");
        assert!(g.decide(-30.0, None), "speech 20 dB above opens");
    }

    #[test]
    fn automatic_uses_the_voice_probability_when_given() {
        let mut g = Gate::new(GateMode::VoiceActivity {
            automatic: true,
            threshold_db: 0.0,
        });
        for _ in 0..50 {
            g.decide(-70.0, Some(0.0));
        }
        assert!(!g.decide(-35.0, Some(0.1)), "loud but not voice");
        assert!(g.decide(-35.0, Some(0.9)), "voice");
    }

    #[test]
    fn push_to_talk_ignores_level_and_keeps_its_tail() {
        let mut g = Gate::new(GateMode::PushToTalk { release_ms: 50 });
        assert!(!g.decide(0.0, Some(1.0)), "loud voice without the key");
        g.set_push_to_talk(true);
        assert!(g.decide(-100.0, None));
        g.set_push_to_talk(false);
        for _ in 0..5 {
            assert!(g.decide(-100.0, None), "50 ms tail");
        }
        assert!(!g.decide(-100.0, None));
    }

    #[test]
    fn ramps_avoid_clicks_and_closed_is_silent() {
        let mut g = vad(-40.0);
        let mut frame = [1.0; FRAME_LEN];
        g.apply(&mut frame, true);
        assert!(frame[0] < 0.05, "starts near zero: {}", frame[0]);
        assert_eq!(frame[FRAME_LEN - 1], 1.0, "fully open within 2.5 ms");
        let mut frame = [1.0; FRAME_LEN];
        g.apply(&mut frame, false);
        assert!(frame[0] > 0.95 && frame[FRAME_LEN - 1] == 0.0);
        let mut frame = [1.0; FRAME_LEN];
        g.apply(&mut frame, false);
        assert!(frame.iter().all(|s| *s == 0.0));
    }
}
