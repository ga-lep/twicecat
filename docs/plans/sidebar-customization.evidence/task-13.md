# Evidence — Task 13: Reset, documentation and wrap-up
Commit: 038ad6f (ea2ca00 plus two fixups)
Environment: CC=clang-22 CXX=clang++-22, cargo run from inside the repo. No docker stack. Gallery PNGs written outside the repo to /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence13/. The scratch test (in-repo, deleted afterwards) and the mutations were run on temp data roots / with saved copies; `git status --short` was empty afterwards.

## Core and keys untouched
Status: PROVEN
```console
$ git diff 6164222..038ad6f --stat -- crates/app/src/keys.rs crates/core | wc -c
0
```
Empty diff.

## Docs test: keyboard table unchanged, no row added; docs grep tests
Status: PROVEN
```console
$ cargo test -p twicecat-app --test docs_sidebar
running 7 tests
test the_keyboard_map_of_accessibility_gets_no_sidebar_row ... ok
test accessibility_names_the_options_menus_and_says_they_need_a_pointer ... ok
test sidebar_md_names_the_menu_entries_the_app_has ... ok
test the_docs_do_not_promise_what_the_sidebar_does_not_do ... ok
test sidebar_md_states_the_caps_the_core_enforces ... ok
test sidebar_md_says_where_the_layout_is_and_for_whom ... ok
test sidebar_md_claims_no_sync_and_no_removal_at_logout ... ok
test result: ok. 7 passed; 0 failed
```
`git diff 6164222..038ad6f -- docs/accessibility.md` only adds the sentence in the intro and a new section "The sidebar: spaces, folders and sections"; the keyboard table is not in the diff.

## Hinted lib run
```console
$ cargo test -p twicecat-app --lib -- sidebar_reset sidebar sections calls member
test result: ok. 329 passed; 0 failed; 0 ignored; 0 measured; 1380 filtered out
```
Includes `sidebar_reset::a_failed_removal_says_so_and_an_overtaken_reset_does_not`, `sidebar::tests::reset_as_b_removes_only_bs_file_and_leaves_as_byte_for_byte`, `sidebar::tests::reset_leaves_a_protected_file_byte_for_byte_the_same`, `calls::tests::a_sweep_of_generated_calls_never_makes_two_people_alike`, `calls::tests::a_name_of_only_filler_characters_shows_the_localpart`.

## Reset as @b:x removes only @b:x's file; @a:x's file byte for byte the same; b logging back in gets the default
Status: PROVEN (SidebarModel + the real DiskSidebar, scratch test, temp root)
```console
$ cargo test -p twicecat-app --lib zz_scratch -- --nocapture
a file "sidebar-82c93996fd659248.json"
b file "sidebar-79ea1e96f850a74d.json"
BEFORE: a sha=cbcfcb88bd1eec607f5c7d6333100862d69c18052b401a3277edb2f1683a53d0 mtime=Some(SystemTime { tv_sec: 1792573505, tv_nsec: 465678821 })
        b sha=b559a7b1f3c8784e4f4c438e38fe4eb080dcaad6be66923edea95f55eea7a87d mtime=Some(SystemTime { tv_sec: 1792573505, tv_nsec: 475220303 })
  dir: ["sidebar-79ea1e96f850a74d.json", "sidebar-82c93996fd659248.json"]
b model state Loaded, rail len 1
  store.reset called for @b:x
AFTER reset as b: state Loaded, rail len 0
  a sha=cbcfcb88bd1eec607f5c7d6333100862d69c18052b401a3277edb2f1683a53d0 mtime=Some(SystemTime { tv_sec: 1792573505, tv_nsec: 465678821 })
  b exists=false
  dir: ["sidebar-82c93996fd659248.json"]
b logs back in: state Loaded, rail len 0, file exists false
```
a's sha256 and mtime identical, b's file gone, the model is back to an empty (default) layout, and a fresh login of b gets the default with no file re-created. (The scratch store wrapper ran `DiskSidebar::reset_job` synchronously; the model, the file logic and the files are the shipped ones.)

## Newer / OtherAccount: Reset off at the model level and the protected file unchanged
Status: PROVEN (model and core level; the greyed button is not visible, see below)
```console
PROTECTED version 2: state Newer
  reset -> Some(Protected)
  apply -> Some(Protected)
  direct write_sidebar -> Err(Protected)
  direct remove_sidebar -> Err(Protected)
  sha before=3b35bbc5...93fb after=3b35bbc5...93fb
PROTECTED other-account: state OtherAccount
  reset -> Some(Protected)
  apply -> Some(Protected)
  direct write_sidebar -> Err(Protected)
  direct remove_sidebar -> Err(Protected)
  sha before=cbcfcb88...53d0 after=cbcfcb88...53d0
```
(sha trimmed in the middle for width; the full values were identical before and after.) The button wiring in settings.rs is `app.sidebar_reset_allowed().then(|| msg(ResetSidebarAsk))`, i.e. no message when not allowed (read from source, plus unit tests in the lib run).

