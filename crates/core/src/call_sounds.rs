//! The call cues (call sounds plan, Task 2): short, quiet sounds for three
//! moments of a call. They are made here, in code, so there is no sound
//! file or licence to ship.
//!
//! - [`Cue::Joined`]: a rising two-note chime (E5, then A5) when this device
//!   joins or starts a call.
//! - [`Cue::Left`]: the same notes falling (A5, then E5) when the user leaves.
//! - [`Cue::OtherJoined`]: one softer G5 blip when another person joins.
//!
//! Where each one is played (exactly one call site per cue) is decided in the
//! engine: the join cue in `join_call`'s success branch only (not on a repeat
//! join, a reconnect or a failure), the leave cue only for `Command::LeaveCall`
//! with a call (not on a failed call, a room switch, shutdown or logout).
//!
//! A cue is played by the call's voice engine, mixed into the call's
//! playback: on the call's own output device, and part of the echo
//! canceller's reference, whatever mute, push-to-talk or deafen say. Echo
//! cancellation therefore removes a cue from the microphone like any remote
//! voice. Two risks remain: with "Echo cancellation" off, speakers (not
//! headphones) let the microphone pick the cue up and send it to the room;
//! and with it on, a faint trace of the join cue can leak at the very start
//! of a call, before the canceller has converged. The cues are short and
//! quiet (about -24 dBFS at the default 60 %) to keep both small.
//!
//! Each note has a 5 ms attack, an exponential decay (time constant 50 ms)
//! and a 15 ms fade to exact zero, so there is no click.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use twicecat_audio::SAMPLE_RATE;

use crate::api::CallSounds;

/// One of the call cues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cue {
    /// This device joined (or started) a call.
    Joined,
    /// The user left the call.
    Left,
    /// Another person joined the call.
    // Played by the participant watcher (call sounds plan, Task 3); until
    // then only the tests build it.
    #[cfg_attr(not(test), allow(dead_code))]
    OtherJoined,
}

impl Cue {
    /// A name for logs (a cue logs nothing else).
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Joined => "joined",
            Self::Left => "left",
            Self::OtherJoined => "other-joined",
        }
    }
}

/// E5, A5 and G5, Hz. All clear of the 1 kHz tone the call tests send.
const E5: f32 = 659.25;
const A5: f32 = 880.0;
const G5: f32 = 784.0;

/// One note of a cue.
const CHIME_NOTE_MS: u32 = 120;
const BLIP_MS: u32 = 90;
/// Peaks at 100 % volume: -20 dBFS for the chimes, -26 dBFS for the blip.
const CHIME_PEAK: f32 = 0.1;
const BLIP_PEAK: f32 = 0.05;
const ATTACK_MS: f32 = 5.0;
const DECAY_TAU_MS: f32 = 50.0;
const FADE_MS: u32 = 15;

/// The three cues' samples, made once.
struct Cues {
    joined: Vec<f32>,
    left: Vec<f32>,
    other_joined: Vec<f32>,
}

fn samples_for_ms(ms: u32) -> usize {
    (SAMPLE_RATE as usize * ms as usize) / 1000
}

/// One note: `hz` for `ms`, peaking at `peak`.
fn note(hz: f32, ms: u32, peak: f32) -> Vec<f32> {
    let rate = SAMPLE_RATE as f32;
    let len = samples_for_ms(ms);
    let attack = ATTACK_MS * rate / 1000.0;
    let tau = DECAY_TAU_MS * rate / 1000.0;
    let fade = samples_for_ms(FADE_MS);
    (0..len)
        .map(|i| {
            let t = i as f32;
            let rise = (t / attack).min(1.0);
            let decay = (-(t - attack).max(0.0) / tau).exp();
            // Reaches exactly 0 at the last sample.
            let left = (len - 1 - i) as f32;
            let fall = (left / fade as f32).min(1.0);
            peak * rise * decay * fall * (TAU * hz * t / rate).sin()
        })
        .collect()
}

fn chime(first: f32, second: f32) -> Vec<f32> {
    let mut out = note(first, CHIME_NOTE_MS, CHIME_PEAK);
    out.extend(note(second, CHIME_NOTE_MS, CHIME_PEAK));
    out
}

fn cues() -> &'static Cues {
    static CUES: OnceLock<Cues> = OnceLock::new();
    CUES.get_or_init(|| Cues {
        joined: chime(E5, A5),
        left: chime(A5, E5),
        other_joined: note(G5, BLIP_MS, BLIP_PEAK),
    })
}

/// The cue's samples at 100 % volume, 48 kHz mono, made on first use. The
/// same slice is returned every time: the audio thread only reads it.
pub(crate) fn samples(cue: Cue) -> &'static [f32] {
    let cues = cues();
    match cue {
        Cue::Joined => &cues.joined,
        Cue::Left => &cues.left,
        Cue::OtherJoined => &cues.other_joined,
    }
}

