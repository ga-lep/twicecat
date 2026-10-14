use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64};

use super::sources::{QUIET_FRAMES, rms_db};
use super::*;
use crate::AudioError;
use crate::chain::{LEVEL_FLOOR_DB, NoiseMode};
use crate::gate::GateMode;

fn plain() -> ChainSettings {
    ChainSettings {
        echo_cancellation: false,
        high_pass: false,
        auto_gain: false,
        auto_gain_max_db: 30.0,
        noise: NoiseMode::Off,
        gate: GateMode::VoiceActivity {
            automatic: false,
            threshold_db: LEVEL_FLOOR_DB,
        },
        input_gain_db: 0.0,
    }
}

fn run(engine: &mut VoiceEngine, capture: &Frame) -> Frame {
    let mut out = SILENCE;
    engine.process(capture, &mut out);
    out
}

#[test]
fn sources_are_mixed_with_their_volumes() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (_, mut a) = control.add_source(100).unwrap();
    let (b_id, mut b) = control.add_source(50).unwrap();
    a.push(&[0.2; FRAME_LEN]);
    b.push(&[0.4; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!(
        played.iter().all(|s| (s - 0.4).abs() < 1e-6),
        "{}",
        played[0]
    );
    assert!((b.level_db() - rms_db(&[0.2; FRAME_LEN])).abs() < 0.01);

    control.set_source_volume(b_id, 0);
    control.set_master_volume(200);
    a.push(&[0.2; FRAME_LEN]);
    b.push(&[0.4; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!(
        played.iter().all(|s| (s - 0.4).abs() < 1e-6),
        "{}",
        played[0]
    );

    control.remove_source(b_id);
    a.push(&[0.6; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!(played.iter().all(|s| *s == 1.0), "clamped at full scale");
}

/// Scales by `gain`, or fails; counts drops and records its limit.
struct Scale {
    gain: f32,
    fail: bool,
    limit: Arc<AtomicU32>,
    dropped: Arc<AtomicU64>,
}

impl Scale {
    fn new(gain: f32, fail: bool) -> (Box<Self>, Arc<AtomicU32>, Arc<AtomicU64>) {
        let (limit, dropped) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU64::new(0)));
        let scale = Self {
            gain,
            fail,
            limit: limit.clone(),
            dropped: dropped.clone(),
        };
        (Box::new(scale), limit, dropped)
    }
}

impl Enhancer for Scale {
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        if self.fail {
            return Err(AudioError::Internal);
        }
        frame.iter_mut().for_each(|s| *s *= self.gain);
        Ok(())
    }
    fn set_attenuation_limit(&mut self, db: f32) {
        self.limit.store(db as u32, Ordering::Relaxed);
    }
}

impl Drop for Scale {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_cleaner_works_on_its_own_voice_before_the_volume() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (a_id, mut a) = control.add_source(200).unwrap();
    let (_, mut b) = control.add_source(100).unwrap();
    let (half, limit, _) = Scale::new(0.5, false);
    control.set_source_cleaner(a_id, Some(half));
    control.set_cleaner_attenuation(30.0);
    a.push(&[0.1; FRAME_LEN]);
    b.push(&[0.2; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    // a: 0.1 halved, then doubled by its volume; b as it came.
    assert!(
        played.iter().all(|s| (s - 0.3).abs() < 1e-6),
        "{}",
        played[0]
    );
    assert_eq!(a.cleaning(), SourceCleaning::Running);
    assert_eq!(b.cleaning(), SourceCleaning::None);
    assert_eq!(limit.load(Ordering::Relaxed), 30);
}

#[test]
fn cleaners_come_back_to_the_control_side_to_be_dropped() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (a_id, mut a) = control.add_source(100).unwrap();
    let (first, _, first_dropped) = Scale::new(0.5, false);
    control.set_source_cleaner(a_id, Some(first));
    run(&mut engine, &SILENCE);

    // Replaced: the old one is handed back, not dropped on this thread.
    let (second, _, second_dropped) = Scale::new(0.5, false);
    control.set_source_cleaner(a_id, Some(second));
    run(&mut engine, &SILENCE);
    assert_eq!(first_dropped.load(Ordering::Relaxed), 0);
    assert_eq!(control.drop_retired(), 1);
    assert_eq!(first_dropped.load(Ordering::Relaxed), 1);

    // Failing: the voice plays as it came, and the cleaner comes back.
    let (failing, _, failing_dropped) = Scale::new(0.5, true);
    control.set_source_cleaner(a_id, Some(failing));
    a.push(&[0.25; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!(played.iter().all(|s| *s == 0.25));
    assert_eq!(a.cleaning(), SourceCleaning::None);
    assert_eq!(
        control.drop_retired(),
        2,
        "the replaced one and the failed one"
    );
    assert_eq!(second_dropped.load(Ordering::Relaxed), 1);
    assert_eq!(failing_dropped.load(Ordering::Relaxed), 1);

    // Removed with its voice.
    let (last, _, last_dropped) = Scale::new(0.5, false);
    control.set_source_cleaner(a_id, Some(last));
    control.remove_source(a_id);
    run(&mut engine, &SILENCE);
    assert_eq!(control.drop_retired(), 1);
    assert_eq!(last_dropped.load(Ordering::Relaxed), 1);
}

/// Halves, and counts its calls.
struct Counting(Arc<AtomicU64>);

impl Enhancer for Counting {
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        frame.iter_mut().for_each(|s| *s *= 0.5);
        Ok(())
    }
}

fn counting() -> (Box<Counting>, Arc<AtomicU64>) {
    let calls = Arc::new(AtomicU64::new(0));
    (Box::new(Counting(calls.clone())), calls)
}

#[test]
fn a_quiet_voice_rests_its_cleaner_until_it_talks_again() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (id, mut a) = control.add_source(100).unwrap();
    let (cleaner, calls) = counting();
    control.set_source_cleaner(id, Some(cleaner));
    for _ in 0..5 {
        a.push(&[0.1; FRAME_LEN]);
        run(&mut engine, &SILENCE);
    }
    assert_eq!(calls.load(Ordering::Relaxed), 5);
    // Quiet (-80 dBFS): the cleaner keeps running for 300 ms, then rests.
    for _ in 0..40 {
        a.push(&[1e-4; FRAME_LEN]);
        run(&mut engine, &SILENCE);
    }
    assert_eq!(calls.load(Ordering::Relaxed), 5 + 29);
    assert_eq!(a.cleaning(), SourceCleaning::Quiet);
    a.push(&[1e-4; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!(
        played.iter().all(|s| *s == 0.0),
        "the floor at a 100 dB limit"
    );
    control.set_cleaner_attenuation(20.0);
    a.push(&[1e-4; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!((played[0] - 1e-5).abs() < 1e-9, "-20 dB: {}", played[0]);
    // Talking again: cleaned from the first loud frame.
    a.push(&[0.1; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert_eq!(calls.load(Ordering::Relaxed), 5 + 29 + 1);
    assert_eq!(a.cleaning(), SourceCleaning::Running);
    assert!((played[0] - 0.05).abs() < 1e-6);
}

#[test]
fn over_the_limit_the_loudest_and_priority_voices_are_cleaned() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    control.set_cleaning_slots(1);
    let (a_id, mut a) = control.add_source(100).unwrap();
    let (b_id, mut b) = control.add_source(100).unwrap();
    let (ca, a_calls) = counting();
    let (cb, b_calls) = counting();
    control.set_source_cleaner(a_id, Some(ca));
    control.set_source_cleaner(b_id, Some(cb));
    let frame = |engine: &mut VoiceEngine, a: &mut SourceInput, b: &mut SourceInput, la, lb| {
        a.push(&[la; FRAME_LEN]);
        b.push(&[lb; FRAME_LEN]);
        run(engine, &SILENCE);
    };
    // b is louder: it gets the one cleaner slot; a plays as it came.
    frame(&mut engine, &mut a, &mut b, 0.1, 0.3);
    assert_eq!(b.cleaning(), SourceCleaning::Running);
    assert_eq!(a.cleaning(), SourceCleaning::OverLimit);
    assert_eq!(a.level_db(), rms_db(&[0.1; FRAME_LEN]), "not cleaned");
    // a gets louder, but b keeps its slot while it talks.
    for _ in 0..20 {
        frame(&mut engine, &mut a, &mut b, 0.5, 0.3);
    }
    assert_eq!(b.cleaning(), SourceCleaning::Running);
    assert_eq!(a_calls.load(Ordering::Relaxed), 0);
    // Priority wins at once.
    control.set_source_priority(a_id, true);
    frame(&mut engine, &mut a, &mut b, 0.1, 0.3);
    assert_eq!(a.cleaning(), SourceCleaning::Running);
    assert_eq!(b.cleaning(), SourceCleaning::OverLimit);
    // b goes quiet: its rest does not count, and a keeps running.
    for _ in 0..40 {
        frame(&mut engine, &mut a, &mut b, 0.1, 0.0);
    }
    assert_eq!(b.cleaning(), SourceCleaning::Quiet);
    assert_eq!(a.cleaning(), SourceCleaning::Running);
    let b_before = b_calls.load(Ordering::Relaxed);
    frame(&mut engine, &mut a, &mut b, 0.1, 0.0);
    assert_eq!(b_calls.load(Ordering::Relaxed), b_before);
}

/// Scales row `r` by `gains[r]` in one call, or fails; records its
/// calls, the rows of the last one, its resets and its drop.
struct Bank {
    gains: Vec<f32>,
    fail: Arc<AtomicBool>,
    log: Arc<Mutex<BankLog>>,
}

#[derive(Default)]
struct BankLog {
    calls: usize,
    last_rows: Vec<usize>,
    resets: Vec<usize>,
    limit: Option<f32>,
    dropped: bool,
}

fn bank(gains: &[f32]) -> (Box<Bank>, Arc<AtomicBool>, Arc<Mutex<BankLog>>) {
    let fail = Arc::new(AtomicBool::new(false));
    let log = Arc::new(Mutex::new(BankLog::default()));
    let bank = Bank {
        gains: gains.to_vec(),
        fail: fail.clone(),
        log: log.clone(),
    };
    (Box::new(bank), fail, log)
}

impl BatchCleaner for Bank {
    fn reset_row(&mut self, row: usize) -> Result<(), AudioError> {
        self.log.lock().unwrap().resets.push(row);
        Ok(())
    }
    fn clean(&mut self, frames: &mut [Frame], rows: &[usize]) -> Result<(), AudioError> {
        let mut log = self.log.lock().unwrap();
        log.calls += 1;
        log.last_rows = rows.to_vec();
        if self.fail.load(Ordering::Relaxed) {
            return Err(AudioError::Internal);
        }
        for (frame, &row) in frames.iter_mut().zip(rows) {
            frame.iter_mut().for_each(|s| *s *= self.gains[row]);
        }
        Ok(())
    }
    fn set_attenuation_limit(&mut self, db: f32) {
        self.log.lock().unwrap().limit = Some(db);
    }
}

impl Drop for Bank {
    fn drop(&mut self) {
        self.log.lock().unwrap().dropped = true;
    }
}

#[test]
fn a_bank_cleans_its_voices_in_one_call_quiet_or_not() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (a_id, mut a) = control.add_source(100).unwrap();
    let (b_id, mut b) = control.add_source(100).unwrap();
    let (_, mut plain_voice) = control.add_source(100).unwrap();
    let (bank, _, log) = bank(&[0.5, 1.0, 0.25]);
    control.set_cleaner_bank(Some(bank));
    control.set_source_row(a_id, Some(0));
    control.set_source_row(b_id, Some(2));
    control.set_cleaner_attenuation(30.0);
    // The bank's voices take no cleaning slot.
    control.set_cleaning_slots(0);
    for _ in 0..(QUIET_FRAMES + 5) {
        a.push(&[0.4; FRAME_LEN]);
        b.push(&[0.0; FRAME_LEN]);
        plain_voice.push(&[0.4; FRAME_LEN]);
        run(&mut engine, &SILENCE);
    }
    let log = log.lock().unwrap();
    assert_eq!(log.calls, QUIET_FRAMES as usize + 5, "one call per frame");
    assert_eq!(log.last_rows, [0, 2], "b runs though quiet");
    assert_eq!(log.resets, [0, 2], "each new row starts afresh");
    assert_eq!(log.limit, Some(30.0));
    assert_eq!(a.cleaning(), SourceCleaning::Running);
    assert_eq!(b.cleaning(), SourceCleaning::Running);
    assert_eq!(plain_voice.cleaning(), SourceCleaning::None);
    assert_eq!(a.level_db(), rms_db(&[0.2; FRAME_LEN]), "row 0's gain");
    assert_eq!(plain_voice.level_db(), rms_db(&[0.4; FRAME_LEN]));
}

#[test]
fn a_failing_bank_comes_back_and_its_voices_play_as_they_came() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (a_id, mut a) = control.add_source(100).unwrap();
    let (bank, fail, log) = bank(&[0.5]);
    control.set_cleaner_bank(Some(bank));
    control.set_source_row(a_id, Some(0));
    a.push(&[0.4; FRAME_LEN]);
    run(&mut engine, &SILENCE);
    assert_eq!(a.cleaning(), SourceCleaning::Running);
    fail.store(true, Ordering::Relaxed);
    a.push(&[0.4; FRAME_LEN]);
    run(&mut engine, &SILENCE);
    assert_eq!(a.cleaning(), SourceCleaning::None);
    assert_eq!(a.level_db(), rms_db(&[0.4; FRAME_LEN]), "as it came");
    assert!(!log.lock().unwrap().dropped, "not on the audio thread");
    assert_eq!(control.drop_retired(), 1);
    assert!(log.lock().unwrap().dropped);
    // Without a bank the voice stays plain, its row gone.
    a.push(&[0.4; FRAME_LEN]);
    run(&mut engine, &SILENCE);
    assert_eq!(log.lock().unwrap().calls, 2);
    assert_eq!(a.cleaning(), SourceCleaning::None);
}

#[test]
fn replacing_the_bank_takes_every_row_away() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (a_id, mut a) = control.add_source(100).unwrap();
    let (first, _, first_log) = bank(&[0.5]);
    control.set_cleaner_bank(Some(first));
    control.set_source_row(a_id, Some(0));
    a.push(&[0.4; FRAME_LEN]);
    run(&mut engine, &SILENCE);
    let (second, _, second_log) = bank(&[0.5]);
    control.set_cleaner_bank(Some(second));
    a.push(&[0.4; FRAME_LEN]);
    run(&mut engine, &SILENCE);
    assert_eq!(
        a.cleaning(),
        SourceCleaning::None,
        "no row in the new bank yet"
    );
    assert_eq!(second_log.lock().unwrap().calls, 0);
    assert_eq!(control.drop_retired(), 1);
    assert!(first_log.lock().unwrap().dropped);
    control.set_source_row(a_id, Some(0));
    a.push(&[0.4; FRAME_LEN]);
    run(&mut engine, &SILENCE);
    assert_eq!(a.cleaning(), SourceCleaning::Running);
}

#[test]
fn missing_audio_is_silence_and_counted() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (_, mut a) = control.add_source(100).unwrap();
    a.push(&[0.5; 100]);
    let played = run(&mut engine, &SILENCE);
    assert_eq!(played[99], 0.5);
    assert_eq!(played[100], 0.0);
    assert_eq!(control.stats().source_underruns, (FRAME_LEN - 100) as u64);
}

#[test]
fn a_source_far_ahead_is_trimmed_to_bound_latency() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (_, mut a) = control.add_source(100).unwrap();
    a.push(&vec![0.1; SOURCE_RING_LEN]);
    run(&mut engine, &SILENCE);
    assert!(a.dropped() >= (SOURCE_RING_LEN - SOURCE_MAX_QUEUED) as u64);
}

#[test]
fn deafen_plays_nothing() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (_, mut a) = control.add_source(100).unwrap();
    control.set_deafened(true);
    a.push(&[0.5; FRAME_LEN]);
    assert_eq!(run(&mut engine, &SILENCE), SILENCE);
}

#[test]
fn microphone_goes_out_and_mute_sends_silence() {
    let (mut engine, mut control, mut out) = VoiceEngine::new(plain());
    let mut frame = SILENCE;
    run(&mut engine, &[0.3; FRAME_LEN]);
    assert!(out.pop_frame(&mut frame));
    assert!(out.transmitting());
    assert!(frame[FRAME_LEN - 1] > 0.29);
    control.set_muted(true);
    run(&mut engine, &[0.3; FRAME_LEN]);
    assert!(out.pop_frame(&mut frame));
    assert_eq!(frame, SILENCE);
    assert!(!out.transmitting());
    let meters = control.meters();
    assert!(
        meters.input_rms_db > -12.0,
        "the meter keeps working: {meters:?}"
    );
}

#[test]
fn loopback_plays_the_previous_microphone_frame() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    control.set_loopback(true);
    run(&mut engine, &[0.25; FRAME_LEN]);
    let played = run(&mut engine, &SILENCE);
    assert!(played[FRAME_LEN - 1] > 0.24, "{}", played[FRAME_LEN - 1]);
}

#[test]
fn the_echo_reference_is_what_is_played() {
    // A perfect echo of the played mix must be cancelled; that only
    // works if render() gets exactly the playback frame (after volume).
    crate::flush_denormals_on_this_thread();
    let (mut engine, mut control, mut out) = VoiceEngine::new(ChainSettings {
        echo_cancellation: true,
        ..plain()
    });
    let (_, mut far) = control.add_source(70).unwrap();
    control.set_master_volume(80);
    let mut seed = 0x2468_ace1_u32;
    let mut previous = SILENCE;
    let (mut echo_energy, mut out_energy) = (0.0_f32, 0.0_f32);
    for i in 0..500 {
        let mut f = SILENCE;
        for s in &mut f {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *s = 0.3 * ((seed as f32 / u32::MAX as f32) * 2.0 - 1.0);
        }
        far.push(&f);
        // The microphone hears the previous frame played, at -6 dB.
        let mic: Frame = std::array::from_fn(|j| previous[j] * 0.5);
        previous = run(&mut engine, &mic);
        let mut sent = SILENCE;
        out.pop_frame(&mut sent);
        if i >= 350 {
            echo_energy += mic.iter().map(|s| s * s).sum::<f32>();
            out_energy += sent.iter().map(|s| s * s).sum::<f32>();
        }
    }
    let erle = 10.0 * (echo_energy / out_energy.max(1e-12)).log10();
    assert!(erle >= 20.0, "ERLE {erle:.1} dB");
    assert!(control.stats().chain.delay_ms.is_some());
}

/// A cue that lives for the rest of the test run (cues are `'static`).
fn leak(samples: Vec<f32>) -> &'static [f32] {
    Box::leak(samples.into_boxed_slice())
}

