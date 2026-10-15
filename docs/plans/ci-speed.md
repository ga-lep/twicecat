# CI Speed and Test Tiers Plan
Status: DRAFT (scope set by the human in chat, 2026-10-15: do the CI speed-ups, and separate validation, regression and performance tests, with performance as an indicator, not a gate). The decisions below are proposals until the human confirms them at the Task 1 checkpoint.

## Goal
Two things, in this order of importance.

1. **A red CI always means a bug.** Today three tests gate on speed: `call_video` (at least 28 fps), `cli` (at least 25 fps) and the timeline batch tests (a median under 16 ms or 50 ms). They fail on a busy or shared runner with nothing wrong in the code. Shard 3 of PR #27 failed that way twice (26.3 fps locally, 27.1 fps on CI). We split the tests into tiers by what a failure means. A gate fails only for a wrong behaviour. Speed and timing are measured in their own job, reported, compared with an expectation and with the last `main` run, and never fail a build.
2. **A faster CI.** Measured on run `38709253505` (Windows 28 min) and `38648005981` (Windows 41 min, last `main`): Windows `test` 28–41 min, the 4 integration shards 15–33 min, ubuntu `test` 14 min, `lint` 7–8 min. Most of it is building and linking about 35 test binaries of 500 MB, each linking matrix-sdk and libwebrtc, five times per run (4 shards and the ubuntu `test` job). We cut the repeated builds, the repeated downloads, the wasted runs, and then the number of binaries.

No test is dropped, no pinned dependency changes (AGENTS.md rule 3), tests keep running only against the local stack and `.env.test` accounts (rule 6), and `crates/app` keeps talking to `crates/core` only through its command and event API (rule 7).

## Facts verified while planning (2026-10-15, branch `feat/ci-speed` at `38115e8`, base `main`)
- Tests that gate on speed or timing today:
  - `crates/core/tests/call_video.rs:142` `fps >= 28.0` (the camera test), `:157` received and sent `fps > 20.0`, `:707` hardware `min_fps` per quality (human-run, NVENC).
  - `crates/core/tests/cli.rs:298,306` the viewer reaches 25 fps at 1280x720.
  - `crates/app/src/timeline.rs:2053` and `crates/app/src/screens/timeline_pane.rs:2037` a median under a frame budget. The first already has a looser bound on CI (`CI` set): a sign that the gate was wrong, not the runner.
  - `crates/audio/tests/deepfilter.rs:102` mean under 5 ms per frame, `crates/audio/tests/onnxruntime.rs:84,243` medians under 10 and 4 ms, `crates/audio/src/dfn/gpu.rs:218` a warm median under 20 ms. All ignored or hardware-bound, all speed gates.
  - `crates/audio/tests/pipewire.rs` `on_time_under_load` (ignored, runs a private daemon): late wake-ups under load.
- Tests with a wall-clock bound that guards against a hang, not against slowness: `bridge.rs:268` and `state.rs:3017` (50 ms: "a full sink does not block"; a blocking implementation would hang for ever, so 50 ms measures the runner, not the code), `rich_text.rs:1566` (linear, 1 s), `avatars.rs:476` (5 s), `devices.rs:294` (5 s). The 50 ms ones flake on a loaded runner; the others have a margin of two orders of magnitude and stay.
- Not timing gates, although they look like it: `crates/app/tests/ptt.rs:299` measures the release tail in audio samples (commit `072206e`), `deepfilter.rs:82` asserts a latency in samples. Both are deterministic. They stay gates.
- Existing measuring tools that already are indicators: `crates/audio/examples/{voice_profile,dfn_profile}.rs`, `crates/core/examples/opus_profile.rs`, `scripts/profile-call-path.sh` ("a report, not an error"), `.github/workflows/profile.yml` (on demand, aarch64), `scripts/time-integration-tests.sh`, `scripts/measure-encoders.sh`. They stay as they are; the new perf job reports next to them.
- CI today (`.github/workflows/ci.yml`): `lint` (clippy before the cheap checks), `test` on 4 OSes (builds everything and runs `cargo test --workspace`), 4 `integration` shards (each builds `-p twicecat-core -p twicecat-app` in full, then runs its 7 or 8 binaries), `integration-ok`. `rust-cache` saves on every run in `lint` and `test`; only the shards restrict saving to `main`. No `concurrency`. No `paths-ignore`.
- Toolchain 1.97.1, stable. No `cargo-nextest` (the earlier plan, `faster-integration-tests.md` D3, decided against it; this plan does not revisit it).

