# Evidence — ui-ux-rework, whole feature (final)
Commit: 6a16d0d
Environment: Linux, CC=clang-22 CXX=clang++-22, repo's own `target/` dir, `.cargo/config.toml` untouched (LK_CUSTOM_WEBRTC kept). Headless only: no window, no local homeserver, no audio.
Gallery scratch dir: the session scratchpad `gallery/` (outside the repo).

## 1. Full gate on the finished branch
Status: PROVEN
```console
$ cargo fmt --all --check            -> exit=0
$ cargo clippy --workspace --all-targets -- -D warnings   -> Finished dev profile, exit=0
$ cargo check --workspace --lib --bins --locked           -> Finished dev profile, exit=0
$ cargo test --workspace             -> exit=0, 2076 passed, 0 failed, 152 ignored (sum of all "test result" lines)
```
Per-target totals (passed/ignored; all 0 failed). Ignored tests are the homeserver/audio/GPU ones that need a stack.
- twicecat-app lib 1260/0; bin twicecat 6/0; tests: bridge 1/3, calls 0/1, menus_ignore_the_wheel 3/0, no_secret_logging 2/0, ptt 2/0, timeline_layout 8/0, ui_gallery 126/1
- twicecat-audio lib 69/3; deepfilter 0/4; onnxruntime 0/0; pipewire 0/6; example voice_profile 1/0
- twicecat-core lib 504/2; bin twicecat-cli 36/0; avatars 0/1, call_media 0/8, call_ringing 0/6, call_settings 6/1, call_signalling 0/3, cli 1/2, e2ee_devices 0/5, e2ee_identity 0/7, e2ee_recovery 0/7, e2ee_rooms 1/6, e2ee_verification 0/9, gpu_enhancement 0/0, livekit_link 2/1, log_hygiene 0/1, login 0/4, login_policy 5/0, media 0/8, notifications 0/7, oauth 0/1, room_list 0/8, rooms 0/14, rtc_stack 0/3, threads 0/8, timeline 0/3, timeline_actions 0/12, timeline_items 0/3, timeline_live 0/4, timeline_items, tls_provider 1/0; example opus_profile 6/0
- twicecat-ptt lib 11/0
- Doc-tests: app 12, audio 0, core 13, ptt 0
```console
$ scripts/check-app-deps.sh              exit=0
$ scripts/check-core-api.sh              exit=0
$ scripts/check-font-assets.sh           OK for 5 files, exit=0
$ scripts/check-no-webrtc-download.sh    exit=0   (also re-run at the very end: exit=0)
$ scripts/check-commits.sh main HEAD     exit=0
$ git log --oneline origin/main..HEAD | grep -E 'feat\(|docs\(app\)'
6a16d0d docs(app): accessibility document and the UI/UX rework checkpoint list
fc2df1d feat(app): make Frost the default colour set
e241470 feat(app): security banners, recovery and verification as cards
c7593b0 feat(app): room dialogs, room settings, members, upload, viewer and switcher as cards
c8a303a feat(app): Settings as pages and Sessions as a card with trust tags
10c7567 feat(app): login, browser sign-in and loading screens as cards
53d65ec feat(app): Voice & audio card as pages with push-to-talk and GPU status
8066763 feat(app): call dock, participants panel and ring card
0a953c9 feat(app): composer field with icon buttons, banners and cards for the bars above it
ab71da6 feat(app): timeline restyle: tokens, kit chips and quotes, icon action bar
924ba15 feat(app): narrow windows: forced rail, panels in the room pane, stacked header
20f0b2a feat(app): redesign the room list with densities and a collapsible rail
523ff31 feat(app): main window frame: header, panes and status bar
942366b feat(app): headless UI gallery for every screen
4332f03 feat(app): theme families, Appearance settings and zoom
6a58698 feat(core): store the appearance choices in the local settings
e7969ed feat(app): bundle the Inter typeface as the default font
6695175 feat(app): add design tokens, an icon module and a component kit
```
That is 18 commits (17 feat plus the ui.18 docs(app) commit).
Trailer counts: `git log --format=%s%n%b origin/main..HEAD | grep 'Step: ui\.'` counts are 2-4 per step because the branch also holds ledger/evidence docs commits with the same trailer (71 commits on the branch in total, 19 distinct `Step: ui.0` .. `ui.18` values, `git log --format=%B | grep -c '^Step: ui\.'` = 71). Every step ui.0..ui.18 appears. Note: the plan's "18 step commits" is 18 feat/docs(app) commits listed above.

