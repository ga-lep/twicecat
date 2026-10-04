//! The remote voices: each frame of every voice, cleaned (by its own
//! cleaner, or in one call of the engine's bank) and mixed at its volume.
//!
//! Cleaners cost the most here, so they run only on voices that talk: a
//! voice quiet for [`QUIET_FRAMES`] rests its cleaner, and at most
//! `cleaning_slots` cleaners run at once (voices with priority first, then
//! those already running, then the loudest).

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use super::control::{AtomicF32, SourceCleaning};
use super::{SOURCE_MAX_QUEUED, SourceId, VoiceEngine, retire};
use crate::chain::LEVEL_FLOOR_DB;
use crate::enhancer::Enhancer;
use crate::frame::{FRAME_LEN, Frame, SILENCE};

/// A remote voice quieter than this (mean energy, about -60 dBFS)...
pub(super) const QUIET_ENERGY: f32 = 1e-6;
/// ...for this many frames in a row (300 ms) stops running its cleaner,
/// until it is louder again: most voices in a call are silent most of the
/// time, and the network is the cost.
pub(super) const QUIET_FRAMES: u32 = 30;

/// One remote voice, on the processing thread.
pub(super) struct Source {
    pub(super) id: SourceId,
    ring: rtrb::Consumer<f32>,
    pub(super) gain: f32,
    level: Arc<AtomicF32>,
    dropped: Arc<AtomicU64>,
    /// Cleans this voice before it is mixed (DeepFilterNet3, Task 16).
    pub(super) cleaner: Option<Box<dyn Enhancer>>,
    /// Its row in the engine's bank, which cleans it instead.
    pub(super) row: Option<usize>,
    cleaning: Arc<AtomicU8>,
    /// Cleaned before others when too many talk at once.
    pub(super) priority: bool,
    /// This frame's audio.
    chunk: Frame,
    /// Frames in a row below [`QUIET_ENERGY`].
    quiet_frames: u32,
    /// Smoothed energy (about 100 ms), to pick the loudest.
    loudness: f32,
    /// Its cleaner ran in the last frame (kept running while it talks).
    pub(super) running: bool,
    /// Picked to run this frame.
    selected: bool,
}

impl Source {
    /// `shared`: its level, dropped-sample count and cleaning flag, also
    /// held by its `SourceInput`.
    pub(super) fn new(
        id: SourceId,
        ring: rtrb::Consumer<f32>,
        gain: f32,
        (level, dropped, cleaning): (Arc<AtomicF32>, Arc<AtomicU64>, Arc<AtomicU8>),
    ) -> Self {
        Self {
            id,
            ring,
            gain,
            level,
            dropped,
            cleaner: None,
            row: None,
            cleaning,
            priority: false,
            chunk: SILENCE,
            quiet_frames: 0,
            loudness: 0.0,
            running: false,
            selected: false,
        }
    }

    pub(super) fn set_cleaning(&self, state: SourceCleaning) {
        self.cleaning.store(state as u8, Ordering::Relaxed);
    }

    fn is_quiet(&self) -> bool {
        self.quiet_frames >= QUIET_FRAMES
    }

    /// Takes this frame from the ring (silence for what is missing; returns
    /// how many samples were) and updates how loud the voice is.
    fn take_frame(&mut self) -> usize {
        // Trim a source that runs ahead of our clock.
        let queued = self.ring.slots();
        if queued > SOURCE_MAX_QUEUED {
            let excess = queued - SOURCE_MAX_QUEUED;
            if let Ok(chunk) = self.ring.read_chunk(excess) {
                chunk.commit_all();
            }
            self.dropped.fetch_add(excess as u64, Ordering::Relaxed);
        }
        let (filled, missing) = self.ring.pop_partial_slice(&mut self.chunk[..]);
        let got = filled.len();
        missing.fill(0.0);
        let energy = self.chunk.iter().map(|s| s * s).sum::<f32>() / FRAME_LEN as f32;
        self.loudness = 0.9 * self.loudness + 0.1 * energy;
        self.quiet_frames = if energy < QUIET_ENERGY {
            self.quiet_frames.saturating_add(1)
        } else {
            0
        };
        self.selected = false;
        FRAME_LEN - got
    }
}

/// The gain that an attenuation limit of `db` leaves (100 dB or more: none).
pub(super) fn floor_gain(db: f32) -> f32 {
    if db >= 100.0 {
        0.0
    } else {
        10_f32.powf(-db.max(0.0) / 20.0)
    }
}