## The test tiers
Every test belongs to exactly one tier. The tier says what a failure means, where the test runs, and whether it may block a merge.

| Tier | A failure means | Runs | Blocks a merge |
|---|---|---|---|
| **1. Validation** (unit, property, golden/pixel, layout) | The code does something wrong. Deterministic, no network, no wall clock | `lint` and `test` on every push | Yes |
| **2. Integration** (the `#[ignore]` tests that need the local homeserver and LiveKit) | A feature is broken across components (login, e2ee, calls, timeline) | the 4 `integration` shards | Yes |
| **3. Regression** (a test written for a bug that was fixed) | The bug is back | in tier 1 or 2, wherever its code is; named `regression_<what>` | Yes |
| **4. Performance indicators** (`perf_` tests, the profile examples) | Nothing. A number moved | the `perf` workflow, on a quiet runner, one test at a time | **Never** |
| **5. Human** (`hardware_`, `keyring`) | Real devices behave differently | by hand, listed in `PROGRESS.md` | No |

Rules the tiers follow:
- **R1. A gate never asserts a rate or a duration of the machine.** It asserts a behaviour: order, completeness, content, state, "it made progress". "Made progress" uses a floor far below the nominal rate, set so that a stalled stream fails and a slow runner does not (for video: at least 5 fps against 30 nominal).
- **R2. A bound on the wall clock in a gate is a hang guard**, at least two orders of magnitude above the typical time and at least 1 s. Where a hang already blocks the test by itself, the bound is dropped.
- **R3. Speed has an expectation, not a threshold.** A `perf_` test records a value with an expectation (`at least 28 fps`). Outside it, the run prints a `::warning` annotation and the summary row is marked. The build stays green.
- **R4. Performance is measured alone.** The perf job runs one `perf_` test at a time (`--test-threads=1`), on its own runner, with no other build running. A rate measured in the middle of a parallel test run measures the contention, not the code.
- **R5. Regression tests are not a tier of their own for the CI**, only for the reader: the name prefix `regression_` and a doc line with the issue or commit. A new bug fix without one is a review comment, not a CI rule.
- Naming is the selector, as `hardware_` and `keyring` already are: `perf_` for tier 4. Gates run `-- --skip perf_`.

## Decisions (proposed; the human confirms at the Task 1 checkpoint)
- **D1. Step trailer: `Step: ci-speed.N`.** Commit types on this branch are `feat`, `fix` and `docs` only (AGENTS.md rule 5); CI, script and test changes are `feat(ci)`, `feat(scripts)` or `feat(test)`-style scopes as in `e88c678` and `a3d69ba`. A change to a task's own earlier work is folded into its commit.
- **D2. `perf_` is the selector, and `twicecat-perf` is the sink.** A new workspace crate `crates/perf` (`publish = false`, no dependency but `std`) records a metric: it prints a `perf:` line, appends a JSON line to `$TWICECAT_PERF_FILE` when set, and prints a `::warning` annotation when a run is outside its expectation under GitHub Actions. It never panics. `crates/core`, `crates/app` and `crates/audio` take it as a dev-dependency, so `crates/app` stays free of `matrix-sdk` (rule 7) and no external dependency is added (rule 3).
- **D3. The perf job is its own workflow, `perf.yml`, not part of `ci.yml`.** It runs on pushes to `main` and on demand (`workflow_dispatch`), and on a pull request that has the `perf` label. Its result is an artifact (`perf.jsonl`) and a table in the job summary, with the difference from the last successful `main` run. It is not in `integration-ok` and not a required check.
- **D4. Tight hang guards are relaxed to the rule R2**, not removed: `bridge.rs:268` and `state.rs:3017` go from 50 ms to 5 s.
- **D5. The timeline batch test moves to tier 4**, because its number is a rendering speed (the 16 ms frame budget of the Phase 3 plan). The regression it guards against ("one reset of 10,000 items blocks the UI for seconds") stays covered by a gate with a hang-guard bound (R2) and the same shapes.
- **D6. Windows and macOS on every push (policy, the human decides).** The Windows `test` job is the critical path. Recommended: the full matrix when the pull request is ready for review and on `main`; Linux only while it is a draft. The macOS job did find a real bug (Cmd against Ctrl in a keys test) that Linux cannot see, so the full matrix stays a requirement for merging. This plan implements the switch behind one expression and leaves it on the current behaviour until the human answers.
- **D7. Merging test binaries (Task 8) goes last and only with repeat-run evidence**, because tests of different files would share a process.