## 2. Non-test app build, run
Status: PROVEN (debug profile, no release build started)
```console
$ cargo build -p twicecat-app            Finished dev profile (already built)
$ target/debug/twicecat --help           exit=0   (usage: [--data-dir <path> | --ephemeral] [--homeserver <url>], --help)
$ target/debug/twicecat --version        twicecat: unknown argument: --version   (no --version flag; not a plan criterion)
```
The GUI itself was NOT started (no display, no homeserver): NOT PROVEN, see section 5.

## 3. Gallery end to end
Status: PROVEN
```console
$ TWICECAT_GALLERY_DIR=<scratch>/gallery cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok     test result: ok. 1 passed ... finished in 201.94s
$ ls <scratch>/gallery | wc -l       2945 (stale files included, see below); target/ui-gallery: 2881 (2880 PNG + index.html)
```
Count: the reproducible count is 2881 files (2880 PNG + index.html), matching PROGRESS.md and `target/ui-gallery`. My scratch directory was not empty at the start, so it showed 2945: 64 stale PNGs (voice-audio-*, voice-audio-diagnostics-*, voice-audio-looking-*, voice-audio-no-gpu-*, 16 each) from an earlier partial run, dated 04:42 against 15:36 for this run's index.html. The current scenario list does not produce those names (`comm` against target/ui-gallery: 64 only in scratch, 0 only in target). There is no discrepancy.
index.html: `grep -c '<script'` = 0; `grep -c 'https\?://'` = 0; every `src`/`href` is a local `*.png`; 152 `<table>`s, each with a header row `twicecat | frost | ember | high-contrast` (one column per family) and rows light-comfortable, light-compact, dark-comfortable, dark-compact.

Inspected with the Read tool (1280x800 unless noted).
Frost dark, comfortable (default):
- login: centred card, "Twicecat" heading, two labelled fields, solid teal "Log in", "or" rule, "Sign in with browser", "Use another server" disclosure. Clean.
- main-room: space rail, room list (filters All/DMs/Rooms/Spaces, search, rows with avatars, lock marks, unread pills, Muted/Voice/Invite chips), timeline (file/audio cards, thread link, sending bar with Cancel, failed message with Retry/Discard, "New messages" rule), composer with attach, emoji and send icons, status bar. Teal selected row.
- main-narrow-room (720 wide): list collapsed to a rail, header stacked to two lines (name + Encrypted chip, then dropdown/people/settings/Call), timeline and composer fill the pane.
- main-call-connected: dock in the list column ("Voice connected, encrypted", lock chip, mic outlined green, volume, settings, red Leave), header shows red "Leave call" and a people toggle, participants panel "In the call (4)" with Speaking, Muted, "Voice cleaned", "Voice cleaning starts...".
- ring-dm: dimmed app behind a card "Alice Example is calling", Decline and Accept below, hint "Answer in a moment..." (arming), Esc/keys line. Row shows red "Ringing" pill.
- audio-talking: Voice & audio card with left page list (Devices ... Diagnostics), banner "Send mode: Push-to-talk (hold F13 to talk)...", Send dropdown, Key F13 with Set key/Clear, gamepad, release delay slider 200 ms.
- settings-appearance: pages list, Follow the system/Light/Dark radios, 4 colour tiles with swatches, "Frost (default)" ticked and outlined, Use the default, Density, Room list, Text size 100% with Ctrl+=/-/0 hint.
- sessions: "This device" + "Verified" tags, "Not verified" and "No encryption keys" tags, Verify/Rename/red Sign out, "Sign out of all other sessions".
- members: 5 members grouped Admins/Moderators/Members/Invited, Creator tag, search and invite fields.
- upload: "Send 4 files", To Lounge + Encrypted, per-file rows incl. a red error and a "Reading..." row, Cancel and a greyed Send.
- verification-emojis: stepper "1 Request > 2 Compare > 3 Done", other person ID in mono, 7 emoji with names, warning text, red "They don't match" and teal "They match", Esc hint.
- recovery-key: key shown as `**** **** **** ****` x3, Show key / Copy key, "cleared after 60 seconds", checkbox "I've saved my recovery key", Done. Key hidden as required. Recovery-off banner at the top.
Ember light, comfortable: main-room, main-call-connected, ring-dm, recovery-key, main-narrow-call (720): same layout in warm cream/orange palette; rail layout in the narrow call shows the dock collapsed to icons (check, lock, people, mic, volume, red door) with the participants panel taking the pane. Text contrast is good in the inspected shots.

