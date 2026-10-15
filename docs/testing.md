# Testing

Every test has a tier. The tier says what a failure means, where the test runs and whether it can block a merge. Plan: `docs/plans/ci-speed.md`.

| Tier | A failure means | Runs | Blocks a merge |
|---|---|---|---|
| 1. Validation: unit, property, golden and pixel, layout | The code does something wrong. Deterministic: no network, no wall clock | `lint` and `test`, every push | Yes |
| 2. Integration: the `#[ignore]` tests that need the local homeserver and LiveKit | A feature is broken across components | the 4 `integration` shards | Yes |
| 3. Regression: a test written for a fixed bug | The bug is back | tier 1 or 2, named `regression_<what>`, with the issue or commit in its doc line | Yes |
| 4. Performance indicators: `perf_` tests, the profile examples | Nothing. A number moved | the `perf` workflow, one test at a time | Never |
| 5. Human: `hardware_`, `keyring` | Real devices behave differently | by hand, listed in `PROGRESS.md` | No |

## Rules
- **R1. A gate never asserts a rate or a duration of the machine.** It asserts a behaviour: order, completeness, content, state, "it made progress". "Made progress" uses a floor far below the nominal rate, so that a stalled stream fails and a slow runner does not (video: at least 5 fps against 30).
- **R2. A wall-clock bound in a gate guards against a hang.** It is at least 1 s and at least two orders of magnitude above the typical time. When a hang already blocks the test by itself, there is no bound.
- **R3. Speed has an expectation, not a threshold.** A `perf_` test records a value with `twicecat_perf::record(name, value, unit, Expect::AtLeast(28.0))`. Outside the expectation the run prints `perf: ... OUTSIDE` and, under GitHub Actions, a `::warning` annotation. The test still passes.
- **R4. Performance is measured alone.** `scripts/run-perf-tests.sh` runs one `perf_` test at a time. A rate measured in the middle of a parallel run measures the contention, not the code.
- **R5. A test that has both a behaviour and a speed to check is split.** The behaviour stays a gate. The speed goes to a `perf_` test that reuses the same setup helper.

## Naming
The prefix is the selector, as `hardware_` already is.
- `perf_`: tier 4. The gates skip it: `cargo test --workspace -- --skip perf_`. The tests never fail on a value, so the plain `cargo test --workspace` of the agent gate runs them harmlessly.
- `hardware_`: tier 5, needs a device. `keyring`: tier 5, needs a desktop session. Both are skipped in the CI shards.

## Recording an indicator
```rust
use twicecat_perf::{Expect, record};

// In a dev-dependency: twicecat-perf = { path = "../perf" } (std only).
record("call_video.camera.received_fps", fps, "fps", Expect::AtLeast(28.0));
```
`$TWICECAT_PERF_FILE`, when set, collects one JSON line per value. `scripts/perf-report.sh CURRENT.jsonl [BASELINE.jsonl]` turns the file into a table with the change against an earlier run. Neither ever fails a build.

## Where the numbers go
The `perf` workflow runs on pushes to `main`, on demand and on a pull request with the `perf` label. It uploads `perf.jsonl` and writes the table into the job summary, with the change against the last successful `main` run. It is not a required check. The table renders on the run's **Summary** page (the first entry on the left of the run, above the jobs); the step's log shows the same table as plain text.
