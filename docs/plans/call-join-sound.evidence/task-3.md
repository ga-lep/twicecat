# Evidence — Task 3: The cue when another person joins
Commit: eb92f54 (branch feat/call-sounds)
Environment: CC=clang-22 CXX=clang++-22, repo's own target dir, .cargo/config.toml untouched. Local stack from `scripts/stack.sh up -d --wait` (accounts alice and carol from .env.test only), shut down at the end (no twicecat containers left). Virtual speakers record what is played; nobody listened. Mutations and one temporary report print were made on the real tree from a copy and restored (`cmp` identical, `git status` clean apart from the untracked docs/plans/custom-theme.md).

## Unit tests on `Arrivals` (baseline, own_seen, new member, own device, 60 s rejoin, unchanged/refresh, 3 s window) plus review fixes
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib call::participants::arrivals
test call::participants::arrivals::a_new_device_of_our_own_user_does_not_cue ... ok
test call::participants::arrivals::an_unchanged_list_or_a_refreshed_member_does_not_cue ... ok
test call::participants::arrivals::a_new_member_of_another_user_cues_once ... ok
test call::participants::arrivals::a_user_back_on_a_new_device_within_a_minute_does_not_cue ... ok
test call::participants::arrivals::a_member_that_comes_back_within_a_minute_does_not_cue_but_after_one_does ... ok
test call::participants::arrivals::arrivals_within_three_seconds_give_one_cue_and_a_later_one_another ... ok
test call::participants::arrivals::a_device_window_shift_does_not_cue_a_long_present_user ... ok
test call::participants::arrivals::nothing_cues_before_our_own_membership_is_seen ... ok
test call::participants::arrivals::odd_membership_data_does_not_panic_or_cue_wrongly ... ok
test call::participants::arrivals::several_arrivals_in_one_update_give_one_cue ... ok
test call::participants::arrivals::rotating_device_ids_of_one_user_cue_once_a_minute ... ok
test call::participants::arrivals::the_first_update_never_cues_whoever_is_there ... ok
test call::participants::arrivals::a_hostile_user_with_many_memberships_does_not_stop_others_cueing ... ok
test call::participants::arrivals::our_own_devices_do_not_use_up_the_slots ... ok
test call::participants::arrivals::many_users_each_get_a_few_devices_and_the_total_is_bounded ... ok
test call::participants::arrivals::a_large_hostile_list_is_bounded_and_does_not_flap ... ok
test result: ok. 16 passed; 0 failed
```
All 16 pass, covering each listed behaviour plus the per-user rejoin-quiet, 8-per-user / 256-total bound and filter-first rules.

## With `others_joining: false` (set live), an arrival plays nothing
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib call::cue
test call::cue::tests::without_a_voice_engine_there_is_no_cue ... ok
test call::cue::tests::the_blip_plays_while_deafened ... ok
test call::cue::tests::the_live_setting_decides_whether_the_blip_plays ... ok
test result: ok. 3 passed; 0 failed
```
`the_live_setting_decides_whether_the_blip_plays` flips the watch channel with others_joining false (silent speaker energy), back on (plays at 60 %), and disabled / 0 % (silent). The `set_sounds` call from `store_audio_settings` and the real call path are not driven from the app here (covered by this core-level test of the same watch channel).

