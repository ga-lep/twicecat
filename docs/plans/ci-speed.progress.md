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
