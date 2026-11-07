# Evidence — Task 3: A theme name and a themes folder in the core
Commit: 84d8793
Environment: CC=clang-22 CXX=clang++-22, rustc 1.97.1, Linux. The GUI app was NOT run; the settings round trip was shown with a scratch consumer program (outside the repo) calling the public DataPaths API (read_settings, text size changed, write_settings).

## Round trips; None writes no theme_name key; stored theme kept
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib paths::
test paths::tests::every_theme_and_bool_round_trips ... ok
test paths::tests::every_appearance_value_round_trips ... ok
test paths::tests::a_theme_name_of_none_leaves_no_key_in_the_file ... ok
test paths::tests::settings_round_trip_atomically ... ok
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 576 filtered out
```
All 31 paths:: tests pass (round-trip and no-key tests named above).

## Legacy files (palette ignored, not renamed .bad, dropped on write, theme_name wins)
Status: PROVEN
```console
$ cat settings.json   (scratch data dir, before)
{"palette":"Frost","theme":"Light"}
$ consumer <scratch data dir>    (read, set text_scale_percent=120, write)
read: UiSettings { theme: Light, ..., theme_name: None, ..., text_scale_percent: 100, ... }
$ cat settings.json   (after)
{"advanced_settings":false,"density":"Comfortable","desktop_notifications":true,"room_list":"Expanded","show_message_content":true,"text_scale_percent":120,"theme":"Light","tray_badge":true}
$ ls scratch data dir
-rw------- 1 galep galep 190 Oct 17 09:44 settings.json
```
After the write there is no palette key, theme is still Light, theme_name is absent, and no .bad file exists. The per-value cases (Frost/Ember/HighContrast/Twicecat/Bogus, palette plus valid theme_name) are in the passing test an_older_palette_reads_as_the_default_and_is_dropped_on_the_next_write; I did not read its body to confirm it covers every listed value.

## Bad names read as None, rest kept
Status: PROVEN (by test)
```console
test paths::tests::a_bad_theme_name_reads_as_none_and_keeps_the_rest ... ok
```
I did not separately inspect that the list of names in the test matches the plan's list exactly.

## ThemeName doctests; UiSettings: Copy
Status: PROVEN
```console
$ cargo test -p twicecat-core --doc rooms
test crates/core/src/api/rooms.rs - api::rooms::ThemeName (line 370) ... ok
test crates/core/src/api/rooms.rs - api::rooms::UiSettings (line 456) ... ok
test result: ok. 2 passed
```

## name_allowed and ThemeName::try_from agree
Status: PROVEN
```console
$ cargo test -p twicecat-app name_allowed
test themes::tests::name_allowed_and_the_core_theme_name_agree ... ok
$ cargo test -p twicecat-app names_differing_only
test themes::tests::names_differing_only_in_capitals_are_one_theme ... ok
```

## The themes folder (at(root), from_os, XDG_CONFIG_HOME)
Status: PROVEN (by test; at(root) also by program)
```console
test paths::tests::the_themes_folder_is_under_the_root_for_a_given_root ... ok
test paths::tests::the_os_themes_folder_is_next_to_the_config_dir ... ok
test paths::tests::the_themes_folder_follows_xdg_config_home ... ok
consumer: themes_dir=<scratch data dir>/themes
```

## create_themes_dir 0o700; data root test still passes
Status: PROVEN (by test)
```console
test paths::tests::creating_the_themes_folder_is_private ... ok
test paths::tests::the_data_root_holds_only_the_marker_and_the_sessions_dir ... ok
```

## Settings Debug carries no secret
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib debug
test result: ok. 25 passed; 0 failed
```

## scripts/check-core-api.sh and check-app-deps.sh
Status: PROVEN
```console
$ scripts/check-core-api.sh; echo exit=$?
exit=0
$ scripts/check-app-deps.sh; echo exit=$?
exit=0
```

## The gate (fmt, clippy, full tests) and commit message
Status: NOT PROVEN (not run here; commit trailer not checked)

## Verdict
EVIDENCE: PROVEN
