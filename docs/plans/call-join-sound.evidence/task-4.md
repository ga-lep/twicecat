# Evidence — Task 4: The "Call sounds" section on the Ringtone page
Commit: 2ad6774
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir, headless iced renderer (ui_gallery). Shots written to the session scratchpad (`.../scratchpad/shots`), not the repo. Shots are 1280x800 (the narrow variants of this suite are 720 wide; the gallery has no 720 variant of this page, only comfortable/compact densities).

## Each checkbox sends one Command::UpdateAudioSettings with only its own field flipped
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib audio_settings
test audio_settings::tests::each_control_sends_the_whole_settings_with_one_change ... ok
test audio_settings::tests::the_sub_options_of_the_call_sounds_need_the_master_switch ... ok
test audio_settings::tests::pages::every_message_belongs_to_a_page_and_the_pages_cover_them_all ... ok
```
The table test (extended with `AudioMessage::CallSounds(false)` etc.) passes; the gallery `each_page_sends_only_its_own_messages_and_together_they_send_them_all` also passed.

## Slider: Slide then SlideReleased sends one update; Step moves by 5, clamped 0..200
Status: PROVEN
```console
test audio_settings::tests::the_call_sounds_volume_follows_its_slider_and_steps_by_five ... ok
test audio_settings::tests::sliders_send_once_on_release_or_after_the_last_pause ... ok
test audio_settings::tests::the_steps_beside_a_slider_send_at_once_and_clamp ... ok
```
The new test asserts page Ringtone, range (0,200), step 5, then Slide/Step behaviour.

## Disabled with the master off (no on_press/on_toggle in the control walk)
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery   (full run)
test result: ok. 136 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 63.53s
```
Includes `the_call_sounds_controls_are_live_only_while_the_master_switch_is_on` (walks audio-ringtone and audio-ringtone-sounds-off). Shots below show the disabled look.

## Ctrl+wheel guard
Status: PROVEN
Baseline: `ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing ... ok` (full run above and re-run after restore, 3 passed).
Mutation: copy of crates/app/src/screens/audio_settings.rs saved, `no_wheel` skipped only when `which == Slider::CallSoundsVolume`:
```console
$ cargo test -p twicecat-app --test ui_gallery ctrl_wheel
test ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing ... FAILED
thread '...' panicked at crates/app/tests/ui_gallery.rs:1505:9:
Ctrl + wheel changed a setting on audio-ringtone: ["Audio(Slide(CallSoundsVolume, 65.0))", "Audio(Slide(CallSoundsVolume, 60.0))", "Audio(Slide(CallSoundsVolume, 65.0))", ... (repeats; trimmed)]
test result: FAILED. 2 passed; 1 failed
```
File restored from the copy; `git status --short` empty; test green again.

## No pick_list outside kit::menu; sends() consistency; layout checks (1280x800, 720 narrow, both densities)
Status: PROVEN
Full ui_gallery run all green, including `settings_pages::nothing_runs_out_of_the_card_sideways_in_any_page_at_both_widths`, `the_page_never_runs_under_its_scrollbar`, `the_card_is_760_by_620_...`, `every_screen_lays_out_in_every_variant_at_both_sizes`; lib `every_page_builds_in_every_state`, `every_message_belongs_to_a_page_and_the_pages_cover_them_all`.

## scripts/check-app-deps.sh
Status: PROVEN
```console
$ bash scripts/check-app-deps.sh; echo app-deps=$?
app-deps=0
```

## Evidence shots (TWICECAT_GALLERY_ONLY=audio-ringtone ... --ignored: `1 passed`; 32 PNGs, 4 families x light/dark x comfortable/compact x 2)
Status: PROVEN (visual, by reading the images)
- audio-ringtone-frost-dark-comfortable.png: Voice & audio card, Ringtone page selected. "Ringtone" heading, one checked box "Play a ringtone when someone calls you directly" and its note; then heading "Call sounds" with three checked (teal) boxes "Play a sound when you join a call", "Also when someone else joins the call", "Also when you leave the call". Row "Sounds volume" with − button, teal slider about 30% filled, + button, "60 %". Note beneath: "Short, quiet sounds in the call, heard even when you are muted or deafened. They follow the volume under Levels. This volume has no effect above 100 %, so values above 100 % sound the same." (corrected wording present).
- audio-ringtone-sounds-off-frost-dark-comfortable.png: master box unchecked; the two sub-boxes drawn dim (still ticked, greyed); slider replaced by a dark grey bar with a short dim fill, no thumb; − and + buttons present but visibly muted; "60 %" kept. Ringtone box still checked/live.
- audio-ringtone-twicecat-light-comfortable.png: same layout, blue accent boxes and slider with thumb, light card.
- audio-ringtone-sounds-off-twicecat-light-comfortable.png: master unchecked, sub-boxes grey ticks, pale bar without thumb, text same note.
- audio-ringtone-frost-dark-compact.png: same section at compact density; rows tighter, nothing clipped or overlapping.
Frost is the default family at this commit. Real-window look, Compact density in a real window and Ctrl+wheel in a real window are human checks.

## Final script
```console
$ bash scripts/check-no-webrtc-download.sh; echo webrtc=$?
webrtc=0
```
Repo `git status --short` empty after all runs (evidence file aside).

## Verdict
EVIDENCE: PROVEN
