# Evidence — Task 16: Security banners, recovery and verification
Commit: c7ddecf
Environment: `CC=clang-22 CXX=clang++-22`, repo's own target dir, `.cargo/config.toml` untouched. Headless iced renderer gallery (no real accounts, no real keys; the fixture key is the dummy `DUMMYKEY...`). Gallery written to a fresh scratch dir (session scratchpad `k/`), `TWICECAT_GALLERY_ONLY=banner-`, `recovery-`, `verification-` one prefix per run (a comma list matches nothing; an earlier scratch dir held stale images, so all screenshots below were re-read from the fresh `k/` dir).

## Banners use `banner` by severity, one primary action, dismiss where allowed
Status: PROVEN (gallery) — identity-changed Danger banner has no gallery scenario (see Defects/notes)

```console
$ cargo test -p twicecat-app --test ui_gallery -- security_screens
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 101 filtered out
$ cargo test -p twicecat-app --lib   (full lib)
test result: ok. 1239 passed; 0 failed; 0 ignored
```
Screens read (Frost dark / Twicecat light, 1280x800):
- `banner-device-frost-dark-comfortable.png`: amber Warning bar across the top, warning triangle, "This device isn't verified. Verify it to read your encrypted history and so others can trust it.", one teal primary button "Verify" at the right on its own line below the words. No dismiss.
- `banner-recovery-off-frost-dark-comfortable.png`: amber Warning bar, "Recovery isn't set up. Without it you can lose your encrypted messages if you sign out everywhere.", plain "Dismiss" text button and teal primary "Set up recovery".
- `banner-recovery-incomplete-frost-dark-comfortable-rail.png` (72 px rail): "This device can't use your key backup yet. Enter your recovery key to read older encrypted messages." with "Enter recovery key" and a close X.
- `banner-notice-twicecat-light-comfortable.png`: pale red Danger bar with red border, red cross icon, "Could not reach the server to set up recovery. Check your connection and try again." and a "Dismiss" button.
The banner/room-pane tests (`the_screens_and_banners_leave_the_room_pane_where_it_is`, `a_banner_keeps_its_words_whole_with_its_buttons_below_them_in_the_rail_and_at_720`, `the_notice_banner_is_danger_with_its_words_and_a_dismiss_button`) pass.

## Recovery and change-key are a modal; key in MONO, groups of four, on surface, Copy and Save; hidden by default; "never in Debug or logs" tests pass
Status: PROVEN (Save button: see note)

```console
$ cargo test -p twicecat-app --lib -- the_agreeing_buttons_wait esc_on_the_emoji_card the_key_is_drawn a_new_key_screen
test security::tests::a_new_key_screen_starts_hidden_and_its_debug_has_no_key ... ok
test security::tests::the_key_is_drawn_in_groups_of_four_four_to_a_line ... ok
test state::tests::keyboard::esc_on_the_emoji_card_cancels_and_never_confirms_a_match ... ok
test state::tests::the_agreeing_buttons_wait_and_a_stale_timer_or_a_new_phase_never_arms_early ... ok
test result: ok. 4 passed
$ (lib, from full list) security::tests::debug_prints_no_key_input_or_password ... ok
  security::tests::clearing_secrets_drops_the_key_the_inputs_and_the_copy ... ok
  security::tests::the_clipboard_is_cleared_only_while_it_still_holds_the_key ... ok
  security::tests::setup_finishes_only_when_ticked ... ok
```
Screens read:
- `recovery-key-frost-dark-comfortable.png` (hidden by default): card "Your recovery key", amber warning box "Store this key somewhere safe, like a password manager. Without it you can't read your encrypted messages on a new device if you're signed out everywhere."; key area shows three lines of `**** **** **** ****` in monospace on a darker surface block; buttons "Show key" and "Copy key", small note "Copy puts the key on the clipboard; it is cleared after 60 seconds."; checkbox "I've saved my recovery key" unticked; "Done" is a dim text button (disabled).
- `recovery-key-shown-frost-dark-comfortable.png` / twicecat-light: same layout, key shown as 3 lines of 4 groups of four (`DUMM YKEY DUMM YKEY / DUMM YKEY 0000 0000 / 0000 0000 0000 0000`), button now "Hide key"; layout does not move. Done still dim while the box is unticked.
- `recovery-key-saved-frost-dark-comfortable.png`: box ticked (teal check), "Done" becomes a teal primary button.
- `recovery-reset-confirm-frost-dark-comfortable.png`: "Reset your identity?" with a red-bordered, dark-red box and X icon: "Resetting your identity makes your other devices and your contacts see you as a new user, and deletes your key backup: older encrypted messages may become unreadable. Only reset if you can't use another device or your recovery key."; "Back" outlined at left, red "Reset identity" at right.
- `recovery-change-key-frost-dark-comfortable.png`: "Change your recovery key?", red box "Your current recovery key will stop working. ...", "Cancel" at left, red "Change key" at right.
- `recovery-confirm-identity-frost-dark-comfortable.png`: "Confirm your identity", teal primary "Use another device", outlined "Use recovery key", separator, "Lost your other devices and your recovery key?" then "Can't confirm? Reset your identity" text button, "Skip for now" at bottom right, close X.
Note: the plan says "Copy and Save buttons". The card has Show/Hide key and Copy key; I saw no Save button in any recovery screenshot. Recorded as a deviation from the plan wording for the human (Save-to-file is not in the rendered card).

