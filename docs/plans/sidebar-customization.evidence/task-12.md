# Evidence — Task 12: Voice channels with their people in the sidebar
Commit: 8d870c9
Environment: CC=clang-22 CXX=clang++-22, cargo from inside the repo. No docker stack started (no integration test in this task). Gallery PNGs written outside the repo to /tmp/claude-1000/-home-galep-projects-github-ga-lep-twicecat/21cbed6c-6961-49de-b763-6542423ef2f3/scratchpad/ui-gallery-evidence12/.

## Core and keys untouched
Status: PROVEN
```console
$ git diff 8faf654..8d870c9 --stat -- crates/app/src/keys.rs crates/core | wc -c
0
```
Empty diff: no change to keys.rs or core.

## Run of the hinted filter
```console
$ cargo test -p twicecat-app --lib -- calls sections spaces
test result: ok. 224 passed; 0 failed; 0 ignored; 0 measured; 1454 filtered out; finished in 0.04s
```

## Tests: members drawn under their room in order; ninth onward becomes "and N more"
Status: PROVEN (unit tests)
```console
test sections::tests::people_are_drawn_under_their_room_in_order ... ok
test sections::tests::the_ninth_member_onward_becomes_and_n_more ... ok
test screens::room_list_pane::tests::the_more_line_says_how_many_are_not_shown ... ok
test sections::tests::only_a_voice_room_lists_its_people ... ok
test sections::tests::a_collapsed_section_hides_the_people_with_its_rooms ... ok
```

## Speaking and muted only from this device's call; other rooms never show them
Status: PROVEN (unit tests + mutation below)
```console
test calls::tests::speaking_and_muted_come_only_from_this_devices_call ... ok
test sections::tests::marks_are_drawn_only_for_the_call_this_device_is_in ... ok
```

## Member with no name shows cleaned localpart; bidi cleaned
Status: PROVEN (unit test); localpart fallback seen in gallery ("galep" line)
```console
test calls::tests::a_member_is_named_by_its_name_else_by_its_localpart_cleaned ... ok
```

## Leaving the call removes the marks at once
Status: PROVEN (unit test)
```console
test calls::tests::leaving_the_call_removes_the_marks_at_once ... ok
test calls::tests::hanging_up_clears_the_call_and_its_participants_without_a_notice ... ok
```

## ActiveCall with zero members removes the lines
Status: PROVEN (unit tests)
```console
test sections::tests::the_app_lists_people_from_active_call_and_removes_them_at_zero ... ok
test calls::tests::call_members_are_kept_per_room_and_dropped_when_empty ... ok
test calls::tests::logging_out_clears_the_call_members_with_the_counts ... ok
```

## Speaking ring >= 3:1 against row background in every test theme
Status: PROVEN (unit test)
```console
test screens::room_list_pane::tests::the_speaking_ring_stands_out_from_every_row_background_in_every_theme ... ok
```

## Flat list shows no participant lines
Status: PROVEN (unit tests)
```console
test sections::tests::the_flat_list_shows_no_people ... ok
test sections::tests::home_a_search_another_tab_or_the_rail_keep_the_flat_list ... ok
```
(The pill itself is not separately asserted by a test I ran; the pre-existing flat-list tests passed.)

## Avatars requested once; Debug hides names
Status: PROVEN (unit tests)
```console
test sections::tests::the_avatars_of_the_people_are_asked_for_once ... ok
test sections::tests::debug_of_the_people_lines_shows_no_user_name_or_room ... ok
```

## Carried fix: FingerLost cancels both drag trackers and drops nothing
Status: PROVEN (unit tests)
```console
test state::tests::rail_dragging::a_lost_finger_cancels_a_rail_drag_and_saves_nothing ... ok
test state::tests::rail_dragging::the_listener_handles_touch_and_a_left_press_too ... ok
```
Only the rail-drag case is named in the test; the list tracker is covered by the same PointerCancelled handler per the commit text, which I did not separately exercise.

## Gallery: voice-channel passes layout checks at both widths
Status: PROVEN
```console
$ TWICECAT_GALLERY_DIR=.../ui-gallery-evidence12 TWICECAT_GALLERY_ONLY=voice-channel cargo test -p twicecat-app --test ui_gallery -- --ignored
test write_the_gallery ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 142 filtered out
```
Eight PNGs written: voice-channel-{twicecat,daylight}-{comfortable,compact}[-rail].png. The variants I found are two themes by two densities; I did not verify what "both widths" maps to beyond the harness passing.

Looked at voice-channel-twicecat-comfortable.png (dark) and voice-channel-daylight-compact.png (light):
- "Voice" section: "vocal" (selected, "Voice" chip, speaker 3) with three indented 20 px avatar lines: Rhaenyra, Septimus, galep.
- Rhaenyra's avatar has a visible ring (the speaking one). Septimus has a small red muted-mic icon at the right edge. galep has neither (the localpart fallback, lowercase).
- Second voice room "coworking" (speaker 1) with one line, "corin", no marks.
- The call card at the bottom reads "Voice connected, encrypted" for vocal. In the dark comfortable shot the lower list is clipped by the call card (Direct messages row is cut off); that is scroll clipping, not a bug I can attribute.
- The ring is thin and small at this size; by eye it is visible in both themes but not strong. The 3:1 figure comes from the unit test, not from my eye.

## Mutation: member_marks uses self.call instead of call_in(room)
Status: PROVEN (fails, reverted)
```console
$ sed -i '480s/self.call_in(room)/self.call.as_ref()/' crates/app/src/calls.rs
$ cargo test -p twicecat-app --lib
test calls::tests::speaking_and_muted_come_only_from_this_devices_call ... FAILED
test sections::tests::marks_are_drawn_only_for_the_call_this_device_is_in ... FAILED
test result: FAILED. 1676 passed; 2 failed
```

## Second mutation: MAX_PEOPLE_SHOWN 8 -> 100
Status: PROVEN (fails, reverted)
```console
test sections::tests::the_ninth_member_onward_becomes_and_n_more ... FAILED
test result: FAILED. 1677 passed; 1 failed
```
Both files were restored from saved copies; `cmp` against the copies said "restored", and `git status --short` was empty.

## Not proven
- Real two-account call on the local stack: NOT PROVEN (not started, per instruction).
- Speaking ring following a real voice, and the muted icon in a real window: NOT PROVEN (human checkpoint). The gallery is a software render of fixture data.

## Verdict
EVIDENCE: PROVEN
