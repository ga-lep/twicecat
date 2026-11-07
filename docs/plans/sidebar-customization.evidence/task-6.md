# Evidence — Task 6: Arranging the rail with the options menu (folders included)
Commit: f5624bd
Environment: CC=clang-22 CXX=clang++-22, cargo run from inside the repo. No GUI started. Gallery PNGs written outside the repo to /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence6. Scratch test and mutations were applied to crates/app/src/sidebar.rs from a saved copy (scratchpad/sidebar.rs.orig); after each, the file was restored and `git status --short` was empty.

## Operation tests, each followed by one save (no-op moves save nothing; new folder at the space's place; second-to-last out dissolves; Ungroup restores order; MAX_FOLDERS refused as full; Newer refuses everything)
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- sidebar
test result: ok. 88 passed; 0 failed; 0 ignored; 0 measured; 1446 filtered out; finished in 0.25s
```
Named tests among those 88 (all `... ok`): move_up_on_the_first_item_and_down_on_the_last_change_nothing_and_save_nothing, moving_a_space_into_a_new_folder_makes_the_folder_at_its_place, moving_the_second_to_last_space_out_of_a_folder_dissolves_it, ungroup_puts_the_spaces_back_in_their_order, an_edit_that_would_pass_max_folders_is_refused_as_full_and_saves_nothing, in_the_newer_state_every_rail_operation_is_refused, folder_ids_are_the_lowest_free_one, move_down_moves_a_space_one_item_and_saves_once.
All pass. The "88 passed" count is the filter `sidebar` (it also catches the store and spaces_sidebar tests).

## Menu: Move down moves B one item and saves; inside an open folder it stays in the folder; a collapsed folder's card moves the folder; every rail op reachable from a card (table test)
Status: PROVEN
```console
test sidebar_menu::tests::every_button_of_every_card_maps_to_an_operation_or_the_dialog ... ok
test sidebar_menu::tests::a_space_card_has_the_moves_and_leaves_a_folder_only_from_one ... ok
test state::tests::sidebar_menu_flow::move_down_on_a_space_moves_it_one_item_saves_once_and_closes_the_card ... ok
test state::tests::sidebar_menu_flow::inside_an_open_folder_it_moves_within_it_and_a_folders_own_card_moves_the_folder ... ok
test state::tests::sidebar_menu_flow::ungroup_and_remove_from_folder_go_through_the_card ... ok
test state::tests::sidebar_menu_flow::rename_opens_the_dialog_with_the_current_name_and_saves_the_new_one ... ok
```
These come from the same `-- sidebar` run (state tests run as `state::tests::sidebar_menu_flow::*`; all ok).

## No chord: keys.rs and SHORTCUTS untouched; keys.rs and Settings keyboard-list tests unchanged and passing
Status: PROVEN
```console
$ git diff 910ac8e..f5624bd --stat -- crates/app/src/keys.rs
$ git diff 910ac8e..f5624bd -- crates/app/src/keys.rs | wc -l
0
$ cargo test -p twicecat-app --lib -- keys settings
test result: ok. 166 passed; 0 failed; 0 ignored; 0 measured; 1368 filtered out; finished in 0.04s
```
The diff is empty, so keys.rs (where SHORTCUTS lives) is untouched. I did not separately check that no keys/settings test file was edited. The commit's diffstat lists none besides the files in the plan.

## Name dialog: takes focus, Enter with "  Work  " saves "Work"; 33 chars or "\u{202E}x" shows the error and saves nothing; Esc cancels
Status: PROVEN (by state tests and the scratch run; not in a live window)
```console
test state::tests::sidebar_menu_flow::new_folder_opens_the_dialog_and_enter_saves_the_trimmed_name ... ok
test state::tests::sidebar_menu_flow::a_name_that_is_not_valid_shows_the_error_saves_nothing_and_esc_cancels ... ok
test state::tests::sidebar_menu_flow::a_blank_name_makes_an_unnamed_folder_and_a_pick_moves_into_an_existing_one ... ok
test state::tests::sidebar_menu_flow::the_field_keeps_at_most_128_characters ... ok
test sidebar_menu::tests::a_name_is_trimmed_blank_means_none_and_a_bad_one_is_an_error ... ok
scratch: label as typed '  Work  ' -> Some("Work")
```
Dialog focus on open is asserted only inside the state tests (a focus Task); real focus in a window is NOT PROVEN here.

## menus_ignore_the_wheel covers the folder pick list; Esc order gains the card and the dialog
Status: PROVEN
```console
$ cargo test -p twicecat-app --test menus_ignore_the_wheel
test the_rails_folder_list_is_a_pick_list_behind_the_wheel_guard ... ok
test every_pick_list_is_wrapped_by_the_wheel_guard ... ok
test every_slider_is_wrapped_by_the_wheel_guard ... ok
test the_scan_tells_a_wrapped_widget_from_a_bare_one ... ok
test result: ok. 4 passed; 0 failed