## Verification is a modal with 3-step indicator; emoji grid of 7 large emoji with names; Match (primary) and Doesn't match (danger); explicit Cancel and Done; every phase has a gallery state
Status: PROVEN

Gallery scenarios written: verification-requested(+arming), -waiting, -ready, -qr, -scanned(+arming), -emojis(+arming), -decimals(+arming), -confirmed, -done, -cancelled(+timeout/us), -long-requested, -long-emojis, -own-requested/-emojis/-done.
- `verification-requested-frost-dark-comfortable.png`: title "@alice:example.org wants to verify", steps `1 Request > 2 Compare > 3 Done` (1 highlighted) with "Step 1 of 3: Request", peer ID block (badge "Another person", id in large monospace), warning text "Only accept a request you expected...", hint "Esc cancels the verification.", "Decline" outlined left, teal "Accept" right.
- `verification-emojis-frost-dark-comfortable.png`: "Compare the emoji", step 2 highlighted and step 1 ticked; seven emoji cards in one row, large glyph with name under each (Dog, Cat, Lion, Horse, Unicorn, Pig, Elephant); outlined info box "Check that these appear in the same order on the other side. If anything differs, choose They don't match: someone may be sitting between you."; "Cancel" text button right; below, red "They don't match" at far left and teal "They match" at far right (a full card width apart).
- `verification-emojis-arming-twicecat-light-comfortable.png`: same, but "They match" is greyed text (inert) and hint reads "Take a moment to look..."; "They don't match" is solid red and live. Twicecat light: red button is crimson, Match button blue when armed (`verification-decimals-twicecat-light-comfortable.png`).
- `verification-decimals-twicecat-light-comfortable.png`: "Compare the numbers", three large boxes `1234 5678 9012`, same warning, red "They don't match" left, blue "They match" right, Cancel above.
- `verification-cancelled-frost-dark-comfortable.png`: "Verification ended", no step highlighted ("Stopped"), dark-red box with X: "The codes did not match, so nothing was verified.", teal "Close".
- `verification-done-frost-dark-comfortable.png`: "Verified", all three steps ticked ("All 3 steps done"), green box with check "Verified. You can trust this person's messages are from their own devices.", teal "Done".
- `verification-long-emojis-frost-dark-compact.png`: long peer ID shows an ellipsised large form plus the full wrapped ID underneath; nothing cut off, buttons not covering it.
Related tests pass: `the_emoji_are_large_with_their_names_under_each_and_in_one_row`, `the_numbers_are_large_and_in_one_row`, `they_match_and_they_dont_match_are_a_card_apart_and_the_mismatch_is_red`, `the_three_steps_say_where_the_flow_is_in_words`, `the_peer_is_shown_prominently_with_the_end_of_a_long_id_and_no_button_covers_it`, `a_mismatch_ends_in_danger_and_the_other_endings_keep_their_words`, `the_red_buttons_are_red_and_cancel_is_not` (all in `security_screens`, 21 passed).

## Esc handling and 600 ms arming
Status: PROVEN (with mutation checks)

