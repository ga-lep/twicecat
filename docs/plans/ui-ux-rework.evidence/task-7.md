# Evidence — Task 7: Room list redesign, density and the rail
Commit: da2c9ed
Environment: CC=clang-22 CXX=clang++-22, `cargo test -p twicecat-app`; gallery written to a scratch dir outside the repo (TWICECAT_GALLERY_DIR, 80 s). Real-window checks (tooltips, Ctrl+B typing, restart persistence) are human checkpoint items and were not run.

## Gallery and layout tests pass (windowing and 10,000 rooms for both densities and the rail)
Status: PROVEN
```console
$ cargo test -p twicecat-app --test ui_gallery
test ten_thousand_rooms_build_only_the_rows_in_view_for_every_density_and_the_rail ... ok
test the_room_list_is_300_wide_or_a_72_px_rail_and_keeps_its_five_slots ... ok
test the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go ... ok
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
(+ 7 more ok)
test result: ok. 11 passed; 0 failed; 1 ignored
$ TWICECAT_GALLERY_DIR=<scratch> cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 11 filtered out; finished in 80.19s
```
Unit tests (`--lib`): room_list 78 passed (incl. `row_heights_follow_the_density_and_the_rail_is_fixed`), room_list_pane 12 passed, keys 18 passed, rail_dock 3 passed; 0 failed in all.
The 72 px rail width, 300 px list, slot retention and 10k-room windowing are checked by passing tests.

## Expanded rows: avatar, name, time, preview, count_badge, call pill, Voice/Invite/Muted tags, selected row with accent bar; segmented tabs, search icon
Status: PROVEN (visual)
Inspected `main-room-twicecat-dark-comfortable.png` and `-compact.png`. Comfortable shows 64 px rows: avatar, name, lock, time, one-line preview, count badge (3 on Lounge, red 2 on Design review), "Muted" and "Voice" tags, a speaker pill with count "2" on Voice hangout, an "Invite" tag on Book club, and the selected Lounge row on a raised background with a left accent bar. Tabs (All/DMs/Rooms/Spaces) render as a segmented control and search has a magnifier icon. Compact shows 44 px rows with no preview.
Defects (minor, visual): in Comfortable the count badge sits over the end of the preview text ("...at the stand-up" runs under the "3" badge, same for Design review), and the preview is clipped by the scrollbar. The clear (x) button was not visible because the search box is empty (not exercised).

## Rail: 72 px, avatars with unread dot, speaker dot, fixed rows, call bar becomes compact dock
Status: PROVEN (visual)
Inspected `main-room-twicecat-dark-comfortable-rail.png`, `main-call-connected-twicecat-dark-comfortable-rail.png`, `main-invite-twicecat-dark-compact-rail.png`. The rail is a narrow column with + / person / expand / search buttons, then avatar rows with unread dots (Lounge grey, Design review red), lock overlays, a speaker and lock chip on Voice hangout (VH), and a "+" invite chip on Book club (BC). With a call, the bottom dock shows speaker, green lock chip, people chip, mic, declined-bell and a red leave button. The selected invite row has the accent bar. Defect (minor): on VH the speaker and lock chips overlap each other and the dot at the bottom of the avatar.

## Rail dock shows mic, encryption and declined state visibly (mutation check)
Status: PROVEN
On a scratch worktree (since removed) `dock_call` was changed to return `mic: MicLook::Plain`, `lock: None`, `declined: None`:
```console
$ cargo test -p twicecat-app --lib rail_dock
test ...the_rail_dock_shows_whether_push_to_talk_is_transmitting ... FAILED
test ...the_rail_dock_marks_encryption_and_the_lack_of_it ... FAILED
test ...the_rail_dock_shows_who_declined_and_the_notice ... FAILED
  left: Plain   right: Talking
  left: None    right: Some(Encrypted)
  left: None    right: Some("@u0:x declined")
test result: FAILED. 0 passed; 3 failed
```
At the real tree the same three tests pass. The tests do detect the removal of each state.

## Ctrl+B toggles and saves; Alt+Up/Down and Ctrl+Shift+Up/Down in the rail; empty states; Step trailer
Status: NOT PROVEN (Ctrl+B in a real window, persistence: human checkpoint; key mapping covered by the passing keys unit tests)

## Verdict
No criterion failed; two minor visual defects noted (badge over preview text in Comfortable; overlapping chips on VH rail row).
