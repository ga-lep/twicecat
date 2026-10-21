# Evidence — Task 4: Unread totals per space (core, D2)
Commit: 510715a
Environment: CC=clang-22 CXX=clang++-22, cargo run inside the repo. Local stack via `TWICECAT_LK_JWT_PORT=18080 scripts/stack.sh up -d --wait` and `scripts/create-test-users.sh`; Synapse on 127.0.0.1:8008; test accounts alice and bob from .env.test only (passwords never printed). Stack stopped afterwards (`scripts/stack.sh stop`); `docker ps` then lists only peppy-client-peppy-client-1. `git status` clean apart from this file.

## Unit tests of `space_totals` (muted, sub-space + top-level once, two top-level spaces, outside/DMs, saturation, unread flag)
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib -- notifications::
test notifications::tests::a_muted_room_counts_nowhere ... ok
test notifications::tests::a_room_in_two_top_level_spaces_counts_in_both ... ok
test notifications::tests::a_room_of_a_sub_space_counts_for_it_and_for_the_top_space_once ... ok
test notifications::tests::rooms_in_no_space_count_outside ... ok
test notifications::tests::the_sums_saturate ... ok
test notifications::tests::unread_is_the_flag_alone ... ok
test notifications::tests::spaces_come_in_id_order ... ok
test notifications::tests::a_space_id_is_not_a_room_even_when_the_list_is_cut_or_odd ... ok
(37 other notification tests elided)
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 631 filtered out; finished in 0.00s
```
One test per listed rule, all pass.

## Runner sends nothing when unchanged and at most once per interval
Status: PROVEN
```console
test notifications::tests::space_counts_are_sent_only_on_a_change_and_at_most_once_a_second ... ok
test notifications::tests::totals_are_sent_at_most_once_a_second_and_only_on_change ... ok
test notifications::tests::totals_stop_when_nobody_listens ... ok
```
The existing `run_totals` tests plus the new space-counts test pass (same run as above).

## Integration: a message from another account raises the space's `notifications` to >= 1; mark-read returns it to 0
Status: PROVEN
```console
$ cargo test -p twicecat-core --test notifications -- --ignored --test-threads=1     (run 1, then run 2)
test space_unread_follows_a_message_in_a_room_of_the_space ... ok
test unread_totals_follow_a_message_and_mark_read ... ok
(6 other cases ok)
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 67.11s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 67.18s
```
The test waits for the space's counts to be quiet, has alice post, waits for `notifications >= 1` (and Home `outside` 0), marks read, and waits for `notifications == 0 && !unread`. Passes twice.

## Live CLI prints `[space-unread] spaces=N outside=(n=..,h=..)` after a message
Status: PROVEN
Setup (curl, local Synapse): alice created room "ev4-room" (invited bob) and space "ev4-space" holding it; bob joined the room. The CLI ran as alice (`--ephemeral --homeserver http://127.0.0.1:8008 --password-stdin`); 25 s after login bob posted "hello from bob" with curl.
```console
$ (printf 'alice\n<pw>\n'; sleep 25; curl -X PUT .../rooms/<room>/send/m.room.message/ev4a ... as bob; sleep 15; echo /quit) | target/debug/twicecat-cli --ephemeral --homeserver http://127.0.0.1:8008 --password-stdin
$ grep -E "Logged in|space-unread|\[unread\]|\[spaces\]" cli.out
Logged in as @alice:localhost
[unread] 0 notifications, 0 highlights
[space-unread] spaces=0 outside=(n=0,h=0)
[spaces] 0 spaces: 
[spaces] 1 spaces: space level=0 rooms=1
[space-unread] spaces=1 outside=(n=0,h=0)
[unread] 1 notifications, 0 highlights
[space-unread] spaces=1 outside=(n=0,h=0)
```
After bob's message the tray total rose to 1 while `outside` stayed 0 and `spaces=1`: the unread landed in the space, not Home. Note the line prints a count of spaces and the Home counts only, so the space's own count is not visible in the CLI (by design); that part is proven by the integration test. Values differ from the plan's example (`spaces=2 outside=(n=1,h=0)`) only because the setup differs (one space, no room outside).

## Log hygiene; the CLI line holds no ID and no name
Status: PROVEN
```console
$ cargo test -p twicecat-core --test call_log_hygiene --test log_hygiene -- --ignored --test-threads=1
test call_keys_never_reach_the_logs ... ok
test no_secret_reaches_the_logs_or_the_disk ... ok
$ cargo test -p twicecat-core --bin twicecat-cli
test result: ok. 41 passed; 0 failed
$ grep -c "hello from bob\|<room id>\|<space id>\|ev4-" cli.out
0
```
There is no `no_secret_logging.rs`; the repo's equivalents are `call_log_hygiene.rs` and `log_hygiene.rs`, both run against the stack. The whole CLI output held no message body, room/space ID or room/space name. The CLI format code prints only `spaces.len()` and `outside` counts.

## Mutations (original saved to the scratchpad first, restored and diffed)
Status: PROVEN
```console
M1: `.filter(|r| !r.muted)` in sum_counts replaced by `.filter(|_| true)`
test notifications::tests::a_muted_room_counts_nowhere ... FAILED
test result: FAILED. 42 passed; 1 failed
$ diff <saved> crates/core/src/notifications.rs && echo RESTORED_IDENTICAL
RESTORED_IDENTICAL

M2: `members` HashSet turned into a Vec (no dedup), sub-space rooms pushed again
test notifications::tests::a_room_of_a_sub_space_counts_for_it_and_for_the_top_space_once ... FAILED
  left: UnreadCountsView { notifications: 321, highlights: 4, unread: false }
 right: UnreadCountsView { notifications: 111, highlights: 2, unread: false }
test result: FAILED. 42 passed; 1 failed
RESTORED_IDENTICAL
$ cargo test -p twicecat-core --lib -- notifications::
test result: ok. 43 passed; 0 failed
```
(A first M2 attempt that kept the HashSet changed nothing and the tests passed, as it should; it is not counted.) Both mutations are caught by the intended test.

## The gate and commit message
Status: NOT PROVEN (internal — covered by the commit and CI; not re-run here)