/// 2.5 frames of a small rising ramp.
fn ramp() -> &'static [f32] {
    leak((0..FRAME_LEN * 5 / 2).map(|i| i as f32 * 1e-4).collect())
}

#[test]
fn cue_is_added_to_the_remote_mix_in_order_across_frames() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (_, mut a) = control.add_source(100).unwrap();
    let cue = ramp();
    assert!(control.play_cue(cue, 100));
    for frame in 0..4 {
        a.push(&[0.1; FRAME_LEN]);
        let played = run(&mut engine, &SILENCE);
        for (j, p) in played.iter().enumerate() {
            let i = frame * FRAME_LEN + j;
            let want = 0.1 + cue.get(i).copied().unwrap_or(0.0);
            assert!((p - want).abs() < 1e-6, "sample {i}: {p} != {want}");
        }
    }
}

#[test]
fn cue_is_followed_by_silence() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let cue = leak(vec![0.2; 10]);
    control.play_cue(cue, 100);
    let played = run(&mut engine, &SILENCE);
    assert_eq!(&played[..10], &[0.2; 10]);
    assert!(played[10..].iter().all(|s| *s == 0.0));
    assert_eq!(run(&mut engine, &SILENCE), SILENCE);
}

#[test]
fn cue_is_scaled_by_its_volume_and_the_master_volume() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let cue = ramp();
    control.set_master_volume(50);
    control.play_cue(cue, 50);
    let played = run(&mut engine, &SILENCE);
    for (p, c) in played.iter().zip(cue) {
        assert!((p - 0.25 * c).abs() < 1e-6, "{p} vs {c}");
    }
}

