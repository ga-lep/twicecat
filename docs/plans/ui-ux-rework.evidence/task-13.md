# Evidence — Task 13: Login, browser sign-in and loading screens
Commit: 8ba4a87
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir, headless iced renderer gallery (no real window) written to a scratch dir outside the repo (TWICECAT_GALLERY_DIR=<scratchpad>/gal, 118 s, all themes/densities, 224 login/waiting/loading/starting/closing/stopped PNGs, all 1280x800). classify_server was run from a temporary untracked example in crates/app/examples (deleted afterwards; git status clean). Real-window focus, Tab, typing and a real sign-in stay human.

## Test runs (real output, trimmed to result lines)
```console
$ cargo test -p twicecat-app --test ui_gallery
test result: ok. 46 passed; 0 failed; 1 ignored (write_the_gallery); finished in 42.21s
  incl. login::the_card_is_420_wide_and_centred_and_nothing_runs_out_of_it, login::tab_walks_the_inputs_in_the_order_of_the_focus_function,
  login::enter_submits_from_every_field_only_when_the_form_is_complete, login::enter_does_nothing_while_the_server_hides_behind_userinfo,
  login::while_signing_in_the_fields_and_every_button_do_nothing, login::the_waiting_card_offers_copy_link_only_after_a_failed_launch_and_never_shows_the_url,
  login::the_waiting_buttons_sit_on_a_row_under_the_words, login::a_long_server_shows_whole_on_lines_of_its_own_under_the_field (1280/720/360 wide),
  login::the_disclosure_opens_on_a_click_and_stays_open_for_a_server_or_a_server_notice, login::the_disclosure_button_is_off_while_the_field_is_held_open
$ cargo test -p twicecat-app --lib
test result: ok. 1205 passed; 0 failed
  incl. screens::login::tests::* (18, incl. the_focus_order_is_user_password_log_in_browser_disclosure_then_server, only_the_text_inputs_take_tab_in_this_iced,
  an_address_with_userinfo_is_hidden_not_echoed, hidden_characters_in_the_input_are_refused_not_echoed, plain_ascii_servers_and_the_dev_homeserver_stay_targets,
  the_server_echo_names_the_host_that_is_contacted, a_long_host_is_cut_in_the_middle_and_its_last_label_stays_visible),
  screens::loading::tests::* (3: sync_labels_name_every_state, progress_steps_with_the_sync_and_stays_between_0_and_1, room_count_label_pluralises),
  state::tests::an_address_that_hides_its_server_sends_nothing_and_keeps_the_password
$ scripts/check-no-webrtc-download.sh; echo exit=$?
exit=0
```
Status: PROVEN (unit/integration level)

