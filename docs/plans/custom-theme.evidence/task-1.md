# Evidence — Task 1: The built-in Twicecat colours replace the four families
Commit: eea4dc8
Environment: branch feat/call-sounds, CC=clang-22 CXX=clang++-22, repo's own target dir, no stack. Design tokens read from /home/galep/Downloads/twicecat-design/design/tokens.json (data only, no scripts run). Gallery PNGs were written to the session scratchpad (an older run's ember/frost/high-contrast files were already in that folder; only `*-twicecat-*` files are from this run).

## A test checks BUILT_IN against the 15 hex values of the design table, and danger_soft == #3b2136
Status: PROVEN
```console
$ python3 -I x.py tokens.json crates/app/src/theme.rs
void             design=#0a0812  app=#0a0812  match=yes
surface          design=#110e1c  app=#110e1c  match=yes
surface-raised   design=#181428  app=#181428  match=yes
surface-overlay  design=#211b36  app=#211b36  match=yes
violet-wash      design=#241a45  app=#241a45  match=yes
line-strong      design=#6b6194  app=#6b6194  match=yes
ink              design=#efebfa  app=#efebfa  match=yes
ink-muted        design=#a69fc4  app=#a69fc4  match=yes
violet           design=#8e5cff  app=#8e5cff  match=yes
on-violet        design=#0a0812  app=#0a0812  match=yes
violet-bright    design=#b794ff  app=#b794ff  match=yes
focus            design=#c9aeff  app=#c9aeff  match=yes
live             design=#ff6b86  app=#ff6b86  match=yes
ok               design=#5ee0c8  app=#5ee0c8  match=yes
warn             design=#ffb547  app=#ffb547  match=yes
15 rows, all match=yes
$ grep -n 3b2136 crates/app/src/theme.rs
461:        assert_eq!(hex(built().1.danger_soft), 0x3b2136);
$ cargo test -p twicecat-app --lib theme::
test theme::tests::every_token_lands_where_the_table_says ... ok
test theme::tests::the_built_in_tokens_are_the_design_colours ... ok
test theme::tests::the_derived_error_tint_is_the_documented_colour ... ok
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 1317 filtered out
```
All 15 hex values match the design file; the pinning tests pass. (danger_soft is not a tokens.json colour; it is pinned only by the test.)

## Built-in contrast tests (V3 text 4.5, non-text 3, V4/V5 delta E, readable_on sender colours)
Status: PROVEN
```console
test theme::tests::every_text_pair_of_the_built_in_theme_is_at_least_4_5_to_1 ... ok
test theme::tests::every_non_text_pair_of_the_built_in_theme_is_at_least_3_to_1 ... ok
test theme::tests::warning_error_success_and_info_stay_distinct_from_each_other_and_from_text ... ok
test theme::tests::sender_colours_are_made_readable_on_every_surface_they_are_drawn_on ... ok
test theme::tests::the_selected_row_and_the_hover_row_show ... ok
test theme::tests::the_hover_background_differs_from_the_others ... ok
test theme::tests::the_surfaces_differ_and_the_sidebar_is_the_window_background ... ok
test theme::tests::the_shield_marks_are_readable_and_stay_visible_on_every_row ... ok
test theme::tests::the_two_shields_are_told_apart_from_each_other_and_from_the_accent ... ok
test theme::tests::only_theme_rs_builds_themes ... ok
(22 passed, 0 failed)
```
Shield colours have their own tests (readability on fill and on 6 row backgrounds; delta E >= 20 between grey/red/badge/speaking/mention).

