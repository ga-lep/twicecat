# Evidence — Task 3: Store the appearance choices in the local settings (core)
Commit: 6a58698
Environment: CC=clang-22 CXX=clang++-22, repo toolchain 1.97.1. Scratch consumer crate (path dependency on twicecat-core) in the session scratchpad, outside the repo, using a temp data dir. No server needed.

## Defaults palette None, Comfortable, Expanded, 100; doc test shows this
Status: PROVEN
```console
$ cargo test -p twicecat-core --doc
test crates/core/src/api/rooms.rs - api::rooms::UiSettings (line 402) ... ok
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
The scratch run below also prints an old file read as palette: None, density: Comfortable, room_list: Expanded, text_scale_percent: 100.

## Old file reads as before; existing tests pass; palette None leaves no key
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib paths::
running 22 tests
... (all 22 ok, including settings_default_when_missing, settings_ignore_unknown_fields_and_fill_missing_ones, an_old_file_reads_with_the_appearance_defaults, a_palette_of_none_leaves_no_key_in_the_file)
test result: ok. 22 passed; 0 failed

(scratch program)
old file: {"theme":"Dark"}
  -> UiSettings { theme: Dark, ..., palette: None, density: Comfortable, room_list: Expanded, text_scale_percent: 100 }
written default: {"density":"Comfortable","desktop_notifications":true,"room_list":"Expanded","show_message_content":true,"text_scale_percent":100,"theme":"System","tray_badge":true}
```
The written default file has no "palette" key. (The diff only adds tests to paths.rs; I did not diff the old tests line by line, though the old ones pass.)

## Unknown value falls back per field; unknown palette is None; file not renamed .bad
Status: PROVEN
```console
(test an_unknown_appearance_value_falls_back_per_field: ok, in the 22 above)
(scratch program)
unknown values: {"palette":"Nope","theme":"Dark","density":"Huge"}
  -> UiSettings { theme: Dark, ..., palette: None, density: Comfortable, ... }
  exists=true bad_exists=false
```
theme stays Dark, palette None, and settings.json.bad does not exist.

## text_scale_percent clamped to 80-200, step 10
Status: PROVEN
```console
test paths::tests::text_scale_is_clamped_and_stepped_when_read ... ok
```
The test covers 0, 79, 85, 200, 250 and u16::MAX (expected 80, 80, 90, 200, 200, 200). The scratch run also read 85 as 90.

## Round trip of every enum value and None; file 0o600
Status: PROVEN
```console
test paths::tests::every_appearance_value_round_trips ... ok
test paths::tests::settings_file_is_private ... ok
$ ls -l <scratch>/data
-rw------- 1 galep galep 165 Oct 14 00:37 settings.json
(scratch) valid: {"palette":"Ember","density":"Compact","room_list":"Rail","text_scale_percent":85}
  -> palette: Some(Ember), density: Compact, room_list: Rail, text_scale_percent: 90
```

## check-core-api.sh passes; UiSettings Copy + Eq; PROGRESS.md records approval
Status: PROVEN
```console
$ scripts/check-core-api.sh; echo check=$?
check=0
$ grep -n "derive" near UiSettings
412:#[derive(Clone, Copy, Debug, PartialEq, Eq)]
$ grep PROGRESS.md
2432:- **Added (core API, additive, approved by the human as Q1, 2026-10-13):** ...
```

## Commit is feat(core) with Step: ui.3
Status: PROVEN
```console
$ git log -1 --format=%B
(subject: feat(core): store the appearance choices in the local settings; trailer: Step: ui.3)
```

Not run: the app crate compile check (`crates/app`) beyond what the task needs; the plan only requires it to compile.