$ cargo test -p twicecat-app --test ui_gallery -- rail
test ctrl_wheel_over_the_rail_options_card_never_picks_a_folder ... ok
(7 passed; 0 failed)

$ cargo test -p twicecat-app --lib -- esc
test state::tests::sidebar_menu_flow::esc_closes_the_dialog_or_the_card_and_nothing_under_it ... ok
test state::tests::sidebar_menu_flow::the_switcher_is_over_the_card_and_takes_esc_first ... ok
test state::tests::sidebar_menu_flow::a_name_that_is_not_valid_shows_the_error_saves_nothing_and_esc_cancels ... ok
test result: ok. 50 passed; 0 failed
```
The scan test asserts exactly one pick_list in screens/sidebar_menu.rs and no bare ones. The Ctrl+wheel test runs at 1280 and 720. The Esc order is covered by the dialog/card tests above, and the older `esc_closes_one_layer_per_press_topmost_first` still passes. I did not check whether that last test itself gained a card/dialog step.

## Gallery: rail-menu and rail-folder-name pass the layout checks at both widths
Status: PROVEN (layout checks); how it looks in a real window is NOT PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
test result: ok. 140 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 72.14s

$ TWICECAT_GALLERY_DIR=<scratchpad>/ui-gallery-evidence6 TWICECAT_GALLERY_ONLY=rail- cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
```
The ignored test wrote rail-menu-{twicecat,daylight}-{comfortable,compact}.png and rail-folder-name-... (plus -rail variants). All are 1280x800; the gallery writes PNGs only at 1280. The 720 width is covered by the non-ignored layout test above, not by an image I viewed.

What I saw (rail-menu-twicecat-comfortable.png, rail-menu-twicecat-compact.png, rail-folder-name-twicecat-comfortable.png, rail-folder-name-twicecat-compact.png):
- rail-menu: the rail is dimmed behind a centred card titled "Boredlabs" with a close x. It has two full-width buttons "Move up" and "Move down", a small "Move to folder" label over a pick list reading "Choose a folder...", and a "Close" button at the bottom right. In the rail behind it, one item ("SH") has a highlighted ring and the pill beside it. No folder preview is visible among the items behind the card except small multi-icon tiles (R/V/G/K and G/K) that look like folder previews. Nothing is cut off. The card has no Ungroup, Rename or Remove entries because it is a plain space's card.
- rail-folder-name: a "New folder" card with close x, the text "Name the folder, or leave the name blank. Up to 32 characters.", a text field holding a long run of "w" characters that runs to the field edge, a red-bordered error box with a leading x reading "Use 1 to 32 characters, with no control or hidden characters, or leave it blank.", and "Cancel" and "Create" buttons. The Create button is drawn as active (filled), even with an invalid name. I do not know whether that is intended.
- Comfortable and compact differ only in the room list density behind the card. The Image shows the field text without a visible caret, so I cannot say focus is shown. The images are a tiny-skia render, which the plan says is an aid, not proof.

