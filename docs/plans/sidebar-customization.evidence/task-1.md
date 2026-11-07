# Evidence — Task 1: The sidebar layout type and its local file (core)
Commit: b0ecb33
Environment: `CC=clang-22 CXX=clang++-22`, cargo run inside the repo (its `.cargo/config.toml` applies). Scratch data root in the session scratchpad. A throwaway integration test `crates/core/tests/zz_scratch_sbc1.rs` used only the public API (`twicecat_core::api`, `twicecat_core::paths::DataPaths`); it was deleted afterwards (`git status` clean). Nothing on screen changes in this task.

Note: the evidence hint `cargo test -p twicecat-core --lib paths:: api::sidebar::` is rejected by cargo ("unexpected argument 'api::sidebar::'"); the working form puts the filters after `--`.

## Evidence hint: the lib tests pass
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib -- paths:: api::sidebar::
test result: ok. 69 passed; 0 failed; 0 ignored; 0 measured; 593 filtered out; finished in 0.86s

```
The "version":1 field is asserted by `a_layout_round_trips_and_the_file_is_private_and_versioned` (the scratch head above shows only the first 60 chars). 69 tests ran and passed (they cover: round trip with mode and version, Missing/Corrupt/Newer, FNV vectors, pinned name `sidebar-0862de498e9b8025.json` for `@alice:example.org`, hostile input, FIFO and symlink-to-FIFO, caps, largest-fitting layout under the cap, Debug redaction per type, settings.json 64 KiB cap). Full `cargo test -p twicecat-core --lib`: `660 passed; 0 failed; 2 ignored`, including `the_data_root_holds_only_the_marker_and_the_sessions_dir`, `an_oversized_settings_file_gives_defaults` and the serializer guard `to_bytes_is_the_only_serializer_and_its_errors_carry_no_secret`. Doctests `cargo test -p twicecat-core --doc sidebar`: `2 passed`.

## Per-account file names; another account's file is ignored and never overwritten; each reads back its own layout
Status: PROVEN
```console
$ SCRATCH_ROOT=<scratchpad>/root cargo test -p twicecat-core --test zz_scratch_sbc1 -- --nocapture
file a: sidebar-82c93996fd659248.json
read b before write: Missing
file b: sidebar-79ea1e96f850a74d.json
sha a before/after b write: a2bccf8d9ea5c3c0394fca5a09ff91e3ded4d633b6c68d9813840a6c3c8a79e8 / a2bccf8d9ea5c3c0394fca5a09ff91e3ded4d633b6c68d9813840a6c3c8a79e8
sha b: 997ead2dd620ad01d2f30ffd7febfd1f2ff265e15fc71a95a85970eb8391a2c5
a reads own: true
b reads own: true
a reads b's? false
{"account":"@a:x","rail":[{"space":"!sa:x"}],"spaces":[{"col
total 8
-rw------- 1 galep galep 185 Oct 20 19:32 sidebar-79ea1e96f850a74d.json
-rw------- 1 galep galep 186 Oct 20 19:32 sidebar-82c93996fd659248.json
-rw------- 1 galep galep 186 Oct 20 19:32 sidebar-82c93996fd659248.json
```
Two different file names, mode 0600, no `.tmp` left. Writing @b:x left @a:x's sha256 identical (a2bccf8d...). `read_sidebar(@b)` before its write was `Missing`. Each account reads back exactly its own layout and not the other's. (A first scratch attempt returned `Invalid` because my layout put one room in both Starred and a custom section, which `fits()` correctly refuses; I removed the duplicate.)

## Planted file at @b:x's path with account @a:x gives OtherAccount; write/remove are Protected
Status: PROVEN
```console
planted read b: OtherAccount
write b: Err("Protected")
remove b: Err("Protected")
planted sha before/after: a2bccf8d9ea5c3c0394fca5a09ff91e3ded4d633b6c68d9813840a6c3c8a79e8 / a2bccf8d9ea5c3c0394fca5a09ff91e3ded4d633b6c68d9813840a6c3c8a79e8
planted .bad exists: false
```
OtherAccount, both operations Protected, sha256 unchanged, no `.bad` rename.

## "version":2 gives Newer(2) and is untouched
Status: PROVEN
```console
v2 read: Newer(2)
v2 write: Err("Protected")
v2 remove: Err("Protected")
v2 sha before/after: 0b80639f0da0f7adf33aab582e7a48ea55006e5d986610e79d3f71284179eeb6 / 0b80639f0da0f7adf33aab582e7a48ea55006e5d986610e79d3f71284179eeb6
```

## Debug shows no ID or label
Status: PROVEN
```console
debug: SidebarLayout { rail: 1, spaces: 1 }
contains secretroom/@me/Payroll/room: false false false 
label debug: <redacted, 7 bytes>
```
Layout holding `!secretroom:x`, account `@me:x` and label "Payroll" prints counts only. Per-type Debug tests (`debug_shows_counts_and_no_id_or_name`, `a_label_debug_hides_the_text`) also pass.

## Mutations (re-run by me, each reverted)
Status: PROVEN
Original saved to `<scratchpad>/paths.rs.orig` before mutating.
```console
$ # A: drop the account check (paths.rs:251 replaced by `if false {`)
test paths::tests::a_file_for_another_account_at_this_path_is_protected ... FAILED
assertion `left == right` failed: {"version":1,"account":"@a:x","rail":[]}
test result: FAILED. 68 passed; 1 failed; 0 ignored; 0 measured; 593 filtered out; finished in 0.97s
error: test failed, to rerun pass `-p twicecat-core --lib`
$ # B: sidebar_file returns <root>/sidebar.json for every account
test paths::tests::another_accounts_file_is_ignored_and_never_overwritten ... FAILED
test paths::tests::remove_deletes_only_this_accounts_file ... FAILED
test paths::tests::the_sidebar_file_name_is_stable_and_per_account ... FAILED
assertion `left == right` failed
assertion `left == right` failed
test result: FAILED. 66 passed; 3 failed; 0 ignored; 0 measured; 593 filtered out; finished in 0.81s
error: test failed, to rerun pass `-p twicecat-core --lib`
$ cp paths.rs.orig crates/core/src/paths.rs && diff paths.rs.orig crates/core/src/paths.rs && echo restored-identical
restored-identical
$ git status --short
(empty)
```
A fails `a_file_for_another_account_at_this_path_is_protected`; B fails `another_accounts_file_is_ignored_and_never_overwritten`, `remove_deletes_only_this_accounts_file` and `the_sidebar_file_name_is_stable_and_per_account`. Restore verified by diff. The other four mutations were not re-run by me.

## check-core-api.sh
Status: PROVEN
```console
$ scripts/check-core-api.sh; echo api=$?
api=0
```

## Existing tests unchanged; no sidebar file created by reading
Status: PROVEN (see the 660-test run above and `a_missing_file_reads_as_missing_and_reading_creates_nothing`, `the_data_root_has_no_sidebar_file_until_one_is_written`).

## Hostile input, caps, largest-fitting layout, FNV vectors
Status: PROVEN via unit tests only (all listed in the passing 69; not exercised separately outside tests).

## Gate / commit message
Status: NOT PROVEN (fmt, clippy, deny and commit-message checks were not part of this run).

## Verdict
EVIDENCE: PROVEN
