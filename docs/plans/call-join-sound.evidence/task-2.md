# Evidence — Task 2: Settings, the cue sounds, and the own join and leave cues in the core
Commit: 0b2db85 (branch feat/call-sounds)
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir, .cargo/config.toml untouched. Local stack from `scripts/stack.sh up -d --wait` (synapse 127.0.0.1:8008), accounts from .env.test only (alice, carol); stack shut down at the end. Virtual speakers record what is played; nobody listened, so audibility/pleasantness is a human check. Mutation-style edits (one temporary report print in tests/call_sounds.rs) were made on the real tree from a copy and restored; `git status` is clean apart from the untracked docs/plans/custom-theme.md.

## Unit tests: cues, volume(), call_settings
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib call_sounds
test api::audio::tests::call_sounds_default_to_all_on_at_60_percent ... ok
test api::audio::tests::the_call_sounds_volume_is_clamped ... ok
test api::audio::tests::call_sounds_missing_or_partial_read_as_the_defaults ... ok
test call_sounds::tests::volume_follows_the_settings ... ok
test call_sounds::tests::each_cue_has_the_length_of_the_plan ... ok
test call_sounds::tests::samples_are_made_once ... ok
test call_sounds::tests::each_cue_starts_and_ends_at_exact_zero ... ok
test call_sounds::tests::the_blip_is_one_g5_note ... ok
test call_sounds::tests::the_join_cue_rises_and_the_leave_cue_falls ... ok
test call_sounds::tests::each_cue_peaks_where_the_plan_says ... ok
test call_sounds::tests::no_cue_clicks ... ok
test call_sounds::tests::each_cue_stays_clear_of_the_1_khz_test_tone ... ok
test result: ok. 15 passed; 0 failed
$ cargo test -p twicecat-core --lib call_settings
test call_settings::tests::a_stored_call_sounds_volume_of_900_loads_as_200 ... ok
test call_settings::tests::a_file_from_before_the_call_sounds_loads_the_defaults ... ok
test call_settings::tests::changed_call_sounds_round_trip ... ok
(+9 other call_settings/paths tests, all ok)
test result: ok. 12 passed; 0 failed
```
Length, peak, zero ends, step, 1 kHz, rise/fall, same pointer, volume() table, defaults/round-trip/900->200 all have passing tests.

## Review fixes: closing session muted, no InputLevel/AudioStats; sessions never overlap; leave cue plays in full
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib closing
test engine::tests::a_closing_session_reports_no_levels_or_stats ... ok
test engine::tests::a_new_call_waits_for_the_closing_devices_and_never_overlaps_them ... ok
test result: ok. 2 passed; 0 failed
$ cargo test -p twicecat-core --lib the_leave_cue
test engine::tests::the_leave_cue_plays_in_full_and_the_devices_close_after_it ... ok
test call_sounds::tests::the_join_cue_rises_and_the_leave_cue_falls ... ok
```
Source (audio_session.rs `begin_closing`) mutes the mic and the level/stats loop `continue`s while closing.

## Ignored end-to-end tests on the local stack
Status: PROVEN
```console
$ cargo test -p twicecat-core --test call_sounds -- --ignored
test a_repeat_join_is_connected_without_a_cue ... ok
test a_failed_join_plays_no_cue ... ok
test switching_rooms_plays_only_the_new_join_cue ... ok
test leaving_deafened_plays_the_falling_cue_without_delaying_left ... ok
test the_settings_turn_the_cues_off ... ok
test joining_plays_a_rising_cue_to_the_one_who_joins ... ok
test the_cue_never_reaches_the_room ... ok
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 24.34s
```
(the 1 filtered-out, non-ignored test is the detector self-test, run by the plain suite.)

## CLI: `/sounds off`, `/sounds 40`, `/sounds 150`, then `/audio`
Status: PROVEN
Driven non-interactively through stdin with sleeps (`--ephemeral --homeserver http://127.0.0.1:8008 --password-stdin`, alice from .env.test; password redacted). Output filtered to the "Call sounds"/Usage lines; first line of each pair is the `/sounds` reply, second is `/audio`.
```console
### session: /sounds off
Call sounds off.
Call sounds off.
exit=0
### session: /sounds 40
Call sounds on (others joining, leaving), 40 %.
Call sounds on (others joining, leaving), 40 %.
exit=0
### session: /sounds 150
Call sounds on (others joining, leaving), 150 % (values above 100 play at 100).
Call sounds on (others joining, leaving), 150 % (values above 100 play at 100).
exit=0
### session: /sounds 201
Usage: /sounds [on|off|others on|off|leave on|off|<0-200>]
Call sounds on (others joining, leaving), 60 %.
exit=0
```
Matches the criterion strings exactly; >100 mentions the cap of 100 (volume cap is documented in the user-visible summary; also the parse tests are in calls/parse.rs).
Note: a first attempt with no sleeps between lines showed the defaults in `/audio` (the settings update is asynchronous); with pauses it is as above. Within one session `/sounds off` then `/sounds 40` keeps it off (volume changes, enabled stays off): observed "Call sounds off." repeatedly in a multi-command session.