## What can be verified where
- **Locally:** `cargo fmt`, clippy, `cargo test --workspace`, the perf crate's unit tests, `scripts/*.sh --check` scripts, `actionlint` if installed, shell checks, the `perf_` tests that need no stack, `cargo build --timings`, and the stack-bound tests with `scripts/stack.sh up -d --wait`.
- **CI only (HUMAN: push, then read the run):** every duration, every cache hit or miss, the perf workflow and its summary, and the number of green runs. A CI change cannot be tested without pushing; `gh workflow run ci.yml --ref feat/ci-speed` runs the branch's workflow without a pull request.

### Task 1: Plan and ledger (checkpoint)
Language: docs
Files: docs/plans/ci-speed.md, docs/plans/ci-speed.progress.md, PROGRESS.md
Depends on: none
Acceptance criteria:
- This plan and the ledger are committed as `docs(plan)`.
- **HUMAN checkpoint:** the human confirms the tiers, D1 to D7, and answers D6.
Evidence: none (docs).

### Task 2: The `twicecat-perf` crate and the report script
Language: rust
Files: crates/perf/{Cargo.toml,src/lib.rs}, crates/core/Cargo.toml, crates/app/Cargo.toml, crates/audio/Cargo.toml, Cargo.lock, scripts/perf-report.sh, docs/testing.md (new), PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- `twicecat_perf::record(name, value, unit, Expect)` with `Expect::{AtLeast(f64), AtMost(f64), Report}`. Prints `perf: <name> = <value> <unit> (<expectation>) <ok|outside>`. Appends one JSON object per line (`name`, `value`, `unit`, `expect`, `ok`) to `$TWICECAT_PERF_FILE` when set, and prints `::warning title=perf::<line>` when outside and `GITHUB_ACTIONS` is `true`. It never panics, even when the file cannot be written.
- `twicecat_perf::median(&mut [Duration])` and a `record_duration` convenience for the timeline tests.
- Unit tests: expectation maths for both directions and equality, the JSON line (escaping a quote in a name), no panic on an unwritable path, no annotation outside Actions.
- `scripts/perf-report.sh CURRENT.jsonl [BASELINE.jsonl]` prints a Markdown table (name, value, unit, expectation, mark, change against the baseline when given), and appends it to `$GITHUB_STEP_SUMMARY` when set. It exits 0 whatever the numbers say, and non-zero only for an unreadable file. It needs only `jq`.
- `docs/testing.md` states the tiers, R1 to R5 and the naming, so a new test has a place.
- The gate passes; `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` pass; `cargo deny` accepts the new crate.
Evidence: `cargo test -p twicecat-perf`; `scripts/perf-report.sh` on a sample file.

