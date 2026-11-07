# Evidence — Whole feature: sidebar customization (docs/plans/sidebar-customization.md)
Commit: 0e270a7
Environment: CC=clang-22 CXX=clang++-22, cargo run from inside the repo. Local stack started with `TWICECAT_LK_JWT_PORT=18080 scripts/stack.sh up -d --wait`, `scripts/create-test-users.sh` (.env.test accounts only), stopped afterwards with `scripts/stack.sh stop` (`docker ps` then lists only the unrelated peppy-client container; it was never touched). The scratch test was appended to crates/app/src/sidebar.rs, run, and removed with `git checkout` (`git status --short` was empty before this file was written). Gallery PNGs are outside the repo in /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-final/. The GUI was not started.

## Goal chain, headless: SidebarModel + the real DiskSidebar on a temp data root (items 1 to 6)
Status: PROVEN (model and disk level; the App `update` paths and the real window are not part of this scratch test, see NOT PROVEN list)
Scratch test `sidebar::zz_scratch::scratch_flow`: a store wrapper ran `DiskSidebar::load_job/save_job/reset_job` synchronously; the model, Ops, file logic and files are the shipped ones. Account IDs `@a:x`, `@b:x`, plus `@c:x` (version 2 file) and `@d:x` (file naming `@other:x`).

```console
$ cargo test -p twicecat-app --lib zz_scratch -- --nocapture
running 1 test
== (1) a builds a layout
state after login (no file): Loaded; file exists: false
apply MoveToFolder { folder: New { named: true } } -> ok=true
apply MoveSpace { dir: Up } -> ok=true
apply ToggleSection { .. } -> ok=true
apply NewSection { with_room: true } -> ok=true
apply Star { .. } -> ok=true
model layout: SidebarLayout { rail: 3, spaces: 1 }
== (2) file sidebar-82c93996fd659248.json
{"account":"@a:x","rail":[{"folder":{"id":0,"name":"Work","open":false,"spaces":["!s1:x"]}},{"space":"!s3:x"},{"space":"!s2:x"}],"spaces":[{"collapsed":["dms"],"custom":[{"id":0,"name":"Ops","rooms":["!room1:x"]}],"order":[],"space":"!s2:x","starred":["!room2:x"]}],"version":1}
(no room/space display names present; labels Work/Ops are the user's own section names)
== (3) restart and logout/login
after logout: NotLoaded, file kept sha=true
restart (new store+model): state Loaded, layout equal = true
logout/login: layout equal = true
== (4) b starts from default
b state Loaded, equals default: true, files: ["sidebar-82c93996fd659248.json"]
files: ["sidebar-79ea1e96f850a74d.json", "sidebar-82c93996fd659248.json"]
a sha before 2b1aa7b1ef05c03f0cd88155ffa0fb7d3b086320b4e838b991b5e034f6f5b526
a sha after  2b1aa7b1ef05c03f0cd88155ffa0fb7d3b086320b4e838b991b5e034f6f5b526
b sha        c8b4a505b63c3875e0a8693e761c4c25ad8ce68e1857baa29532f3a84369b796
== (5) protected files
state Newer; apply -> Some(Protected); reset -> Some(Protected); read "Newer"; sha unchanged true
state OtherAccount; apply -> Some(Protected); reset -> Some(Protected); read "OtherAccount"; sha unchanged true
files: ["sidebar-697dc096ef44275b.json", "sidebar-70e0eb96f316db76.json", "sidebar-79ea1e96f850a74d.json", "sidebar-82c93996fd659248.json"]
== (6) reset as b
reset ok=true
b file exists: false; a sha same: true; c,d same: true; files: ["sidebar-697dc096ef44275b.json", "sidebar-70e0eb96f316db76.json", "sidebar-82c93996fd659248.json"]
test sidebar::zz_scratch::scratch_flow ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1709 filtered out; finished in 0.07s
```
(1) the Ops (new folder from a space, move space up, collapse a section, new custom section with a room, star) were accepted and no file existed before the first change. (2) The file holds only keys, the account, room/space IDs and the user's own folder/section labels; no room or space display name. (3) A new store and model, and a logout/login, give back an equal layout; logout kept the file (same sha). (4) b starts as the default, writes its own file; a's sha256 is identical before and after. (5) The version-2 and other-account files are Protected for apply and reset and unchanged by sha256. (6) Reset as b removes only b's file; a, c and d are unchanged.

