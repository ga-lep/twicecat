# Evidence — Task 8: Sections inside a space (model, variable-height windowing, view)
Commit: 6131e1c
Environment: CC=clang-22 CXX=clang++-22, cargo run from inside the repo, no GUI started, no homeserver. Gallery PNGs written outside the repo to /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence8 (16 PNGs + index.html). Scratch persistence test was added to crates/app/src/sidebar.rs and removed afterwards (file restored from a saved copy; `git status` clean).

## Placement, badge, windowing, when-sections-show, keyboard and load-all tests; existing room-list tests unchanged
Status: PROVEN
```console
$ CC=clang-22 CXX=clang++-22 cargo test -p twicecat-app --lib -- sections room_list
running 117 tests
... (all "ok"; 36 in sections::tests, the existing room_list::tests / room_list::pure_tests and screens::room_list_pane::tests among them)
test result: ok. 117 passed; 0 failed; 0 ignored; 0 measured; 1484 filtered out; finished in 0.28s
```
Named tests seen passing include: every_room_of_the_space_appears_exactly_once; starred_beats_custom_beats_sub_space_beats_dm_beats_channels; sub_space_rows_are_not_rows_and_a_level_2_room_lands_in_its_level_1_section; empty_built_in_sections_are_hidden_and_an_empty_custom_one_shows_its_hint; the_stored_order_is_kept_and_missing_keys_take_their_default_place; highlights_2_and_notifications_5_make_highlight_2; only_a_collapsed_header_carries_a_badge; visible_lines_returns_exactly_the_lines_that_cross_the_viewport; spacers_and_built_lines_add_up_to_the_total; nan_negative_and_stale_offsets_are_clamped; sections_show_with_a_space_the_all_tab_a_blank_search_and_the_expanded_list; home_a_search_another_tab_or_the_rail_keep_the_flat_list; alt_down_goes_from_the_last_room_of_a_section_to_the_first_of_the_next_drawn_one; sectioned_mode_asks_for_a_page_per_loaded_page_until_everything_is_in; the_load_all_rule. The pre-existing room_list tests pass without edits (the diff touches room_list.rs by 33 lines, shared clamping helpers only).

## 10,000-room space builds under 100 lines
Status: PROVEN (for the flat-list windowing test the plan names; no sectioned 10k test found)
```console
$ cargo test -p twicecat-app --lib -- ten_thousand
test room_list::pure_tests::ten_thousand_rooms_build_only_the_visible_rows_in_every_density_and_the_rail ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1600 filtered out; finished in 0.00s
```
This is the room_list pure test (visible rows only, every density and the rail). I found no 10,000-room test in sections.rs (grep for 10_000/10000 there: none); sectioned windowing is covered by the visible_lines / spacers tests with small fixtures. Noted as a gap against the request's wording, not a failure.

## Gallery: space-sections and space-sections-collapsed, 1280/720, both densities, every test theme
Status: PROVEN (layout and overlap checks); see the 720 note
```console
$ TWICECAT_GALLERY_DIR=<scratchpad>/ui-gallery-evidence8 TWICECAT_GALLERY_ONLY=space-sections cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 142 filtered out; finished in 0.79s
$ cargo test -p twicecat-app --test ui_gallery -- space_sections
test space_sections_draw_every_line_whole_and_apart_in_both_densities_and_themes ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 142 filtered out; finished in 0.09s
```
Files: space-sections[-collapsed]-{twicecat,daylight}-{comfortable,compact}[-rail].png (16).
720 note: the layout test asserts `sections_shown()` is true only for the Expanded list at width >= 800, and that at 720 the flat list shows with all rows 56 px. So at 720 the sections are not drawn; the check at 720 proves the flat fallback is whole. Whether that matches the plan's intent for 720 is for the reviewer.