#[test]
fn cue_plays_deafened_while_the_voices_do_not() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    let (_, mut a) = control.add_source(100).unwrap();
    let cue = ramp();
    control.set_deafened(true);
    control.set_master_volume(50);
    control.play_cue(cue, 100);
    a.push(&[0.5; FRAME_LEN * 2]);
    for frame in 0..2 {
        let played = run(&mut engine, &SILENCE);
        for (j, p) in played.iter().enumerate() {
            let want = cue[frame * FRAME_LEN + j] * 0.5;
            assert!((p - want).abs() < 1e-6, "frame {frame} sample {j}: {p}");
        }
    }
}

#[test]
fn non_finite_remote_samples_never_reach_the_speaker() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for deafened in [false, true] {
            let (mut engine, mut control, _out) = VoiceEngine::new(plain());
            let (_, mut a) = control.add_source(100).unwrap();
            control.set_deafened(deafened);
            a.push(&[bad; FRAME_LEN]);
            let played = run(&mut engine, &SILENCE);
            if deafened {
                assert_eq!(played, SILENCE, "deafened, {bad}");
            }
            assert!(
                played.iter().all(|p| p.is_finite()),
                "deafened {deafened}, {bad}: not finite"
            );
        }
    }
}

#[test]
fn cue_volume_is_capped_at_100_percent() {
    let frames = |percent| {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        control.play_cue(ramp(), percent);
        (0..3)
            .map(|_| run(&mut engine, &SILENCE))
            .collect::<Vec<_>>()
    };
    assert!(frames(200) == frames(100), "200 % must play as 100 %");
}