## Cue audibility (recorded virtual-speaker samples, default 60 %)
Status: PROVEN (hearing is a human check)
Temporary print added to the e2e test (restored afterwards):
```console
$ cargo test -p twicecat-core --test call_sounds -- --ignored --nocapture joining_plays
REPORT alice: total samples 188640 (3930 ms), cue windows 102..121 => ~200 ms long, peak 0.0595 = -24.5 dBFS, note sequence ["E5", "A5"]
REPORT carol: total samples 102240 (2130 ms), cue windows 123..142 => ~200 ms long, peak 0.0595 = -24.5 dBFS, note sequence ["E5", "A5"]
test joining_plays_a_rising_cue_to_the_one_who_joins ... ok
```
Rising E5 then A5 for both joiners; the ~200 ms is the span of 10 ms windows above the detector's threshold (the cue itself is 240 ms by the unit test; the tail decays below threshold). -24.5 dBFS at 60 % matches -20 dBFS at 100 % (-24.4 expected).

## call_ringing flakiness measurement (`--ignored --test-threads=1`, 5 runs each)
Status: measurement only (no verdict)
Ok = pass, F = fail.

HEAD 0b2db85:
| test | r1 | r2 | r3 | r4 | r5 |
|---|---|---|---|---|---|
| a_dm_call_rings_and_answering_stops_it | F | F | F | ok | ok |
| a_dm_ring_plays_the_ringtone_until_silenced | ok | ok | ok | ok | ok |
| a_group_call_notifies_quietly_until_it_ends | ok | ok | ok | ok | ok |
| answering_stops_the_ringtone | F | ok | ok | ok | F |
| call_rooms_are_marked_in_the_room_list | ok | ok | ok | ok | ok |
| declining_stops_the_ring_and_tells_the_caller | ok | ok | ok | ok | F |
| run result | FAIL (2) | FAIL (1) | FAIL (1) | PASS | FAIL (2) |

Base 3994f82 (detached, rebuilt):
| test | r1 | r2 | r3 | r4 | r5 |
|---|---|---|---|---|---|
| a_dm_call_rings_and_answering_stops_it | ok | ok | ok | F | ok |
| a_dm_ring_plays_the_ringtone_until_silenced | ok | F | ok | ok | ok |
| a_group_call_notifies_quietly_until_it_ends | ok | ok | ok | ok | ok |
| answering_stops_the_ringtone | F | ok | F | F | ok |
| call_rooms_are_marked_in_the_room_list | ok | ok | ok | ok | ok |
| declining_stops_the_ring_and_tells_the_caller | ok | ok | ok | ok | ok |
| run result | FAIL (1) | FAIL (1) | FAIL (1) | FAIL (2) | PASS |

Failure messages (all timeouts waiting for the ring to reach carol, i.e. `is_ring_in`; no panic was an assertion on sounds):
- HEAD r1/r2/r3 `a_dm_call_rings_and_answering_stops_it`: `call_ringing.rs:160 timed out; matched so far: [None, Some(ActiveCall { room_id: ..., participants: 1 })]` (the ring event never matched).
- HEAD r1/r5 `answering_stops_the_ringtone` (line 460) and r5 `declining_stops_the_ring_and_tells_the_caller` (line 261): `timed out after 30s waiting for an event ...; last events seen: [RoomListChanged x7, UnreadTotals]`.
- Base: same shapes (answering_stops_the_ringtone at line 446 r1/r3/r4; a_dm_ring_plays_the_ringtone_until_silenced line 404 r2; a_dm_call_rings_and_answering_stops_it r4 line 160).
- The "held for its room key" notification text did not appear in any captured test output (grep "room key" over all 10 logs: no hits). Test output carries event names only, not core log lines, so this does not rule it out.
Totals: HEAD 6 failures over 30 test runs, 4 of 5 runs failed; base 6 failures over 30, 4 of 5 runs failed.

## Housekeeping
```console
$ git checkout feat/call-sounds; git rev-parse --short HEAD; git status --short
0b2db85
?? docs/plans/custom-theme.md
$ scripts/stack.sh down   (all containers and network removed)
$ bash scripts/check-no-webrtc-download.sh; echo check_exit=$?
check_exit=0
```

## Verdict
Every Task 2 criterion observable from outside was demonstrated. Not exercised: the reconnect path (cannot be forced locally, per plan) and the plan's two mutation runs (not requested here).
