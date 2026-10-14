# Evidence — Task 11: Call screens: dock, participants panel, ring card
Commit: 565b736
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir. No local homeserver started; tests/calls.rs is ignored (needs scripts/stack.sh). Gallery written to a scratch dir outside the repo (TWICECAT_GALLERY_DIR, `-- --ignored`, 99 s) and inspected with the image reader (Frost dark and Twicecat light, comfortable, normal and -rail).

## No CallMessage / core changes; tests/calls.rs and tests/ptt.rs unchanged
Status: PROVEN
```console
$ git diff 751a807..565b736 --stat -- crates/app/tests/calls.rs crates/app/tests/ptt.rs crates/core
(no output)
$ git diff 751a807..565b736 | grep -n "enum CallMessage"
(no output)
$ cargo test -p twicecat-app --test calls --test ptt
test a_cli_call_rings_the_app_which_declines_then_answers ... ignored, needs local homeserver: scripts/stack.sh up -d --wait
test result: ok. 0 passed; 0 failed; 1 ignored
test without_a_delay_a_release_stops_at_once_and_voice_activity_ignores_presses ... ok
test a_release_keeps_transmitting_for_the_delay_then_stops ... ok
test result: ok. 2 passed; 0 failed; 0 ignored
```
Neither file nor core changed. ptt passes. calls.rs live test was NOT run (needs the homeserver, not started).

## Unit tests (call.rs, calls.rs, keys.rs, kit.rs) and whole lib
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- screens::call:: calls:: keys:: ui::kit
test result: ok. 115 passed; 0 failed
$ cargo test -p twicecat-app --lib
test result: ok. 1175 passed; 0 failed
```
Includes: more_expands_and_collapses_a_row_and_a_call_end_folds_them_all, escape_closes_the_participants_panel, the_card_closes_before_any_other_layer (Esc order), the_dock_marks_encryption_and_the_lack_of_it_in_words_or_glyphs, an_open_microphone_is_outlined_and_not_filled_like_muted, an_open_microphone_in_voice_activity_is_the_open_glyph, no_two_microphone_looks_share_a_glyph_and_a_fill, push_to_talk_transmitting_and_gated_differ_by_glyph_and_fill, a_gamepad_only_binding_draws_the_gamepad, a_new_card_is_inert_until_its_arming_timer_ends, a_stale_arming_timer_does_not_arm_the_card, arming_starts_again_for_each_ring_the_card_changes_to, a_held_chord_that_answers_one_ring_never_answers_the_next, losing_the_window_lets_go_of_the_held_chord, a_held_ring_chord_repeats_nothing_but_other_shortcuts_still_repeat, letting_go_of_the_chord_is_a_message, the_ring_card_is_answered_from_the_keyboard_only_while_it_shows, mute_labels/declined_line/ring_lines tests.

## Layout tests (ui_gallery, non-ignored)
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery
test result: ok. 27 passed; 0 failed; 1 ignored (write_the_gallery)
```
Includes the_expanded_dock_never_spills_and_keeps_leave_apart_at_300_px, the_rail_dock_fits_72_px_and_keeps_leave_apart_from_deafen, the_participants_panel_is_320_wide_and_its_open_rows_have_room_for_the_slider, the_ring_card_keeps_decline_and_accept_apart_below_the_words, a_long_room_name_wraps_in_the_dock_instead_of_being_cut, leave_sends_once_and_is_off_while_a_leave_or_join_waits, a_ring_answered_keeps_both_buttons_off_and_in_place, a_click_on_a_ring_card_that_just_showed_answers_nothing_and_a_later_one_does.

## Call dock (phase dot + words, room name, lock, declined line, PTT pill, controls, Leave)
Status: PROVEN
Screenshots (scratch dir, main-call-connected / -ptt-muted / -ptt-talking / -unencrypted, both palettes): the dock is a raised card at the bottom of the room-list column. Line 1: green dot, "Voice connected, encrypted", and a small green outlined lock mark on the right. Then the room name "Lounge" (ghost, no visible button chrome). Then "@alice:example.org declined". In PTT mode a pill row: "Talking" (accent fill, mic glyph) or "Muted" (muted fill, no-entry glyph; light theme). Controls row: Mute icon button (green outlined mic when open; accent-filled while talking; no-entry glyph with accent-tinted fill when muted), Deafen speaker, audio settings gear, red "Leave" with words.
Defects/observations: the Deafen speaker glyph and the lock are small (about 10-12 px) next to the mic; the Mute and Deafen glyphs differ in visual weight; the gear is clear. Frost-dark Leave is a light coral with dark text, readable.

