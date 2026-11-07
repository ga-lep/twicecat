# Evidence — Task 9: User-defined sections, Starred, and moving rooms by the options menu
Commit: a9f3e91
Environment: no GUI started. `CC=clang-22 CXX=clang++-22 cargo test` from inside the repo; scratch test over SidebarModel + DiskSidebar on a temp data root (deleted afterwards); gallery PNGs written outside the repo to /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence9/.

## Operation tests (Star/Unstar, NewSection, Rename, Delete, MoveRoomToSection one place per space, MoveRoomInSection, MoveSection, Full refusal)
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- sidebar sections
test result: ok. 158 passed; 0 failed; 0 ignored; 0 measured; 1475 filtered out; finished in 0.30s
```
Includes (named in the run) sidebar::tests::section_ops::delete_returns_its_rooms_to_their_default_sections_and_drops_its_keys, ...::move_room_to_section_takes_it_out_of_every_other_place, sections::tests::menu::{star_on_a_rooms_card_toggles_its_star_and_saves_once, rename_keeps_the_rooms_and_move_down_in_a_section_moves_only_inside_it, move_down_on_a_headers_card_moves_the_section_over_the_drawn_ones, delete_returns_the_rooms_to_their_default_sections}. These are the repo's own unit tests; I did not read each assertion against every bullet, only the delete and move-to-section ones (see mutations).

## Menu: every section and room operation reachable from a card (table test)
Status: PROVEN (at the level of the pure entry-to-Op mapping)
```console
$ cargo test -p twicecat-app --lib -- every_button_of_every
test sidebar_menu::tests::every_button_of_every_card_maps_to_an_operation_or_the_dialog ... ok
test sidebar_menu::tests::every_button_of_every_section_and_room_card_maps_to_an_operation_or_the_dialog ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1631 filtered out; finished in 0.00s
```
Right-click, the "..." buttons and a real click on a card button were not exercised: NOT PROVEN (needs the GUI). The sections::tests::menu::a_right_click_or_the_dots_open_the_card_of_a_drawn_room_or_header test passes in the 158 above, but it is a message-level test.

## No chord: keys.rs and SHORTCUTS untouched, their tests unchanged
Status: PROVEN
```console
$ git diff 197a232..a9f3e91 --stat -- crates/app/src/keys.rs crates/core | wc -c
0
$ cargo test -p twicecat-app --lib -- keys shortcuts
test result: ok. 35 passed; 0 failed; 0 ignored; 0 measured; 1598 filtered out; finished in 0.02s
```
The path diff is empty (keys.rs and all of crates/core unchanged). The full-commit diffstat lists no keys.rs.

## menus_ignore_the_wheel covers the "Move to section" pick list
Status: PROVEN (source-scan test, not a rendered wheel event)
```console
$ cargo test -p twicecat-app --test menus_ignore_the_wheel
test the_scan_tells_a_wrapped_widget_from_a_bare_one ... ok
test the_sidebar_cards_lists_are_pick_lists_behind_the_wheel_guard ... ok
test every_pick_list_is_wrapped_by_the_wheel_guard ... ok
test every_slider_is_wrapped_by_the_wheel_guard ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```
The test (diff in a9f3e91) now expects 2 pick lists in screens/sidebar_menu.rs and checks that "Move to section" is inside kit::menu before its pick_list. It reads source text; it does not send a Ctrl+wheel event.

## Gallery: scenarios space-sections-custom and section-menu pass the layout checks
Status: PROVEN (layout checks); the images were looked at
```console
$ cargo test -p twicecat-app --test ui_gallery
test result: ok. 142 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 71.49s
$ TWICECAT_GALLERY_DIR=.../ui-gallery-evidence9 TWICECAT_GALLERY_ONLY=space-sections-custom cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok   (1 passed)
$ (same with TWICECAT_GALLERY_ONLY=section-menu)
test write_the_gallery ... ok   (1 passed)
```
16 PNGs + index.html written (2 scenarios x 2 themes x 2 densities x with/without rail).

What I saw in space-sections-custom-twicecat-comfortable.png: the room list shows section headers each with a collapse triangle and a "..." button: "Starred" (tag "personal", one room Roadmap), "Channels" (General, selected, badge 3), "Reading list" (Random, Announcements: a custom section), "Later" (a custom section, empty, with the hint text "Drop or move rooms here"), then "Tech" (Rust help, DevOps, Async queries...). So two custom sections, one empty with its hint, as specified. The selected row also has a "..." at its right edge. No text overlap or clipping visible; the last row is cut at the bottom by the scroll viewport, which is normal for a scrolled list.

What I saw in section-menu-twicecat-comfortable.png: the same list dimmed behind a modal card titled "Random" with a close X. Buttons: Star, Move up (dimmed, as Random is first in its section), Move down, "Back to its default section", then a labelled "Move to section" drop-down reading "Choose a section...", and Close. Layout is clean, nothing clipped. I did not open the drop-down (static image only) and did not view the other 14 PNGs.

## Written sidebar-<digest>.json holds `custom` with label and room ID, no room name; survives restart and logout/login
Status: PROVEN (model + real DiskSidebar on a temp root; not the running app)
Scratch test (appended to crates/app/src/sidebar.rs, removed afterwards) applied NewSection "Ops" with room "!roomid1:x" through SidebarModel and DiskSidebar, printed the file, built a new store + model (restart), then unload/load (logout/login).
```console
$ cargo test -p twicecat-app --lib scratch_t9 -- --nocapture
apply ok = true
FILE sidebar-82c93996fd659248.json
JSON {"account":"@a:x","rail":[],"spaces":[{"collapsed":[],"custom":[{"id":0,"name":"Ops","rooms":["!roomid1:x"]}],"order":[],"space":"!sp:x","starred":[]}],"version":1}
AFTER RESTART custom = [(0, "Ops", ["!roomid1:x"])]
AFTER LOGOUT/LOGIN custom = [(0, "Ops", ["!roomid1:x"])]
test sidebar::scratch_t9::scratch_ops_persist ... ok
```
The file holds `custom` with the label "Ops" and the room ID and nothing else about the room (the model never receives a room name). The layout reloads identical after restart and after logout/login. The in-app right-click flow ("Move to section" -> New section) was NOT PROVEN (GUI).

## Mutation: deleting a section without returning its rooms fails the delete test
Status: PROVEN
Mutation 1 (sidebar.rs, Op::DeleteSection): `record.custom.remove(at);` replaced by `let gone = record.custom.remove(at); record.starred.extend(gone.rooms);` (rooms kept in Starred instead of returning to their default sections).
```console
$ cargo test -p twicecat-app --lib -- sidebar sections
test sections::tests::menu::delete_returns_the_rooms_to_their_default_sections ... FAILED
test sidebar::tests::section_ops::delete_returns_its_rooms_to_their_default_sections_and_drops_its_keys ... FAILED
assertion failed: at("# Channels") < at("chan1") && at("chan1") < at("# tech")
assertion failed: !placed.contains(&r("a")) && !placed.contains(&r("b"))
test result: FAILED. 156 passed; 2 failed; 0 ignored; 0 measured; 1475 filtered out
```
Mutation 2 (my choice): removed `take_room_out(record, &room);` from Op::MoveRoomToSection (one place per space).
```console
test sidebar::tests::section_ops::move_room_to_section_takes_it_out_of_every_other_place ... FAILED
assertion failed: model.apply(to_section("x", 2), &store).is_ok()
test result: FAILED. 157 passed; 1 failed; 0 ignored; 0 measured; 1475 filtered out
```
Note: the failing assertion for mutation 2 is the first `is_ok()`, not the one-place check; I did not investigate why the op was refused after the mutation. The test did fail, which is what was asked.
Both were reverted by copying back the saved original: `git status --short` and `git diff --stat` printed nothing afterwards.

## Not observable here
Status: NOT PROVEN
Right-click opening a card, the "..." buttons, the dialog's focus, the open "Move to section" drop-down, and the in-app Ops flow need the running GUI, which I did not start.

## Verdict
All outside-observable criteria that could be exercised without the GUI were demonstrated.
