# Evidence — Task 15: Room dialogs, room settings, members, upload, viewer and switcher
Commit: 1012fae
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir, headless iced renderer (gallery tests). Gallery written to a scratch dir (TWICECAT_GALLERY_DIR, `-- --ignored`, TWICECAT_GALLERY_ONLY per prefix; an earlier stale file in the scratch dir was discarded and room-leave-confirm regenerated in a fresh dir). Screenshots were inspected with the Read tool and are not stored in the repo.

## Test suites
```console
$ cargo test -p twicecat-app --test ui_gallery --test menus_ignore_the_wheel
test result: ok. 100 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out   (ui_gallery; ignored = write_the_gallery)
(menus_ignore_the_wheel: ctrl_wheel_over_the_menus_of_the_settings_and_call_screens_changes_nothing ... ok)
$ cargo test -p twicecat-app --lib
test result: ok. 1228 passed; 0 failed
$ cargo test -p twicecat-app --lib -- rooms:: members upload switcher keys:: media_viewer room_dialogs room_settings
test result: ok. 141 passed; 0 failed
$ cargo test -p twicecat-app --lib -- esc      -> 35 passed (incl. screens::room_dialogs::tests::escape_closes_what_is_drawn_on_top_and_a_question_before_its_panel, state::tests::keyboard::esc_closes_the_upload_confirmation, esc_closes_the_viewer, esc_closes_the_switcher_and_focuses_the_composer)
$ cargo test -p twicecat-app --lib -- switcher -> 19 passed (enter_opens_the_highlighted_room_and_closes_the_switcher, enter_with_no_match_keeps_the_switcher_open, esc_closes_the_switcher..., arrows)
$ scripts/check-no-webrtc-download.sh ; echo exit=$?
exit=0
```
Status of the module-test criterion: PROVEN (all pass; diff touches only tests added, no message/update logic besides a view-only member search field and `media::file_title`).

## Every dialog is a modal with header (title + close icon_button), body, footer with primary last and destructive as danger_button
Status: PROVEN (visual, gallery scenarios create-room, room-settings, room-settings-long, room-leave-confirm, room-encrypt-confirm, members*, user-card*, upload*; Frost dark and Twicecat light, comfortable and compact)
- create-room (Frost dark): "New room" header with x, labelled Name/Topic fields, Encrypted checkbox with help text, footer Cancel then primary Create. Clean.
- room-settings: header "Room settings" plus room name subtitle, labelled Name/Topic with Save, Change picture, Address, Enable encryption..., a divider, red "Leave room..." (danger) and Close in the footer.
- room-leave-confirm: inline confirm card "Leave this room?" with the warning line ("Twicecat could not check whether this room is encrypted or whether you are its last admin."), Cancel then red Leave (danger, Frost dark and light). Fits in compact too.
- room-encrypt-confirm (Twicecat light): "Turn on encryption?" card with the irreversible warning, Cancel then red "Enable encryption". The Leave room... button remains visible below as the destructive section.
- user-card (Twicecat light): header "Person" + x, avatar, name, ID, "Not verified" tag, primary Message button last.
Confirmation flows unchanged: tests `esc_*`, `the_buttons_that_confirm_a_question_are_drawn_red_and_the_others_are_not` pass.

## Members: search, sections Admins/Moderators/Members/Invited, avatar, name, trust tags, "in the call" tag, empty_state, 100-row cap
Status: PROVEN (visual + tests)
- members-call-trust (Frost dark): "Members (8)", search field, sections Admins (2), Moderators (1), Members (4), Invited (1, partly below the fold). Rows show avatar, name, ID and tags: Creator, Verified (green outline), Not verified, "Identity changed" (red outline), "In the call" (speaker tag) on Bea, Cy and Me. Spoofed names show quoted (`"Admin (75)"`) with the real ID visible. Very long ID is middle-elided with the end kept.
- members-search (Frost dark compact): query "alice" leaves only "Moderators (1)" with Alice.
- members-many (Twicecat light): header "Members (450)", a single Members (450) section, avatars P0.. in a scrolled list; the invite row ("Checking what you may do..." with disabled field) is a pinned footer. Row cap/"show more" and search are covered by unit tests (the_member_search_is_view_state_only_capped_and_cleared_when_the_panel_opens, MEMBERS_PAGE tests) in the 141 passing tests; the 100-row cap itself was not seen in a screenshot (only the top of the list is visible).
- members-none / members-loading scenarios exist and render (empty_state not individually inspected visually; the layout test `every_screen_lays_out_in_every_variant_at_both_sizes` passes for them).

