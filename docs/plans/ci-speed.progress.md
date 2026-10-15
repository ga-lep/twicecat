# CI speed and test tiers: ledger

Plan: docs/plans/ci-speed.md. Branch: `feat/ci-speed`, from `main` at `38115e8`.

## Baseline (CI, before this plan)
Measured from runs `38709253505` (28 min Windows) and `38648005981` (41 min Windows, last `main`), and from PR #27's run `38716596416`.

| Job | Wall clock | What dominates |
|---|---|---|
| Windows `test` | 28–41 min | 15 min `cargo test` build, 4 min clippy, 97 s libwebrtc fetch, 3–4 min cache save |
| `integration` shards | 15–33 min | 5–13 min building the same ~35 binaries, 7–13 min running |
| ubuntu `test` | 14 min | 11 min build, 75 s `ui_gallery` |
| `lint` | 7–8 min | clippy 5.4 min, cheap checks after it |

Shard run times in the last run: 444, 647, 436 and 797 s.

Speed gates that failed without a bug: `call_video` fps (27.1 on CI, 26.3 locally; `PROGRESS.md` Phase 9 Task 6 also records 27.5), the timeline batch median (loosened for CI once already).

## Log
- Task 1: plan and ledger written. Waiting for the human's confirmation of the tiers and D1 to D7, and an answer to D6.
- Task 2: done (ci-speed.2). `crates/perf` (`twicecat-perf`, std only, 11 unit tests), `scripts/perf-report.sh` (jq; checked on a sample with a baseline, without one, with a NaN and a repeated name; exit 0 whatever the numbers, 1 for an unreadable file), `docs/testing.md`. Checks: fmt; clippy `--workspace --all-targets --all-features`; `cargo test -p twicecat-perf`; check-app-deps and check-core-api. Not run: `cargo deny` (not installed locally; CI runs it), shellcheck (not installed).
- Task 3: done (ci-speed.3).
  - Converted to indicators (the value is recorded with its old threshold as the expectation): `call_video` camera (delivered, received and sent fps; the hardware test's per-quality fps), `cli` 720p rate, `timeline` batches (5 shapes), `room_list` batches (4 shapes), `timeline_pane` view build, DeepFilterNet3 CPU frame time, ONNX Runtime encoder and GPU frame times, PipeWire real-time late wakes and overruns.
  - Split into a gate and a `perf_` test: `call_video` camera (`a_test_pattern_reaches_the_other_member_in_order_in_an_encrypted_call` + `perf_a_camera_reaches_the_viewer_at_the_frame_rate`, one helper), `cli` (`two_clis_send_and_receive_a_test_pattern` + `perf_two_clis_receive_a_test_pattern_at_the_frame_rate`), `timeline_batches_do_not_take_seconds` + `perf_timeline_batches`, `room_list_batches_do_not_take_seconds` + `perf_room_list_batches`, `building_the_view_for_10000_items_does_not_take_seconds` + `perf_building_the_view_for_10000_items`. Gate floors: 5 fps for a feed (R1), 1 s for a batch (R2). The tier-5 tests (audio, hardware video) record inline.
  - Hang guards relaxed from 50 ms to 5 s: `bridge.rs`, `state.rs` ("a full sink does not block").
  - Kept, on purpose: `ptt.rs:299` (500 ms of audio samples, not of the clock), `gpu.rs:218` (median of 25 calls of a no-op closure under 20 ms: margin of about 10^6 and part of `keeps_up()`'s logic), `rich_text`, `avatars`, `devices` hang guards (5 s or 1 s against milliseconds).
  - Gates skip `perf_`: `ci.yml` (`cargo test --workspace -- --skip perf_`) and `run-integration-tests.sh`.
  - Test counts: app lib 1031 before, 1034 after (3 timing tests became 3 gates and 3 `perf_` indicators); core `call_video` 3 to 4 tests, `cli` 4 to 5.
  - **The case that failed in CI, on purpose:** 48 busy loops on 24 cores (2 per core) while `call_video` and `cli` ran, twice. The camera test delivered 27.1 and 25.6 fps, both under the old 28 fps gate, and passed both times; `cli` passed both times. On the quiet machine afterwards the indicators read 29.8 fps delivered, 30.0 received, 30.0 sent and 30 fps in the CLI, 0 outside their expectation.
  - Checks: fmt; clippy `--workspace --all-targets --all-features`; `cargo test --workspace -- --skip perf_` (all pass); `scripts/integration-shards.sh --check`; `scripts/run-perf-tests.sh --check`.
  - **Expectations tightened from the first CI perf run (2026-10-15)** instead of the old 16 ms and 20/25/28 fps: the room-list batches 2 ms (1 ms for the mixed one; measured 0.4 to 1.1 ms), the timeline batches 8 to 12 ms (measured 4 to 7.5 ms on CI, 2.5 to 4 on a laptop; about 1.6 times the worst), the view build 0.5 ms (0.1 to 0.16), the camera and CLI rates at least 29 fps (29.6 to 30 measured). The 16 ms frame budget stays the line a user would feel, in the comments. They are indicators: revisit them when the numbers move, never turn them into gates.
- Task 4: done (ci-speed.4), the parts that run locally. `scripts/run-perf-tests.sh` (all, `--no-stack`, `--check`): run with the stack, 5 tests and 15 values, 0 outside; without it, 3 tests and 10 values. `perf.yml` written: pushes to main, on demand, PRs with the `perf` label; one job, `continue-on-error`, not in `integration-ok`; downloads the last successful main artifact for the change column; uploads `perf.jsonl`; the table goes to the job summary. YAML parsed with PyYAML (through `uv`); **actionlint is not installed here and was not run**. **HUMAN:** `gh workflow run perf.yml --ref feat/ci-speed`, then read the summary (the first run has no baseline).
- Task 5: done (ci-speed.5), the parts that run locally. In `ci.yml`: `concurrency` (a pull request's newer push cancels its older run; every other run has its own group, its commit, so a run on main is never cancelled or replaced while waiting); the lint job split into `checks` (commit format, shard and perf checks, fmt, cargo-deny, the dependency-graph scripts: nothing compiles, so a mistake shows in about a minute) and `lint` (clippy and the libwebrtc check, with the toolchain setup and the cache); every `rust-cache` of `lint` and `test` saves only on main; docs-only pull requests (every changed file is `*.md` or under `docs/`) skip `lint`, `test` and the shards through a `changes` job instead of `paths-ignore`, which would leave required checks pending; `integration-ok` accepts skipped shards only for a docs-only change; `ci-ok` is the new aggregate check (fails on any failure or cancellation, passes on skips).
  - The detection shell was checked on five file lists with the real `grep` (the shell here wraps `grep` in a function that mishandles `-qv`). YAML parsed; **actionlint not installed, not run**.
  - **Rebase note:** PR #27 (`feat/ui-ux-rework`) adds two lines to `ci.yml` that `main` does not have: `cargo check --workspace --lib --bins --locked` (now to go in `lint`) and `scripts/check-font-assets.sh` (now to go in `checks`). The first rebase of one branch onto the other conflicts there; keep both lines in the new jobs. It also adds `twicecat-app:ui_gallery` to `NOT_SHARDED` in `integration-shards.sh`.
  - **HUMAN:** two quick pushes to a draft pull request show the first run cancelled; a docs-only push shows only `changes`, `checks` and `ci-ok` running; the durations of `checks` against the old 8 minutes to the first failure.
- Task 6: done (ci-speed.6), the parts that run locally.
  - **Found in run `38716596416` (per-shard, seconds):** the "Build integration tests" step took 593 / 676 / 800 / 869 s (shards 1 to 4) and compiled all 35 binaries; the run step then compiled again for 327 / 511 (117 + 394) / 473 / 637 (150 + 487) s, because it made one cargo call per package with other targets. The tests themselves took 198 / 398 / 256 / 425 s.
  - **Now:** one `cargo test -p twicecat-core -p twicecat-app --test … --lib` call per shard, made by `run-integration-tests.sh --shard N [--no-run]`; the CI build step and the run step make the same call. `--check` also fails when a test file name exists in both packages.
  - **Rebalanced** by measured seconds plus 20 s of linking per binary (weight / run seconds):

    | Shard | Binaries | Weight | Run |
    |---|---|---|---|
    | 1 | bridge, call_signalling, e2ee_verification, core lib, timeline_actions, timeline_live (6) | 462 | 342 |
    | 2 | call_video, cli, e2ee_devices, e2ee_recovery, log_hygiene, oauth, timeline (7) | 457 | 317 |
    | 3 | call_ringing, calls, login, media, notifications, rooms, rtc_stack (7) | 457 | 317 |
    | 4 | avatars, call_log_hygiene, call_media, e2ee_identity, e2ee_rooms, room_list, threads, timeline_items (8) | 470 | 310 |

  - **Local, with the stack:** all four shards pass (rc 0 each, 3 to 4 minutes each), and no run step compiled anything. 28 binaries in 4 shards, as before.
  - **HUMAN:** the per-shard build and run step durations on CI, against the numbers above.
- Task 7: done (ci-speed.7), the parts that run locally.
  - **Measured first** (run `38716596416`, steps over 15 s): Docker cargo-deny action 61 s in `lint`; libwebrtc 58 s on Windows, 21 s on macOS; rust-cache restore 72 s Windows, 58 s macOS; the models, the CUDA header and Linux libwebrtc under 15 s; the stack start about 30 s on a shard's critical path. So the Docker images and the models are not cached (a restore costs about what a pull or a download does), and the stack start is overlapped with the build instead.
  - `.github/actions/setup-build` (Linux dependencies, clang 22, libwebrtc from the Actions cache keyed on the checksum file and the fetch script, restore everywhere, save only on main; the script still downloads and verifies on a miss; CUDA header) replaces four copies in `lint`, `test`, `integration` and `perf`.
  - `scripts/fetch-cargo-deny.sh` and `scripts/checksums/cargo-deny.sha256`: cargo-deny 0.20.2, x86-64 Linux, SHA-256 from GitHub's asset digest (it matched the download). `cargo deny --workspace check` with it: advisories, bans, licenses and sources ok, with the new `twicecat-perf` crate.
  - Shards start `stack.sh up -d` in the background (own session) as the first step and wait for it after the build.
  - **Not verified:** nothing of this has run on CI; actionlint not installed. **HUMAN:** two consecutive runs on the same pins (the first saves the libwebrtc cache on main, the second restores) and the step durations; revert the libwebrtc cache if it is not faster on Windows and macOS.
- Task 8: done (ci-speed.8), the parts that run locally. The plan's "merge the test files into about 4 binaries" was measured and read, and is not done:
  - **Link cost** (`cargo build --timings` after touching `crates/core/src/lib.rs`, 24 cores): 31 core test units at 8–20 s each (they run in parallel, so each looks longer), `twicecat-core` lib 5 s; 169 s of system time, the cost of writing ~17 GB of binaries. After Task 6 a shard links 6 to 8 of these, about 20–30 s of CPU on a 4-core runner. Merging them into 3 per shard would save at most 15 s per shard.
  - **Not safe to share a process:** `log_hygiene` and `call_log_hygiene` install the global tracing subscriber (`try_init`) and scan all of its output for sentinels, so other tests' logs would enter the scan; `tls_provider` installs rustls's process-wide default provider; `call_video_fallback` calls `std::env::set_var`; `common::E2EE_LOCK` and `DM_LOCK` and the `SERIAL`/`CAROL_PROFILE` mutexes are per binary today (merged they would serialise across files); `reset_to_b0` deletes the other devices of an account and resets its identity, and only the 7 files that take `e2ee_serial()` are safe against it, while the files today run one after the other. Merged, libtest would also start running the tests of different files in parallel, which loads the machine and moves the audio-window tests.
  - **The linker** of the x86 Linux test binaries is already LLD 22 (`readelf -p .comment`). **Debug info:** `--config 'profile.dev.package.twicecat-*.debug="line-tables-only"'` left `login` at 508 413 400 bytes, as before. Neither is changed. (Windows and macOS are not measurable here.)
  - **What does cost:** 26 of the 36 integration test files have only `#[ignore]` tests, and every OS job links them for "0 passed". `scripts/unit-test-targets.sh` skips them in the `test` job (`--lib --bins --examples --test …`, with `cargo test --doc` apart). Counts, locally: `cargo test --workspace` 1794 passed, 158 ignored; the selected targets plus `--doc` 1794 passed, 19 ignored (the other 139 run in the shards).
  - **HUMAN:** the `test` job step durations on CI.

## CI measurements, PR #29 (draft), runs `38730680927` (Tasks 1 to 7) and `38733670661` (Task 8 added)
All jobs green in both, `ci-ok` too; the perf workflow green (run `38730695735`: 15 values, 0 outside; the camera test delivered 29.7 fps on the quiet runner, the CLI 30 fps; its `perf_` step took 1262 s, mostly a cold build).

| Job (seconds) | Before `38716596416` | Run 1 | Run 2 |
|---|---|---|---|
| checks (new) / first failure | about 480 (a shard-list error came after clippy) | 26 | 29 |
| lint | 428 | 387 | 332 |
| test ubuntu | 722 | 643 | 613 |
| test ubuntu-arm | 826 | 640 | 540 |
| test windows | 830 | 942 | 635 |
| test macOS | 588 | 1401 | 1382 |
| integration shard 1 / 2 / 3 / 4 | 1212 / 1705 / 1632 / 2026 | 1174 / 856 / 1180 / 745 | 1060 / 1030 / 967 / 976 |
| slowest job | 2026 (shard 4) | 1401 (macOS) | 1382 (macOS) |

- **Shards:** the slowest went from 2026 s to 1060 s, and the run step no longer compiles (shard 2: run step 921 s to 253 s; shard 4: 1066 s to 250 s). Run-to-run variation of one shard is about 30 % (shard 4: 745 and 976 s), so read these as ranges.
- **Not comparable for the `test` jobs:** the "before" run restored a cache that PR #27 had saved for itself (every run of a PR saved then); these runs restore only `main`'s, and on macOS `rust-cache` finds none (`No cache found`), so macOS builds cold every time until `main` saves one: clippy 350 s (was 36 s) and the unit tests 973 s. The numbers for Windows, ubuntu and arm are on a cold-ish cache as well. They improve once `main` has run with these keys.
- **Task 8, same cache state, run 1 to run 2:** Windows 942 to 635 s (unit tests 356 s, doc tests 43 s), arm 640 to 540 s, ubuntu 643 to 613 s, macOS 1401 to 1382 s (macOS is dominated by the cold build, not the links).
- **libwebrtc cache:** `Cache not found` on both runs, as expected: only `main` saves. Windows `setup-build` took 94 to 99 s, which is the uncached fetch. Whether the cache is faster is read on the first run after `main` has saved it.
- **Still to read after a merge to main:** warm-cache `test` and `lint` durations; the libwebrtc restore against the 58 s fetch on Windows; the perf workflow's second run with a baseline.
- Task 9: done (ci-speed.9) for D6, as the human decided on 2026-10-15 (Windows and macOS only on ready pull requests; Task 8's merging dropped, D7). `changes` outputs the `test` matrix (two Linux legs on a draft pull request, four otherwise); `ready_for_review` triggers a run. The `ui_gallery` half waits for PR #27's rebase. **HUMAN:** a draft run with two legs, then a ready run with four.
- Task 10: done (ci-speed.10). CI run `38736783282` failed in shard 1 (`reaction_toggles_on_and_off_for_both_sides` at `timeline_actions.rs:96`, `typing_notices_reach_the_other_user` at `timeline_live.rs:332`); both tests had failed in earlier runs (`38722052074`, `38636816641`, `38453477513`). My cancel of the redundant runs missed this one, so it ran to the end.
  - **Reproduced locally:** 30 busy loops on 24 cores, the reaction test alone: 6 of 8 failed (several different waits: "carol sees her own reaction", "carol sees it removed", once a login that took over 30 s). With the timeout message extended to list the timeline, a failing run showed carol's list holding item `11` twice, once without and once with the reaction.
  - **Cause:** `apply_until` in `timeline_actions.rs` and `timeline_live.rs` broke out of the loop over a batch's diffs as soon as `done` held, so the rest of the batch was never applied and the mirrored list diverged from the SDK's. Fix: apply every diff, remember that `done` held. 16 of 16 pass under the same load (two runs of 8); the full `timeline_actions` (12) and `timeline_live` (4) binaries pass on the quiet stack.
  - **Not fixed, left for the human (AGENTS.md rule 8):** `typing_notices_reach_the_other_user` passed 8 of 8 under the same load, so the cause is not found; it failed in 3 of about 30 recent CI runs. It may be a real issue of typing notices over sliding sync. Proposed: a follow-up that records what carol's core receives on a failure.
  - **Second CI run on the fix (`38756123588`, draft, two Linux legs):** everything green except shard 1's `typing_notices_reach_the_other_user` again (twice in a row: the rule 8 stop). Locally, on a quiet stack, the typing test passes alone and after `e2ee_verification`.
  - **What else the pinned reproduction showed** (shard 1's sequence on 4 cores with 8 busy loops pinned to them, the 30 s waits): other waits in `timeline_actions` time out too (`redact_turns_the_item_into_a_placeholder`, `edit_keeps_the_item_id_and_marks_it_edited`, the reaction test once more), so the 30 s `DEFAULT_TIMEOUT` is too tight for a loaded 4-core runner, which runs Synapse, LiveKit and two debug cores at once. In one failure the list had 19 items with unique IDs from 13 up: the timeline had been rebuilt, which changes the IDs a test took earlier.
  - **Changes:** `common::DEFAULT_TIMEOUT` is 30 s, or `TWICECAT_TEST_TIMEOUT_SECS` read when the tests are built; the shards set 90 (`ci.yml`, `docs/testing.md` R2b). The typing test makes `max(4, timeout / 8)` tries of 8 s (4 as before locally, 11 on CI). An attempt to resend the notice every second instead made it fail: Synapse reports a change, and a notice sent while one is running changes nothing, so a try has to end with a false. A timeout message of `timeline_actions` lists the diffs seen and the items.
  - **Check:** the shard 1 sequence (`bridge`, `call_signalling`, `e2ee_verification`, `timeline_actions`, `timeline_live` with the core lib) on a fresh stack, pinned to 4 cores with 8 busy loops, 90 s timeout: all green once (typing took 60 s: several tries were needed, so delivery of a typing notice to Carol is slow or lost in that state; not explained). Runs after a killed run are not valid (a killed test leaves a device behind: "other devices survived the reset"); I discarded those.
  - **Open (human):** why a typing notice sometimes takes longer than 8 s to reach Carol on a loaded runner; whether item IDs should survive a timeline rebuild (the API says "stable within its timeline").
