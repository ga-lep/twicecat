# Evidence — Task 2: The storage boundary and the sidebar model in the app
Commit: 3f2c378
Environment: CC=clang-22 CXX=clang++-22, `cargo test -p twicecat-app --lib` run inside the repo. Scratch test (a temporary module `crates/app/src/scratch_evidence.rs` plus one line in lib.rs) was deleted and lib.rs restored; `git status` is clean apart from this file. The local homeserver (localhost:8008) was not running, so the GUI step was not done.

## With MemorySidebar: load once, Newer/OtherAccount refuse, apply saves once, logout makes no store call, account switch, apply between sessions refused
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- sidebar
running 35 tests
test sidebar::tests::a_successful_change_saves_once_for_the_account_with_the_new_layout ... ok
test sidebar::tests::a_change_that_passes_a_limit_is_refused_as_full_and_saves_nothing ... ok
test sidebar::tests::newer_other_account_and_unavailable_turn_customizing_off ... ok
test sidebar::tests::unload_drops_everything_and_calls_no_store ... ok
test state::tests::sidebar_layout::logging_in_loads_once_for_that_account ... ok
test state::tests::sidebar_layout::newer_and_other_account_refuse_every_change_without_a_save ... ok
test state::tests::sidebar_layout::a_change_saves_once_for_the_logged_in_account ... ok
test state::tests::sidebar_layout::logout_and_session_expiry_call_no_store_and_drop_the_layout ... ok
test state::tests::sidebar_layout::another_account_logging_in_loads_and_saves_for_itself_only ... ok
(... 26 more, all ok)
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 1438 filtered out
```
The App-level tests (state::tests::sidebar_layout) cover each bullet; the scratch run below also showed `apply` between logout and login returning `Some(NotLoaded)`.

## DiskSidebar: coalescing, failed write yields false, kept at logout, protected file
Status: PROVEN
Tests `two_saves_in_quick_succession_leave_the_newer_layout_on_disk`, `a_failed_write_yields_false_without_a_panic`, `the_file_is_kept_at_logout_and_loads_again`, `a_protected_file_loads_as_such_and_a_forced_save_yields_false`, `a_layout_of_another_account_is_never_written` are in the 35 above, all ok.

## Another account's file is never overwritten (scratch run through SidebarModel over DiskSidebar, temp data root)
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib scratch_two -- --nocapture
a saved, sha256 before logout: 7cd4cc630a39322c95c9fbd6b87a7310ae2db7dfd4aebcce676e3000422bcfff
after logout: state=NotLoaded layout_none=true file_exists=true
apply between logout and login: Some(NotLoaded)
b logged in: state=Loaded default=true
b file sidebar-79ea1e96f850a74d.json sha 33918f6b644503c71ef49135cb4f3f083e431c2145d2ac430a93bc4666a666be
a sha256 after b's two changes: 7cd4cc630a39322c95c9fbd6b87a7310ae2db7dfd4aebcce676e3000422bcfff
a back in: state=Loaded same_layout=true
files: ["sidebar-82c93996fd659248.json", "sidebar-79ea1e96f850a74d.json"]
test scratch_evidence::scratch_two_accounts ... ok
```
@a's hash is identical before and after @b's two saves, the file survives logout, and @a's layout loads unchanged. (The scratch store ran each save future synchronously via block_on.)

## screens/ source-scan boundary
Status: PROVEN
`sidebar_store::tests::screens_never_name_the_disk_store_or_the_path_functions` and `only_the_store_and_main_name_the_sidebar_files_or_the_disk_store` pass (in the 35 above). Note: with my scratch file present the second one fails, which shows the scan does catch a stray file naming `DiskSidebar`.

## Debug has no room ID
Status: PROVEN
`sidebar::tests::debug_shows_no_room_id_and_no_account` and `state::tests::sidebar_layout::debug_of_the_app_sidebar_holds_no_room_id` pass.

## Existing tests unchanged
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib
test result: ok. 1473 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
`git diff 17ab228..3f2c378 --stat` shows only PROGRESS.md, lib.rs, main.rs, sidebar.rs, sidebar_store.rs and state.rs changed; no existing test files.

## Mutations (both reverted from saved copies; `git status` clean afterwards)
Status: PROVEN
```console
$ # drop `self.sidebar.unload();` (state.rs line 2615, in back_to_login)
test state::tests::sidebar_layout::logout_and_session_expiry_call_no_store_and_drop_the_layout ... FAILED
test sidebar_store::tests::only_the_store_and_main_name_the_sidebar_files_or_the_disk_store ... FAILED   (caused by my scratch file being present)
test result: FAILED. 1472 passed; 2 failed
$ # drop `self.account.as_ref() != Some(account) ||` in SidebarModel::loaded
test sidebar::tests::a_late_answer_for_another_account_is_ignored ... FAILED
test state::tests::sidebar_layout::another_account_logging_in_loads_and_saves_for_itself_only ... FAILED
test sidebar_store::tests::only_the_store_and_main_name_the_sidebar_files_or_the_disk_store ... FAILED   (scratch file)
test result: FAILED. 1471 passed; 3 failed
```
Each mutation is killed by the intended tests.

## Run the app, log in, no sidebar-*.json appears
Status: NOT PROVEN
The local homeserver was not running (`curl localhost:8008` refused) and I did not start it or drive the GUI. Only the unit-level evidence above covers "nothing is written until a change".

## The gate (fmt, clippy, workspace tests) and commit message
Status: NOT PROVEN (not re-run here; only the app lib tests above were run)

## Verdict
All outside-observable criteria that could be run were demonstrated; the GUI/homeserver check was not run.
