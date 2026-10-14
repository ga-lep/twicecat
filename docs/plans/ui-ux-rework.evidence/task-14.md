# Evidence — Task 14: Settings and Sessions as pages
Commit: d4b7fd6
Environment: headless (no window). `CC=clang-22 CXX=clang++-22`, repo's own target dir, `.cargo/config.toml` untouched. Gallery written to the session scratchpad (`TWICECAT_GALLERY_DIR`, `TWICECAT_GALLERY_ONLY=settings` and `=sessions`, `-- --ignored`) and viewed with the Read tool. The PNGs come from tiny-skia, not the wgpu window; open `pick_list` menus are not drawn. The gallery writes 1280x800 PNGs only. The 720 px layouts are checked by layout tests (not drawn).

## Settings is a modal (760 x 620) with page_nav; seven pages
Status: PROVEN (layout tests and PNGs); real window stays human

```console
$ cargo test -p twicecat-app --test ui_gallery --test menus_ignore_the_wheel
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s   (menus_ignore_the_wheel)
test result: ok. 73 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 52.61s (ui_gallery)
 (includes) settings_pages::the_card_is_760_by_620_with_the_page_list_at_its_left ... ok
 settings_pages::the_page_never_runs_under_its_scrollbar ... ok
 settings_pages::nothing_runs_out_of_the_card_sideways_in_any_page_at_both_widths ... ok
 settings_pages::each_page_sends_only_its_own_messages_and_together_they_send_them_all ... ok
 settings_pages::the_voice_and_sessions_pages_have_their_one_button ... ok
 settings_pages::the_notice_slot_is_there_on_every_page_and_takes_no_room_when_empty ... ok
 the_four_colour_sets_fit_the_page_without_spilling (720 px, 4 swatches on screen)
```
Viewed settings-frost-dark / appearance-frost-dark / keyboard-twicecat-light / about-twicecat-light / sessions-twicecat-light (comfortable): a card with the title "Settings" and Close at the top, a left list (Profile, Appearance, Notifications, Voice & audio, Keyboard, Sessions, About; the active one outlined), a divider, and the page at the right. Profile: name field with Save, "Picture: none", Change/Remove picture. Keyboard: a table of chords (Ctrl+K ... Esc) with a scrollbar clear of the text, nothing covered. Sessions page: a line of text and "Manage sessions...". No defects seen.

## Appearance (swatches) and About (drag-and-drop note, version, Inter attribution)
Status: PROVEN

Appearance (Frost dark): Follow the system / Light / Dark radios, "Colours" with four 2x2 swatches (Twicecat (default), Frost with check mark and a teal outline, Ember, High contrast; each shows three colour chips), Use the default, Density, Room list, Text size with Smaller/Larger/Reset and a Ctrl+=/-/0 hint. Nothing overlaps. About (Twicecat light) shows "Twicecat 0.1.0", the Wayland drag-and-drop note, "Inter typeface © 2016 The Inter Project Authors, SIL Open Font License 1.1." and a notice that "The licence text is in OFL.txt in the source (crates/app/assets/fonts/inter) and in THIRD-PARTY-NOTICES.md; see also https://openfontlicense.org". The wording reads fine and nothing is cut off.

## Ctrl+, opens Profile; Esc / Tab documented
Status: NOT PROVEN (real key events in a window: human checkpoint); unit-covered

```console
$ cargo test -p twicecat-app --lib   (1216 tests)
test result: ok. 1216 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.45s
$ grep -n "Esc" docs/plans/../accessibility.md   (trimmed)
25:| Esc | Close the topmost dialog, panel, Settings, the Sessions card or the switcher |
```
The Keyboard page in the PNG lists "Ctrl+, Open Settings" and the Esc row. State-level Esc tests are under the Esc section below.

## Sessions card: current session first, trust tags, ghost actions, destructive buttons apart
Status: PROVEN

