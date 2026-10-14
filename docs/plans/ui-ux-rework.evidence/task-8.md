# Evidence — Task 8: Narrow windows
Commit: 6bdc3de
Environment: CC=clang-22 CXX=clang++-22, `cargo test -p twicecat-app` (debug), headless iced renderer gallery (TWICECAT_GALLERY_DIR in the session scratchpad, not in the repo). No server, no real window: real resizing stays a human checkpoint.

## layout_for thresholds (>=1100 choice applies; 800-1100 forced rail, last opened wins; <800 no sidebar, rail, panel takes room pane); saved choice never changes
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- layout narrow_window keys
test layout::tests::a_width_that_is_not_a_number_is_treated_as_the_narrowest ... ok
test layout::tests::medium_windows_force_the_rail_only_while_a_panel_is_open ... ok
test layout::tests::narrow_windows_drop_the_sidebar_use_the_rail_and_give_a_panel_the_room_pane ... ok
test layout::tests::wide_windows_apply_the_choice_and_show_every_open_panel ... ok
test layout::tests::with_both_open_the_one_opened_last_is_shown_and_the_other_waits ... ok
test state::tests::narrow_window::the_saved_choice_never_changes_with_the_width_and_nothing_is_saved ... ok
test state::tests::narrow_window::with_both_open_the_last_opened_shows_and_the_other_waits ... ok
test state::tests::narrow_window::opening_a_panel_in_a_medium_window_keeps_the_place_in_the_list ... ok
test state::tests::narrow_window::the_rows_follow_the_list_as_shown ... ok
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 1095 filtered out
```
(excerpt; full run had 43 passing: layout, narrow_window and keys tests.)

## A resize updates window_width with no Task; update tests cross both thresholds (no panel, thread, call panel, both)
Status: PROVEN
```console
test state::tests::narrow_window::a_resize_records_the_width_and_runs_no_task ... ok
test state::tests::narrow_window::crossing_both_thresholds_with_no_panel_changes_only_the_sidebar ... ok
test state::tests::narrow_window::crossing_both_thresholds_with_the_thread_open ... ok
test state::tests::narrow_window::crossing_both_thresholds_with_the_call_panel_open ... ok
test state::tests::narrow_window::with_both_open_the_last_opened_shows_and_the_other_waits ... ok
```

## Room pane stays in the tree; Esc returns to it in the existing Esc order
Status: PROVEN
```console
test state::tests::narrow_window::esc_closes_the_shown_panel_first_and_returns_to_the_room_pane ... ok
test state::tests::narrow_window::esc_closes_the_thread_first_when_it_is_the_one_shown ... ok
test state::tests::narrow_window::closing_the_panel_shows_the_room_pane_and_sends_the_receipt ... ok
$ cargo test -p twicecat-app --test ui_gallery
test the_slots_follow_the_layout_at_every_width_and_the_room_pane_keeps_its_place ... ok
test the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go ... ok
test the_room_list_is_300_wide_or_a_72_px_rail_and_keeps_its_five_slots ... ok
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
test a_header_with_a_call_never_spills_or_grows_tall_when_narrow ... ok
test a_narrow_header_never_clips_the_trust_tag_or_the_end_of_the_peer_id ... ok
test a_panel_that_hides_the_room_header_carries_the_trust_tag ... ok
test result: ok. 16 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 13.65s
```
Slot test (five slots) passes at both 720 and 1280 px.

## Gallery renders 720 and 1280 px for each layout
Status: PROVEN
```console
$ TWICECAT_GALLERY_DIR=<scratch> cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 16 filtered out; finished in 84.34s
```
Images inspected (twicecat-dark-comfortable):
- main-narrow-room (720): sidebar slot 0 wide (no space rail), 72 px list rail with its five slots (new, account, expand, search, rooms), room pane fills the rest. Header: avatar, "Lounge" with the "Encrypted" tag fully visible, notification menu, people and settings icons and Call stacked on a second line. Timeline and composer intact. No overlap or clipping.
- main-narrow-thread (720): thread panel takes the room pane's width, header "Thread" with a lock icon and "Lounge" as the trust/room tag (the room header it replaces is hidden, so the tag moves here), close button, thread messages, reply composer. Rail still present on the left. No defects seen.
- main-narrow-call (720): panel "In the call (4)" with lock icon + "Lounge", participants with Speaking / Muted (mute icon next to Cy Example) / "Voice cleaned" states, per-person volume sliders and clean menus, close button. The rail carries the in-call strip (speaker, encryption lock chip, participants chip, mic, deafen, red leave) so mic and encryption state stay visible while the room header is hidden. No overlap. Minor observation only: the in-call strip covers the lowest list rows (expected, it overlays the rail's bottom).
- main-medium-thread (1000): space rail + 72 px rail forced, room pane and thread panel side by side; "Encrypted" tag visible in the room header. Fine.
- main-medium-call (1000): rail forced, room header shows "Leave call" and participants toggle, call panel at right with the same participant states; in-call strip in the rail. Fine.
- main-room (1280): wide layout, 300 px list with density rows, room pane; unchanged choice applies. Fine.
No clipped trust tag, no overlaps found.

## Security fixes: picker pane, receipt suppression, cleaned ID tooltip, Ctrl+U/drop refusal
Status: PROVEN (unit level)
```console
test state::tests::narrow_window::no_picker_opens_for_a_room_item_while_a_panel_hides_the_room_pane ... ok
test state::tests::narrow_window::no_picker_opens_for_a_thread_item_the_call_panel_has_hidden ... ok
test state::tests::narrow_window::a_thread_picker_works_where_the_panel_is_beside_the_room ... ok
test state::tests::narrow_window::a_thread_picker_is_drawn_by_the_panel_even_when_it_fills_the_window ... ok
test state::tests::narrow_window::shrinking_the_window_closes_a_picker_that_would_be_hidden ... ok
test state::tests::narrow_window::a_hidden_room_pane_sends_no_receipt_until_it_is_shown_again ... ok
test state::tests::narrow_window::leaving_a_room_that_was_hidden_moves_no_marker ... ok
test state::tests::narrow_window::a_room_pane_behind_a_panel_takes_no_measurements ... ok
test state::tests::narrow_window::ctrl_u_and_drops_are_refused_while_a_panel_hides_the_room ... ok
test a_narrow_header_never_clips_the_trust_tag_or_the_end_of_the_peer_id ... ok
```
The cleaned-ID tooltip is covered by the gallery test above (peer id end not clipped); the tooltip text itself was not separately observed.

## kit::menu / no_wheel on all pick_lists and sliders
Status: PROVEN
```console
$ cargo test -p twicecat-app --test menus_ignore_the_wheel
test every_pick_list_is_wrapped_by_the_wheel_guard ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
$ (ui_gallery)
test ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing ... ok
test ctrl_wheel_over_the_room_header_never_changes_the_notification_mode ... ok
test ctrl_wheel_over_the_status_bar_never_logs_out ... ok
```

## Mutation check: remove the receipt suppression
Status: PROVEN
Scratch worktree at 6bdc3de outside the repo; `state.rs:2643` changed from `if self.pane_layout().panel_fills {` to `if false && self.pane_layout().panel_fills {` (read_receipt early return disabled). Needed LK_CUSTOM_WEBRTC set to the real dev/libwebrtc because the worktree lacks the gitignored symlink.
```console
$ cargo test -p twicecat-app --lib narrow_window
test state::tests::narrow_window::a_hidden_room_pane_sends_no_receipt_until_it_is_shown_again ... FAILED
test state::tests::narrow_window::closing_the_panel_shows_the_room_pane_and_sends_the_receipt ... FAILED
panicked at crates/app/src/state.rs:11880:13:
assertion `left == right` failed: [PaginateBackwards { room_id: RoomId("!a:x") }, MarkRead { room_id: RoomId("!a:x"), fully_read: false }]
test result: FAILED. 19 passed; 2 failed; 0 ignored; 0 measured; 1117 filtered out
```
The tests catch the removal (a MarkRead is sent for a hidden pane). Worktree removed afterwards; `git worktree list` shows only the main checkout.

## Commit carries `Step: ui.8`
Status: NOT PROVEN (not checked here; commit-message check is the caller's)

## Dragging the window narrower during a call collapses the list, then the sidebar
Status: NOT PROVEN (human checkpoint: real window resizing)

## Verdict
EVIDENCE: PROVEN