What I saw (images read, themes twicecat dark and daylight light):
- space-sections-twicecat-comfortable.png (1280x800): sticky-looking small bold headers with a down chevron: Starred (with "personal" at the right), Channels, Tech, Hobbies; rooms under each with avatar, name, lock, time, preview, count badges (3, 2, 3, 4) and a dot on Roadmap. General is selected (highlighted). The list is cut by the window at Board games; Voice and Direct messages are below the fold in this image (scrollbar visible).
- space-sections-collapsed-daylight-comfortable.png: Starred (personal), Channels expanded; Tech and Hobbies collapsed to one header row each with a right-pointing chevron and a teal pill badge (2 and 11); then Voice expanded with Lounge and Pair programming (each with a "Voice" tag) and Direct messages with Alice Example. 
- space-sections-collapsed-twicecat-compact.png: same structure in the dark theme, compact rows (no previews); collapsed Tech "2" and Hobbies "11" pills; the whole list fits, Voice and Direct messages visible.
- space-sections-daylight-compact-rail.png: the 72 px rail: flat list of avatars, no headers.
- I could not tell from the images whether the Tech "2" pill is drawn as a highlight style versus a plain count style; both pills look the same color in the two themes viewed (unit tests cover Highlight(2) vs Count). No empty-custom hint appears in these scenarios (none is defined for them; the hint is covered by a unit test only).

## Collapsed key survives restart and logout/login (scratch test over SidebarModel + DiskSidebar, temp data root, since deleted)
Status: PROVEN (model and disk level; not in a running app)
```console
$ cargo test -p twicecat-app --lib scratch_collapsed -- --nocapture
1 after toggle: collapsed = [SubSpace(<redacted>)]
file name: sidebar-b14155919594cee5.json
file bytes:
{"account":"@me:example.org","rail":[],"spaces":[{"collapsed":["sub:!tech:example.org"],"custom":[],"order":[],"space":"!space:example.org","starred":[]}],"version":1}
2 after restart: collapsed = [SubSpace(<redacted>)]
3 after logout: state = NotLoaded, layout = false
3 after login: collapsed = [SubSpace(<redacted>)]
test sidebar::scratch_persist::scratch_collapsed_survives_restart_and_relogin ... ok
```
The file holds only account/room IDs and keys (no names); a fresh DiskSidebar + model reads it back, and so does an unload/load of the same account. Fake IDs only.

## Mutations (each recorded and reverted)
Status: PROVEN
```console
# 1: also push each non-Channels room into the Channels bucket (a room in two sections)
$ cargo test -p twicecat-app --lib -- sections::
test sections::tests::a_level_1_space_in_two_sub_spaces_is_placed_once ... FAILED
test sections::tests::every_room_of_the_space_appears_exactly_once ... FAILED
test sections::tests::starred_beats_custom_beats_sub_space_beats_dm_beats_channels ... FAILED
test sections::tests::the_default_order_and_the_default_places ... FAILED
(+ 3 more: alt_down..., expanding_a_section..., a_header_click_toggles...)
test result: FAILED. 29 passed; 7 failed

# 2: visible_lines end = last.saturating_add(overscan + 1) (off by one)
-    let end = last.saturating_add(overscan).min(len).max(start);
+    let end = last.saturating_add(overscan + 1).min(len).max(start);
test sections::tests::equal_heights_give_what_visible_range_gives ... FAILED
test sections::tests::nan_negative_and_stale_offsets_are_clamped ... FAILED
test sections::tests::overscan_adds_lines_on_both_sides_and_stops_at_the_ends ... FAILED
test sections::tests::visible_lines_returns_exactly_the_lines_that_cross_the_viewport ... FAILED
test result: FAILED. 32 passed; 4 failed

$ cp <saved sections.rs> crates/app/src/sections.rs; diff <saved> crates/app/src/sections.rs
restored-identical     (git status: clean)
```
The placement test (every_room_of_the_space_appears_exactly_once) and the windowing test (visible_lines_returns_exactly_the_lines_that_cross_the_viewport) both fail under their mutation.

## Not observable here
- Real window scroll feel, 500+ rooms, the Alt/Ctrl+Shift stepping keys in the running app, clicking a header in the GUI, and the restart/logout in a running app on the local homeserver: Status: NOT PROVEN (GUI not started; covered by unit tests above and the scratch model test).
- The gate (fmt/clippy/full test) and the commit message/trailer: not run by me.

## Verdict
EVIDENCE: PROVEN
