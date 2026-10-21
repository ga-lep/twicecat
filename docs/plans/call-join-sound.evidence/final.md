# Evidence — call-join-sound (whole feature, final)
Commit: 5bb2a81 (branch feat/call-sounds)
Environment: CC=clang-22 CXX=clang++-22, the repo's own target dir, LK_CUSTOM_WEBRTC left as in .cargo/config.toml. Local stack from `scripts/stack.sh up -d --wait` (alice, carol from .env.test), shut down with `stack.sh down -v` at the end. Headless: virtual speakers record what each call plays; no real audio device, nobody listened.

## 1. Full gate
Status: PROVEN
```console
$ cargo fmt --all --check            -> exit=0
$ cargo clippy --workspace --all-targets -- -D warnings   -> exit=0
$ cargo test --workspace             -> exit=0; 59 "test result" lines, sum: 2303 passed, 0 failed, 171 ignored
$ cargo check --workspace --lib --bins --locked           -> exit=0
$ bash scripts/check-app-deps.sh          -> exit 0
$ bash scripts/check-core-api.sh          -> exit 0
$ bash scripts/check-font-assets.sh       -> "OK for 5 files", exit 0
$ bash scripts/check-no-webrtc-download.sh -> exit 0 (run first with the gate, and again as the last step after the stack was down)
$ bash scripts/check-commits.sh origin/main HEAD -> exit 0
$ git log --oneline origin/main..HEAD
5bb2a81 docs(plan): resolve call-join-sound questions Q8-Q10 and record the label deviation
fb8530d docs(plan): ledger: call-join-sound task 4 complete
1403d1b docs(plan): evidence: call-join-sound task 4
027f169 feat(app): call sounds settings on the Ringtone page        Step: snd.4
cadc330 docs(plan): custom-theme
a025390 docs(plan): ledger: call-join-sound task 3 complete, start task 4
6745ec3 docs(plan): evidence: call-join-sound task 3
994bf7e feat(core): a soft cue when someone joins the call           Step: snd.3
b01970a docs(plan): ledger: call-join-sound task 2 complete, start task 3
58d8757 docs(plan): evidence: call-join-sound task 2
0b2db85 feat(core): call sounds for joining, starting and leaving a call   Step: snd.2
```
Note: the snd.1 commit (835c33f `feat(audio): play a one-shot cue ...`, Step: snd.1) is already an ancestor of origin/main, so it is not in origin/main..HEAD; the three feat commits above carry snd.2, snd.3, snd.4.

## 2. Ignored end-to-end suites on the local stack
Status: PROVEN (call_ringing: one known-flaky failure, reported)
```console
$ cargo test -p twicecat-core --test call_sounds -- --ignored --test-threads=1
test a_failed_join_plays_no_cue ... ok
test a_repeat_join_is_connected_without_a_cue ... ok
test joining_plays_a_rising_cue_to_the_one_who_joins ... ok
test leaving_deafened_plays_the_falling_cue_without_delaying_left ... ok
test other_joins_play_one_blip_to_the_others_only ... ok
test switching_rooms_plays_only_the_new_join_cue ... ok
test the_cue_never_reaches_the_room ... ok
test the_settings_turn_the_cues_off ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; finished in 104.49s
```
(+ the non-ignored detector self-test `the_detector_tells_rising_from_falling_and_nothing`, filtered out by `--ignored`, passed in the workspace run.)
```console
$ call_media:        9 passed; 0 failed (198.52s)  incl. alices_tone_reaches_carols_speakers_and_mute_stops_it, a_mic_test_during_a_call_leaves_the_call_playing, remote_voices_are_cleaned_when_asked
$ call_signalling:   3 passed; 0 failed (81.32s)
$ call_log_hygiene:  1 passed; 0 failed (23.05s)   call_keys_never_reach_the_logs
$ call_ringing (run ONCE): 5 passed, 1 failed (143.96s)
test answering_stops_the_ringtone ... FAILED
timed out after 30s waiting for an event at crates/core/tests/call_ringing.rs:460:5; last events seen: [RoomListChanged x7, UnreadTotals]
```
call_ringing is the known flake (it does not touch call cues; failure is a 30 s wait for a ring event). Not chased. No NVENC/video tests were in these four suites; none failed for hardware.