## End-to-end on the local stack: carol in call, alice joins -> one G5 blip for carol, alice hears only her rising cue, repeat JoinCall -> no blip
Status: PROVEN
```console
$ scripts/stack.sh up -d --wait   (all containers Healthy)
$ cargo test -p twicecat-core --test call_sounds -- --ignored --test-threads=1
test a_failed_join_plays_no_cue ... ok
test a_repeat_join_is_connected_without_a_cue ... ok
test joining_plays_a_rising_cue_to_the_one_who_joins ... ok
test leaving_deafened_plays_the_falling_cue_without_delaying_left ... ok
test other_joins_play_one_blip_to_the_others_only ... ok
test switching_rooms_plays_only_the_new_join_cue ... ok
test the_cue_never_reaches_the_room ... ok
test the_settings_turn_the_cues_off ... ok
test result: ok. 8 passed; 0 failed; finished in 101.82s
```
Recorded audio, from a temporary eprintln in the test (scratch copy, restored), run as `cargo test ... --ignored --test-threads=1 --nocapture other_joins`:
```console
REPORT carol-blip: samples=236640 (4.93s) peak=0.0297 (-30.5 dBFS) active_ms~80 G5=0.0242 E5=0.0039 A5=0.0062
REPORT alice-own-cue: samples=237120 (4.94s) peak=0.0595 (-24.5 dBFS) active_ms~170 G5=0.0120 E5=0.0494 A5=0.0486
test other_joins_play_one_blip_to_the_others_only ... ok
```
Carol's capture after alice's join: peak 0.0297 (-30.5 dBFS, the 0.03 the test comment expects), G5 energy 0.0242 versus E5 0.0039 and A5 0.0062 (the blip is G5, others are leakage), and the test asserts exactly one blip and no chime. Alice's own capture is the two-note E5/A5 rising cue (peak -24.5 dBFS), the G5 reading of 0.012 being leakage from A5, and the test asserts it is exactly `[Rising]`. The repeat join gives 0 blips and no cue energy for carol or alice. "active_ms" counts 10 ms windows above the detector thresholds, so it is not the cue length (which has fades).

## Mutations (each recorded, then reverted): no baseline, own user not excluded, no 3 s window
Status: PROVEN
```console
M1 no baseline (drop `&& was_baselined`)
test ...the_first_update_never_cues_whoever_is_there ... FAILED
(plus 6 more FAILED: a_user_back_on_a_new_device..., a_device_window_shift..., a_member_that_comes_back..., a_hostile_user..., an_unchanged_list_or_a_refreshed_member..., many_users_each_get_a_few_devices...)
test result: FAILED. 9 passed; 7 failed

M2 own user not excluded (filter only drops empty user IDs)
test ...a_hostile_user_with_many_memberships_does_not_stop_others_cueing ... FAILED
test ...nothing_cues_before_our_own_membership_is_seen ... FAILED
test result: FAILED. 14 passed; 2 failed

M3 no 3 s window (`is_none_or(|_| true)`)
test ...arrivals_within_three_seconds_give_one_cue_and_a_later_one_another ... FAILED
test result: FAILED. 15 passed; 1 failed
```
M1 and M3 fail the tests the plan names. M2 FAILS two tests but NOT `a_new_device_of_our_own_user_does_not_cue`, which the plan names: since the review fix, with our own user not excluded the user is already in the per-user "recently seen" map from the baseline, so the per-user rule hides a new own device and that test passes. The mutation is still caught, by the two tests above; the named test is no longer discriminating on its own. A finding, not a feature failure.

## Review-fix mutations: per-user rule off; cap before filter
Status: PROVEN
```console
M4 per-user rule off (`if !rejoined && !user_recent` -> `if !rejoined`)
test ...a_user_back_on_a_new_device_within_a_minute_does_not_cue ... FAILED
test ...a_device_window_shift_does_not_cue_a_long_present_user ... FAILED
test ...rotating_device_ids_of_one_user_cue_once_a_minute ... FAILED
test result: FAILED. 13 passed; 3 failed

M5 cap before filter (take 256 of all memberships, then drop own/empty)
test ...our_own_devices_do_not_use_up_the_slots ... FAILED
test ...a_hostile_user_with_many_memberships_does_not_stop_others_cueing ... FAILED
test result: FAILED. 14 passed; 2 failed
```
Each review rule has failing tests when removed. The mutated M5 code was written by me as a scratch edit of the filter/bound block (cap first, then filter and per-user bound).

## The gate / webrtc check
Status: PROVEN (webrtc check); NOT PROVEN for fmt/clippy/full workspace test (not part of this run)
```console
$ bash scripts/check-no-webrtc-download.sh; echo "exit=$?"
exit=0
```

## Not exercised (human checks)
- Hearing the blip: pleasantness, audibility, and the echo/leak behaviour on real speakers. Only recorded virtual-speaker samples were inspected.
- A real roster reload (sync reload of the room's memberships in a running client) and an app-driven live setting toggle.
- A long call of more than 256 devices: bounds are proven only by unit tests with synthetic lists.
- Windows/macOS outputs, Bluetooth headsets, network drop and reconnect.

## Verdict
EVIDENCE: PROVEN (with the M2 note above)
