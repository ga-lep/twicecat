# Evidence — Task 2: Theme files are parsed and validated (no UI yet)
Commit: f0890f9
Environment: local, `CC=clang-22 CXX=clang++-22`, repo's own `target/`. No server; the surface is the library API (`themes::parse`, `load_dir`, `theme::resolve`) exercised by the crate's own tests. Nothing on screen changes.

## parse of twicecat.tokens.json gives BUILT_IN, no adjustment; sha256 = 0388d3cf…cf2b
Status: PROVEN
```console
$ sha256sum crates/app/assets/themes/twicecat.tokens.json
0388d3cff0874d672be530e62d153b8085558331e31d5bad6e67c1dfda8bcf2b  crates/app/assets/themes/twicecat.tokens.json
$ cargo test -p twicecat-app --lib themes::
test themes::tests::the_design_tokens_file_parses_to_the_built_in_colours_unchanged ... ok
test themes::tests::the_design_tokens_file_is_the_copied_one ... ok
...
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 1354 filtered out; finished in 0.03s
```
Hash matches; the parse test passes.

## Fixtures (Paper, Duo, Faint; kit readability over test_themes())
Status: PROVEN
```console
test themes::tests::paper_and_duo_resolve_with_no_adjustment_and_faint_with_one ... ok
$ cargo test -p twicecat-app --lib theme::    (36 passed; 0 failed)
```
Kit/rich_text/call tests ran in the same pass of the lib suite (no failures). I did not separately run the full `cargo test --workspace` gate or inspect the "1 colour made more readable" wording beyond the passing test.

## Hostile corpus (results, no panic, < 1 s, report lines hold no file values)
Status: PROVEN (via the tests below; I did not read each assertion's wording)
```console
test themes::tests::hostile_all_tokens_the_same_grey_are_refused ... ok
test themes::tests::hostile_every_token_with_an_alpha_falls_back ... ok
test themes::tests::hostile_values_that_are_not_colours_fall_back_each ... ok
test themes::tests::hostile_a_colour_space_other_than_srgb_falls_back ... ok
test themes::tests::hostile_status_colours_that_collide_are_refused ... ok
test themes::tests::hostile_a_selected_row_that_would_not_show_is_refused ... ok
test themes::tests::hostile_light_void_over_dark_surfaces_is_refused ... ok
test themes::tests::hostile_a_name_that_disagrees_with_the_background_is_refused ... ok
test themes::tests::hostile_wrong_types_give_no_colours_or_per_token_fallbacks ... ok
test themes::tests::hostile_a_file_over_64_kib_is_refused ... ok
test themes::tests::hostile_deep_nesting_is_not_json_and_does_not_overflow_the_stack ... ok
test themes::tests::hostile_forty_valid_files_load_thirty_two ... ok
test themes::tests::hostile_names_that_are_not_allowed_are_counted_not_shown ... ok
test themes::tests::hostile_the_same_name_and_mode_twice_keeps_the_first_by_file_name ... ok
test themes::tests::a_utf8_byte_order_mark_is_skipped ... ok
test themes::tests::hostile_a_symlink_to_a_valid_theme_is_refused ... ok
test themes::tests::hostile_a_fifo_is_refused_without_being_opened ... ok
```

## Property: 2,000 seeded random sets keep V3, V4, V5
Status: PROVEN
```console
test theme::tests::every_accepted_random_theme_keeps_every_promise ... ok
```

## Mutations (each reverted with `git checkout -- .`; `git status --porcelain` printed nothing after each)
Status: PROVEN
```console
=== M1 remove V4 (status_alike returns false early)
test theme::tests::every_accepted_random_theme_keeps_every_promise ... FAILED
test theme::tests::distinctness_covers_the_status_colours_the_text_and_the_focus_ring ... FAILED
test themes::tests::hostile_status_colours_that_collide_are_refused ... FAILED
test themes::tests::hostile_all_tokens_the_same_grey_are_refused ... FAILED
test result: FAILED. 66 passed; 4 failed
=== M2 remove symlink_metadata is_file check (`if false {`)
test themes::tests::hostile_a_fifo_is_refused_without_being_opened ... FAILED
thread '...hostile_a_fifo_is_refused_without_being_opened' panicked at crates/app/src/themes.rs:1147:17
test result: FAILED. 30 passed; 1 failed
=== M3 V3_TEXT lowered to 3.0
test theme::tests::a_faint_text_colour_is_lifted_and_named ... FAILED
test theme::tests::every_accepted_random_theme_keeps_every_promise ... FAILED
test themes::tests::hostile_all_tokens_the_same_grey_are_refused ... FAILED
test result: FAILED. 67 passed; 3 failed
```
M1: the all-same case and the `live == ink` / `live == ok` case (one test, `hostile_status_colours_that_collide`) fail as named. M2: FIFO test fails. M3: property test fails. Note M3 lowered the text constant `V3_TEXT` (non-text was already 3.0).

## only_theme_rs_builds_themes still passes
Status: PROVEN
```console
test theme::tests::only_theme_rs_builds_themes ... ok
```

## check-app-deps passes; cargo check --locked passes; Cargo.lock change is one serde_json line
Status: PROVEN
```console
$ bash scripts/check-app-deps.sh; echo exit=$?
exit=0
$ cargo check --workspace --lib --bins --locked
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.90s
$ git diff f563813 f0890f9 -- Cargo.lock
@@ -8435,6 +8435,7 @@ dependencies = [
  "open",
  "rfd",
+ "serde_json",
  "tokio",
$ git diff --exit-code Cargo.lock; echo lock=$?
lock=0
```
Exactly one added line between the commits; the lock is unchanged in the working tree and `--locked` passes.

## The gate passes; commit message and Step trailer
Status: NOT PROVEN (not run here: fmt/clippy/full workspace test; commit message not inspected)

## Verdict
EVIDENCE: PROVEN