## Settings gallery images (looked at: base, -ask, -newer, -other, Twicecat theme comfortable)
Status: PROVEN as an aid only (the plan says gallery PNGs are not proof)
```console
$ TWICECAT_GALLERY_DIR=<scratchpad>/ui-gallery-evidence13 TWICECAT_GALLERY_ONLY=settings-appearance-sidebar cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok   (16 PNGs plus index.html written, outside the repo)
```
What I saw:
- base: Settings, Appearance page; below Text size a rule, a line "The sidebar layout (folders, sections, starred rooms) is kept on this device only." and an outlined button "Reset sidebar layout...".
- -ask: the same line, then an amber-outlined box with a warning triangle and "Reset the sidebar layout? The folders, sections and starred rooms you arranged on this device are removed, and the rail and the room lists go back to the default."; below it "Cancel" (outlined, left) and a red/pink filled "Reset sidebar layout" (right, far from Cancel).
- -newer: the button, then the note "The saved sidebar layout was made by a newer Twicecat, so it is left as it is and cannot be reset here."
- -other: the button, then "The saved sidebar layout belongs to another account, so it is left as it is and cannot be reset here."
- Honest finding: in -newer and -other the button looks the same as in base (same outline and text brightness) to my eye; I could not see a visibly "off" styling. The note text makes the state clear, and the source gives it no press message, but a disabled look is not demonstrable from the PNG. A human should look at this in a window.

## Carried fix: reset_job outcomes (Removed / Failed / Overtaken)
Status: PROVEN (unit test passes; Removed shown in the scratch run above)
`state::tests::sidebar_reset::a_failed_removal_says_so_and_an_overtaken_reset_does_not` passes in the 329-test run. Code read: (removed, ok) maps to Removed / Failed / Overtaken / Failed.

## Carried fix: lookalike rows of one call pairwise distinct (3000-call sweep); invisible-only name falls back to the localpart
Status: PROVEN, with a finding on the mutation
`a_sweep_of_generated_calls_never_makes_two_people_alike` (3000 rounds) and `a_name_of_only_filler_characters_shows_the_localpart` pass.

## Mutation 1: Reset also removes every sidebar-*.json (in `run_pending`, Pending::Remove)
Status: PROVEN (killed)
```console
test sidebar::tests::reset_leaves_a_protected_file_byte_for_byte_the_same ... FAILED
test sidebar_store::tests::a_protected_file_loads_as_such_and_a_forced_save_yields_false ... FAILED
test sidebar::tests::reset_as_b_removes_only_bs_file_and_leaves_as_byte_for_byte ... FAILED
```
Three tests fail. Restored from the saved copy; `diff` against the copy was identical.

## Mutation 2: skip `make_distinct` (call commented out)
Status: FINDING, mutation survived
```console
$ cargo test -p twicecat-app --lib -- calls member
test result: ok. 129 passed; 0 failed
```
Neither skipping `make_distinct` alone, nor setting the repeat loop `for _ in 0..4` to `0..0` or `0..1` alone, fails any test (105 passed in the calls run each time). Each is covered by the other: the 3000-call sweep never needs the last step if the other one is present. Disabling both at once is killed:
```console
test calls::tests::a_raw_name_that_looks_like_a_composed_one_does_not_pass_for_it ... FAILED
test calls::tests::a_raw_name_equal_to_a_fallback_row_does_not_pass_for_a_real_user ... FAILED
test calls::tests::a_sweep_of_generated_calls_never_makes_two_people_alike ... FAILED
test result: FAILED. 102 passed; 3 failed
```
So the distinctness guarantee is tested as a whole, but `make_distinct` on its own is a backstop with no test that needs it. Restored; diff against the saved copy identical.

## Docs versus shipped behaviour
Status: PROVEN where checkable
- sidebar.md: "this device only", `sidebar-<digest>.json` (16 lowercase hex, seen: `sidebar-82c93996fd659248.json`), same folder as settings.json: matches paths code. "File is kept at logout": the model's `unload` calls no store (lib tests on logout pass); no removal at logout. "Another account never reads or changes it": matches the scratch run (a untouched after b's reset; protected files refuse writes and removal). Reset removes only the logged-in account's file: shown. Newer / other-account text, Reset off, file left as is: shown at model level. No sync claim: it says the server is not told and other devices do not share it. Caps and menu entries are covered by docs tests (`sidebar_md_states_the_caps_the_core_enforces`, `sidebar_md_names_the_menu_entries_the_app_has`).
- Not verified: the per-OS folder table (macOS, Windows paths) and the `--data-dir` and `--ephemeral` statements were not exercised; the described menu card, drag behaviour and the voice-line speaking ring need a real window.
- accessibility.md: names the right-click and "⋯" options menus, says they need a pointer and that keyboard-only users cannot change the arrangement, no new chord, no auto-scroll: consistent with the plan and the docs tests; the underlying iced 0.14 "buttons take no focus" claim was not tested by me.

## Real confirmation dialog and the live rail in a window
Status: NOT PROVEN (needs a window; on the human checkpoint). The Cancel-changes-nothing path and the rail returning to the default were seen only through the model and unit tests (the ask/confirm UI state in the -ask PNG), not by clicking.

## The gate
Status: NOT PROVEN (not part of this evidence run)

## Verdict
All outside-observable criteria were demonstrated at model, store and file level. Findings: (1) `make_distinct` mutation survives alone (redundant with the earlier pass); (2) the Reset button in the Newer/OtherAccount PNGs does not look visibly disabled.