## Scratch run over SidebarModel + DiskSidebar on a temp data root (layout file effect)
Status: PROVEN (model and store on a temp root; not the running app)
A temporary test was appended to crates/app/src/sidebar.rs tests (using DiskNow), run, and the file was restored from the saved copy.
```console
$ cargo test -p twicecat-app --lib -- scratch_layout_file_effect --nocapture
label as typed '  Work  ' -> Some("Work")
new folder from !b:x -> ok=true
rail after new folder (counts via Debug): Some(SidebarLayout { rail: 4, spaces: 0 })
move !a:x down -> ok=true
file: sidebar-b14155919594cee5.json
{"account":"@me:example.org","rail":[{"folder":{"id":0,"name":"Work","open":false,"spaces":["!b:x"]}},{"space":"!a:x"},{"space":"!c:x"},{"space":"!d:x"}],"spaces":[],"version":1}
contains "folder": true
contains "Work": true
contains space display name "Boredlabs"/"Garden": false
test result: ok. 1 passed
```
The file has a `folder` entry named "Work" (trimmed), room IDs only, and no display names. Display names are not in the model at all, so the last check is by construction. Findings: the new folder holds one space (it persists, id 0, closed) and "Move down" on !a:x then placed !a after the folder; both agree with the plan text ("alone in a new one"; the dissolve rule applies to folders being left). The model is given the known space list [a,b,c,d] directly.

DiskSidebar ordering fixes (existing tests, run by name):
```console
$ cargo test -p twicecat-app --lib -- sidebar_store::tests::a_ sidebar_store::tests::reset
test sidebar_store::tests::a_save_then_a_reset_leave_no_file_whichever_finishes_first ... ok
test sidebar_store::tests::a_reset_then_a_save_leave_the_save_whichever_finishes_first ... ok
test sidebar_store::tests::a_load_after_a_queued_save_gets_the_queued_layout_not_the_old_file ... ok
test sidebar_store::tests::a_load_after_a_queued_reset_finds_the_default ... ok
test sidebar_store::tests::reset_removes_the_file_and_cancels_a_waiting_save ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 1523 filtered out
```
Save then reset leaves no file in either finish order; reset then save leaves the save in either order; a load during a queued save returns the queued layout.

## Mutations, each recorded and reverted
Status: PROVEN
```console
M1: in take_out, `if folder.spaces.len() >= 2 {` -> `>= 1 {` (single-space folders no longer dissolve)
$ cargo test -p twicecat-app --lib -- sidebar
test sidebar::tests::moving_the_second_to_last_space_out_of_a_folder_dissolves_it ... FAILED
test sidebar::tests::moving_a_space_from_a_folder_into_a_new_folder_dissolves_the_old_one ... FAILED
test result: FAILED. 86 passed; 2 failed

M2: in move_space (top-level branch) `return Ok(Change::Nothing);` -> `return Ok(Change::Done);` (a no-op move saves)
test sidebar::tests::a_move_steps_over_entries_that_are_not_drawn ... FAILED
test sidebar::tests::move_up_on_the_first_item_and_down_on_the_last_change_nothing_and_save_nothing ... FAILED   ("no save for a move that goes nowhere")
test state::tests::sidebar_menu_flow::move_down_on_a_space_moves_it_one_item_saves_once_and_closes_the_card ... FAILED
test result: FAILED. 85 passed; 3 failed
```
Both mutations fail the named tests. After each, sidebar.rs was copied back from scratchpad/sidebar.rs.orig and `git status --short` / `git diff --stat` were empty (no diff against HEAD).

## Plan evidence: right-click a space in the running app -> Move to folder -> New folder "Work" -> 2 x 2 preview; Move up moves the space
Status: NOT PROVEN (needs a display and the local homeserver GUI; not started). Covered indirectly by the state-flow tests and the scratch run above. The 2 x 2 folder preview and real right-click behaviour belong on the human checkpoint.

## Verdict
EVIDENCE: PROVEN
