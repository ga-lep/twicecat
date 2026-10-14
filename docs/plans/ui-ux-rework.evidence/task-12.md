# Evidence — Task 12: Voice & audio card as pages, with push-to-talk and GPU status
Commit: 286ce4b
Environment: CC=clang-22 CXX=clang++-22, repo target dir, headless iced renderer gallery (light build, no `gpu` feature) written to a scratch dir outside the repo (TWICECAT_GALLERY_ONLY=audio, 352 audio PNGs, 27 s). Real audio, GPU hardware, PTT per OS and the Wayland portal stay human.

## Test runs (real output, trimmed to result lines)
```console
$ cargo test -p twicecat-app --test ptt --test menus_ignore_the_wheel --test ui_gallery
ui_gallery: test result: ok. 35 passed; 0 failed; 1 ignored (write_the_gallery)
  incl. audio::the_card_is_760_by_640_with_the_page_list_at_its_left, audio::the_page_never_runs_under_its_scrollbar,
  audio::each_page_sends_only_its_own_messages_and_together_they_send_them_all,
  audio::the_talk_line_in_the_dock_opens_the_talking_page, audio::the_key_row_keeps_its_size_and_its_buttons_apart_from_the_words,
  audio::the_status_lines_stay_when_the_page_changes_and_a_notice_has_its_slot,
  audio::nothing_runs_out_of_the_card_sideways_in_any_page_at_both_widths
menus_ignore_the_wheel: ok. 3 passed; 0 failed (every_slider_is_wrapped_by_the_wheel_guard, every_pick_list_is_wrapped_by_the_wheel_guard ...)
ptt: ok. 2 passed; 0 failed
$ cargo test -p twicecat-app --lib audio_settings
test result: ok. 58 passed; 0 failed   (incl. pages::every_page_builds_in_every_state, the_card_says_why_no_gpu_is_offered, the_meter_reads_the_level, esc_focus_loss_cancel_and_close_stop_waiting)
$ cargo test -p twicecat-app --lib before_the_settings
before_the_settings_arrive_the_call_state_is_still_said: ok (asserts "Loading", and "deafened"/mute words)
```
Status: PROVEN (unit/integration level)

## Card is a 760x640 modal with page_nav at left; nine pages
Status: PROVEN
Gallery images read: audio-devices-frost-dark-comfortable.png. Modal with left list Devices, Talking, Microphone, Levels, Mic test, Ringtone, Other voices, Announce, Diagnostics; selected page has a highlighted row; page title and content on the right behind a divider. Devices page shows Microphone and Speakers pick lists, "In use" lines, "A new device is used from the next call." Test card_is_760_by_640 passes. Opening from the dock/account menu -> Devices and from the talk line -> Talking is covered by tests (the_talk_line_in_the_dock_opens_the_talking_page); not shown as pictures.

## Always-visible send-mode / mic banner, names the release delay
Status: PROVEN
Every page image (devices, talking, microphone, levels, diagnostics) shows above the nav: "Send mode: Push-to-talk (hold F13 to talk). Microphone: closed until you use F13 (keeps sending 200 ms after you let go)." With very long key name (audio-devices-long-names) the banner wraps to 4 lines with ellipsised key, nav shifts down, no overflow.

## Talking page, key capture state
Status: PROVEN
audio-talking-capturing-frost-dark-comfortable.png: Send pick list, key row shows "Press a key..." in a focus-bordered (teal) box with Cancel and Clear buttons, help "Esc cancels.", Gamepad button, Release delay slider with - / + and "200 ms", then the PTT source note ("no global shortcut here ... works while Twicecat's window has focus"). audio-talking-long-key-twicecat-light: long key label wraps inside its cell (8 lines of W, ellipsis) and "Set key..." / "Clear" stay beside it, fully left of the scrollbar (clearance visible). Test the_page_never_runs_under_its_scrollbar passes. Esc-cancels is covered by esc_focus_loss_cancel_and_close_stop_waiting.

