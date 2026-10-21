# Evidence — Task 7: Dragging on the rail
Commit: 596b2df (drag commit 111d710 plus fixup)
Environment: cargo with CC=clang-22 CXX=clang++-22, from inside the repo; no GUI started. Scratch outputs in /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad (outside the repo). All outputs below were produced by the commands shown (trimmed where noted).

## State-machine tests, Op table, no-op drops, subscription, fixup regressions (one run)
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- drag rail_drop rail_dragging
running 31 tests
test drag::tests::a_movement_under_4_px_followed_by_a_release_is_a_click ... ok
test drag::tests::four_px_or_more_starts_a_drag_and_it_is_no_click ... ok
test drag::tests::the_thirds_give_before_into_and_after ... ok
test drag::tests::a_release_with_no_target_esc_or_focus_loss_cancels ... ok
test drag::tests::a_second_press_during_a_drag_is_ignored ... ok
test drag::tests::the_same_target_is_evaluated_once_until_the_pointer_leaves_or_things_change ... ok
test rail_drop::tests::every_drop_is_the_options_cards_buttons_in_order_and_gives_the_same_layout ... ok
test rail_drop::tests::dropping_onto_itself_or_a_folder_into_a_folder_saves_nothing ... ok
test rail_drop::tests::a_drop_that_would_pass_a_limit_is_refused_as_full_and_keeps_nothing ... ok
test state::tests::rail_dragging::the_window_listener_runs_only_while_the_button_is_down_on_an_item ... ok
test state::tests::rail_dragging::a_lifted_finger_leaves_no_press_behind_to_start_a_drag_later ... ok
test state::tests::rail_dragging::a_press_seen_by_the_window_while_one_is_tracked_cancels_it ... ok
test state::tests::rail_dragging::the_same_item_and_zone_is_planned_once_while_the_pointer_moves_in_it ... ok
test state::tests::rail_dragging::esc_and_losing_the_focus_cancel_and_save_nothing ... ok
test state::tests::rail_dragging::a_drop_with_no_target_or_onto_itself_or_to_where_it_is_saves_nothing ... ok
(16 more, all ok; list trimmed)
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 1534 filtered out
```
31 pass: click under 4 px, drag at 4 px, thirds, cancel paths, second press ignored; the table test compares drop Ops and resulting layout to the card's buttons over every target kind; self/folder-into-folder drops save 0 times; the listener runs only while a button is down; fixup tests (lifted finger, window press cancels a tracked press, planner once per item+zone) pass.

## keys.rs and SHORTCUTS untouched
Status: PROVEN
```console
$ git diff 40c0dc7..596b2df --stat -- crates/app/src/keys.rs
(empty)
$ git diff 40c0dc7..596b2df | grep -n SHORTCUTS      # trimmed to the cause
6: ... `SHORTCUTS` has no new row.   (PROGRESS.md prose only)
38: ... `keys.rs` and `SHORTCUTS` are untouched.  (PROGRESS.md prose only)
$ git diff 40c0dc7..596b2df -- crates/app/src/composer.rs crates/app/src/screens/settings.rs | wc -l
0
```
No diff to keys.rs; the only SHORTCUTS mentions in the diff are PROGRESS.md text; the other files that define/use SHORTCUTS are unchanged.

## Gallery scenarios rail-dragging and rail-dragging-into
Status: PROVEN (file output); drawing seen in the PNGs, see notes
```console
$ TWICECAT_GALLERY_DIR=/tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence7 TWICECAT_GALLERY_ONLY=rail-dragging cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 140 filtered out
$ ls /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence7   (trimmed)
rail-dragging-{daylight,twicecat}-{comfortable,compact}[-rail].png
rail-dragging-into-{daylight,twicecat}-{comfortable,compact}[-rail].png
index.html
```
I read rail-dragging-twicecat-comfortable-rail.png: the left rail shows a horizontal lilac line between the "QL" and "B" avatars (the "before" line), an outlined selected "SH" avatar, and a folder tile (G, K) grouped at the bottom. I read rail-dragging-into-twicecat-comfortable-rail.png: the "QL" avatar has a lilac/pink ring around it (the "into" ring) and no line between avatars. The other eight variants (daylight, compact) were generated but not viewed. The rest of the window (room list, "Select a room") is the usual static fixture; the dragged item itself (a ghost under the pointer) is not drawn in either image, which I could not tell is intended.

## Drop writes the same sidebar-<digest>.json as the options-card Ops (scratch test, now deleted)
Status: PROVEN
Scratch test in rail_drop.rs over SidebarModel + DiskSidebar on temp roots, three spaces a,b,c, dragging c onto a; drag path (plan + apply_all_known) vs card buttons (menu_op + apply_known).
```console
$ cargo test -p twicecat-app --lib scratch_evidence_disk -- --nocapture   (trimmed)
top drag ops: [MoveSpace { dir: Up }, MoveSpace { dir: Up }]
top card op: MoveSpace { dir: Up }
top card op: MoveSpace { dir: Up }
middle drag ops: [MoveToFolder { folder: New { named: false } }, MoveToFolder { folder: Existing(0) }]
middle card op: MoveToFolder { folder: New { named: false } }
middle card op: MoveToFolder { folder: Existing(0) }
bottom drag ops: [MoveSpace { dir: Up }]
bottom card op: MoveSpace { dir: Up }
$ cd /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/root7 && find . -name 'sidebar-*.json' | sort | xargs sha256sum
a0ab43784f6931c9f729ef9559c81b32c59c102348cc2484c47a1caf0dcb259c  ./bottom-card/sidebar-ea7cb42cfb340723.json
a0ab43784f6931c9f729ef9559c81b32c59c102348cc2484c47a1caf0dcb259c  ./bottom-drag/sidebar-ea7cb42cfb340723.json
2038c76fc6b23e3a0e7c7f0e2964b4641636d990317944c4076c4f17024fb3d6  ./middle-card/sidebar-ea7cb42cfb340723.json
2038c76fc6b23e3a0e7c7f0e2964b4641636d990317944c4076c4f17024fb3d6  ./middle-drag/sidebar-ea7cb42cfb340723.json
331f847c2a93675e85c405c470ba8acaa75429178489d285c4668fcdcd8741b0  ./top-card/sidebar-ea7cb42cfb340723.json
331f847c2a93675e85c405c470ba8acaa75429178489d285c4668fcdcd8741b0  ./top-drag/sidebar-ea7cb42cfb340723.json
```
Top, middle (folder) and bottom drops each give a byte-identical file to the matching card buttons; the three outcomes differ from each other. The scratch edit was reverted (rail_drop.rs diffed identical to a saved copy; git status clean).

## Mutation: dropping the 4 px threshold fails the click test
Status: PROVEN
```console
$ sed THRESHOLD 4.0 -> 0.0 in drag.rs; cargo test ... -- drag rail_drop rail_dragging
test drag::tests::four_px_or_more_starts_a_drag_and_it_is_no_click ... FAILED
test drag::tests::a_movement_under_4_px_followed_by_a_release_is_a_click ... FAILED
test state::tests::rail_dragging::a_press_that_wobbles_is_a_click_and_a_drag_is_not ... FAILED
test result: FAILED. 28 passed; 3 failed
```

## Fixup regression: a lifted finger or missed release cannot start a drag later or save
Status: PROVEN (by mutation)
```console
$ remove the FingerLifted mapping in spaces.rs::pointer_message
test state::tests::rail_dragging::the_listener_handles_touch_and_a_left_press_too ... FAILED
test state::tests::rail_dragging::a_lifted_finger_leaves_no_press_behind_to_start_a_drag_later ... FAILED
test result: FAILED. 29 passed; 2 failed
$ replace "let _ = self.rail_drag.cancel();" by a comment (this sed matched two sites: the PointerPressed arm and one other cancel, which I did not identify)
test state::tests::rail_dragging::esc_and_losing_the_focus_cancel_and_save_nothing ... FAILED
test state::tests::rail_dragging::a_press_seen_by_the_window_while_one_is_tracked_cancels_it ... FAILED
test result: FAILED. 29 passed; 2 failed
$ cp saved copies back; diff drag.rs/spaces.rs against copies; git status --short
restored   (git status empty; final run: 31 passed)
```
Both mutations make the targeted tests fail; the files were restored from saved copies and are identical. The cancel mutation was broader than only the PointerPressed arm, so it does not isolate that single line.

## Not proven
Status: NOT PROVEN (needs a real GUI)
Real mouse/touch dragging, Grab/Grabbing cursors, the drop feel, dragging a space onto another in a running app. Listed for the human checkpoint; no GUI was started.

## Verdict
EVIDENCE: PROVEN
