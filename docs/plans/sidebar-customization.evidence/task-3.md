# Evidence — Task 3: A space's rooms in SpaceView (core, D1)
Commit: e0d7cdf
Environment: CC=clang-22 CXX=clang++-22, cargo run inside the repo. Local stack via `TWICECAT_LK_JWT_PORT=18080 scripts/stack.sh up -d --wait` and `scripts/create-test-users.sh`; Synapse on 127.0.0.1:8008; alice from .env.test only. Stack stopped afterwards (`scripts/stack.sh stop`); `docker ps` then lists only peppy-client-peppy-client-1. `git status` clean apart from this file.

## Unit tests: rooms equals descendants up to the cap; 2,005 gives 2,000; Debug shows count only
Status: PROVEN
```console
$ cargo test -p twicecat-core --lib -- spaces::
running 5 tests
test spaces::tests::a_space_filter_becomes_a_view ... ok
test spaces::tests::debug_shows_the_room_count_and_no_id ... ok
test spaces::tests::descendants_are_kept_by_space ... ok
test spaces::tests::rooms_are_the_descendants_in_order ... ok
test spaces::tests::rooms_are_capped ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 660 filtered out; finished in 0.00s
```
`rooms_are_capped` builds 2,005 children and asserts `rooms.len() == 2000` (and `children == 2005`).

### Mutation: remove `.take(MAX_SPACE_ROOMS)` from `room_ids` (original saved to the scratchpad first)
```console
$ sed -i '/\.take(MAX_SPACE_ROOMS)/d' crates/core/src/spaces.rs
$ cargo test -p twicecat-core --lib -- spaces::
test spaces::tests::rooms_are_capped ... FAILED
thread 'spaces::tests::rooms_are_capped' panicked at crates/core/src/spaces.rs:169:9:
assertion `left == right` failed
  left: 2005
 right: 2000
test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 660 filtered out; finished in 0.00s
$ cp <saved copy> crates/core/src/spaces.rs; diff <saved copy> crates/core/src/spaces.rs && echo RESTORED_IDENTICAL
RESTORED_IDENTICAL
$ cargo test -p twicecat-core --lib -- spaces::
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 660 filtered out; finished in 0.00s
```
The cap test fails without the cap and passes after restoring.

## Integration test against the local stack: level 0 holds the two rooms and the sub-space ID; level 1 holds its room (own ID last)
Status: PROVEN
```console
$ cargo test -p twicecat-core --test spaces --locked -- --ignored     (run 1)
running 1 test
test a_space_view_lists_its_rooms_by_level ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.66s
$ cargo test -p twicecat-core --test spaces --locked -- --ignored     (run 2)
test a_space_view_lists_its_rooms_by_level ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.95s
```
Passes twice. The test asserts level 0 ids == {room_a, room_b, sub}, level 1 ids == {room_c, sub} with sub last, and Debug "rooms: 3" with no IDs.

## CLI against the local stack prints counts only
Status: PROVEN
Setup: with curl as alice, created 3 rooms, a sub-space holding 1 room, and a top space holding 2 rooms plus the sub-space (names ev-*). Left them again afterwards.
```console
$ printf 'alice\n<pw>\n' ... | cargo run -q -p twicecat-core --bin twicecat-cli -- --ephemeral --homeserver http://127.0.0.1:8008 --password-stdin
Logged in as @alice:localhost
[spaces] 0 spaces: 
[spaces] 2 spaces: space level=0 rooms=3, space level=1 rooms=2
```
(Log lines trimmed; the first `[spaces]` is the pre-sync empty list.) Counts match the integration test (3 and 2 including the sub-space itself); no names or IDs are printed.

## App compiles and its tests pass with the field unused
Status: PROVEN
```console
$ cargo test -p twicecat-app
test result: ok. 1473 passed; 0 failed; ...   (lib)
... all other test binaries ok (8, 1, 0, 1, 3, 2, 2, 8, 138, 12 passed; 0 failed)
```

## The gate (fmt, clippy) and the commit message
Status: NOT PROVEN (not run here; commit/gate belong to the implementer)

## Verdict
EVIDENCE: PROVEN