#[test]
fn cue_is_in_the_echo_reference_deafened_or_not() {
    for deafened in [false, true] {
        let (mut engine, mut control, _out) = VoiceEngine::new(ChainSettings {
            echo_cancellation: true,
            ..plain()
        });
        let (_, mut a) = control.add_source(100).unwrap();
        control.set_deafened(deafened);
        control.set_master_volume(80);
        control.play_cue(ramp(), 100);
        for _ in 0..3 {
            a.push(&[0.3; FRAME_LEN]);
            let played = run(&mut engine, &SILENCE);
            assert_ne!(played, SILENCE);
            assert_eq!(engine.rendered, played, "deafened: {deafened}");
        }
    }
}

#[test]
fn cue_never_reaches_the_outgoing_stream() {
    let (mut engine, mut control, mut out) = VoiceEngine::new(plain());
    control.play_cue(ramp(), 100);
    let mut sent = [1.0; FRAME_LEN];
    for frame in 0..4 {
        let played = run(&mut engine, &SILENCE);
        assert_eq!(
            played != SILENCE,
            frame < 3,
            "the cue plays in frame {frame}"
        );
        assert!(out.pop_frame(&mut sent));
        assert_eq!(sent, SILENCE, "the cue leaked into the microphone");
    }
}

#[test]
fn cue_ignores_mute_and_push_to_talk() {
    let cue = ramp();
    let mut reference = Vec::new();
    for (muted, ptt) in [(false, false), (true, false), (false, true), (true, true)] {
        let (mut engine, mut control, _out) = VoiceEngine::new(plain());
        control.set_muted(muted);
        control.push_to_talk(ptt);
        control.play_cue(cue, 100);
        let frames: Vec<Frame> = (0..3).map(|_| run(&mut engine, &SILENCE)).collect();
        assert_ne!(frames[0], SILENCE);
        if reference.is_empty() {
            reference = frames;
        } else {
            assert_eq!(frames, reference, "muted {muted}, push-to-talk {ptt}");
        }
    }
}

#[test]
fn cue_restarts_without_mixing_the_first() {
    let (mut engine, mut control, _out) = VoiceEngine::new(plain());
    control.play_cue(leak(vec![0.3; 100]), 100);
    run(&mut engine, &SILENCE);
    control.play_cue(leak(vec![0.1; 20]), 50);
    let played = run(&mut engine, &SILENCE);
    assert!(played[..20].iter().all(|s| (s - 0.05).abs() < 1e-6));
    assert!(played[20..].iter().all(|s| *s == 0.0));
}
