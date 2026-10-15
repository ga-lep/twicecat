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
- **R2b. A wait for an event has a generous timeout that CI scales.** `common::DEFAULT_TIMEOUT` is 30 s, and 90 s in the integration shards (`TWICECAT_TEST_TIMEOUT_SECS`, read when the tests are built). A shared 4-core runner runs Synapse, LiveKit and two debug cores at once; a timeout only ends a wait that has failed, so a long one costs a passing run nothing. A test that must see something happen resends or retries until then, as a client does, and does not rely on one message arriving within a window.
- **R3. Speed has an expectation, not a threshold.** A `perf_` test records a value with `twicecat_perf::record(name, value, unit, Expect::AtLeast(28.0))`. Outside the expectation the run prints `perf: ... OUTSIDE` and, under GitHub Actions, a `::warning` annotation. The test still passes.
- **R4. Performance is measured alone.** `scripts/run-perf-tests.sh` runs one `perf_` test at a time. A rate measured in the middle of a parallel run measures the contention, not the code.
- **R5. A test that has both a behaviour and a speed to check is split.** The behaviour stays a gate. The speed goes to a `perf_` test that reuses the same setup helper. A human-run test (tier 5, which nothing gates on) may record inline instead.

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

## Running them
```bash
scripts/run-perf-tests.sh             # all perf_ tests; the #[ignore] ones need the local stack
scripts/run-perf-tests.sh --no-stack  # only those that need no homeserver
scripts/run-perf-tests.sh --check     # every file with a perf_ test is listed in the script
```
The values go to `target/perf/<timestamp>.jsonl`. A new `perf_` test in a file the script does not list fails `--check`, which the `lint` job runs, in the same way `integration-shards.sh --check` places a new homeserver test in a shard.

## Where the numbers go
The `perf` workflow runs on pushes to `main`, on demand and on a pull request with the `perf` label. It uploads `perf.jsonl` and writes the table into the job summary, with the change against the last successful `main` run. It is not a required check. The table renders on the run's **Summary** page (the first entry on the left of the run, above the jobs); the step's log shows the same table as plain text.

## Which legs run when
- Every push: `changes`, `checks`, `lint`, the Linux `test` legs (x86-64 and aarch64) and the 4 integration shards.
- Also Windows and macOS: on `main`, on a manual run, and on a pull request that is **ready for review**. A draft pull request skips them; marking it ready starts a full run. They stay required for a merge, because they find real bugs Linux cannot see (a Cmd-versus-Ctrl key test).
- A pull request that changes only markdown and `docs/` skips `lint`, `test` and the shards.
