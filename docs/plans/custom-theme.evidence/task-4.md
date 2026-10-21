# Evidence — Task 4: Choosing a theme on the Appearance page, off-thread loading, docs/themes.md
Commit: ca24f09
Environment: CC=clang-22 CXX=clang++-22, Linux, Wayland session (wayland-1). The GUI started but could NOT be driven (no input automation), so no click-through claim is made. Everything below is test output and gallery PNGs (written to the scratchpad, outside the repo, then looked at).

## State tests (pick applies at once and saves once; single-look themes ignore choice and OS mode; pairs follow choice and OS mode; fallback; before first load)
Status: PROVEN (by tests; names match each bullet)
```console
$ cargo test -p twicecat-app --lib   (full lib run)
test result: ok. 1424 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
$ cargo test -p twicecat-app --lib -- theme pick_look load_off_thread themes selected_is_paired reload open_themes
test state::tests::settings_ui::picking_a_theme_applies_it_at_once_and_saves_it_once ... ok
test state::tests::settings_ui::single_look_themes_ignore_the_choice_and_the_system_mode ... ok
test state::tests::settings_ui::a_paired_theme_follows_the_choice_and_the_system_mode ... ok
test state::tests::settings_ui::a_missing_or_refused_theme_shows_twicecat_and_says_why ... ok
test state::tests::settings_ui::before_the_first_load_a_stored_theme_shows_twicecat_without_a_note ... ok
test state::tests::settings_ui::the_theme_choice_is_kept_but_the_single_look_theme_stays_as_it_is ... ok
test state::tests::the_single_look_theme_is_shown_whatever_the_system_mode_is ... ok
test result: ok. 103 passed; 0 failed
```
Named tests exist and pass for each state bullet. I did not read each assertion body.

## pick_look unit tests
Status: PROVEN
```console
(theme.rs tests at :1840-1883, in the 1424-test lib run above, all ok; includes a_single_look_is_returned_whatever_the_arguments_are)
```
Part of the passing lib run; not separately listed by name in my filtered output.

## Off the UI thread (reload in flight, load_off_thread with blocking loader, source scan)
Status: PROVEN (by tests)
```console
test state::tests::settings_ui::reloading_runs_in_a_task_and_only_one_at_a_time ... ok
test themes::tests::a_stalled_loader_does_not_stall_the_caller ... ok
test themes::tests::load_off_thread_reads_the_folder ... ok
test themes::tests::a_loader_that_panics_gives_none ... ok
test state::tests::settings_ui::the_theme_code_loads_only_off_the_ui_thread ... ok
```

## OpenThemesFolder creates the folder and opens it inside Task::perform
Status: NOT PROVEN (no dedicated test found by name in my filter; the real folder opening needs the GUI)
```console
$ grep -n "fn open_themes_folder" crates/app/src/settings.rs
1201:    fn open_themes_folder(&mut self) -> Task<Message>
```
The handler exists; I did not run or find a test that exercises it, and did not click the button. The existing source scans over state.rs are in the passing lib run.

## docs/themes.md and fixtures in sync (4 tests)
Status: PROVEN
```console
test themes::tests::the_doc_shows_daylight_exactly_as_the_fixture_has_it ... ok
test themes::tests::the_doc_lists_the_tokens_in_order ... ok
test themes::tests::the_doc_names_the_three_folders_and_says_nothing_it_should_not ... ok
test themes::tests::the_doc_explains_pairs_and_the_choice ... ok
```

## menus_ignore_the_wheel.rs passes; Ctrl + wheel over the menu changes nothing
Status: PROVEN
```console
$ cargo test -p twicecat-app --test menus_ignore_the_wheel
test the_scan_tells_a_wrapped_widget_from_a_bare_one ... ok
test every_pick_list_is_wrapped_by_the_wheel_guard ... ok
test every_slider_is_wrapped_by_the_wheel_guard ... ok
test result: ok. 3 passed; 0 failed
$ cargo test -p twicecat-app --test ui_gallery   (includes ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing)
test result: ok. 138 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 69.33s
```

## Gallery scenarios, layout/overlap checks, radio test
Status: PROVEN
```console
test settings_pages::the_radios_are_shown_for_the_pair_only ... ok
test settings_pages::the_pair_is_drawn_light_and_dark_by_the_system_mode ... ok
test settings_pages::the_appearance_page_names_the_theme_and_has_no_swatches_or_mode_radios ... ok
$ TWICECAT_GALLERY_DIR=/tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/gallery TWICECAT_GALLERY_ONLY=settings-appearance cargo test -p twicecat-app --test ui_gallery write_the_gallery -- --ignored
test write_the_gallery ... ok
```
PNGs I looked at (in /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/gallery/):
- settings-appearance-themes-daylight-comfortable.png: light window; Theme row shows a closed dropdown "Daylight"; no radios; buttons "Reload themes" and "Open themes folder"; two report lines ("Faint (dark): 1 colour made more readable", "Mixed: backgrounds mix light and dark"). The open menu list is not drawn (closed pick list), so the four-name menu was not seen.
- settings-appearance-pair-daylight-comfortable.png: light look; dropdown "Duo"; three radios, "Follow the system" selected; same buttons and report lines.
- settings-appearance-pair-dark-twicecat-comfortable.png: dark look of the same page; "Duo", three radios, "Follow the system" selected.
- settings-appearance-fallback-twicecat-comfortable.png: dark; dropdown "Twicecat (default)"; no radios; warning banner: Theme "Mixed" could not be used: backgrounds mix light and dark. Showing Twicecat.
- settings-appearance-loading-twicecat-comfortable.png: dark; "Reload themes" greyed out, "Open themes folder" normal, text "Reading the themes folder…".
Report lines fit inside the card in all five. The 720-wide layout was covered by the passing gallery tests, not looked at.

## Running the real app
Status: NOT PROVEN
```console
$ (saved docs/themes.md's json block as <scratch>/data/themes/Daylight.light.json)
$ timeout 25 cargo run -q -p twicecat-app -- --data-dir <scratch>/data
WARN wgpu_hal::vulkan::instance: Unable to find extension: VK_EXT_physical_device_drm
WARN wgpu_hal::gles::egl: Re-initializing Gles context due to Wayland window
exit=124
```
The app started a Wayland window and ran until killed by the timeout (124). I could not click Settings, pick Daylight, check settings.json, press Open themes folder or Reload, or build the Nord pair. Those hint steps are NOT PROVEN and stay on the human checkpoint. settings.json was not created (nothing was picked).

## Mutations (7 listed)
Status: NOT PROVEN (not run by me; recorded in the plan ledger by the implementer only)

## Verdict
All test-observable criteria pass; GUI steps, OpenThemesFolder and the mutations were not exercised.
