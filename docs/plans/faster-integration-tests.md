# Faster Homeserver Integration Tests Plan
Status: APPROVED (human, 2026-10-12)

## Goal
Cut the wall-clock time of `scripts/run-integration-tests.sh`, locally and in the CI `integration` job, without losing coverage or adding flakiness (issue ga-lep/twicecat#13). We measure first. Then we replace fixed sleeps with condition waits, stop first-sync cost growing during a run, reuse setup sessions instead of logging in again, split the CI `integration` job into 4 shards that each run their own stack, and overlap stack start-up with the build in CI. No test is dropped or newly ignored. The `keyring` and `hardware_` skips stay. No pinned dependency changes (AGENTS.md rule 3). Tests run only against the local stack and the `.env.test` accounts (rule 6). `crates/app` keeps talking to `crates/core` only through its command and event API (rule 7).

## Facts verified while planning (2026-10-12, branch `feat/faster-integration-tests` at `d568ac9`)
- `crates/core/tests/` holds 26 test files plus `common/mod.rs` (1178 lines) and `common/stack_env.rs`. Each file is its own binary. `crates/app/tests/` has 3 binaries; `bridge.rs` has 3 ignored homeserver tests.
- `run-integration-tests.sh` runs `prune-test-rooms.sh`, then `cargo test -p twicecat-core -p twicecat-app --locked -- --ignored --skip keyring --skip hardware_`. Cargo runs the binaries one after another. Tests inside one binary already run in parallel threads.
- Isolation today: `common::e2ee_serial()` is a per-binary `tokio::sync::Mutex`. It is safe only because binaries never overlap. `reset_to_b0` deletes every other device of the account and resets its identity, so it logs out any other core of that user running at the same time. 7 binaries take the lock or reset accounts: `e2ee_identity`, `e2ee_recovery`, `e2ee_verification`, `e2ee_rooms`, `e2ee_devices`, `media`, `log_hygiene`.
- **There is no sync filter in `common/mod.rs`.** The core syncs through `matrix_sdk_ui::sync_service::SyncService` (sliding sync, `crates/core/src/engine/mod.rs:2929`). First-sync cost comes from the SDK's per-room event-cache work (about 0.1 s per room in debug, see `prune-test-rooms.sh`), not from a filter. A filtered or lazy-loading `/sync` is not available on this path, so the plan keeps the room count small instead.
- **The wait helper from `5ebc5da` is not on this branch.** It lives only on the unmerged `feat/phase6-finish`. On `main`, `expect_event_within` panics with only "timed out waiting for an event". `git cherry-pick --no-commit 5ebc5da` applies cleanly on `d568ac9` (tried, then reverted).
- Fixed sleeps, `file:line`:
  - `call_media.rs`: 319, 333, 413, 560, 702, 729, 738, 810, 824, 839, 917, 1004, 1068, 1082, 1106. Most are audio-accumulation windows before `tone_level`/`level(hz, secs)`.
  - `call_signalling.rs`: 190, 258. Both are 25 s, on purpose: they wait past `DELAYED_LEAVE` = 18 s, a const in `crates/core/src/call/membership.rs:19`. It is restarted every `DELAYED_LEAVE_RESTART` = 4 s (`membership.rs:20`, used in `own_membership.rs:189`). `CoreConfig` (`crates/core/src/engine/mod.rs:78`) is built in 7 places: `common/mod.rs:116,181`, `login_policy.rs:27`, `twicecat-cli.rs:955`, `crates/app/src/main.rs:125`, `crates/app/tests/bridge.rs:163,279,382`.
  - `rooms.rs`: 289 (poll loop), 648 (3 s negative wait).
  - `e2ee_identity.rs`: 174, 278, 310 (poll loops).
  - `e2ee_recovery.rs`: 70, 607.
  - `e2ee_verification.rs`: 140, 715, 875 (poll loops).
  - `e2ee_rooms.rs`: 74.
  - `log_hygiene.rs`: 494.
  - `rtc_stack.rs`: 152.
  - `livekit_link.rs`: 52.
- `raw_login`/`raw_login_named` create a new device on every call. `matrix_sdk::authentication::matrix::MatrixAuth::restore_session` exists in the pinned 0.19.1 (`matrix-sdk-0.19.1/src/authentication/matrix/mod.rs:722`).
- Toolchain 1.97.1 is stable, so libtest's `--report-time` (it needs `-Zunstable-options`) is not available. `cargo-nextest` is not installed locally or in CI.
- CI run 38172367324 (`integration` job, 2026-10-11): stack start 30 s, user registration 2 s, and "Run homeserver integration tests" 9 min 32 s. That step includes the build, so build time and run time are not separated yet. Job timeout is 30 min. `Swatinem/rust-cache@v2` runs with its default key, which is per job.
- Tests that reach the SFUs directly: `call_media`, `call_signalling`, `rtc_stack`, `login_policy`. `call_ringing`, `call_settings`, `livekit_link` and `gpu_enhancement` need to be checked in Task 9.

## Decisions (2026-10-12, by the human)
- **D1. Step trailer: accepted.** Every commit carries `Step: ci-speed.N`, with N the task number below. Commit types on this branch: `feat`, `fix`, `docs` only (AGENTS.md rule 5). Test-only and CI changes fold into the step commit they belong to.
- **D2. `5ebc5da`: cherry-pick now.** Do not wait for `feat/phase6-finish`. The cherry-pick is Task 3, a prerequisite of Task 4. It is a `fix(core)`, which is allowed because `expect_event_within` is already on `main`. Known consequence: when `feat/phase6-finish` merges, rebasing this branch on `main` may conflict in `crates/core/tests/common/mod.rs`. Keep `main`'s version, and drop the cherry-pick if it becomes empty.
- **D3. Parallelism: 4 CI shards, each with its own stack (Task 7), instead of one merged `homeserver` binary with an account lock (changed 2026-10-12, by the human; supersedes the first D3).** A shard is a CI job with its own Synapse and test accounts, so shards cannot interfere. Tests inside one binary keep running in parallel threads as today, and binaries inside a shard run one after another. Task 8 is dropped. No `cargo-nextest`, and no new CI tool. Local runs are not sharded; they gain only from Tasks 4 to 6 and 9.
- **D4. Shorter delayed leave in tests: approved, as a core start-config change.** `CoreConfig` gets an optional `delayed_leave`. `call_signalling` sets it to about 4 s and waits about 6 s instead of 2 × 25 s. With `None`, the default behaviour (18 s delay, 4 s restart) is unchanged for the app, the CLI and every other test. This is folded into Task 6.

## Open question
- **D5. Target X%.** Stays open until the Task 2 baseline checkpoint, where the human sets it.

## What can be verified where
- **Locally, without the stack:** `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (non-ignored tests), `cargo test --no-run` build and link times, `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, the shard coverage check (Task 7), and shellcheck on the scripts.
- **Locally, with the stack (`scripts/stack.sh up -d --wait` + `scripts/create-test-users.sh`):** all timings, the ignored suite, and the 3 consecutive passing runs. If the controller's environment has no Docker stack, each of these is a **HUMAN** item.
- **CI only (HUMAN: push and read the run):** job step durations, whether the rust-cache key hits, the 3 consecutive CI passes, and the overlapped stack start-up.

### Task 1: Timing harness and a CI build/run split
Language: rust
Files: scripts/time-integration-tests.sh (new), scripts/run-integration-tests.sh, .github/workflows/ci.yml, PROGRESS.md
Depends on: none
Acceptance criteria:
- `scripts/time-integration-tests.sh` builds once (`cargo test -p twicecat-core -p twicecat-app --no-run --locked --message-format=json`) and prints the build seconds. It then runs every test binary that has ignored tests, with the same arguments as `run-integration-tests.sh`, and writes `binary<TAB>seconds<TAB>result` to `target/integration-timings/<UTC timestamp>.tsv`.
- `--per-test` also runs each ignored test alone (`--exact`) and writes `binary<TAB>test<TAB>seconds<TAB>result` rows. `--first-sync` prints, per test user, the joined-room count before the run (read through `joined_rooms`, room ids never printed).
- The script refuses a non-local homeserver (sources `require-local-homeserver.sh`) and never prints tokens, passwords, room names or message bodies. It passes `shellcheck`.
- In `ci.yml`, the `integration` job gets a separate "Build integration tests" step (`cargo test -p twicecat-core -p twicecat-app --no-run --locked`) before the run step, so the build and run durations show separately. Coverage is unchanged.
- The gate passes. No test changes.
Evidence: `scripts/time-integration-tests.sh` → prints build time and writes the TSV. `scripts/time-integration-tests.sh --per-test` → per-test rows. CI integration job → two separate step durations. Needs the stack (local: HUMAN if unavailable; CI: HUMAN).

### Task 2: Record the baseline (checkpoint)
Language: rust
Files: docs/plans/faster-integration-tests.progress.md (new), PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- The progress ledger records, from a fresh `create-test-users.sh`: build time; per-binary run time; the 15 slowest tests (`--per-test`); and the joined-room count per user at the start and end of a full run, as evidence for first-sync growth.
- It records the CI `integration` step durations of one run with the Task 1 split, and the rust-cache step's log line ("Cache restored from key …" or "No cache found").
- The same numbers are posted as a comment on issue #13 (`gh issue comment 13 --repo ga-lep/twicecat`).
- **HUMAN checkpoint:** the human sets X (D5) and confirms the order of Tasks 4 to 10 against the numbers. Tasks whose share is negligible may be dropped by the human, never silently. Task 3 is decided (D2) and is not up for dropping.
Evidence: `gh issue view 13 --repo ga-lep/twicecat --comments` shows the baseline comment. Docs only, `docs(plan)` commit.

### Task 3: Cherry-pick the `5ebc5da` wait-helper diagnostics (prerequisite, D2)
Language: rust
Files: crates/core/tests/common/mod.rs (from `5ebc5da`), PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- `git cherry-pick -x 5ebc5da` is applied on `feat/faster-integration-tests`. It applies without conflicts on `d568ac9` (checked while planning: only `crates/core/tests/common/mod.rs` changes). The commit stays `fix(core): say which event a test timed out waiting for`. It is amended only to replace the `Step: phase4.9` trailer with `Step: ci-speed.3`, keep the `(cherry picked from commit 5ebc5da…)` line, add the PROGRESS.md entry (rule 5), and carry this branch's attribution line.
- `expect_event_within` on a timeout panics with the caller location and the variant names of the last skipped events (never their fields). A non-ignored unit test, or `cargo test -p twicecat-core --test <binary> -- --list` plus a code read recorded in the ledger, shows this. The call sites are unchanged.
- PROGRESS.md and the ledger note: "`5ebc5da` is also on `feat/phase6-finish`. When that branch merges to `main`, rebasing this branch may conflict in `crates/core/tests/common/mod.rs`. Resolve by keeping `main`'s version and dropping the cherry-pick if it became empty."
- The gate passes.
Evidence: none — test-helper diagnostics only. `git log --format=%B -1` shows the `cherry picked from` line and `Step: ci-speed.3`.

### Task 4: Condition-wait helpers in `common`
Language: rust
Files: crates/core/tests/common/mod.rs
Depends on: Task 3
Acceptance criteria:
- The helpers build on the `expect_event_within` diagnostics from Task 3 (same caller-location and skipped-event reporting style).
- `common::poll_until(what: &str, timeout, interval, check: impl FnMut() -> Fut<Output = Option<T>>) -> T` polls an async condition and panics with `what`, the caller location (`#[track_caller]`) and the elapsed time on timeout.
- `common::assert_holds_for(what, window, interval, check)` is the explicit form of a negative wait ("nothing changes for `window`"). Every remaining fixed window goes through it with a one-line reason, so `grep -n "sleep(" crates/core/tests` lists only `common/mod.rs` and Task 5's audio-window helpers.
- Unit tests (not ignored, run in `cargo test --workspace`) cover: `poll_until` returns as soon as the condition holds (paused tokio time, `test-util` is already a dev-dependency); it panics with `what` on timeout; and `assert_holds_for` fails early when the condition breaks.
- No existing call site changes in this task. The gate passes.
Evidence: none — test-only helpers, exercised by `cargo test -p twicecat-core --test <binary> poll_until`.

### Task 5: Event-driven waits in `call_media.rs`
Language: rust
Files: crates/core/tests/call_media.rs
Depends on: Task 4
Acceptance criteria:
- The recorder gets `mark()` and `wait_level(hz, seconds, at_least, timeout)`. The latter polls the recorded level and returns as soon as the last `seconds` of audio after the mark reach `at_least`. Positive checks (tone heard, both directions heard, cleaning measured) use it instead of `sleep(3|4 s)` + `level`.
- Negative checks (after mute, intruder or stranger not heard, cleaned below a threshold) wait for exactly `seconds` of audio recorded after the mark (`wait_window(seconds)`) instead of a fixed sleep. The assertion thresholds are unchanged.
- The sleep after "carol leaves" (line 729) is replaced by an event wait, either on alice's participant list dropping carol or on the key-rotation event. The 500 ms poll loop (line 738) goes through `poll_until`.
- `grep -c "sleep(" crates/core/tests/call_media.rs` is 0. The thresholds and the number of tests are unchanged (`cargo test -p twicecat-core --test call_media -- --list --ignored` count equal before and after).
- With the stack: `call_media` passes 3 times in a row, and its run time, compared with the Task 2 baseline, is recorded in the ledger.
Evidence: `scripts/time-integration-tests.sh` row for `call_media` drops against the baseline (needs the stack; HUMAN if unavailable).

### Task 6: Event-driven waits in the other binaries
Language: rust
Files: crates/core/tests/{rooms,e2ee_identity,e2ee_recovery,e2ee_verification,e2ee_rooms,log_hygiene,rtc_stack,livekit_link,call_signalling,login_policy}.rs; crates/core/tests/common/mod.rs; crates/core/src/engine/mod.rs (`CoreConfig`); crates/core/src/engine/calls.rs; crates/core/src/call/{mod,membership,own_membership}.rs; crates/core/src/bin/twicecat-cli.rs; crates/app/src/main.rs; crates/app/tests/bridge.rs (each only adds `delayed_leave: None` to its `CoreConfig` literal)
Depends on: Task 4
Acceptance criteria:
- The poll loops (`rooms.rs:289`, `e2ee_identity.rs:174,278,310`, `e2ee_recovery.rs:70,607`, `e2ee_verification.rs:140,715,875`, `log_hygiene.rs:494`, `rtc_stack.rs:152`) use `poll_until`, or `expect_event` when a core event signals the condition.
- `rooms.rs:648` and `e2ee_rooms.rs:74` become either an event wait or `assert_holds_for` with the reason written down. `livekit_link.rs:52` (std thread sleep) is replaced or documented.
- Delayed leave (D4, approved as a core start-config change, not a command-API change): `CoreConfig` gets `pub delayed_leave: Option<Duration>`, documented next to `call_service_override`. `None` keeps today's behaviour exactly: an 18 s delay (`DELAYED_LEAVE`) restarted every 4 s (`DELAYED_LEAVE_RESTART`), the Element Call defaults. `Some(d)` sets the delay to `d`, and the restart interval to `min(4 s, d / 4)`, so a restart always lands well inside the delay. The value reaches `schedule_delayed_leave` and `keep_membership` through the `OwnMembership` built in `call/mod.rs`, not through a global.
- Unit tests (not ignored): `None` gives 18 s and 4 s, and `Some(4 s)` gives 4 s and 1 s. The existing `membership.rs` unit tests pass unchanged.
- Every other `CoreConfig` literal passes `delayed_leave: None`: the CLI, `crates/app/src/main.rs`, `crates/app/tests/bridge.rs`, `login_policy.rs`, and `common`'s default config. So the app, the CLI and every test except `call_signalling` keep the 18 s default. `crates/app` takes no new dependency, `scripts/check-core-api.sh` and `scripts/check-app-deps.sh` pass, and `Duration` is `std`.
- `call_signalling` starts its core with `delayed_leave: Some(4 s)`. The two 25 s sleeps (`call_signalling.rs:190,258`) become `assert_holds_for("membership kept past the delayed leave", 6 s, …)`, a window of delay + 2 s. It checks the membership at each interval, not only at the end. The comments say "past the configured delayed leave" instead of "18 s". `grep -n "from_secs(25)" crates/core/tests/call_signalling.rs` is empty.
- No test is removed. The ignored-test count per binary is unchanged (`--list --ignored`).
- With the stack: each touched binary passes 3 times in a row.
Evidence: `scripts/time-integration-tests.sh` per-binary rows against the baseline (needs the stack; HUMAN if unavailable).

### Task 7: Shard the CI integration job into 4 jobs
Language: rust
Files: scripts/integration-shards.sh (new), scripts/run-integration-tests.sh, scripts/time-integration-tests.sh, .github/workflows/ci.yml
Depends on: Task 2
Acceptance criteria:
- `scripts/integration-shards.sh` holds the shard table: shard name (`1` to `4`) to its test binaries (core and app). The shards are balanced by the per-binary seconds recorded in the Task 2 baseline, which must come from a run with no failures. The table and the sums per shard are written into the ledger.
- `run-integration-tests.sh --shard N` runs the pruning, then `cargo test --locked` with one `--test <binary>` per binary of that shard (`-p` chosen from the binary's crate), and the same trailing `--ignored --skip keyring --skip hardware_` as today. With no `--shard`, behaviour is unchanged: everything runs.
- Nothing is dropped. A check, in the same script (`integration-shards.sh --check`, also run in the CI lint job and passing locally without the stack), fails if a test binary that has `#[ignore = "needs local homeserver…"]` tests is in no shard or in two shards. A new test file therefore fails the lint until it is placed. The unit-test side (`cargo test --workspace`) is untouched.
- CI: `integration` becomes a matrix job of 4 shards (`fail-fast: false`), each with its own checkout, build (rust-cache with a per-job `shared-key`), local Synapse, test users and `--shard N` run, plus a final `integration-ok` job that needs all four and is the required check. The open-files print stays in the run step.
- The ignored-test count across the four shards equals the baseline count.
- With CI (**HUMAN**: push and read the runs): 3 consecutive green runs, and the wall time of the slowest shard against the Task 2 baseline.
- The gate passes.
Evidence: `scripts/integration-shards.sh --check` → exit 0; `scripts/run-integration-tests.sh --shard 1` (needs the stack; HUMAN if unavailable); the CI run's per-shard durations (**HUMAN**: `gh run view <id> --json jobs`).

### Task 8: Dropped (D3)
The merged `homeserver` binary, the process-wide account lock and the `check-test-isolation.sh` audit are dropped in favour of Task 7. The number stays so later tasks keep theirs.

### Task 9: Keep first-sync cost flat and reuse setup sessions
Language: rust
Files: crates/core/tests/common/mod.rs, crates/core/tests/*.rs, scripts/run-integration-tests.sh
Depends on: Task 6
Acceptance criteria:
- Room cleanup: `common` gets a `CreatedRooms` record. `create_room`/`create_encrypted_room`/`create_dm`/`create_space`/`create_room_raw`/`create_space_raw` register the room. Each test's finish path (`EncryptedPair::finish` and the equivalent end of the other tests) leaves and forgets the rooms it created, for every invited test user, through the existing `leave_all_created`. Rooms of a panicking test are cleaned by the next `prune-test-rooms.sh`, which stays as the pre-run safety net.
- After a full passing run, `scripts/time-integration-tests.sh --first-sync` reports a joined-room count per user that does not exceed a small bound set from the Task 2 numbers (for example ≤ 5), instead of growing with the run.
- Setup-session reuse: `common::setup_client(n)` logs in once per process and caches the `MatrixSession` (never printed). Every later call builds a fresh `Client` in the calling test's runtime and calls `matrix_auth().restore_session(...)`, so no client crosses `#[tokio::test]` runtimes. A generation counter, bumped in `reset_to_b0` while the binary's existing `e2ee_serial()` lock is held, forces a re-login after a reset. Setup-only `raw_login` call sites (room creation, invites, sends that do not depend on device identity) switch to it. e2ee call sites whose device identity matters keep `raw_login_named`.
- The number of devices left per user after a full run is no higher than before (`prune-test-devices.sh` output, recorded in the ledger).
- The gate passes. With the stack, the full suite passes 3 times in a row.
Evidence: `scripts/time-integration-tests.sh --first-sync` before and after counts; the full-run wall time against the baseline (needs the stack; HUMAN if unavailable).

### Task 10: CI: overlap stack start-up, check the cache
Language: rust
Files: .github/workflows/ci.yml, scripts/stack.sh (only if a non-waiting start needs a flag)
Depends on: Task 7
Acceptance criteria:
- The `integration` job starts the stack detached (`scripts/stack.sh up -d`, no `--wait`) before "Fetch pinned libwebrtc" and the build. After the build it runs `scripts/stack.sh up -d --wait`, which returns at once if the stack is already healthy, then `create-test-users.sh`. The 30 s start-up runs under the build.
- `Swatinem/rust-cache@v2` gets an explicit `shared-key` for the `integration` job and saves only on `main` (`save-if: ${{ github.ref == 'refs/heads/main' }}`), so pull requests restore the main cache instead of writing their own. A CI run on the PR shows "Cache restored from key" in the rust-cache step (**HUMAN**: read the log).
- Decision recorded in the ledger, from the Task 2 and Task 7 numbers: does the 30 s stack start-up still sit on a shard's critical path once it overlaps the build? Write down the measured saving per shard.
- `actionlint` (if available) or a CI run validates the workflow. The lint job passes.
Evidence: the CI `integration` job's step durations (no separate 30 s "Start local Synapse" wait on the critical path) and the rust-cache restore line. **HUMAN**: push, then `gh run view <id> --json jobs`.

### Task 11: After-timings, docs, three green runs (checkpoint)
Language: rust
Files: scripts/run-integration-tests.sh (comment), scripts/prune-test-rooms.sh (comment), PROGRESS.md, docs/plans/faster-integration-tests.progress.md
Depends on: Task 9, Task 10
Acceptance criteria:
- The `run-integration-tests.sh` header states the new typical wall time (local and CI, per shard), the repeat-run recipe (`create-test-users.sh`, then the script, repeatable without `down -v`), and the `--shard N` option with a pointer to `integration-shards.sh`.
- The ledger and a comment on issue #13 record the after-numbers next to the baseline, with the percentage cut against the target X.
- 3 consecutive local passes and 3 consecutive CI passes, with no new flakes, are recorded with their run IDs. Local passes need the stack (HUMAN if unavailable); the CI passes are **HUMAN**.
- The ignored-test count across all binaries equals the baseline. `--skip keyring --skip hardware_` is unchanged. `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` pass.
- The gate passes.
Evidence: `gh issue view 13 --repo ga-lep/twicecat --comments` shows before and after. `scripts/run-integration-tests.sh` passes 3 times (HUMAN where the stack or CI is needed).