## Switcher: command-palette card near the top, large field, rows with avatar, name, unread badge, call pill; Up/Down/Enter/Esc tests unchanged
Status: PROVEN
- switcher (Frost dark): card at the top of the window (y~72), large query field ("e"), first row highlighted with a teal outline, rows with avatar and name, grey unread badge (3), red unread/mention badge (2), grey dot for muted, teal call pill ("2" with speaker) for Voice hangout, hint line "Up and Down to move, Enter to open, Esc to close". Scrollbar sits to the right of the badges with a gap; no overlap seen.
- Up/Down/Enter/Esc: 19 switcher tests pass (listed above).

## Viewer: dimmed backdrop, image centred, bottom bar with name, size, Save, Close. Upload: preview, caption, size, type
Status: PROVEN for viewer; upload PROVEN except caption/preview
- viewer (Frost dark): dimmed app behind, the gradient image centred and fitted, a bottom bar "holiday.png / image/png . 180.0 KB . 320 x 240 px" with secondary Save... and primary Close.
- upload (Twicecat light): "Send 4 files", row "To Lounge" with an Encrypted tag, entries with kind label (Image/File), file name, "180.0 KB . image/png", a red warning icon + "This file could not be read." and "Reading..." states, footer Cancel then Send (primary). No image preview thumbnail and no caption field is drawn in this scenario (Image/File text labels instead). If the plan's "preview, caption" means a thumbnail and caption field, these are not visible in the gallery: noted, not proven.

## Upload names its target room and Send is disabled for another room
Status: PROVEN
- upload-wrong-room (Frost dark): composer is on "Design review" but the card reads "To Lounge", with a red banner "These files were picked for a different room than the one on screen. Cancel and attach them again in the room you mean." and Send greyed out (disabled), Cancel enabled.
- Mutation (a): in `crates/app/src/screens/upload_dialog.rs` line ~217 changed `can_send() && matches` to `can_send() || matches`:
```console
test dialogs::files_picked_for_another_room_cannot_be_sent_and_say_so ... FAILED
test dialogs::the_upload_card_names_its_room_and_shows_size_and_type_and_send_is_its_own_button ... FAILED
test result: FAILED. 98 passed; 2 failed; 1 ignored   (ui_gallery)
```
(the lib-only upload tests stayed green, 61 passed: the guard is covered by the gallery view tests.) File restored from a copy; `git status` clean.

## Esc order
Status: PROVEN with a caveat
- Unit tests pass (esc_closes_what_is_drawn_on_top_and_a_question_before_its_panel gives order dialog, card, question, panel; plus esc_closes_the_upload_confirmation / viewer / switcher).
- Mutation (b1): in `room_dialogs.rs::topmost` swapped Dialog and Card:
```console
test screens::room_dialogs::tests::escape_closes_what_is_drawn_on_top_and_a_question_before_its_panel ... FAILED
panicked at crates/app/src/screens/room_dialogs.rs:610:17
test result: FAILED. 34 passed; 1 failed (-- esc)
```
- Mutation (b2), a finding: in `crates/app/src/keys.rs::on_escape` swapped the upload-dialog and viewer branches: all 1228 lib tests still passed (no test opens both at once, so that pair's order is untested). Both files restored; `git status` clean.

## Enter while a dialog is open (reported, not requested as a criterion)
Status: FINDING at update level; real window NOT CHECKED
A temporary probe test (removed afterwards) typed "hello" in the composer, opened a room overlay and sent `ComposerKey::Send`:
```console
PROBE create: overlay_open=true sends=1 draft_after=""
PROBE settings: overlay_open=true sends=1 draft_after=""
PROBE members: overlay_open=true sends=1 draft_after=""
```
So at the state level `ComposerKey::Send` is not gated on an open dialog: if the key reaches the editor (focus still in the composer under the modal), Enter would send the draft. Whether the real window routes the key there (iced `opaque` blocks the mouse, not necessarily the keyboard, and focus on open is unverified) is a human check.

## Defects noted from screenshots
- None of clipping, overlap or text under a scrollbar seen in the inspected states. Minor: in Frost dark the members list (comfortable) cuts the "Invited (1)" section header off at the bottom behind the footer (it is a scroll region); compact switcher/members only inspected for members-search.
- Not inspected: all remaining Ember/High-contrast variants, user-card-long, room-settings-long, viewer-loading/long, switcher-long/empty images (layout tests `nothing runs out of a card sideways ... all fit a short window` pass).
- Human checkpoint: native file picker, Esc stacking feel, focus, real Ctrl+K.

## Cleanup
All edits restored from copies; `git status --short` empty before writing this file. `scripts/check-no-webrtc-download.sh` exit 0.

## Verdict
EVIDENCE: PROVEN