Visible defects / nits (none blocks a criterion):
- Ring card (Frost dark and Ember light): Decline and Accept are plain text with a small glyph, not filled buttons, and while arming they look identical to a normal state; Accept has no emphasis. Reads as flat/low affordance.
- Recovery key card: the "I've saved my recovery key" checkbox is almost invisible in Frost dark (box has nearly no outline against the card) and faint in Ember light; "Done" disabled text is low contrast (intended, but hard to tell from enabled).
- Participants panel: Cy's muted-mic mark is a tiny glyph (about 8 px) floating between the name and the "..." button; in the 720 call shot it is also tiny.
- Room list rail (720): lock and call marks on avatars are ~8 px, hard to read; a stray selected-row accent bar sits left of the avatar.
- Sessions card: third row is clipped by its scroll area ("Sign out" half visible) because the list scrolls; expected but the cut looks abrupt.
- Header "Use default" dropdown (call-device selector) sits in the room header beside Call with no label; its meaning is unclear.
- Dim overlay behind the cards is strong in Frost dark; the dock under the Voice & audio card remains readable. No overlaps found at 1280 or 720.
- All of this was rendered by the headless renderer; wgpu rendering may differ.

## 4. Safety-critical spot checks (real output)
Status: PROVEN
```console
$ cargo test -p twicecat-app --test menus_ignore_the_wheel
test every_pick_list_is_wrapped_by_the_wheel_guard ... ok
test every_slider_is_wrapped_by_the_wheel_guard ... ok
test the_scan_tells_a_wrapped_widget_from_a_bare_one ... ok     3 passed
$ cargo test -p twicecat-app --lib escape       (15 passed) includes
keys::tests::escape_and_tab, keys::tests::a_held_escape_acts_once,
screens::room_dialogs::tests::escape_closes_what_is_drawn_on_top_and_a_question_before_its_panel,
audio_settings::tests::escape_follows_the_layers_over_the_card, calls::tests::escape_hides_the_card_and_silences_the_ringtone,
state::tests::rooms_ui::escape_closes_the_topmost_screen_first, calls::tests::escape_closes_the_participants_panel
$ cargo test -p twicecat-app --test ui_gallery escape
test dialogs::escape_closes_the_layer_that_is_drawn_on_top_and_only_that_one ... ok
$ cargo test -p twicecat-app --lib verification   (22 passed) includes
state::tests::keyboard::esc_on_the_verification_card_closes_nothing_under_it ... ok
$ cargo test -p twicecat-app --lib arming          (3 passed)
a_new_card_is_inert_until_its_arming_timer_ends, a_stale_arming_timer_does_not_arm_the_card, arming_starts_again_for_each_ring_the_card_changes_to
$ cargo test -p twicecat-app --lib chord           (7 passed)
a_held_chord_that_answers_one_ring_never_answers_the_next, losing_the_window_lets_go_of_the_held_chord,
the_held_chord_is_let_go_when_the_key_capture_dialog_opens_or_closes, a_held_ring_chord_repeats_nothing_but_other_shortcuts_still_repeat (keys), letting_go_of_the_chord_is_a_message
$ cargo test -p twicecat-app --lib screens::login::tests   (21 passed) includes
honest_names_the_allow_list_refuses_get_an_accurate_notice_not_hides_its_server, hidden_characters_in_the_input_are_refused_not_echoed,
userinfo_is_refused, plain_http_is_for_this_computer_only (http://localhost:8008, 127.0.0.1, [::1] accepted; http://localhost.evil.com, 192.168.0.5, HTTP://example.org refused),
plain_ascii_servers_and_the_dev_homeserver_stay_targets, an_address_with_userinfo_is_hidden_not_echoed
$ composer inert: composer::tests::an_inert_editor_passes_only_shortcuts ... ok;
state::tests::keyboard::room_shortcuts_wait_while_an_overlay_is_open ... ok;
state::tests::keyboard::the_switcher_does_not_open_over_another_overlay_or_off_the_main_screen ... ok;
ui_gallery the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go ... ok
$ rail dock mic looks: screens::call::tests::no_two_microphone_looks_share_a_glyph_and_a_fill ... ok;
an_open_microphone_is_outlined_and_not_filled_like_muted ... ok; an_open_microphone_in_voice_activity_is_the_open_glyph ... ok
$ cargo test -p twicecat-app --lib accessibility_doc_lists_every_shortcut
test keys::tests::accessibility_doc_lists_every_shortcut ... ok
```
Note: I did not find one test literally named "composer inert under overlay"; the closest names are above (composer inert editor, shortcuts wait under an overlay). The classifier look-alike coverage is the honest_names/userinfo/hidden-character tests (no test is named "look-alike"). I did no mutation of tracked files; `git status --short` printed nothing before writing this file.