```console
$ sed -i '424s/ && !model.armed()/ && false/' crates/app/src/verification.rs      # (a) arming guard off
-        if matches!(message, M::Accept | M::QrConfirmed | M::Match) && !model.armed() {
+        if matches!(message, M::Accept | M::QrConfirmed | M::Match) && false {
$ cargo test -p twicecat-app --lib -- the_agreeing_buttons_wait
test state::tests::the_agreeing_buttons_wait_and_a_stale_timer_or_a_new_phase_never_arms_early ... FAILED
panicked at crates/app/src/state.rs:5775:9
test result: FAILED. 0 passed; 1 failed
$ (restored from copy; git status --short -> empty)

$ sed -i '622s/VerificationMessage::Cancel/VerificationMessage::Match/' crates/app/src/keys.rs   # (b) Esc agrees
-                VerificationMessage::Cancel
+                VerificationMessage::Match
$ cargo test -p twicecat-app --lib -- esc_on_the_emoji_card esc_on_the_other_cards
test state::tests::keyboard::esc_on_the_other_cards_cancels_and_never_accepts_or_confirms ... FAILED
test state::tests::keyboard::esc_on_the_emoji_card_cancels_and_never_confirms_a_match ... FAILED
test result: FAILED. 0 passed; 2 failed
$ (restored from copy; git status --short -> empty)

$ sed -i '442s/if revealed {/if revealed || true {/' crates/app/src/screens/security.rs      # (c) key always revealed
$ cargo test -p twicecat-app --test ui_gallery -- the_recovery_key_is_hidden the_key_is_in_the_monospace
test security_screens::the_recovery_key_is_hidden_until_it_is_shown_and_then_drawn_in_groups_of_four ... FAILED
panicked at crates/app/tests/ui_gallery.rs:4751:9
test security_screens::the_key_is_in_the_monospace_face_on_the_surface_colour_and_hides_with_the_toggle ... ok
test result: FAILED. 0 passed... 1 passed; 1 failed
$ (restored from copy, cmp identical; git status --short -> empty)
```
Each mutation was caught by a test; baseline (unmutated) ran green first. The first attempt at (a) used a filter (`verification`) that did not select the arming test and passed with 22 unrelated tests; re-run with the right filter failed as shown. Also: `ARMING = Duration::from_millis(600)` in `crates/app/src/verification.rs`; Esc source: `keys.rs on_escape` sends Cancel (or Close when over) and `security.rs security_escape` only cancels/backs out/is swallowed (key screen swallows Esc). Unit tests `esc_*` in `state.rs` (7) pass in the full lib run.

## No flow or command changes; existing tests pass unchanged
Status: PROVEN

```console
$ git diff b103b10..c7ddecf --stat -- crates/core | wc -l
0
$ cargo test -p twicecat-app --test ui_gallery
test result: ok. 121 passed; 0 failed; 1 ignored   (ignored = write_the_gallery)
$ cargo test -p twicecat-app --lib
test result: ok. 1239 passed; 0 failed
```
No change under `crates/core`. Removed lines in `state.rs`/`security.rs`/`keys.rs`: 0. In `ui_gallery.rs` the removed lines are test helper fns (`senders`, `escape`, `texts_of`, ...) that moved/shared, not test deletions (not audited line by line).

## The commit carries `Step: ui.16`
Status: PROVEN
```console
$ git log -1 --format=%B | tail -4
Step: ui.16
Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>
```

## Real verification between two accounts / native clipboard
Status: NOT PROVEN (human checkpoint — needs two live accounts and the OS clipboard)

## Repo hygiene
```console
$ scripts/check-no-webrtc-download.sh; echo "exit=$?"
exit=0
$ git status --short
(empty before writing this file)
```

## Defects / notes
1. Plan says recovery modal has "Copy and Save buttons"; rendered card has Show/Hide key and Copy key only, no Save.
2. The Danger "identity changed" banner has no gallery scenario (gallery banners: device, notice, recovery-off, recovery-incomplete); not seen rendered.
3. Minor: the "banner-notice" severity is Danger (red) for a network error, fine; the ended-by-mismatch card shows "Close" rather than "Done" (Done appears only on success). Step chips show none highlighted on a stopped flow (labelled "Stopped").
4. The gallery tool: `TWICECAT_GALLERY_ONLY` takes a single prefix (comma lists silently write nothing).

## Verdict
All outside-observable criteria were demonstrated; the Save-button deviation is flagged for the human.