Viewed sessions-frost-dark, sessions-twicecat-light, sessions-confirm-all-twicecat-light, sessions-long-names-twicecat-light-compact, sessions-logout-warning-frost-dark, sessions-reauth-failed-frost-dark (comfortable unless noted).
- The list: "Twicecat on this computer" first, with tags "This device" and "Verified", device id in monospace, "Last active Today 09:43", and only Rename. "Phone" has an orange "Not verified" tag and Verify / Rename at the left, with a red "Sign out" at the far right. "OLDLAPTOP" has a "No encryption keys" tag. The current session has no Sign out. "Sign out of all other sessions" is a red button in a footer under the list, apart from Refresh/Close.
- The scrollbar sits clear of the rows. The third row is cut off by the scroll viewport; that is the scroll area, not a defect.
- Confirm-all: a Warning banner ("Sign out of all other sessions? Each one will have to sign in again."), then Cancel at the left and "Sign out of all" at the right, separated.
- Logout warning: a modal "Sign out?" with a Warning banner, "Set up recovery", then Cancel at the left and "Sign out anyway" at the right.
- Reauth (failed): a modal "Enter your password" with a Warning banner, an error banner "Wrong password", the field and Cancel / Continue (Continue greyed while empty).
- Observations, not failures: the Rename/Verify ghost buttons have inner padding, so their text is indented from the card text. The "long names" PNG shows the same three top rows; the long names are further down the scroll, so that PNG does not show the long-name case. That case is checked by `sessions_screen::nothing_runs_out_of_a_card_sideways_at_both_widths_and_densities ... ok`.

```console
$ cargo test -p twicecat-app --test ui_gallery destructive_buttons
sessions_screen::destructive_buttons_sit_at_least_one_gap_from_every_other_control ... ok   (in the full run above)
```

## Sessions model/handlers unchanged; existing tests pass unchanged
Status: PROVEN (by tests); diff not part of this run

`cargo test -p twicecat-app --lib`: 1216 passed, 0 failed (includes `sessions::tests::*`, `state::tests::*` with sessions).

## Review fixes: Esc closes/cancels without confirming; look-alike names quoted; Inter notice wording
Status: PROVEN for the state level; the real key feel is human

```console
$ cargo test -p twicecat-app --lib  (relevant tests, all ok)
state::tests::keyboard::esc_closes_the_sessions_card
state::tests::keyboard::esc_cancels_the_sign_out_question_and_keeps_the_card
state::tests::keyboard::esc_cancels_the_password_prompt_and_forgets_the_password
state::tests::keyboard::esc_while_the_password_is_in_flight_is_swallowed
state::tests::keyboard::esc_cancels_the_logout_warning_and_never_logs_out
sessions::tests::a_name_that_reads_like_a_tag_or_an_id_is_quoted ... ok
```
The Inter notice wording is seen in the About PNG above.

## Mutation checks (real tree, file copies restored, `git status` clean after each)
Status: PROVEN (the tests detect each mutation)

```console
$ # (a1) crates/app/src/keys.rs: sessions_escape sends LogoutAnyway instead of LogoutCancelled
$ cargo test -p twicecat-app --lib esc
test state::tests::keyboard::esc_cancels_the_logout_warning_and_never_logs_out ... FAILED
test result: FAILED. 33 passed; 1 failed
$ # (a2) drop the in-flight swallow (return Some(Task::none()) removed)
$ cargo test -p twicecat-app --lib esc
test state::tests::keyboard::esc_while_the_password_is_in_flight_is_swallowed ... FAILED
assertion failed: app.sessions.reauth.as_ref().is_some_and(|p| p.waiting)
$ # (b) crates/app/src/screens/sessions.rs: action row spacing 0 and the spacer removed
$ cargo test -p twicecat-app --test ui_gallery destructive_buttons
sessions: Sessions(SignOutAsk(DeviceId("PHONE1"))) is 0 px from Sessions(RenameStart(DeviceId("PHONE1")))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
$ git status --short
(empty)
```
After restoring, the ui_gallery and menus_ignore_the_wheel tests passed again (3 and 73), and `git status --short` was empty.

## Final checks
```console
$ scripts/check-no-webrtc-download.sh; echo exit=$?
exit=0
```
Status: PROVEN

## Left for the human (not provable headless)
Ctrl+, in the real window then each page; Tab order (page, then the page list); the Esc feel and focus; a real sign-out; wgpu rendering vs the tiny-skia PNGs.

## Verdict
All outside-observable criteria were demonstrated; no failures. Minor observations are noted above (the long-names PNG does not show a long name in its top viewport; the ghost button text is indented).
