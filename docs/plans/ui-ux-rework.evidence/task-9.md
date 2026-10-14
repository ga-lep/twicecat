# Evidence — Task 9: Timeline restyle (same grouped layout)
Commit: a92dcfb
Environment: CC=clang-22 CXX=clang++-22, cargo test on the repo; gallery written to a scratch dir outside the repo (TWICECAT_GALLERY_DIR, `-- --ignored`, 87 s) and inspected with the image reader. Mutation run in a scratch git worktree (LK_CUSTOM_WEBRTC pointed at the repo's dev/libwebrtc), removed afterwards. The hover action bar is NOT drawn in any PNG (fixtures never hover), so the icon bar, tooltips and danger colour are only covered by unit tests here. Real-window checks stay human.

## Same layout / grouping (is_continuation unchanged, same order of parts)
Status: PROVEN (tests + gallery)
```console
$ cargo test -p twicecat-app --lib -- timeline timeline_pane kit rich_text
test result: ok. 150 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
The existing grouping/echo/shield tests pass unchanged in that set. In the gallery (frost dark comfortable, main-room-states) "Bea Example 02:41" shows a header row, then "A second message in a row" as a continuation without a header; header then body then reactions/edited order is intact in main-room-text.

## Restyled parts (gallery, inspected)
Status: PROVEN for what the fixtures show; call notices NOT PROVEN visually
- Frost dark comfortable and compact, main-room-states / main-room-text: sender names bold (BODY_SEMIBOLD look), muted small timestamps beside them (02:41, 04:21), a centred "Yesterday"/"Today" tag between two thin rules, state lines (created the room, joined) muted.
- Reactions: rounded chips "3", "1", "+"; the own reaction (thumbs up, 3) has a teal outline and tinted fill, the others are plain. The emoji glyph in the first chip is slightly small and its top looks clipped by the chip border (minor defect, both densities).
- Reply quote on surface with a 3 px teal bar (Alice Example / "A long message that has to wrap"). Block quote "Measure twice, cut once." and the code block (fn main) on surface with the teal bar. Inline code and spoiler pills are on surface.
- Unverified-device shield: red "!" badge next to "Dee Example"; unchanged words. Mention row highlighted in amber band, readable.
- main-room (frost dark comfortable and compact): failed-send row "This one did not go through." with red "Failed to send. Will retry when back online" and Retry / Discard buttons; "Sending..." echo with Cancel and a 40 % bar; "New messages" divider in teal; thread summary "3 replies" in teal. No overlaps or clipping seen.
- Twicecat light comfortable (states + text): same structure; dark text on white, indigo bars and tags, red mention pill, red shield badge, amber highlight on pale yellow; contrast fine.
- Comfortable vs compact: compact rows are visibly tighter (header-to-body and row gaps smaller), same content.
- Call notices (Icon::Call state lines): no call item in any inspected fixture, so NOT PROVEN visually (unit test a_call_state_line_has_the_call_icon_and_other_state_lines_have_none passes in the 150).
- Minor: the unread badge "3" on the Lounge sidebar row overlaps its preview text (sidebar, not Task 9 scope).

## estimate_height within +-25 % for both densities; 10,000-item test
Status: PROVEN
```console
$ cargo test -p twicecat-app --test timeline_layout
running 8 tests
test decorated_rows_are_estimated_within_a_quarter_in_both_densities ... ok
test estimates_stay_within_a_quarter_of_the_laid_out_rows_for_both_densities ... ok
test a_compact_timeline_lays_out_shorter_rows_on_average ... ok
test only_the_visible_rows_of_10000_items_are_laid_out ... ok
(+4 more ok)
test result: ok. 8 passed; 0 failed
```

## Density switch keeps place / gallery renders every variant
Status: PROVEN (layout) / NOT PROVEN (interactive place keeping, human)
```console
$ cargo test -p twicecat-app --test ui_gallery
test result: ok. 16 passed; 0 failed; 1 ignored
$ TWICECAT_GALLERY_DIR=<scratch> cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok   (87.30s; 96 main-room*.png written)
```

## Action bar: icon buttons, Delete set apart and in danger colour
Status: PROVEN for the gap (mutation); danger colour and tooltips NOT PROVEN (not drawn; unit tests only)
Unmodified:
```console
$ cargo test -p twicecat-app --lib -- timeline_pane
test screens::timeline_pane::tests::delete_sits_a_full_step_away_from_the_other_bar_icons ... ok
```
Mutation (worktree at a92dcfb, `bar = bar.push(space().width(tokens::M));` before Delete replaced by a comment):
```console
$ cargo test -p twicecat-app --lib delete_sits
test screens::timeline_pane::tests::delete_sits_a_full_step_away_from_the_other_bar_icons ... FAILED
thread '...' panicked at crates/app/src/screens/timeline_pane.rs:2380:9:
Delete is only 0 px further from Copy than Thread is
test result: FAILED. 0 passed; 1 failed
```
The test catches the removed spacer; the worktree was removed and the repo is clean.

## Trust signals unchanged (shields, UTD, echo, receipts)
Status: PROVEN
Their tests are in the 150 passing lib tests above, unchanged by the diff; the gallery shows "You don't have access to this message" (UTD), "Sending...", "Failed to send. Will retry when back online" and the shield badge with the same words and colours (red).

## Commit carries Step: ui.9
Status: NOT PROVEN (not checked by this run)
