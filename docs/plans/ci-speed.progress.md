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
