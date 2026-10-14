# Evidence — Task 10: Composer and the bars above it
Commit: 9d0c368
Environment: CC=clang-22 CXX=clang++-22, cargo in the repo's own target dir (LK_CUSTOM_WEBRTC untouched). Gallery written to a scratch dir outside the repo (`TWICECAT_GALLERY_DIR`, `-- --ignored`, 88 s) and inspected with the image reader. Gallery fixtures have no link-confirmation bar or command bar scenario, so those bars are covered by unit tests plus the mutation check only. Real-window checks (focus kept through bars, Enter/Tab/Ctrl+Shift+M/D with focus, typing `@al`) stay human.

## Unit and integration tests
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib
test result: ok. 1160 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.50s
$ cargo test -p twicecat-app --test ui_gallery
test result: ok. 16 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 13.94s
$ cargo test -p twicecat-app --lib -- main_view composer links state   (selected lines)
test composer::tests::enter_sends_and_shift_enter_adds_a_line ... ok
test links::tests::hosts_past_the_dns_limits_are_refused ... ok
test screens::main_view::tests::the_send_button_sends_what_enter_sends_and_only_when_it_may ... ok
test screens::main_view::tests::the_send_button_picks_while_suggestions_are_open ... ok
test screens::main_view::tests::the_link_bar_shows_only_the_host ... ok
test screens::main_view::tests::the_link_bar_keeps_the_end_of_a_long_host_inside_its_text ... ok
test screens::main_view::tests::the_command_bar_names_the_command_and_gives_the_hint ... ok
test screens::main_view::tests::the_command_bar_keeps_a_long_command_name_inside_its_text ... ok
test state::tests::composer_enter_sends_the_message_and_clears_the_field ... ok
test state::tests::edit_mode_sends_an_edit_and_reply_mode_a_reply ... ok
test state::tests::a_failed_send_or_edit_shows_a_line_for_the_open_room ... ok
test state::tests::error_and_send_error_text_is_one_capped_line ... ok
test state::tests::changing_room_resets_spoilers_and_the_link_bar ... ok
test state::tests::sending_clears_the_stored_draft ... ok
test ui_gallery: the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go ... ok
```
Send/Enter parity (send button test), host limits, long link/command names kept inside their text, and the unchanged Enter/Shift+Enter tests all pass. Mention-row ID elision has no test of that name in the selected set; it is not separately shown here.

## Composer on a rounded raised field, Attach left, Emoji and Send right; placeholder "Message <room>"
Status: PROVEN (gallery)
- main-room-twicecat-light-comfortable.png: a full-width rounded field at the bottom of the room pane, paperclip Attach at left, placeholder "Message Lounge", smiley Emoji and a Send arrow at right. Send is greyed (blank text). No clipping.
- main-narrow-room-frost-dark-comfortable.png (720 px): same field and icons, placeholder "Message Lounge", fits the width.
- main-reply-suggestions-frost-light/dark-comfortable.png: text ":sm" typed; Send is a filled teal square button (enabled) in both themes.

## Reply bar and suggestions are cards
Status: PROVEN (gallery)
- Frost light and dark: above the composer a rounded card with "Replying to" small label, "Alice Example: A long message that has to wrap over several lines..." on ONE line, capped, and a close X icon at right.
- Above that a rounded card listing 8 emoji suggestions (:smiley:, :smile:, ...); the first row is highlighted with the selection tint (pale teal in light, dark teal in dark), text readable. The "Bea Example and Cy Example are typing..." line sits above the card.

## send_error / notice use banner(Danger) with the same words
Status: PROVEN for words and layout by tests; banner look NOT PROVEN in a composer-adjacent PNG
- Tests `a_failed_send_or_edit_shows_a_line_for_the_open_room` and `error_and_send_error_text_is_one_capped_line` pass. Fixtures `banner-*` exist in the gallery but I did not inspect them for this task.

## Anti-phishing: link bar and command bar text uses full-width WordOrGlyph, buttons below
Status: PROVEN (tests + mutation)
```console
$ sed -i '656s/WordOrGlyph/Word/' crates/app/src/ui/kit.rs   # kit::BANNER_WRAPPING = Wrapping::Word (temporary)
$ cargo test -p twicecat-app --lib -- link_bar command_bar
test screens::main_view::tests::the_command_bar_names_the_command_and_gives_the_hint ... ok
test screens::main_view::tests::the_link_bar_shows_only_the_host ... ok
test state::tests::changing_room_resets_spoilers_and_the_link_bar ... ok
test screens::main_view::tests::the_command_bar_keeps_a_long_command_name_inside_its_text ... FAILED
test screens::main_view::tests::the_link_bar_keeps_the_end_of_a_long_host_inside_its_text ... FAILED
thread '...the_command_bar_keeps_a_long_command_name_inside_its_text' panicked at crates/app/src/screens/main_view.rs:1791:13:
thread '...the_link_bar_keeps_the_end_of_a_long_host_inside_its_text' panicked at crates/app/src/screens/main_view.rs:1772:13:
test result: FAILED. 3 passed; 2 failed; 0 ignored; 0 measured; 1155 filtered out; finished in 0.02s
$ git checkout -- crates/app/src/ui/kit.rs; git status --short
(clean)
```
With Word wrapping both long-text bar tests fail, so they really guard the wrapping; restored and the tree is clean.

## Hidden composer inert; editor keeps slot and focus
Status: NOT PROVEN (internal / human)
Slot stability is covered by the passing `the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go`. Actual focus and the Ctrl+Shift+M/D keys need a real window (human checkpoint).

## Repo guard
```console
$ scripts/check-no-webrtc-download.sh; echo exit=$?
exit=0
```

## Defects seen
- None in the inspected PNGs. Minor: the unfocused Send in main-room is only a pale arrow (disabled look), by design.

## Verdict
EVIDENCE: PROVEN (outside-observable parts; human items listed above)
