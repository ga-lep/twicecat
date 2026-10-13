# Evidence — Task 2: Bundle the Inter typeface
Commit: e7969ed
Environment: local build, CC=clang-22 CXX=clang++-22; no server needed. shellcheck is not installed (not checked).

## scripts/check-font-assets.sh checks the four TTFs and OFL.txt against SOURCE.md
Status: PROVEN
```console
$ bash scripts/check-font-assets.sh; echo exit=$?
OK for 5 files
exit=0
```
Five files verified by `sha256sum -c`. SOURCE.md records URL, tag v4.1, zip SHA-256 and the five file hashes.

Tamper checks on a temp copy (tracked files untouched):
```console
$ # first hash in the copy's SOURCE.md altered
15c15
< 40d692fce188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82  Inter-Regular.ttf
---
> 00000000e188e4471e2b3cba937be967878f631ad3ebbbdcd587687c7ebe0c82  Inter-Regular.ttf
$ bash $T/scripts/check-font-assets.sh; echo exit=$?
Inter-Regular.ttf: FAILED
sha256sum: WARNING: 1 computed checksum did NOT match
exit=1
$ # Inter-Bold.ttf line deleted from the copy's SOURCE.md
expected 5 entries in .../SOURCE.md, found 4
exit=1
$ # (earlier run) one byte appended to the copy's Inter-Bold.ttf
Inter-Bold.ttf: FAILED
sha256sum: WARNING: 1 computed checksum did NOT match
exit=1
```
The script fails (exit 1) on a wrong hash, a missing entry and a modified font.

## ui::fonts constants, byte/magic tests, ALL == main.rs, no Font::DEFAULT, headless bold-face test
Status: PROVEN
```console
$ cargo test -p twicecat-app ui::fonts
test ui::fonts::tests::families_use_inter ... ok
test ui::fonts::tests::main_loads_exactly_all ... ok
test ui::fonts::tests::every_face_is_truetype_and_has_its_size ... ok
test ui::fonts::tests::no_default_font_outside_tests ... ok
test ui::fonts::tests::bold_face_is_found ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 1064 filtered out
$ grep -rn "Font::DEFAULT" crates/app/src
crates/app/src/ui/fonts.rs:122/123 (test), screens/room_dialogs.rs:489, screens/timeline_pane.rs:2095, state.rs:6820, state.rs:10083
```
(Note: plain `cargo test -p twicecat-app fonts` runs against the bin target first and shows 0 tests; the tests live in the lib, so they match with the `ui::fonts` filter or the full run.) The grep hits outside fonts.rs are all after `#[cfg(test)]` (room_dialogs test mod at line 284, timeline_pane at 1227, state at 2665), and the grep test passes.

## THIRD-PARTY-NOTICES.md; cargo deny
Status: PROVEN (deny) / notices covered by diff review only
```console
$ cargo deny check 2>&1 | tail -1
advisories ok, bans ok, licenses ok, sources ok
```
deny.toml is not in the diff. Notices file content and the PROGRESS.md binary-size entry were not exercised here (the release build was not run).

## Rendering in Inter in a real window
Status: NOT PROVEN (human checkpoint — needs a real window; the headless width test only shows that the bold face resolves)

## Verdict
EVIDENCE: PROVEN
