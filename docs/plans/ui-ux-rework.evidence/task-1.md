# Evidence — Task 1: Design tokens, icon module and component kit
Commit: 6695175
Environment: `CC=clang-22 CXX=clang++-22 cargo test -p twicecat-app ui::` (no outside surface; internal kit, visible from Task 4/5)

## Tokens, icons, kit, contrast, moved items (all criteria)
Status: PROVEN (by targeted unit tests only; no user-facing surface)

```console
$ CC=clang-22 CXX=clang++-22 cargo test -p twicecat-app ui::
test ui::icons::tests::every_icon_has_a_distinct_glyph_and_a_label ... ok
test ui::icons::tests::the_old_marks_keep_their_glyphs ... ok
test ui::icons::tests::the_scanner_skips_comments_and_test_code_and_finds_symbols ... ok
test ui::icons::tests::this_module_is_the_only_place_that_writes_a_symbol ... ok
test ui::kit::tests::badges_read_as_numbers ... ok
test ui::kit::tests::every_button_style_is_readable_in_both_palettes_and_every_status ... ok
test ui::kit::tests::severities_have_distinct_marks_and_the_banner_text_is_readable ... ok
test ui::kit::tests::the_person_slot_is_the_old_one ... ok
test ui::kit::tests::the_modal_card_is_raised_and_round ... ok
test ui::kit::tests::the_widgets_build ... ok
test ui::tokens::tests::density_sets_the_row_heights_and_trims_only_the_vertical_padding ... ok
test ui::tokens::tests::the_scales_are_the_documented_ones ... ok
test result: ok. 81 passed; 0 failed; 0 ignored; 0 measured; 983 filtered out
```
(Output trimmed to the ui:: tests; the filter also matched 81 tests in total, including the existing settings_ui tests that pass unchanged.) The theme contrast-table test for the new Semantic fields was not separately listed in the output tail; not individually shown.

## Commit trailer
Not checked (not observable via a running feature).
