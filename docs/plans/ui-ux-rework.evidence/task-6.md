# Evidence — Task 6: Main window frame: header, panes and status bar
Commit: 523ff31
Environment: CC=clang-22 CXX=clang++-22, headless iced gallery (no real window). Gallery written to the session scratchpad with `TWICECAT_GALLERY_DIR=<scratch> cargo test -p twicecat-app --test ui_gallery -- --ignored` (1 passed, 83.04 s). Images inspected: main-room / main-room-states / main-dm / main-invite / main-thread / main-call-connected, frost-dark-comfortable only. Deliberate changes per caller: Log out is its own button, trust badge is severity-coloured, peer ID on its own line.

## Tests: gallery slot invariants
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery
test variant_tags_are_file_name_safe ... ok
test the_index_is_a_plain_image_grid ... ok
test a_button_label_stays_on_one_line_when_the_header_is_squeezed ... ok
test clicking_the_log_out_button_requests_a_logout ... ok
test the_fixture_reaches_every_screen_it_names ... ok
test ctrl_wheel_over_the_status_bar_never_logs_out ... ok
test a_narrow_header_never_clips_the_trust_tag_or_the_end_of_the_peer_id ... ok
test the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go ... ok
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
test write_the_gallery ... ignored, writes PNGs for review
test result: ok. 9 passed; 0 failed; 1 ignored
```
Layout invariants hold in every variant at both sizes.

## Tests: existing main_view/state tests pass; message-reachability unit test
Status: PROVEN
```console
$ cargo test -p twicecat-app main_view   (trimmed to relevant lines; also filtered "state")
test screens::main_view::tests::every_header_action_sends_what_the_old_header_sent ... ok
test screens::main_view::tests::the_status_bar_still_reaches_every_message_the_old_one_sent ... ok
test screens::main_view::tests::the_call_button_presses_are_still_reachable ... ok
test screens::main_view::tests::leave_is_the_only_danger_button_and_keeps_its_words ... ok
test screens::main_view::tests::the_sync_dot_has_a_tone_for_every_state_and_the_words_stay ... ok
test screens::main_view::tests::each_trust_badge_has_its_own_severity ... ok
test screens::main_view::tests::the_account_menu_has_no_log_out_and_adds_voice_and_audio ... ok
test screens::main_view::tests::the_frame_builds_for_every_header_state ... ok
test screens::main_view::tests::the_panes_keep_their_place_when_the_banner_and_security_screens_come_and_go ... ok
test calls::tests::the_main_view_builds_in_every_call_state ... ok
test state::tests::the_main_view_and_the_security_cards_fill_the_window ... ok
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 1073 filtered out
```
(Other test binaries in the run showed 0 matching tests.) The old-message coverage tests exist and pass. I did not diff the pre-existing tests to confirm they are unchanged.

## Room header: avatar, name, topic line, tags, icon buttons, call button
Status: PROVEN for the rendered parts, with a gap on the topic
Observed in main-room-frost-dark-comfortable.png: round "L" avatar, "Lounge" title, an "Encrypted" tag under it, a notification-mode dropdown ("Use default"), a People icon, a settings gear, and a filled teal "Call" button. No topic line appears: the fixture room seems to have no topic, so the one-line capped topic is covered only by the unit test `the_topic_is_one_capped_line_or_nothing`, not by an image. No Voice tag appears on this room. A Members icon distinct from People is not identifiable (one person-style icon and one gear). Not verified from the image.

## Call button states / Leave call not clipped / People toggle
Status: PROVEN (connected state only)
main-call-connected-frost-dark-comfortable.png: the header shows a red/coral "Leave call" button with its full label, not clipped, followed by a teal-outlined People toggle (people icon and a check). The right pane "In the call (4)" has a close "x" button. `a_button_label_stays_on_one_line_when_the_header_is_squeezed` also passes. Other call states (Join call (N), Joining…) were not opened as images.
Possible defect seen (outside the header): the call card at the bottom of the left pane shows its "Audio..." button cut off at the card's right edge, and "@alice:example.org declined" text is tight. This area is outside the Task 6 header and status bar scope.

## DM with trust badge and peer ID line
Status: PROVEN (one trust state)
main-dm-frost-dark-comfortable.png: the header shows "Alice Example", then tags "Encrypted" and "Not verified" (teal outline), a "Verify" button, and the peer "@alice:example.org" on its own line below. The header grows taller than in a room. Only the Not-verified state appears in the fixture's DM image. Other trust states are covered by the unit test `each_trust_badge_has_its_own_severity` only. The no-clipping test `a_narrow_header_never_clips_the_trust_tag_or_the_end_of_the_peer_id` passes.

## Invite keeps Accept and Decline
Status: PROVEN
main-invite-frost-dark-comfortable.png: header "Book club" with "You were invited", a teal "Accept" button and a grey "Decline" button.

## Panes separated by 1 px rules; sidebar / thread panel; thread header with close button
Status: PROVEN visually (colour and pixel width not measured)
main-thread-frost-dark-comfortable.png: thin vertical dividers are visible between the rail, sidebar, room pane and thread panel. The thread panel has a "Thread" header with an "x" close button at top right, and a slightly lighter background than the sidebar. I did not measure the 1 px width or the Semantic::border value from pixels.

## Status bar: sync dot plus words, account, account menu, Log out
Status: PROVEN
In every main-* image I looked at, the bottom bar shows a green dot with "Sync: Running", an account dropdown "@me:example.org" (menu not opened in the image, so its entries come from the unit test `the_account_menu_has_no_log_out_and_adds_voice_and_audio`), and a separate "Log out" button at the bottom right. `clicking_the_log_out_button_requests_a_logout` and `ctrl_wheel_over_the_status_bar_never_logs_out` pass.

## Commit carries `Step: ui.6`
Status: NOT PROVEN (not checked here)

## Human checkpoint
Real-window check of the header in a room with a call (Join call and People) stays with the human, per the plan.

## Verdict
No criterion failed. The topic line and other call and trust states were not shown in images (unit tests only).
