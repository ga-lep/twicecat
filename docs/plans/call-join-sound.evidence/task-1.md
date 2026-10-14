# Evidence — Task 1: A one-shot cue in the voice engine's playback
Commit: 6d9f68a
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir, .cargo/config.toml untouched. Surface is a library API (`VoiceControl::play_cue`) with no outside consumer until Task 2, so the proof is the crate's own tests run for real, plus mutations edited into the real tree and restored from copies (git status clean afterwards). Hearing the cue is a human check (Task 2 wires the actual chime); nothing here is audible.

## Run of the new tests
```console
$ cargo test -p twicecat-audio --lib voice::tests::cue
running 9 tests
test voice::tests::cue_is_followed_by_silence ... ok
test voice::tests::cue_is_scaled_by_its_volume_and_the_master_volume ... ok
test voice::tests::cue_restarts_without_mixing_the_first ... ok
test voice::tests::cue_never_reaches_the_outgoing_stream ... ok
test voice::tests::cue_is_added_to_the_remote_mix_in_order_across_frames ... ok
test voice::tests::cue_plays_deafened_while_the_voices_do_not ... ok
test voice::tests::cue_ignores_mute_and_push_to_talk ... ok
test voice::tests::cue_volume_is_capped_at_100_percent ... ok
test voice::tests::cue_is_in_the_echo_reference_deafened_or_not ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 73 filtered out
$ cargo test -p twicecat-audio --lib voice::tests
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 58 filtered out
```
(The 24 include the 9 cue tests, `non_finite_remote_samples_never_reach_the_speaker`, and the 14 existing voice tests, all green.)

## The cue's samples are played in order, added to the remote mix, across frame boundaries, followed by silence (2.5-frame ramp, one source at 0.1)
Status: PROVEN
Tests: `cue_is_added_to_the_remote_mix_in_order_across_frames` (each sample == 0.1 + ramp[i], 4 frames, ±1e-6) and `cue_is_followed_by_silence`.
Real played samples, via a temporary println in that test (restored afterwards):
```console
$ cargo test -p twicecat-audio --lib voice::tests::cue_is_added -- --nocapture   (temporary print, reverted)
frame 0: first [0.1, 0.1001, 0.100200005] .. last two [0.1478, 0.1479]
frame 1: first [0.148, 0.1481, 0.1482] .. last two [0.1958, 0.1959]
frame 2: first [0.19600001, 0.1961, 0.1962] .. last two [0.1, 0.1]
frame 3: first [0.1, 0.1, 0.1] .. last two [0.1, 0.1]
test result: ok. 1 passed
```
The ramp continues across frame boundaries on top of 0.1, ends mid-frame 2, and the output returns to the bare 0.1 remote voice.

## At master 50 % and cue 50 %, played = 0.25 x cue (±1e-6)
Status: PROVEN — `cue_is_scaled_by_its_volume_and_the_master_volume` passes (see run above).

## Deafened: played frames are exactly cue x master_gain
Status: PROVEN — `cue_plays_deafened_while_the_voices_do_not` (source at 0.5, master 50 %, deafened; played == cue*0.5), and `non_finite_remote_samples_never_reach_the_speaker` (NaN/±inf in a source: deafened output == SILENCE exactly, never non-finite when not deafened). Both pass.

## Cue volume capped at 100 %
Status: PROVEN — `cue_volume_is_capped_at_100_percent` (200 plays identical to 100) passes.

## The frame handed to the echo canceller's render contains the cue, deafened or not
Status: PROVEN — `cue_is_in_the_echo_reference_deafened_or_not` (echo_cancellation: true; cfg(test) `rendered` == played, both deafen values) passes.

## The cue never reaches Outgoing
Status: PROVEN — `cue_never_reaches_the_outgoing_stream` passes (every Outgoing frame is silence for 4 frames while the cue plays 3).

## Mute and push-to-talk do not change what the cue plays; a second play_cue restarts without mixing
Status: PROVEN — `cue_ignores_mute_and_push_to_talk` (4 mute/PTT combos give identical frames) and `cue_restarts_without_mixing_the_first` (0.05 for 20 samples, then 0) pass.

## No allocation in `process` on the cue path
Status: PROVEN by reading the code (no allocation counter exists in `voice::tests`): `next_cue_frame` writes a stack `Frame` from a `'static` slice; `Command::Cue` stores the slice reference and integer state only; `rendered` is a cfg(test) fixed array. No Vec/Box on the path.

## Mutations (each edited into the real tree, run, restored from a pre-made copy)
Status: PROVEN
```console
M1 cue mixed after render (render sees voices only)
test voice::tests::cue_is_in_the_echo_reference_deafened_or_not ... FAILED
assertion `left == right` failed: deafened: false
test result: FAILED. 23 passed; 1 failed
M2 deafen silences the cue
test voice::tests::cue_plays_deafened_while_the_voices_do_not ... FAILED
test voice::tests::cue_is_in_the_echo_reference_deafened_or_not ... FAILED
test result: FAILED. 22 passed; 2 failed
M3 cue added to the mic
test voice::tests::cue_never_reaches_the_outgoing_stream ... FAILED
assertion `left == right` failed: the cue leaked into the microphone
test result: FAILED. 23 passed; 1 failed
M4 NaN guard removed (`*p = out;`)
test voice::tests::non_finite_remote_samples_never_reach_the_speaker ... FAILED
test result: FAILED. 23 passed; 1 failed
```
Each mutation fails exactly the intended test(s). After restoring: `voice::tests` 24 passed, `git status --short` empty.

## Gate / commit message
Status: NOT PROVEN (internal — fmt/clippy/workspace test and commit format not re-run here). The README-level check `bash scripts/check-no-webrtc-download.sh` was run: exit=0. (Run with `sh` it fails with "set: Illegal option -o pipefail"; it needs bash.)

## Audible cue
Status: NOT PROVEN (human check — Task 2 wires the actual chime).

## Verdict
EVIDENCE: PROVEN