### Task 3: Speed gates become indicators; tight guards become hang guards
Language: rust
Files: crates/core/tests/call_video.rs, crates/core/tests/cli.rs, crates/app/src/timeline.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/bridge.rs, crates/app/src/state.rs, crates/audio/tests/{deepfilter,onnxruntime,pipewire}.rs, crates/audio/src/dfn/gpu.rs, docs/testing.md, PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- For each test listed under "Tests that gate on speed or timing today": the behaviour checks stay (order, completeness, content, sizes, "frames flow" at the R1 floor), the speed assertion is replaced by `twicecat_perf::record`, and the test's name gets the `perf_` prefix **only when it has no behaviour check left**. A test with both is split: the behaviour half stays a gate, the measuring half becomes a `perf_` test that reuses the same setup helper. No test is deleted without its checks living on elsewhere (`--list` counts are compared before and after and written in the ledger).
- `call_video`: the camera test keeps ordering, readable stamps, the top layer size and a floor of 5 fps; `perf_a_camera_reaches_the_viewer_at_the_frame_rate` records received fps (expect at least 28), sent fps (at least 20), and the hardware test records its per-quality fps. `cli`: the viewer test keeps "received the 1280x720 layer" and records the rate (at least 25).
- The two 50 ms guards use 5 s (D4).
- The timeline batch test: the median is recorded (D5) and a gate with the R2 bound (1 s per batch) stays.
- `cargo test --workspace -- --skip perf_` passes, and so does the full default `cargo test --workspace` (the `perf_` tests never fail on a value).
- With the stack: `call_video` and `cli` each pass 3 times in a row while the machine is also loaded (`stress`-style, two busy loops per core), which is the case that failed in CI. Recorded in the ledger.
- The gate passes.
Evidence: `cargo test -p twicecat-core --test call_video -- --ignored` under load, 3 passes (needs the stack; HUMAN if unavailable). `grep -rnE "fps >=|fps >|< Duration::from_millis\(" crates` lists no gate.

### Task 4: The `perf` workflow
Language: ci
Files: .github/workflows/perf.yml (new), scripts/run-perf-tests.sh (new), docs/testing.md, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- `scripts/run-perf-tests.sh` builds once, starts nothing itself, and runs every `perf_` test with `--test-threads=1 --nocapture`, the ignored ones too when the stack is up, `TWICECAT_PERF_FILE` set to `target/perf/<UTC timestamp>.jsonl`. It never fails because of a value; it fails if a `perf_` test panics, or finds none.
- `perf.yml` runs on `push` to `main`, `workflow_dispatch`, and `pull_request` with `types: [labeled, synchronize]` when the label is `perf`. One job on `ubuntu-26.04` with the same setup as the shards (build dependencies, libwebrtc, models, local Synapse), the script, then `scripts/perf-report.sh` against the last successful `main` artifact (`gh run download`, skipped quietly when there is none), `actions/upload-artifact` for `perf.jsonl`, and the table in the summary.
- The job has `continue-on-error: true` and is not in `integration-ok`. Nothing in `ci.yml` needs it.
- `actionlint` (if available) accepts the workflow.
- **HUMAN:** `gh workflow run perf.yml --ref feat/ci-speed`, then read the summary.
Evidence: the run's summary table, and the artifact.

### Task 5: Pipeline quick wins (cancel, fail fast, save, skip)
Language: ci
Files: .github/workflows/ci.yml, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- `concurrency: { group: ci-${{ github.workflow }}-${{ github.ref }}, cancel-in-progress: ${{ github.event_name == 'pull_request' }} }`: a newer push to a pull request cancels the older run; `main` runs are never cancelled.
- `lint` runs the checks that need no compilation first (commit format, `integration-shards.sh --check`, the `check-*.sh` scripts, `cargo fmt --check`, cargo-deny), then the build setup and clippy. A shard list error shows in about 1 minute instead of 8.
- Every `rust-cache` step has `save-if: ${{ github.ref == 'refs/heads/main' }}`.
- `paths-ignore` for `docs/**`, `**/*.md` and `docs/plans/**` on `pull_request` and `push`. `integration-ok` is a required check, so it must still report: the workflow is split so that a docs-only change cannot leave a required check pending. (Written down in the ledger how it was solved: a final `ci-ok` job with `if: always()`.)
- `actionlint` accepts the workflow. A docs-only branch is pushed and its checks are read (**HUMAN**).
Evidence: **HUMAN:** two quick pushes to a draft pull request show the first run cancelled; a docs-only push shows only the `ci-ok` job.

### Task 6: Each shard builds its own binaries; rebalance
Language: ci
Files: .github/workflows/ci.yml, scripts/integration-shards.sh, scripts/run-integration-tests.sh, docs/plans/ci-speed.progress.md
Depends on: Task 5
Acceptance criteria:
- The "Build integration tests" step of a shard builds only the `--test` targets of `scripts/integration-shards.sh --list N` (one `cargo test --no-run` per package, as `run-integration-tests.sh` runs them) instead of everything. The `lib` entry builds the package's `--lib` test.
- The run step reuses that build (no rebuild). The ignored-test count across the four shards equals the baseline (`--list --ignored`).
- The shard table is rebalanced from the measured per-binary durations of runs `38709253505` and `38648005981`, written in the ledger with the sums per shard. `--check` still passes.
- With CI (**HUMAN**): the per-shard build step against 5–13 min today.
Evidence: **HUMAN:** `gh run view <id> --json jobs` per-shard step durations.

