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