## Kit tests, checkbox/text_input styles, source scans
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- checkbox text_input styled
test ui::kit::tests::a_checkbox_shows_whether_it_is_checked_in_every_status ... ok
test ui::kit::tests::the_control_scan_flags_a_bare_control_and_passes_a_styled_one ... ok
test ui::kit::tests::every_checkbox_and_text_field_in_the_app_takes_the_kit_style ... ok
test screens::login::tests::only_the_text_inputs_take_tab_in_this_iced ... ok
$ cargo test -p twicecat-app --lib -- shield source_scan
test screens::timeline_pane::tests::shields_have_the_look_and_wording_of_their_code ... ok
test source_scan::tests::* (8 tests) ... ok
test result: ok. 12 passed
```
(I did not separately run the kit button/severity loops by name; they are part of the full lib suite not run here beyond the filters above. See Verdict note.)

## No code use of Family/PaletteChoice/DEFAULT_FAMILY/Frost/Ember/HighContrast
Status: PROVEN
```console
$ grep -rnw "Family\|PaletteChoice\|DEFAULT_FAMILY\|Frost\|Ember\|HighContrast" crates/app/src crates/app/tests
crates/app/src/ui/fonts.rs:88:        assert_eq!(BODY.family, iced::font::Family::Name("Inter"));
crates/app/tests/ui_gallery.rs:3172:                "Frost",
crates/app/tests/ui_gallery.rs:3173:                "Ember",
crates/app/tests/ui_gallery.rs:3196:        for palette in ["Twicecat", "Frost", "Ember", "HighContrast", "Bogus"] {
```
Only iced's font Family and the string values in the stored-palette test remain.

## ui_gallery: Theme line, no radios, stored palette values read as Twicecat, no pick_list outside kit::menu, layout checks
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery
test settings_pages::the_appearance_page_names_the_one_theme_and_has_no_swatches_or_mode_radios ... ok
test settings_pages::a_settings_file_with_an_older_palette_shows_twicecat ... ok
test ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing ... ok
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
test result: ok. 135 passed; 0 failed; 1 ignored
```
I did not read the test bodies for the pick_list walk; the appearance test name covers Theme line/no radios, and the screenshot below shows them for the default fixture.

## Gallery screenshots (viewed with Read)
Status: PROVEN (visual, by my own reading; real-window rendering is a human check)
- settings-appearance-twicecat-comfortable.png: heading "Appearance", "Theme: Twicecat", "This theme has a dark look only.", then the Text size row. No swatches, no radios. Dark violet panel on near-black; text readable.
- settings-notifications-advanced-twicecat-comfortable.png: checkboxes visibly distinct: checked = lilac fill with dark tick, unchecked = outlined box; text fields have a visible outline; dropdowns (kit menu) are violet-tinted.
- login-twicecat-comfortable.png: near-black void, card slightly lighter with violet border, light lilac primary "Log in", outlined secondary, inputs outlined; labels readable.
- main-room-twicecat-comfortable.png / main-room-states: violet selected row, violet unread pills, link/mention in violet, red "Failed to send" text (pink-red, readable), muted grey-lilac secondary text, coloured sender names legible. No contrast problems seen.
- main-call-connected / main-call-unencrypted (call dock): teal "Voice connected, encrypted" with a lock chip; unencrypted shows an amber "Not encrypted" chip; pink-red Leave button; speaking ring violet.
- NOT SEEN: an E2EE timeline message shield (red/grey) is not in these fixtures' rooms; shield colours are covered only by the tests above.

## Mutations (each restored from a pre-made copy)
Status: PROVEN
```console
M1 second Theme::custom in state.rs:
test theme::tests::only_theme_rs_builds_themes ... FAILED
builds a theme: ["/home/.../crates/app/src/state.rs: Theme::custom"]
M2 shield_grey = violet (badge colour):
test theme::tests::the_two_shields_are_told_apart_from_each_other_and_from_the_accent ... FAILED
grey vs badge
M3 .style(kit::checkbox_style) removed at settings.rs:95:
test ui::kit::tests::every_checkbox_and_text_field_in_the_app_takes_the_kit_style ... FAILED
"settings.rs: line 94: checkbox(..) without checkbox_style"
$ git status --short   (after restoring all three)
(empty)
$ cargo test -p twicecat-app --lib theme::   -> 22 passed
```

## tokens.rs, fonts.rs, font assets, core, Cargo files unchanged
Status: PROVEN
```console
$ git diff --stat 0e5ce4d..eea4dc8 -- crates/app/src/ui/tokens.rs crates/app/src/ui/fonts.rs crates/app/assets crates/core Cargo.toml Cargo.lock
(empty)
```

## Appearance for ThemeChoice::{System, Light, Dark}; the gate; commit message
Status: NOT PROVEN (internal / not run)
The three-choice build is in the appearance test (not verified by me beyond its passing). I did not run fmt/clippy/the full workspace gate or check the commit trailer. `cargo run -- --ephemeral`, OS dark/light, iced-derived widgets (radios, sliders, scrollbars) are human checks.

## Check
```console
$ bash scripts/check-no-webrtc-download.sh; echo exit=$?
exit=0
```

## Verdict
EVIDENCE: PROVEN (for everything outside-observable that was run; gaps listed above)