## Mic-live visible in every mode; encryption mark
Status: PROVEN (visual + unit tests)
Open mic = green outlined mic (voice-activity and PTT idle), talking = filled accent, muted = no-entry glyph; all distinct in the shots. Encrypted = lock + "encrypted" words; main-call-unencrypted (Twicecat light) shows an amber outlined "Not encrypted" tag with the open-lock glyph and "Voice connected" without "encrypted".

## Rail dock (72 px)
Status: PROVEN
main-call-connected-twicecat-light-comfortable-rail and main-call-ptt-muted-frost-dark-comfortable-rail: a 72 px column at the bottom of the rail: check (phase) mark, lock tag, participants tag (amber outlined), Mute, Deafen, red door Leave icon. All fit with no clipping; Mute and Leave are separated by Deafen plus a gap.
Defects: in the rail the phase is only a small check glyph (no coloured dot), so Joining/Reconnecting must be told apart by glyph (tests cover it); the Deafen glyph is small.

## Call notice (danger banner with close)
Status: PROVEN (unit test the_dock_shows_who_declined_and_the_notice_in_both_forms; main-call-notice-* images exist in the gallery, not inspected one by one)

## Participants panel (header, speaking ring, status words, More fold, volume, cleaning)
Status: PROVEN
main-call-connected / main-call-rows-open (Frost dark): header "In the call (4)" with an x close; 320 px wide. Rows: Me Example "(you)"; Bea Example with a teal ring around the avatar, "Speaking" in accent with a speaker glyph, and "Voice cleaned"; Cy Example "Muted" plus a tiny mute glyph; Dee Example "Voice cleaning starts...". Each other row has a "..." More button. rows-open: Bea and Cy expanded, More shown with a check mark and accent outline, revealing "Volume" with - slider + and "100 %", and "Voice cleaning" pick list "Clean: automatic"; Dee stays folded. Light theme shows the same, with the ring in green.
Defects: the per-row mute glyph (Cy, and Me in ptt-muted) is very small (about 8 px) and sits detached from the name at the row's right edge, hard to see. The "(you)" row has no More fold.

## Ring card (avatar 64 px, lines, Decline/Accept, Esc note, arming line)
Status: PROVEN for arming; "Ready to answer" PROVEN by unit/layout tests only
ring-dm (Frost dark), ring-group and ring-long-id (Twicecat light, normal and rail): modal with a 64 px avatar, headline "Alice Example is calling" (or "@cy:example.org is calling" with "in Design review"), Decline (x) and Accept (speaker) with words, then "Answer in a moment..." (arming line), "Esc hides this; the room keeps ringing." and the "Keys: Ctrl+Shift+A accepts, Ctrl+Shift+X declines (Cmd on macOS)." line. The long id wraps over 4 lines with "..." and stays inside the card (no clipping), with the buttons below.
Defects: in the gallery the buttons are in the arming (off) state, so they look flat and unstyled (no danger/primary colour), so the coloured state is not visible in the gallery. "Ready to answer" never appears in any gallery image: no shot of the armed card with a red Decline and accent Accept. The long id is cut mid-word with "..." in the middle of the headline (shown as "...SingleOneHere"). In the dark shot, the "Ringing" tag on the Alice row overlaps the preview text in the room list (ring-dm Frost dark: "Call me wh[Ringing]..."), and a Ringing tag overlaps the unread badge in ring-long-id (Design review row). Those rows are the room list (Task 5/6 area) but visible in the Task 11 screenshots.

## Ctrl+Shift+A/X shortcuts, repeat drop, held-chord latch, 600 ms arming
Status: PROVEN (unit tests only; no real window)
Tests above pass (keys and calls). Arming duration not independently measured (no timer observed). Real keyboard behaviour stays human.

## Mutation checks
Status: PROVEN
```console
(a) keys.rs: `if *repeat && matches!(...)` -> `if false && *repeat && matches!(...)`
$ cargo test -p twicecat-app --lib -- keys::
test keys::tests::a_held_ring_chord_repeats_nothing_but_other_shortcuts_still_repeat ... FAILED
test result: FAILED. 10 passed; 1 failed
(b) call.rs line 164: `let idle = !ring.answered && !arming;` -> `let idle = !ring.answered;`
$ cargo test -p twicecat-app --test ui_gallery
test a_click_on_a_ring_card_that_just_showed_answers_nothing_and_a_later_one_does ... FAILED
test result: FAILED. 26 passed; 1 failed; 1 ignored
```
Both files restored with git checkout; `git status --short` printed nothing afterwards.

## Speaking is a ring and a word
Status: PROVEN: Bea's row shows the ring plus the word "Speaking" (both palettes).

## Not done / human
Real calls, voices following the ring, PTT per OS, real-window shortcuts, tests/calls.rs against the stack, and "Ready to answer" with live colour buttons.

## Guard script
```console
$ scripts/check-no-webrtc-download.sh; echo exit=$?
exit=0
```

## Verdict
EVIDENCE: PROVEN