## Per-task Evidence lines
- Task 1 (kit): none. Task 2 fonts: check-font-assets OK for 5 files; app font tests are in the 1260. GUI login font = human.
- Task 3 (core settings): core lib tests pass (504). `paths::` named tests are inside it.
- Tasks 4 (Settings), 5 (gallery): PNGs inspected; gallery count above. Ctrl+, in a real app = human.
- Tasks 6-10, 12-16: gallery PNGs exist for each family and were inspected as above; the in-app items are human checkpoints.
- Task 11 (calls): dock/panel/ring PNGs inspected; real calls via `scripts/stack.sh` NOT run.
- Task 17: Frost default: Appearance PNG shows "Frost (default)"; fresh-data-dir run of the app = human.
- Task 18: check-commits exit=0; accessibility test passes.

## 5. NOT PROVEN
- Real window, real GPU rendering (wgpu vs the tiny-skia renderer used for the gallery), fonts in a live window.
- Real sign-in (password/browser), real homeserver, `.env.test` accounts, `scripts/stack.sh`: not started.
- Real calls and audio (speaking rings, how it sounds, PTT per OS, global hotkeys, gamepad), key-capture dialog, Wayland portal.
- Real verification between two accounts, real recovery key.
- Native file pickers, drag-and-drop, clipboard clearing after 60 s.
- Ctrl+, / Ctrl+B / Ctrl+K / Ctrl+= in a live window, window-drag narrowing during a call, focus behaviour of the hidden composer.
- Ignored (152) homeserver, audio and GPU tests were not run.

## Verdict
EVIDENCE: PROVEN (headless-observable parts; section 5 is human-only; gallery count reproduces as 2881)