## Task 1 Evidence (core paths and api::sidebar) — `cargo test -p twicecat-core --lib paths:: api::sidebar::`
Status: NOT PROVEN (not re-run here; it is part of the `cargo test --workspace` run below, 0 failures)

## Task 2 Evidence — `cargo test -p twicecat-app --lib sidebar`; no `sidebar-*.json` is created at login
Status: PROVEN for the test part (see the hinted lib run below); the "cargo run on the local homeserver creates no file" part is NOT PROVEN (real window not started; the scratch run shows "file exists: false" after login with no change)

## Integration tests against the local stack (spaces, level 0 / level 1 rooms; space_unread; the call list)
Status: PROVEN (the plan's twicecat-cli lines were replaced by the integration tests the caller asked for; the tests assert the same facts)
```console
$ cargo test -p twicecat-core --test spaces --locked -- --ignored --nocapture
created space as user 1, 1 children
created space as user 1, 3 children
test a_space_view_lists_its_rooms_by_level ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 21.37s

$ cargo test -p twicecat-core --test notifications --locked -- --ignored --nocapture
test space_unread_follows_a_message_in_a_room_of_the_space ... ok
test unread_totals_follow_a_message_and_mark_read ... ok
test an_invite_notifies ... ok
test own_messages_never_notify ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 68.15s

$ cargo test -p twicecat-core --test call_ringing --locked -- --ignored the_room_list_learns_who_is_in_a_call
test the_room_list_learns_who_is_in_a_call ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10.17s
```
Fresh stack, all pass. Only the tail of the first two outputs was kept (8 tests ran in notifications; 4 names visible in the trimmed tail). The `space level=.. rooms=..` and `active-call participants=1 members=1` CLI lines of the plan were not run; the tests above cover the same behaviour.

## Unit tests hinted by the plan
Status: PROVEN
```console
$ cargo test -p twicecat-app --lib -- sidebar sections spaces drag rail_drop list_drop calls
test result: ok. 391 passed; 0 failed; 0 ignored; 0 measured; 1318 filtered out; finished in 0.30s
```

## Docs tests (Task 13)
Status: PROVEN
```console
$ cargo test -p twicecat-app --test docs_sidebar
test the_keyboard_map_of_accessibility_gets_no_sidebar_row ... ok
test accessibility_names_the_options_menus_and_says_they_need_a_pointer ... ok
test sidebar_md_names_the_menu_entries_the_app_has ... ok
test sidebar_md_claims_no_sync_and_no_removal_at_logout ... ok
test sidebar_md_says_where_the_layout_is_and_for_whom ... ok
test sidebar_md_states_the_caps_the_core_enforces ... ok
test the_docs_do_not_promise_what_the_sidebar_does_not_do ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Gallery (rail-, space-sections, voice-channel, section-menu, settings-appearance-sidebar)
Status: PROVEN (the PNGs are written and look as described; layout correctness of a real window is NOT PROVEN)
```console
$ TWICECAT_GALLERY_DIR=<scratchpad>/ui-gallery-final TWICECAT_GALLERY_ONLY=<each of the five> cargo test -p twicecat-app --test ui_gallery -- --ignored
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out   (x5: rail-, space-sections, voice-channel, section-menu, settings-appearance-sidebar)
$ ls ui-gallery-final | wc -l   -> index.html plus PNGs: rail-states, rail-dragging(-into), rail-folder-name, rail-menu, space-sections (+collapsed, custom, dragging, -header, -line, -refused), voice-channel, section-menu, settings-appearance-sidebar (+ask, newer, other), each in twicecat/daylight x comfortable/compact (1280 x 800)
```
Images read (all 1280 x 800), in `.../ui-gallery-final/`:
- `rail-states-twicecat-comfortable.png`: a dark theme. A narrow left rail has a Home circle with a "7" badge, circles with initials (H, HH, QL, B with "3", selected SH drawn as a rounded square with a light ring), then two folder tiles showing small 2 x 2 previews (one with "99+", one with "1"), then an open-folder block with two circles (G, K with "1"). Small white pills sit at the left edge at different heights (small, taller, tallest by the selected one). Next to it the room list shows a "Channels" header with "..." and rows (Lounge with a "3" badge, Design review "2", Build alerts Muted, Voice hangout with a Voice chip and "2", Book club Invite). One thing I noticed: the 99+ badge sits over and hides part of the folder preview tile's lower right square. Hover, tooltip and mention colours cannot be judged from a still.
- `space-sections-twicecat-comfortable.png`: the rail has Home and one selected "TH". The list has section headers with a triangle and "..." at the right: Starred (tagged "personal", one room Roadmap with a dot), Channels (General selected, Random, Announcements), Tech (Rust help 2, DevOps 3, Async questions), Hobbies (Board games 4...). Header text is small and muted; rows are about 64 px tall, so the list is not as dense as the plan's 28 px channel-row figure.
- `voice-channel-twicecat-comfortable.png`: collapsed headers Starred (dot), Channels "3", Tech "2", Hobbies "11" show rolled-up counts. The Voice section is open with rooms "vocal" (chip Voice, 3) and "coworking" (1); under vocal: Rhaenyra (avatar with a ring, the speaking look), Septimus (a red muted-mic icon), galep; under coworking: corin. A "Voice connected, encrypted" call dock is at the bottom, and Direct messages below.
- `section-menu-twicecat-comfortable.png`: a modal card titled "Random" over a dimmed app with buttons Star, Move up (dimmed), Move down, Back to its default section, and a "Move to section" dropdown "Choose a section...", plus Close. Behind it the list has a user section "Reading list" with Random and Announcements and an empty "Later" section reading "Drop or move rooms here".
- `settings-appearance-sidebar-twicecat-comfortable.png`: Settings, Appearance page: theme picker, text size, then a line "The sidebar layout (folders, sections, starred rooms) is kept on this device only." and a "Reset sidebar layout..." button.
- (the other gallery files, for example rail-dragging, were listed but not opened.)

## Gate
Status: PROVEN
```console
$ cargo fmt --all --check ; echo $?
0
$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.96s    (exit 0)
$ cargo test --workspace   (exit 0; the "test result" lines summed over 63 binaries)
passed=2847 failed=0 ignored=181
   (app lib: 1709 passed; ui_gallery: 143 passed, 1 ignored; docs_sidebar 7 passed; menus_ignore_the_wheel 4 passed)
$ scripts/check-app-deps.sh              -> no output, exit 0
$ scripts/check-core-api.sh              -> no output, exit 0
$ scripts/check-font-assets.sh           -> OK for 5 files, exit 0
$ scripts/check-no-webrtc-download.sh    -> no output, exit 0
$ scripts/check-commits.sh f035942 HEAD  -> no output, exit 0   (52 commits in f035942..HEAD)
```
(`scripts/check-version.sh` needs a version argument and was not part of the request; `scripts/check-commits.sh` without arguments only prints its usage.)

## NOT PROVEN (needs the real window, which was not started)
- The rail look: circle / squircle, pills (8, 20, 32 px), the ring, badges, the 500 ms tooltip.
- Dragging: space onto space making a folder, a space or a room onto a section, drop lines and refused drops (only the static gallery frames exist), and the `Grab` / `Grabbing` cursors.
- Right-click cards and the "..." buttons on headers and rows, as live interactions.
- The Reset sidebar layout... confirmation dialog, and the greyed Reset for a protected file (the model refusal is proven above, the dialog is not).
- The speaking ring following a real voice, and muted icons from a live call.
- Scrolling in the room list with sections (windowing, no auto-scroll while dragging).
- A real `cargo run` with `--data-dir` on the local homeserver (rail lighting from a second account's message, restart of the real app, the file made by real clicks).
- The App `update` message paths driven end to end (covered only by the lib unit tests that passed).

## Verdict
Every outside-observable criterion that could be run headlessly was demonstrated; the window items above are unproven by design.