### Task 7: Cache the downloads
Language: ci
Files: .github/workflows/ci.yml, .github/workflows/perf.yml, scripts/fetch-libwebrtc.sh, scripts/fetch-cuda-headers.sh, scripts/fetch-models.sh, scripts/stack.sh (only if needed)
Depends on: Task 5
Acceptance criteria:
- `actions/cache` (a pinned major version) for the libwebrtc archive, the CUDA headers and the models, each keyed on the SHA-256 file the script already pins (`scripts/checksums/…`), so a changed pin is a miss and a verified download still runs on a miss. The scripts keep verifying the SHA-256 whether the file came from the cache or from the network.
- The Synapse and LiveKit images are saved with `docker save` into a cache keyed on the image tags in `docker-compose.yml`, and loaded with `docker load` on a hit. A miss pulls as today.
- `cargo-deny` runs from a cached binary instead of the Docker-based action, with the version pinned.
- `scripts/check-no-webrtc-download.sh` still passes in every job.
- With CI (**HUMAN**): the second run on the same pins shows a cache hit for each, and the fetch steps drop under 10 s (libwebrtc was 97 s on Windows).
Evidence: **HUMAN:** step durations of two consecutive runs.

### Task 8: Fewer test binaries (linking is the cost)
Language: rust
Files: crates/core/tests/**, scripts/integration-shards.sh, Cargo.toml (profile only), .cargo/config.toml (only after measuring)
Depends on: Task 6
Acceptance criteria:
- Measured first, locally, with `cargo build --timings` after a one-line change in `crates/core`: link time per binary, the total, and what each of `debug = line-tables-only` for the workspace crates (in CI only, through an environment variable) and the x86 Linux linker choice (`lld` against the default) changes. Numbers in the ledger. A change with no measurable gain is not made.
- The core integration tests move from about 26 binaries to one directory per shard (`tests/<shard>/main.rs` with a `mod` per file), starting with the files that already share accounts. The per-binary `e2ee_serial()` mutex keeps its meaning because the files that take it end up in the same binary. A file that cannot share a process stays alone, with the reason written next to it.
- `integration-shards.sh` follows (entries are the new binaries); `--check` passes; the ignored-test count is unchanged.
- Each merged binary passes 3 times in a row with the stack, and the full suite twice.
- **HUMAN:** the build step and the total per shard on CI against Task 6.
Evidence: the ledger's timings table; `scripts/run-integration-tests.sh` passing 3 times.

### Task 9: Run `ui_gallery` once, and the draft policy (D6)
Language: ci
Files: .github/workflows/ci.yml, crates/app/tests/ui_gallery.rs (only if a check is platform-specific)
Depends on: Task 6
Acceptance criteria:
- Checked: which `ui_gallery` checks depend on the platform (fonts, layout). The ones that do stay on every OS. The rest (pixel and structure checks that read only our own renderer output) run on ubuntu only, with the reason written next to the skip.
- D6, as the human decides: the Windows and macOS legs run on `main` and on non-draft pull requests (`github.event.pull_request.draft == false`), or on every push as today.
- `actionlint` accepts the workflow.
Evidence: **HUMAN:** the run's job list for a draft and for a ready pull request.

### Task 10: After-numbers and the three green runs (checkpoint)
Language: docs
Files: docs/plans/ci-speed.progress.md, PROGRESS.md
Depends on: Task 4, Task 6, Task 7, Task 8, Task 9
Acceptance criteria:
- The ledger holds the before and after of every job from `gh run view --json jobs`, the percentage cut, and the run ids of 3 consecutive green runs with no new flake. **HUMAN:** the runs and the reading.
- The `perf` workflow has run at least once on `main` and its first summary is linked.
Evidence: **HUMAN.**
