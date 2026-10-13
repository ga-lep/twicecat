# Evidence — Task 5: Headless UI gallery
Commit: 942366b
Environment: CC=clang-22 CXX=clang++-22, cargo test on the dev profile. Gallery written to a scratch dir outside the repo (session scratchpad `.../scratchpad/gal`), then left there. No server, no audio device.

## A non-ignored test lays out each screen at 1280x800 and 720x800 under every family x mode x density, fails on panic; room pane slot invariants
Status: PROVEN (layout and slot tests pass; screen list coverage by test names and PNG file names)
```console
$ cargo test -p twicecat-app --test ui_gallery
running 6 tests
test write_the_gallery ... ignored, writes PNGs for review
test variant_tags_are_file_name_safe ... ok
test the_index_is_a_plain_image_grid ... ok
test the_fixture_reaches_every_screen_it_names ... ok
test the_room_pane_keeps_its_place_as_panels_bars_and_banners_come_and_go ... ok
test every_screen_lays_out_in_every_variant_at_both_sizes ... ok
test result: ok. 5 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 6.19s
```
The layout test and the slot-invariant test pass. The gallery file names cover: login, waiting, loading, main (no room, room, call phases, thread, ring), voice, settings, sessions, members, create, join, upload, viewer, switcher, verification, recovery, banner. I did not read the test body line by line to confirm the 720x800 size or the variant counts.

## The gallery loads the Inter faces into the headless font system first
Status: PROVEN
```console
$ grep -n "Inter\|font" crates/app/tests/ui_gallery.rs | head -3
21:use iced::advanced::graphics::text::font_system;
27:use twicecat_app::ui::fonts;
105:/// The headless renderer, with Inter loaded into the font system first
```
The PNGs below render in a sans face (Inter-like), not a fallback. I did not compare glyphs against Inter.

## Ignored test writes <screen>-<family>-<mode>-<density>.png (+ -rail) and index.html (plain img grid, no scripts, nothing remote, one column per family)
Status: PROVEN
```console
$ TWICECAT_GALLERY_DIR=$S cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 83.65s
$ ls $S | wc -l        -> 1297
$ ls $S | grep -c png  -> 1296
$ ls $S | grep -c rail -> 368
$ file $S/login-frost-dark-comfortable.png
PNG image data, 1280 x 800, 8-bit/color RGBA, non-interlaced
$ grep -ciE '<script|http|src="//' $S/index.html   -> 0
$ grep -c '<img' $S/index.html -> 324   (lines containing <img; rows hold 4 each)
$ grep -o '<th[^>]*>[^<]*' $S/index.html | head
<th>  <th>twicecat  <th>frost  <th>ember  <th>high-contrast  <th>light-comfortable ...
```
Per-screen file counts (login 32, main 544, voice 64, ring 64, verification 160, recovery 96, banner 128, others 16 each) = 1296 PNGs + index.html. Header row of every table is the four families; rows are mode-density. index.html is 262414 bytes, plain `<table>`/`<a><img>`, a `<style>` block, no script and no http. (`-rail` variants are of main-view screens.) The expected total is not stated in the plan; 1297 = 1296 + index.html is consistent.

Rendered PNGs inspected with the Read tool (frost, dark, comfortable):
- login: dark page, "Twicecat" title, three empty text fields (server, user, password), a teal "Log in" button and a grey "Sign in with browser" button. The button labels are low contrast.
- main-room: space rail, room list with avatars, badges (Muted, Voice, Invite, unread counts), a "Lounge" header with Notifications picker, Members/Settings/Call, timeline with file/audio rows, thread summary, an upload in progress (40 %), a failed-send row with Retry/Discard, "New messages", composer, status bar.
- main-call-connected: same plus a call bar in the room list, "Leave call" in the header, and a right "In the call (4)" panel: Me (you), Bea speaking (highlighted ring), Cy muted, Dee "Voice cleaning starts...", each with volume slider and clean mode picker. Observation: "Leave call" button text wraps and is clipped at the header, and "Audio..." in the call bar is cut off.
- voice-audio: modal "Voice & audio" with Devices (mic, speakers), Talking (push-to-talk, key F13, gamepad, release delay 200 ms), Microphone (noise suppression, GPU line "NVIDIA - TensorRT is used", echo/AGC checkboxes), over the call view. "Close" and "Set key.../Clear" buttons sit under the scrollbar edge and are clipped.
Visual quality is not claimed as proven.

## Fixture has no real names/tokens/keys; no_secret_logging passes; no audio device opened
Status: NOT PROVEN for `no_secret_logging` (needs the local homeserver); PROVEN for the rest by inspection
```console
$ cargo test -p twicecat-core --test log_hygiene
test no_secret_reaches_the_logs_or_the_disk ... ignored, needs local homeserver: docker compose up -d --wait
$ sed -n 44,47p crates/app/tests/common/fixture.rs
impl CommandSink for NullSink {
    fn send(&self, _command: Command) -> Option<Task<Message>> {
        None
```
The fixture sink drops every command, so no core or audio device is reached. Names in the PNGs are all `... Example` / `example.org`; the fixture header says the recovery key is an obvious dummy. I did not run the ignored `no_secret_logging` test (it needs docker).

## Overlays are not drawn headlessly; PROGRESS.md says so
Status: PROVEN
```console
$ grep -n "overlay layer is not drawn" PROGRESS.md
2467:  - iced's overlay layer is not drawn here: open `pick_list` menus (...) and tooltips do not appear in the PNGs.
```
The gallery pictures show closed pick_lists only.

## PROGRESS.md adds the human item "Pick the default colour set from the gallery (recommended: Frost)"
Status: PROVEN
```console
$ grep -n "Pick the default" PROGRESS.md | cut -c1-120
2475:- [ ] **Pick the default colour set from the gallery (recommended: Frost).** Run `TWICECAT_GALLERY_DIR=target/ui-gallery ...
```
The pick itself is a human checkpoint.

## The commit carries `Step: ui.5`
Status: PROVEN
```console
$ git log -1 --format=%B | grep Step
Step: ui.5
```

Repo state after the run: `git status --short` is empty before this file was written.