/// RMS level of a frame, dBFS.
pub(super) fn rms_db(frame: &Frame) -> f32 {
    let mean = frame.iter().map(|s| s * s).sum::<f32>() / FRAME_LEN as f32;
    if mean <= 1e-10 {
        LEVEL_FLOOR_DB
    } else {
        (10.0 * mean.log10()).max(LEVEL_FLOOR_DB)
    }
}

impl VoiceEngine {
    /// The bank failed: it goes back to be dropped, its voices play as they
    /// came (their flags say so).
    pub(super) fn give_up_bank(&mut self) {
        if let Some(bank) = self.bank.take() {
            retire(&mut self.retired, bank);
        }
        for s in &mut self.sources {
            s.row = None;
        }
    }

    /// Cleans every voice with a row in one call of the bank. Returns
    /// whether it did (false: no bank, no such voice, or the bank failed).
    fn clean_batch(&mut self) -> bool {
        let Some(bank) = self.bank.as_mut() else {
            return false;
        };
        self.batch_frames.clear();
        self.batch_rows.clear();
        self.batch_sources.clear();
        for (i, s) in self.sources.iter().enumerate() {
            if let Some(row) = s.row {
                self.batch_frames.push(s.chunk);
                self.batch_rows.push(row);
                self.batch_sources.push(i);
            }
        }
        if self.batch_rows.is_empty() {
            return false;
        }
        if bank
            .clean(&mut self.batch_frames, &self.batch_rows)
            .is_err()
        {
            self.give_up_bank();
            return false;
        }
        for (frame, &i) in self.batch_frames.iter().zip(&self.batch_sources) {
            self.sources[i].chunk = *frame;
        }
        true
    }

    /// Picks the cleaners that run this frame: those of voices that talk,
    /// at most `cleaning_slots`; priority first, then those already running
    /// (no switching back and forth), then the loudest.
    fn select_cleaners(&mut self) {
        self.talking.clear();
        self.talking.extend(
            self.sources
                .iter()
                .enumerate()
                .filter(|(_, s)| s.cleaner.is_some() && s.row.is_none() && !s.is_quiet())
                .map(|(i, _)| i),
        );
        if self.talking.len() > self.cleaning_slots {
            let sources = &self.sources;
            self.talking.sort_unstable_by(|&a, &b| {
                let (a, b) = (&sources[a], &sources[b]);
                (b.priority, b.running)
                    .cmp(&(a.priority, a.running))
                    .then(b.loudness.total_cmp(&a.loudness))
            });
            self.talking.truncate(self.cleaning_slots);
        }
        for &i in &self.talking {
            self.sources[i].selected = true;
        }
    }

    /// This frame of every remote voice, cleaned and mixed into `mix`.
    pub(super) fn mix_sources(&mut self) {
        self.mix = SILENCE;
        let mut missing = 0;
        for source in &mut self.sources {
            missing += source.take_frame();
        }
        if missing > 0 {
            self.shared
                .source_underruns
                .fetch_add(missing as u64, Ordering::Relaxed);
        }

        // The bank's voices, every frame, all in one call.
        let batched = self.clean_batch();
        self.select_cleaners();

        for source in &mut self.sources {
            let quiet = source.is_quiet();
            let state = match &mut source.cleaner {
                _ if source.row.is_some() => {
                    if batched {
                        SourceCleaning::Running
                    } else {
                        SourceCleaning::None
                    }
                }
                None => SourceCleaning::None,
                Some(cleaner) if source.selected => {
                    if cleaner.enhance(&mut source.chunk).is_ok() {
                        SourceCleaning::Running
                    } else {
                        // The voice plays as it came; the owner sees it.
                        if let Some(cleaner) = source.cleaner.take() {
                            retire(&mut self.retired, cleaner);
                        }
                        SourceCleaning::None
                    }
                }
                Some(_) if quiet => {
                    // What the cleaner would leave of a quiet voice.
                    for s in &mut source.chunk {
                        *s *= self.cleaner_floor;
                    }
                    SourceCleaning::Quiet
                }
                Some(_) => SourceCleaning::OverLimit,
            };
            source.running = state == SourceCleaning::Running;
            source.set_cleaning(state);
            for s in &mut source.chunk {
                *s *= source.gain;
            }
            source.level.store(rms_db(&source.chunk));
            for (m, s) in self.mix.iter_mut().zip(source.chunk.iter()) {
                *m += s;
            }
        }
    }
}