## 3. The chain in numbers (scratch test, temporary prints; file restored from a copy, `git status` clean)
Status: PROVEN (as measured on virtual speakers)
Scratch test `chain_numbers` appended to a copy-saved crates/core/tests/call_sounds.rs: carol joins a shared room (default settings), then alice joins, alice sets deafened then leaves. Detector: Goertzel levels in 10 ms windows; peak = max |sample| of the sound segment; duration = first to last sample above 0.0005 (gap > 100 ms splits).
```console
CHAIN carol own join:          bursts=[Rising]  peak 0.0595 = -24.5 dBFS, duration 238 ms, order ["E5","A5"]  (E5 0.0497, A5 0.0483)
CHAIN alice own join:          bursts=[Rising]  peak 0.0595 = -24.5 dBFS, duration 238 ms, order ["E5","A5"]
CHAIN carol hears alice join:  bursts=[]        peak 0.0297 = -30.5 dBFS, duration  88 ms, order ["G5"]       (G5 0.0245; 1 segment)
CHAIN alice speaker, deafened, between mute-set and leave: 0 segments (silence)
CHAIN alice leave, deafened:   bursts=[Falling] peak 0.0595 = -24.5 dBFS, duration 239 ms, order ["A5","E5"]
CHAIN carol after alice left:  0 segments (no sound when another leaves, Q8)
CHAIN ROOM alice speaker:      bursts=[Rising]  peak 0.0595 = -24.5 dBFS, duration 238 ms
CHAIN ROOM carol recording during alice's join (carol's own cues off): 208800 samples (4.3 s), peak abs 0.000000, max E5 0.000000, A5 0.000000, G5 0.000000, cue_level 0.000000 (SILENT limit 0.001)
test chain_numbers ... ok (50.38 s)
```
Matches plan: join E5 then A5 at about -24.4 dBFS (60 %), about 240 ms; leave A5 then E5 same level; blip G5 about -30.4 dBFS, about 90 ms; cues played while deafened; carol's room recording is exactly silent, so the cue never reaches the room (mic silent, so this does not cover an acoustic echo path).
```console
$ git status --short   (after restoring from the copy)  -> empty
```

## 4. CLI as alice against the local stack
Status: PROVEN
`twicecat-cli --ephemeral --homeserver http://127.0.0.1:8008 --password-stdin --silent-audio`, `/login`, alice, password (fed from .env.test, redacted), then the commands. Each `/sounds` prints the audio summary, then `/audio` prints it again. Separate sessions, grep "Call sounds|Usage":
```console
### /sounds off | /audio
Call sounds off.
Call sounds off.
### /sounds 40 | /audio
Call sounds on (others joining, leaving), 40 %.
Call sounds on (others joining, leaving), 40 %.
### /sounds 150 | /audio
Call sounds on (others joining, leaving), 150 % (values above 100 play at 100).
Call sounds on (others joining, leaving), 150 % (values above 100 play at 100).
### /sounds off | /sounds 40 | /audio
Call sounds off.
Call sounds off.
Call sounds off.
### /sounds bogus
Usage: /sounds [on|off|others on|off|leave on|off|<0-200>]
```
Observation (not a failure): `/sounds <n>` only sets the volume; with the master off it stays off ("Call sounds off.") until `/sounds on`. In a single long session the "others off", "leave off" variants printed "Call sounds on (joining only), ..." as expected.

## 5. App page (Ringtone), headless gallery
Status: PROVEN (visual, by reading the PNGs; not a real window)
`TWICECAT_GALLERY_DIR=<scratch>/final-shots TWICECAT_GALLERY_ONLY=audio-ringtone cargo test -p twicecat-app --test ui_gallery -- --ignored` -> `1 passed`, 32 PNGs; shots live in the session scratchpad, not the repo. Viewed audio-ringtone and audio-ringtone-sounds-off in ember-dark and high-contrast-light (comfortable).
- audio-ringtone (both colour sets): Voice & audio card, "Ringtone" selected. Ringtone box checked; heading "Call sounds"; three checked boxes "Play a sound when you join a call", "Also when someone else joins the call", "Also when you leave the call"; "Sounds volume" row with - button, slider about 30 % filled with thumb (orange in ember-dark, blue in high-contrast-light), + button, "60 %"; note "Short, quiet sounds in the call, heard even when you are muted or deafened. They follow the volume under Levels. This volume has no effect above 100 %, so values above 100 % sound the same."
- audio-ringtone-sounds-off (both): master box unchecked; the two sub-boxes greyed (still ticked, dim); slider drawn dim without a thumb (ember-dark) or as a flat bar (high-contrast-light); +/- and "60 %" kept; the ringtone box above stays live.

## 6. NOT PROVEN
- Hearing: nothing was listened to; loudness, "quiet, not startling", recognisability are on the human checkpoint.
- Echo: with echo cancellation on or off on speakers (cue leaking via the mic), and the not-yet-converged start; the e2e mic is silent so only "never sent by core itself" is shown.
- Reconnect behaviour (no cue on `Reconnected`, `ours.gone` re-send) beyond unit tests; not driven on the stack here.
- Real devices: WASAPI, macOS CoreAudio, Bluetooth (device open/close around the leave cue), PipeWire/system default devices; the CLI ran with `--silent-audio`.
- A real app window and real Ctrl+wheel/keyboard use (gallery is headless).
- Long calls with more than 256 devices (the arrival detector's window), other-leaves (intentionally no sound), 60 s rejoin suppression and 3 s folding in e2e (unit tests only).
- call_ringing: 1 of 6 failed in the single permitted run (known flake).

## Cleanup
Stack stopped (`stack.sh down -v`, no twicecat containers left); `bash scripts/check-no-webrtc-download.sh` run last: exit 0; `git status --short` empty apart from this file.
