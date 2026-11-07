# Evidence — Task 5: The rail's look (RailStates) and its badges
Commit: 3f968ae
Environment: cargo from inside the repo, CC=clang-22 CXX=clang++-22. No server started. Gallery PNGs in /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence/ (outside the repo).

## Pure tests: pill_height, icon_radius, badge_text, folder counts
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- spaces
running 49 tests
test screens::spaces_sidebar::tests::the_pill_is_0_8_20_or_32_and_the_strongest_state_wins ... ok
test screens::spaces_sidebar::tests::the_icon_is_a_circle_idle_and_a_squircle_when_hovered_or_selected ... ok
test screens::spaces_sidebar::tests::the_badge_shows_mentions_first_then_notifications_and_never_unread_alone ... ok
test screens::spaces_sidebar::tests::the_badge_caps_at_99_plus ... ok
test screens::spaces_sidebar::tests::the_pill_follows_any_sign_of_unread ... ok
test spaces::tests::counts_sum_with_saturation_and_a_folder_is_the_sum_of_its_spaces ... ok
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 1438 filtered out
```
(only the relevant lines of the 49 are shown). All pass.

## rail_items: order kept, new space appended, no repeat from a folder, unknown skipped but stored
Status: PROVEN
```console
test spaces::tests::the_layout_order_is_kept ... ok
test spaces::tests::a_new_space_is_appended_in_the_cores_order ... ok
test spaces::tests::a_space_in_a_folder_is_not_repeated_at_top_level ... ok
test spaces::tests::a_stored_space_the_core_does_not_report_is_skipped_but_stays_stored ... ok
test spaces::tests::entries_are_the_top_level_spaces_only ... ok
```
Same run as above.

## State: SpaceUnread updates badges; click selects; folder click toggles and saves once
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- folder click   (relevant lines)
test state::tests::spaces_and_threads::a_click_is_a_press_and_a_release_on_one_item_with_no_leaving ... ok
test state::tests::sidebar_layout::a_click_on_a_folder_toggles_it_and_saves_once ... ok
test state::tests::spaces_and_threads::space_unread_updates_the_badges_and_home_shows_what_is_outside ... ok   (in the spaces run)
test state::tests::spaces_and_threads::selecting_a_space_filters_the_list_and_keeps_tab_and_search ... ok   (in the spaces run)
```

## Gallery: rail-states draws with no overlap at 1280 and 720; existing scenarios pass at the new width; badge colours at least dE 20 in every test theme
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery
test rail_states_draws_every_slot_whole_and_apart_at_every_width ... ok
test rail_marks_and_dots_never_overlap_each_other ... ok
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
test the_rail_dock_fits_72_px_and_keeps_leave_apart_from_deafen ... ok
test result: ok. 139 passed; 0 failed; 1 ignored (the gallery writer)
$ cargo test -p twicecat-app --lib -- spaces   (relevant line)
test screens::spaces_sidebar::tests::the_rail_marks_read_on_the_rail_background_in_every_test_theme ... ok
```

## Gallery images (looked at: rail-states-twicecat-comfortable.png, rail-states-daylight-comfortable-rail.png)
Status: PROVEN
```console
$ TWICECAT_GALLERY_DIR=<scratchpad>/ui-gallery-evidence TWICECAT_GALLERY_ONLY=rail- cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
  -> rail-states-{daylight,twicecat}-{comfortable,compact}[-rail].png (8 PNGs + index.html)
```
What I saw, in the dark "twicecat" theme and the light "daylight" theme (compact/rail variant), 72 px rail on the left:
- Home: grey house icon with a "7" badge (in daylight, a dark teal badge).
- "H": circle, small pill (8) at the left edge, no badge.
- "HH": squircle (hovered), a medium pill (20 px). Pill taller than H's.
- "QL": circle, small pill, no badge. "B": circle, small pill, badge "3".
- "SH": squircle with a ring, the tallest pill (32), selected.
- Collapsed folder: 2x2 preview of small tiles (R, V, G and a fourth) with a "99+" badge. The badge sits over the bottom-right tile and hides most of it (it is drawn, no clash with another slot, but it does cover part of the preview).
- Second collapsed folder: two tiles (G, K) and a badge "1".
- Open folder: a darker rounded band behind its spaces "G" and "K"; K has a small pill and a "1" badge.
- Nothing overlaps another slot in either image. I did not see a tooltip (static render). Pills 0 (default, no pill) / 8 / 20 / 32 all appear.

## Mutations
Status: PROVEN
Hover over selected (swapped the first two branches of `ItemState::of`):
```console
$ cargo test -p twicecat-app --lib -- spaces_sidebar
test screens::spaces_sidebar::tests::the_pill_is_0_8_20_or_32_and_the_strongest_state_wins ... FAILED
panicked at crates/app/src/screens/spaces_sidebar.rs:539:9:  left: 20.0  right: 32.0
test result: FAILED. 6 passed; 1 failed
```
`home_counts` ignoring `outside` (returns `UnreadCountsView::default()`):
```console
$ cargo test -p twicecat-app --lib -- space_unread
test state::tests::spaces_and_threads::space_unread_updates_the_badges_and_home_shows_what_is_outside ... FAILED
panicked at crates/app/src/state.rs:10625:13: Home shows the rooms outside
  left: UnreadCountsView { notifications: 0, highlights: 0, unread: false }
  right: UnreadCountsView { notifications: 7, highlights: 0, unread: true }
```
Both were restored from a saved copy (`diff` reported "same"), and `git status --short` was clean afterwards.

## The gate passes; commit message and trailer
Status: NOT PROVEN (not part of this run: fmt, clippy and the full workspace test were not re-run; commit trailer not checked)

## Live run: a message from the second test account lights its space's pill
Status: NOT PROVEN (a Wayland window cannot be driven or observed here; I did not start the stack or the app, so nothing needed stopping)