/// The volume to play `cue` at, or `None` when the settings say it stays
/// silent.
pub(crate) fn volume(sounds: &CallSounds, cue: Cue) -> Option<u16> {
    let wanted = sounds.enabled
        && match cue {
            Cue::Joined => true,
            Cue::Left => sounds.leaving,
            Cue::OtherJoined => sounds.others_joining,
        };
    wanted.then_some(sounds.volume_percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = SAMPLE_RATE as f32;
    const ALL: [Cue; 3] = [Cue::Joined, Cue::Left, Cue::OtherJoined];

    /// Goertzel magnitude of `hz` in `s`, scaled like the amplitude of a
    /// sine of that frequency.
    fn level(s: &[f32], hz: f32) -> f32 {
        let w = TAU * hz / RATE;
        let coeff = 2.0 * w.cos();
        let (mut a, mut b) = (0.0_f32, 0.0_f32);
        for x in s {
            let next = x + coeff * a - b;
            b = a;
            a = next;
        }
        let power = a * a + b * b - coeff * a * b;
        2.0 * power.max(0.0).sqrt() / s.len().max(1) as f32
    }

    fn expected_ms(cue: Cue) -> f32 {
        match cue {
            Cue::Joined | Cue::Left => 240.0,
            Cue::OtherJoined => 90.0,
        }
    }

    fn expected_peak(cue: Cue) -> f32 {
        match cue {
            Cue::Joined | Cue::Left => 0.1,
            Cue::OtherJoined => 0.05,
        }
    }

    #[test]
    fn each_cue_has_the_length_of_the_plan() {
        for cue in ALL {
            let ms = samples(cue).len() as f32 * 1000.0 / RATE;
            assert!((ms - expected_ms(cue)).abs() <= 20.0, "{cue:?}: {ms} ms");
        }
    }

    #[test]
    fn each_cue_peaks_where_the_plan_says() {
        for cue in ALL {
            let peak = samples(cue).iter().fold(0.0_f32, |m, s| m.max(s.abs()));
            let want = expected_peak(cue);
            assert!(
                (peak - want).abs() <= 0.05 * want,
                "{cue:?}: peak {peak}, want {want}"
            );
        }
    }

    #[test]
    fn each_cue_starts_and_ends_at_exact_zero() {
        for cue in ALL {
            let s = samples(cue);
            assert_eq!(s.first(), Some(&0.0), "{cue:?}");
            assert_eq!(s.last(), Some(&0.0), "{cue:?}");
        }
    }

    #[test]
    fn no_cue_clicks() {
        for cue in ALL {
            let step = samples(cue)
                .windows(2)
                .fold(0.0_f32, |m, w| m.max((w[1] - w[0]).abs()));
            assert!(step < 0.02, "{cue:?}: largest step {step}");
        }
    }

    #[test]
    fn each_cue_stays_clear_of_the_1_khz_test_tone() {
        for cue in ALL {
            let s = samples(cue);
            let main = [E5, A5, G5]
                .into_iter()
                .map(|hz| level(s, hz))
                .fold(0.0_f32, f32::max);
            let at_1k = level(s, 1000.0);
            let db = 20.0 * (at_1k / main).log10();
            assert!(
                db <= -30.0,
                "{cue:?}: 1 kHz is {db:.1} dB from the main note"
            );
        }
    }

    /// The louder of E5 and A5 in `s`: the note that dominates it.
    fn dominant(s: &[f32]) -> f32 {
        if level(s, E5) > level(s, A5) { E5 } else { A5 }
    }

    #[test]
    fn the_join_cue_rises_and_the_leave_cue_falls() {
        let halves = |cue| {
            let s = samples(cue);
            let (a, b) = s.split_at(s.len() / 2);
            (dominant(a), dominant(b))
        };
        assert_eq!(halves(Cue::Joined), (E5, A5));
        assert_eq!(halves(Cue::Left), (A5, E5));
    }

    #[test]
    fn the_blip_is_one_g5_note() {
        let s = samples(Cue::OtherJoined);
        assert!(level(s, G5) > 3.0 * level(s, E5));
        assert!(level(s, G5) > 3.0 * level(s, A5));
    }

    #[test]
    fn samples_are_made_once() {
        for cue in ALL {
            assert!(std::ptr::eq(samples(cue).as_ptr(), samples(cue).as_ptr()));
        }
        assert!(!std::ptr::eq(
            samples(Cue::Joined).as_ptr(),
            samples(Cue::Left).as_ptr()
        ));
    }

    #[test]
    fn volume_follows_the_settings() {
        let all_on = CallSounds::default();
        for cue in ALL {
            assert_eq!(volume(&all_on, cue), Some(60), "{cue:?}");
            let off = CallSounds {
                enabled: false,
                ..all_on
            };
            assert_eq!(volume(&off, cue), None, "{cue:?} when disabled");
        }
        let quiet_others = CallSounds {
            others_joining: false,
            volume_percent: 40,
            ..all_on
        };
        assert_eq!(volume(&quiet_others, Cue::OtherJoined), None);
        assert_eq!(volume(&quiet_others, Cue::Joined), Some(40));
        assert_eq!(volume(&quiet_others, Cue::Left), Some(40));
        let stay = CallSounds {
            leaving: false,
            ..all_on
        };
        assert_eq!(volume(&stay, Cue::Left), None);
        assert_eq!(volume(&stay, Cue::Joined), Some(60));
        assert_eq!(volume(&stay, Cue::OtherJoined), Some(60));
    }
}
