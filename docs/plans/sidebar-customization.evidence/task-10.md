# Evidence — Task 10: Dragging rooms and sections
Commit: 2d6fa41 (drag commit 9f4eb40 plus two fixups)
Environment: no server; this is a GUI app, so the surface is the app's own tests, the headless UI gallery renderer and a scratch consumer test. CC=clang-22 CXX=clang++-22, cargo run from inside the repo. Gallery PNGs in /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence10 (outside the repo). The scratch test was deleted afterwards; `git status` is clean apart from this file.

## Table test: each room-list drop gives the same layout as the matching card operation
Status: PROVEN

```console
$ cargo test -p twicecat-app --lib -- list_drop dragging drag sections
test list_drop::tests::a_room_onto_a_custom_header_goes_to_the_end_of_that_section ... ok
test list_drop::tests::a_room_between_rows_of_a_custom_section_lands_there ... ok
test list_drop::tests::a_section_onto_a_header_goes_before_or_after_it_among_the_drawn_ones ... ok
test list_drop::tests::every_dropped_operation_is_one_the_cards_produce ... ok
test sections::tests::dragging::every_kind_of_drop_gives_the_layout_of_the_matching_card_operation ... ok
...
test result: ok. 105 passed; 0 failed; 0 ignored; 0 measured; 1553 filtered out; finished in 0.03s
```
105 passed, 0 failed, including the table test.

### Same file on disk (scratch test over SidebarModel + DiskSidebar, temp data root, since deleted)
Two models loaded from the same layout; one applied the drop plan from `list_drop::plan`, the other the card's op; each wrote through `DiskSidebar` to its own temp root.
```console
$ SCRATCH_ROOT=<scratchpad>/scratchroot cargo test -p twicecat-app --lib scratch_evidence -- --nocapture
SCRATCH t1->Ops: drop ops = [MoveRoomToSection { section: 0 }]; card ops = [MoveRoomToSection { section: 0 }]
SCRATCH c1->Channels: drop ops = [RoomToDefault { .. }]; card ops = [RoomToDefault { .. }]
test list_drop::tests::scratch_evidence_drop_vs_card_on_disk ... ok
$ sha256sum .../scratchroot/*/sidebar-82c93996fd659248.json
bc2756d76b99360eb760fa632f4aa8c352a3711577352c5a52e24eed29a82c73  c1_Channels-card/sidebar-82c93996fd659248.json
bc2756d76b99360eb760fa632f4aa8c352a3711577352c5a52e24eed29a82c73  c1_Channels-drop/sidebar-82c93996fd659248.json
1c5eb9d815583b078dcaa0df56e489a5515fea2045a16c1ce708cb0676b09e9b  t1_Ops-card/sidebar-82c93996fd659248.json
1c5eb9d815583b078dcaa0df56e489a5515fea2045a16c1ce708cb0676b09e9b  t1_Ops-drop/sidebar-82c93996fd659248.json
```
(Hashes copied from the real sha256sum output; the console block was tidied only in those two lines.)

## Back to default only into the room's own default section; elsewhere refused and drawn not allowed
Status: PROVEN (logic and drawing; not the live pointer)
```console
ok  list_drop::tests::a_room_onto_a_default_header_goes_back_only_to_its_own_default_section
ok  sections::tests::dragging::a_target_that_is_not_the_rooms_default_section_is_refused_and_drops_nothing
```
Gallery `space-sections-dragging-refused-twicecat-comfortable.png` (viewed): the "Tech" header is outlined in red/pink with the label "Not allowed" at its right while the other sections look normal. That is the refused drawing. Image shows only the state, not a cursor.

## Row click still opens the room; right-press card still opens
Status: PROVEN (tests)
```console
ok  sections::tests::dragging::a_press_and_release_with_no_movement_is_still_a_click
ok  sections::tests::dragging::a_drag_is_no_click
ok  sections::tests::menu::a_right_click_or_the_dots_open_the_card_of_a_drawn_room_or_header
ok  state::tests::sidebar_menu_flow::a_right_click_opens_the_card_of_a_space_or_a_folder_and_not_of_home
ok  room_list::tests::the_selection_survives_a_diff_that_moves_the_room (+ other selection tests)
```
14 passed in that filtered run.