## Microphone page with GPU block
Status: PROVEN
- found (frost dark): "Graphics card" card with GPU icon, line "GPU: NVIDIA · TensorRT is used. Also here: NVIDIA · CUDA.", green tag "NVIDIA · TensorRT" and muted tag "NVIDIA · CUDA".
- fallback (twicecat light): same card plus an amber warning banner "Running on the CPU for now: the GPU fell behind (a game may be using it). It is tried again in a while."
- looking (frost dark): card with line "GPU: looking for one..."
- none (frost dark, this gallery is a light build): card shows the existing light-build note "GPU: This is the light build: it cleans on the CPU only. The full build (cargo feature gpu) can use the GPU." So the 'no GPU feature' note IS shown in a gallery image (audio-microphone-gpu-none-*), and also in voice-audio-no-gpu-*. The "none found" empty_state wording of a gpu build cannot be seen in this light build gallery; only unit tests cover it (the_card_says_why_no_gpu_is_offered).
- Scrollbar clearance: all cards end left of the scrollbar.

## Levels and Diagnostics
Status: PROVEN
audio-levels-loud-frost-dark: full-width red (danger) progress bar, text "-2 dB, peak -0 dB, sending" and "Level is too loud: clipping.", then Microphone gain, Volume sliders with +/- and values, Voice quality pick list. audio-diagnostics-dropouts-twicecat-light: "Hide diagnostics" button, red error banner "Audio was dropped: 3 capture overruns, 5 playback underruns...", then a two-column monospace table (Processing / Late wake-ups / Capture overruns with values). Green/amber meter tiers are covered by the_meter_reads_the_level (not looked at as pictures: only the loud and normal images checked).

## Wheel guard on all menus and sliders
Status: PROVEN
Mutation (a): removed `crate::ui::guard::no_wheel(...)` around the slider in a_slider (screens/audio_settings.rs).
```console
test every_slider_is_wrapped_by_the_wheel_guard ... FAILED
slider without no_wheel (Ctrl + wheel would move it): ["/home/.../screens/audio_settings.rs:424 (inside None)"]
test result: FAILED. 2 passed; 1 failed
```
Restored with git checkout; the test fails as expected, then passes (see above). (The reported line number :424 is the pre-mutation numbering of the original file region, i.e. points near a_slider; the mutation shifted lines, so it is approximate.)

## Review fix: changing Send mode stops key capture
Status: PROVEN
Mutation (b): deleted `self.stop_capture();` (and its comment) in the `M::Mode` arm.
```console
test audio_settings::tests::pages::changing_the_send_mode_stops_waiting_for_a_key ... FAILED
assertion failed: !app.audio_ui.capturing   (audio_settings.rs:2834)
test result: FAILED. 0 passed; 1 failed
```
Restored; the test passes in the full run.

## Review fix: banner names the PTT release delay
Status: PROVEN
Mutation (c): replaced the wording with "Microphone: closed until you use {trigger}.{delay}".
```console
test audio_settings::tests::pages::the_closed_words_mention_the_release_delay ... FAILED
panicked at crates/app/src/audio_settings.rs:2820:13
test result: FAILED. 0 passed; 1 failed
```
Restored. Gallery banner text above shows the real wording.

## Review fix: unloaded-settings banner shows mute state
Status: PROVEN (unit only)
before_the_settings_arrive_the_call_state_is_still_said passes (checks "Loading" and "deafened" in unloaded_mic_words). No gallery picture of this state.

## Other criteria
- Call notice at the top of every page: the_status_lines_stay_when_the_page_changes_and_a_notice_has_its_slot passes; no notice image viewed.
- Each page's AudioMessages cover all: each_page_sends_only_its_own_messages_and_together_they_send_them_all passes.
- Commit carries `Step: ui.12` trailer: see git log (checked below).
- Real hardware (devices, GPU, PTT keys per OS, Wayland portal): human checkpoint.

## Defects / observations
1. In the gpu-fallback state the warning text appears twice on the Microphone page: once as a plain line under "Most removed" and once as the banner inside the Graphics card (audio-microphone-gpu-fallback-*). Redundant, cosmetic.
2. The Graphics card icon (Icon::Gpu) renders as a small, ambiguous glyph (looks like a small device) at 16 px; cosmetic.
3. Long-names scenario: the left dock text behind the modal is garbled (outside the card, expected for the stress scenario).
4. Mutation (a) failure message names line 424, not the mutated slider's line; harmless.

## Cleanup
git status clean after each mutation (`git checkout -- <file>`); gallery PNGs in the session scratchpad, outside the repo.
$ scripts/check-no-webrtc-download.sh; echo exit=$?   -> exit=0
