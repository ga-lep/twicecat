# Evidence — Task 2b: Neutral token names, no dark wording, no note for single-look themes, Daylight
Commit: f749142
Environment: CC=clang-22 CXX=clang++-22, repo target dir, no servers. Gallery PNGs in /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/0644a62e-7bf6-4bc0-a666-3e983e2d5af7/scratchpad/gal (the dir held older PNGs from earlier runs; the settings-appearance-* files are the fresh ones).

## Unit tests: `cargo test -p twicecat-app --lib -- themes:: theme::` (alias, pair/duplicate, Daylight, hostile)
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- themes:: theme::
test result: ok. 82 passed; 0 failed; 0 ignored; 0 measured; 1319 filtered out; finished in 0.27s
(selected lines)
test themes::tests::the_token_names_are_the_neutral_ones_in_order ... ok
test themes::tests::the_starting_file_is_the_built_in_colours_under_the_neutral_names ... ok
test themes::tests::the_neutral_name_wins_over_the_older_one_and_the_older_one_is_counted ... ok
test themes::tests::presence_decides_so_a_bad_neutral_value_does_not_fall_back_to_the_older_name ... ok
test themes::tests::hostile_neutral_and_older_names_together_use_the_neutral_ones ... ok
test themes::tests::a_dark_and_a_light_file_of_one_name_are_one_pair ... ok
test themes::tests::a_pair_with_one_refused_side_loads_the_other_alone ... ok
test themes::tests::the_same_name_and_mode_twice_is_a_duplicate_even_when_a_pair_exists ... ok
test themes::tests::hostile_the_same_name_and_mode_twice_keeps_the_first_by_file_name ... ok
test themes::tests::daylight_loads_as_one_light_theme_with_nothing_adjusted ... ok
test themes::tests::daylights_fifteen_values_are_pinned ... ok
test themes::tests::daylight_paper_and_duo_resolve_with_no_adjustment_and_faint_with_one ... ok
test themes::tests::the_drafts_surface_overlay_would_hide_the_hover_row_and_is_refused ... ok
test theme::tests::the_test_themes_are_twicecat_daylight_the_duo_pair_faint_and_paper ... ok
test theme::tests::every_accepted_random_theme_keeps_every_promise ... ok
test theme::tests::only_theme_rs_builds_themes ... ok
(17 hostile_* tests, all ok)
```
All pass. Daylight: one theme "Daylight", dark false, both_modes false, empty report (asserted in daylight_loads_as_one_light_theme_with_nothing_adjusted via load_dir on the fixture crates/app/tests/fixtures/themes/Daylight.light.json).

## Built-in design file unchanged
Status: PROVEN
```console
$ sha256sum crates/app/assets/themes/twicecat.tokens.json
0388d3cff0874d672be530e62d153b8085558331e31d5bad6e67c1dfda8bcf2b  crates/app/assets/themes/twicecat.tokens.json
```
Matches 0388d3cf…cf2b.

## Appearance page: "Theme" / "Twicecat", no radios, no note, no dark wording
Status: PROVEN
```console
$ TWICECAT_GALLERY_DIR=/tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/0644a62e-7bf6-4bc0-a666-3e983e2d5af7/scratchpad/gal TWICECAT_GALLERY_ONLY=settings-appearance cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out ...
$ cargo test -p twicecat-app --test ui_gallery
test settings_pages::the_appearance_page_names_the_theme_and_has_no_swatches_or_mode_radios ... ok
test result: ok. 135 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 63.28s
```
I viewed /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/0644a62e-7bf6-4bc0-a666-3e983e2d5af7/scratchpad/gal/settings-appearance-twicecat-comfortable.png: Appearance page shows the row "Theme" with value "Twicecat" at right, then Text size; no radios, no swatches, no note. Also written: settings-appearance-twicecat-compact.png, settings-appearance-advanced-twicecat-{comfortable,compact}.png (same dir; advanced ones not viewed). No paired-theme screenshot exists (gallery covers Twicecat only); paired-radio display is not shown on screen: NOT PROVEN visually (radio code is under `if resolved.both_modes` at crates/app/src/screens/settings.rs:333).

## History: one thm.1, one thm.2, no fixup!
Status: PROVEN
```console
$ git log --format='%h %s' main..HEAD | grep -E 'one built-in Twicecat|read custom theme'
a74eb86 feat(app): read custom theme files safely and keep them readable
8820345 feat(app): one built-in Twicecat theme from the design colours
$ git log --format=%B main..HEAD | grep -E 'Step: thm|fixup'
Step: thm.2
Step: thm.1
$ git log --oneline main..HEAD | grep -c fixup
0
```
Note: `origin/main..HEAD` lists the same two feature commits (plus more call-sound/UI commits not on origin/main). Observation: the thm.1 commit body still says "dark-only theme" (commit message, outside the plan's grep scope; the plan's No-dark-wording grep covers files only).

## grep checks
Status: PROVEN
```console
$ git grep -n -i violet -- crates/app/src crates/app/tests docs/accessibility.md ':!crates/app/tests/fixtures/themes/hostile/alias_and_neutral.json'
crates/app/src/themes.rs:205 (KNOWN_UNUSED incl. "violet-deep"), :210-213 (LEGACY_ALIASES), :1081-1084, :1096, :1103, :1111, :1120, :1173 (alias tests), :1371 (test doc with violet-deep)
$ git grep -n -i -E 'dark look only|dark-only|dark only|the dark theme|dark violet' -- crates/app docs/accessibility.md   -> no output, rc=1
$ grep -n -i -E (same) PROGRESS.md -> no output, rc=1
$ grep -nE '^.{101,}' crates/app/src/themes.rs crates/app/src/theme.rs -> no output, rc=1
```
Violet only in the alias block, the known-unused entry and alias tests; no dark-only wording; no line over 100 columns.

## Mutations (each run, seen failing, reverted with git checkout -- .; status empty after each)
Status: PROVEN
```console
=== M1 V4 removed (status_alike returns false first)
FAILED: distinctness_covers_the_status_colours_the_text_and_the_focus_ring, every_accepted_random_theme_keeps_every_promise,
 a_pair_with_one_refused_side_loads_the_other_alone, hostile_all_tokens_the_same_grey_are_refused, hostile_status_colours_that_collide_are_refused
test result: FAILED. 77 passed; 5 failed
status after revert: []
=== M2 file-type check removed (if false && !looked.file_type().is_file())
FAILED: hostile_a_fifo_is_refused_without_being_opened (themes.rs:1488)
test result: FAILED. 81 passed; 1 failed
status after revert: []
=== M3 V3_TEXT = 3.0
FAILED: every_accepted_random_theme_keeps_every_promise, a_faint_text_colour_is_lifted_and_named, hostile_all_tokens_the_same_grey_are_refused
test result: FAILED. 79 passed; 3 failed
status after revert: []
```
Caveat: M2 as in Task 2 removed the `is_file` check on the symlink_metadata result; the symlink test still passed (the later opened-handle checks also refuse a symlink), only the FIFO test caught it. The plan's 2b mutations (alias wins, name-only duplicate, both_modes forced false, Daylight overlay #dfe3ea, note back) were not requested here and not run.

## Final state
Status: PROVEN
```console
$ git status --porcelain
(only this evidence file)
```

## Verdict
EVIDENCE: PROVEN