## classify_server / server_echo table (real output of a temporary example calling the public API)
```console
$ cargo run -q -p twicecat-app --example tmp_classify      (long inputs shortened by me in the label column only)
matrix.org                      -> Target("matrix.org")                 echo: Signs in with the server of matrix.org
matrix.org:8448                 -> Target("matrix.org:8448")            echo: Signs in with the server of matrix.org:8448
localhost:8008                  -> Target("localhost:8008")             echo: Signs in with the server of localhost:8008
127.0.0.1:8008                  -> Target("127.0.0.1:8008")             echo: Signs in with the server of 127.0.0.1:8008
http://localhost:8008           -> Target("http://localhost:8008")      echo: Signs in with the server of http://localhost:8008
https://matrix.example:8448/    -> Target("https://matrix.example:8448") echo: Signs in with the server of https://matrix.example:8448
[::1]:8008                      -> Target("[::1]:8008")                 echo: Signs in with the server of [::1]:8008
https://matrix.org+300 'a'+@evil.example -> Hidden                      echo: This address hides its real server. Enter just the server name.
https://m<U+0430>trix.org (Cyrillic a)   -> Hidden                      (same HIDDEN_SERVER echo)
m<U+0430>trix.org (bare)                 -> Hidden                      (same)
https://matrix.org<U+2044>login.evil.com -> Hidden                      (same)
https://matrix.org<U+2215>x.evil.com     -> Hidden                      (same)
https://matrix.org%2eevil.com            -> Hidden                      (same)
0x7f.1                                   -> Hidden                      (same)
ftp://evil                               -> Hidden                      (same)
matrix.org.   (trailing dot)             -> Hidden                      (same)
my_synapse    (underscore)               -> Hidden                      (same)
bücher.de     (IDN)                      -> Hidden                      (same)
https://matrix.org@evil.example          -> Hidden                      (same)
```
Status: PROVEN. Allow-list holds: every look-alike/userinfo/odd-scheme/numeric-host input is Hidden and the echo never names a destination; ordinary names, ports, IPv6 and the dev homeserver (localhost:8008, 127.0.0.1:8008, http://) stay Target; the path is dropped from the echo. Echo text is exactly "Signs in with the server of X".
Finding (not a failure of a criterion, a usability note): `matrix.org.` (trailing dot), `my_synapse` (underscore) and IDN names such as `bücher.de` are refused as hidden; the user must type the punycode/ASCII name. This is the stated conservative design, but the banner text ("hides its real server") is misleading for these honest inputs.

## Hidden/look-alike address: nothing sent, password kept
Status: PROVEN (unit)
state::tests::an_address_that_hides_its_server_sends_nothing_and_keeps_the_password passes: for the 300-char userinfo URL, the userinfo URL and all 14 LOOKALIKES, Log in and Sign in with browser send no Command, login_in_flight stays false, notice == HIDDEN_SERVER, password still "hunter2-x". Gallery gap: there is no PNG of the hidden-server banner (no such scenario in the fixture).

### Mutation checks (edit real tree, restore from a copy, git status clean afterwards)
```console
M1: allow '@' and '%' in the host charset of plain_authority
$ cargo test -p twicecat-app --lib screens::login
test screens::login::tests::an_address_with_userinfo_is_hidden_not_echoed ... FAILED
test result: FAILED. 18 passed; 1 failed   (left: Target("https://matrix.orgaaaa...@evil.example"))
M2: delete the refuse_hidden_server() guard from submit_login
$ cargo test -p twicecat-app --lib an_address_that_hides
panicked at crates/app/src/state.rs:3215:13: password login sent a command
test result: FAILED. 0 passed; 1 failed
M3: swap User and Password in focus_order
$ cargo test -p twicecat-app --lib focus_order
test screens::login::tests::the_focus_order_is_user_password_log_in_browser_disclosure_then_server ... FAILED
$ git status --short        (after restoring all three files from copies)
(empty)
```
Status: PROVEN (each guard is caught by its test)

## Login is a centred 420 px card: Twicecat in DISPLAY + tagline, fields, Log in (primary), "or", Sign in with browser (secondary), server behind "Use another server"
Status: PROVEN
Images read: login-frost-dark-comfortable.png, login-twicecat-light-comfortable.png (via -busy/-server-open). Centred card, about 420 px wide: large "Twicecat" title (Inter SemiBold), tagline "Chat and voice calls on Matrix.", User name and Password fields, filled "Log in" (teal in Frost dark with dark label, indigo with white label in Twicecat light; both legible), "or" divider with rules, outlined "Sign in with browser", then a collapsed "> Use another server". Card test the_card_is_420_wide_and_centred passes at all sizes.

## Disclosure opens on its own when the field is not empty or the notice is about the server
Status: PROVEN
login-server-frost-dark (field holds a long https URL, open, disclosure shown dimmed with "-" mark), login-server-notice-frost-dark (empty field, banner "Enter a server name to sign in with the browser.", field open with "Leave it empty to use the server of your user ID."), login-server-open-twicecat-light (opened by hand). Tests: the_disclosure_opens_on_a_click_and_stays_open..., the_disclosure_button_is_off_while_the_field_is_held_open.

## Server echo with the end visible
Status: PROVEN
login-server-frost-dark: "Signs in with the server of https://a-very-long-homeserver-name.and-a-longer-subdomain.example.org:8448" wraps onto two lines under the field inside the card and the ends (".example.org:8448") are visible. 720 and 360 wide are NOT in PNG form (gallery draws login at 1280 only); they are covered by a_long_server_shows_whole_on_lines_of_its_own_under_the_field (echo several lines tall, inside the field's x range at 1280, 720 and 360) and unit a_long_host_is_cut_in_the_middle_and_its_last_label_stays_visible (400-char host is elided in the middle with U+2026, ends with ".example.org:8448").

## Tab order user -> password -> Log in -> browser -> disclosure (-> server), on a pure function; Enter submits from every field
Status: PROVEN (function and event-level tests); real Tab/Enter in a window stays human
focus_order(server_open) unit test passes and M3 breaks it. only_the_text_inputs_take_tab_in_this_iced documents that iced buttons are not Tab stops, so Tab visits only the inputs in this order. tab_walks_the_inputs_in_the_order_of_the_focus_function and enter_submits_from_every_field_only_when_the_form_is_complete pass.

## Notice is a banner; in flight the fields are off and the button reads "Signing in…"
Status: PROVEN
login-error-twicecat-light: red banner "action not allowed" above the fields. login-busy-twicecat-light: user, password and server fields greyed, primary button greyed and reading "Signing in..." (rendered as three dots in the image; the source string is the U+2026 ellipsis, which the headless font may draw like that), "Sign in with browser" faded. Test while_signing_in_the_fields_and_every_button_do_nothing passes.

## Waiting for the browser is a card: status line, Copy link only after failure, Cancel; URL never shown
Status: PROVEN
waiting-opening-twicecat-light: card "Continue in your browser", "Opening your browser...", Cancel only. waiting-failed-frost-dark: warning banner "The browser could not be started. Copy the link and open it yourself." then Copy link and Cancel side by side. No URL appears. Test the_waiting_card_offers_copy_link_only_after_a_failed_launch_and_never_shows_the_url passes.
Minor visual: Cancel has no visible border (ghost style), which is the intended secondary look but reads as plain text next to Copy link.

## Loading is a card with sync label and progress_bar; Starting/Closing/Stopped are empty_state
Status: PROVEN
loading-frost-dark: card "Loading your rooms...", filled teal bar at about 60 %, "Sync: Running", "0 rooms loaded". loading-reconnecting-twicecat-light: bar at about 30 %, "Sync: Reconnecting (attempt 2)". starting-twicecat-light: centred "Starting..." with "Looking for your saved session."; closing-frost-dark: "Closing..." / "Saving your drafts and settings."; stopped-frost-dark: "The core stopped" / "Close the window and start Twicecat again." Titles and detail are small, centred and legible, with no icon or card. Unit progress/label tests pass.

## Commit carries `Step: ui.13`
Status: NOT PROVEN (commit-message check, not run here)

## Defects / notes
- No hidden-server banner PNG and no 720/360 PNGs for login; those cases rely on tests.
- Honest inputs `matrix.org.`, `my_synapse`, `bücher.de` are refused with a message that says the address "hides its real server".
- No contrast or overlap defect seen in the 1280x800 Frost dark and Twicecat light images read.

## Verdict
EVIDENCE: PROVEN
