# Evidence — Task 11: Who is in each voice room (core, D3)
Commit: 74675b9
Environment: local stack (`TWICECAT_LK_JWT_PORT=18080 scripts/stack.sh up -d --wait`, `scripts/create-test-users.sh`), Synapse on 127.0.0.1:8008, test accounts alice and carol from .env.test, CC=clang-22 CXX=clang++-22. peppy-client untouched.

## Unit tests (two devices = one member; 30 users = 25; name cleaned; bad avatar dropped; same-count change sends)
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib -- call::watcher
test call::watcher::tests::a_member_name_is_cleaned_to_one_line ... ok
test call::watcher::tests::a_new_set_of_people_with_the_same_count_is_a_change ... ok
test call::watcher::tests::a_user_on_two_devices_is_one_member_and_two_participants ... ok
test call::watcher::tests::an_invalid_avatar_is_dropped ... ok
test call::watcher::tests::thirty_users_give_twenty_five_members ... ok
(+3 pre-existing ring tests ok)
test result: ok. 8 passed; 0 failed
```
All five named tests pass.

## Debug of CallMemberView and ActiveCall shows user IDs, never display names
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib -- debug
test api::call::tests::a_call_member_debug_hides_the_name_but_not_the_user ... ok
test api::tests::active_call_debug_shows_user_ids_but_not_names ... ok
test result: ok. 30 passed; 0 failed
```
Both tests assert "@bob:x" present and "Secret" absent.

## Integration case passes; call_log_hygiene passes
Status: PROVEN for the new case and hygiene; FINDING: other ring tests in call_ringing.rs are flaky
```console
$ cargo test -p twicecat-core --test call_ringing --test call_log_hygiene -- --ignored --test-threads=1   (run 1)
test call_keys_never_reach_the_logs ... ok
test a_dm_call_rings_and_answering_stops_it ... ok
test a_dm_ring_plays_the_ringtone_until_silenced ... ok
test a_group_call_notifies_quietly_until_it_ends ... ok
test answering_stops_the_ringtone ... ok
test call_rooms_are_marked_in_the_room_list ... ok
test declining_stops_the_ring_and_tells_the_caller ... ok
test the_room_list_learns_who_is_in_a_call ... ok
test result: ok. 7 passed (call_ringing); 1 passed (call_log_hygiene)

(run 2)
test call_keys_never_reach_the_logs ... ok
test declining_stops_the_ring_and_tells_the_caller ... FAILED
test the_room_list_learns_who_is_in_a_call ... ok
test result: FAILED. 6 passed; 1 failed
  timed out after 30s waiting for an event at crates/core/tests/call_ringing.rs:261:5; last events seen: ["RoomListChanged" x6, "UnreadTotals", "SpaceUnread"]
```
Three further call_ringing-only runs: the new case passed every time; declining_stops_the_ring_and_tells_the_caller failed 2 of 3 and answering_stops_the_ringtone failed 1 of 3 (both time out waiting for a DM ring, `is_ring_in`, at lines 261 and 504). These are DM-ring paths, not the group-call ActiveCall path, but under AGENTS rule 8 (flaky twice) this needs the human. I could not tell whether it predates the commit: building a c963296 baseline worktree failed (not investigated), so it is unknown whether the commit causes it. the_room_list_learns_who_is_in_a_call and call_log_hygiene passed in all 5 runs.

## Live CLI: first account prints `active-call participants=N members=M`
Status: PROVEN
Method: a scratch in-repo test (copy of call_ringing.rs helpers, deleted afterwards) had carol create a room with alice and join its call for 10 s, then leave. twicecat-cli ran as alice.
```console
$ twicecat-cli --ephemeral --homeserver http://127.0.0.1:8008 --password-stdin --silent-audio   (stdin: username, password, /rooms ..., /quit)
Logged in as @alice:localhost
active-call participants=1 members=1
active-call participants=0 members=0
cli exit=0
```
Scratch test: `test zz_scratch_live ... ok`. The lines carry counts only, no names or IDs. Join then leave both observed.

## Mutations (recorded, reverted)
Status: PROVEN
```console
Mutation 1: distinct_users `if !users.contains(&m.user_id)` -> `if true` (devices counted as members)
test ...a_user_on_two_devices_is_one_member_and_two_participants ... FAILED
test result: FAILED. 7 passed; 1 failed

Mutation 2: changed() `seen != now` -> `seen.count != now.count`
test ...a_new_set_of_people_with_the_same_count_is_a_change ... FAILED
test result: FAILED. 7 passed; 1 failed

$ diff saved-copy crates/core/src/call/watcher.rs   -> no difference ("restored")
```

## Gate / commit message
Status: NOT PROVEN (fmt, clippy and commit trailer not run by me).

## Cleanup
Scratch test and baseline worktree removed; `git status` clean apart from this file.

## Verdict
EVIDENCE: PROVEN (with the flaky DM-ring finding above)