## In flat mode no row is draggable
Status: PROVEN (tests)
```console
ok  sections::tests::dragging::in_the_flat_list_or_without_a_layout_nothing_is_draggable
ok  sections::tests::home_a_search_another_tab_or_the_rail_keep_the_flat_list
```

## Gallery scenarios space-sections-dragging(+ -line, -refused, -header)
Status: PROVEN (rendered; honest description below)
```console
$ TWICECAT_GALLERY_DIR=<scratchpad>/ui-gallery-evidence10 TWICECAT_GALLERY_ONLY=space-sections-dragging cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.54s
```
32 files written: 4 scenarios x {twicecat, daylight} x {comfortable, compact} x {pane, -rail}, plus index.html.
What I saw (twicecat theme, comfortable):
- `space-sections-dragging` (into): the "Reading list" header has a light-purple outline box (drop-into highlight). Rows otherwise unchanged. I cannot tell from the still which row is the lifted one.
- `-line`: a thin light-purple horizontal line sits between "Random" and "Announcements" in "Reading list", spanning the pane width. Rust help row looks faintly highlighted (hover or lifted row; unclear which).
- `-refused`: "Tech" header outlined in red with "Not allowed" (see above).
- `-header`: a thin purple line under the General row / above the "Reading list" header, and the "Later" header has a filled lighter background. The "Later" fill is presumably the lifted section; I could not confirm from the image alone.

## Fixups
Status: PROVEN (tests; mutations below show the tests have teeth)
```console
ok  sections::tests::dragging::pointer_moves_over_an_unchanged_list_rebuild_nothing_but_a_real_change_settles
ok  sections::tests::dragging::a_rail_press_ends_a_tracked_list_press
ok  sections::tests::dragging::a_click_works_again_after_a_cancelled_drag_and_after_the_window_press_cancel
ok  sidebar::tests::section_ops::moving_a_section_among_more_than_256_drawn_ones_is_refused_without_full
```
The last test drives a model with 300 drawn sections (MAX_SECTION_KEYS + 44): MoveSection returns Refused::Full for the op, state stays Loaded (not Full), zero saves, and a later star still saves once.

## keys.rs and SHORTCUTS untouched
Status: PROVEN
```console
$ git diff f226d33..2d6fa41 --stat -- crates/app/src/keys.rs crates/core
(empty)
$ git diff f226d33..2d6fa41 -- crates/app/src/keys.rs | wc -l
0
```
SHORTCUTS lives in keys.rs (grep also lists settings.rs, composer.rs as users); neither is in the diff stat.

## Mutation checks
Status: PROVEN (tests fail on the mutants; files restored and diffed)
```console
# A: list_drop.rs  `SectionKey::Starred => Op::Star` changed to `Op::Unstar`
FAILED list_drop::tests::a_room_onto_the_starred_header_stars_it_and_a_starred_one_stays
FAILED sections::tests::dragging::every_kind_of_drop_gives_the_layout_of_the_matching_card_operation
  panicked at crates/app/src/sections.rs:2090:21: assertion `left != right` failed: a room onto Starred: the drop did something
test result: FAILED. 8 passed; 2 failed

# B: state.rs update(): skip list neutralised (`let settles = true || !matches!(...)`)
FAILED sections::tests::dragging::pointer_moves_over_an_unchanged_list_rebuild_nothing_but_a_real_change_settles
  panicked at crates/app/src/sections.rs:2362:13: no rebuild for pointer moves or hover
  left: 208
  right: 8
```
Both mutants were restored from saved copies: `diff` of list_drop.rs against its pre-mutation copy printed "restored", and `git status --short` was empty.

## NOT PROVEN (needs a real GUI and pointer)
- Real mouse dragging, cursor shapes, the feel of a drop, and wheel scrolling during a drag: NOT PROVEN (GUI not started, per instructions).
- "In a running app, dragging a room onto Ops moves it there": NOT PROVEN live; the equivalent state machine, plan and on-disk write are shown above.
- The gate (fmt, clippy, full test) was not re-run here beyond the filtered runs.

## Verdict
EVIDENCE: PROVEN
