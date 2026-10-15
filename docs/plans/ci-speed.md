# CI Speed and Test Tiers Plan
Status: DRAFT (scope set by the human in chat, 2026-10-15: do the CI speed-ups, and separate validation, regression and performance tests, with performance as an indicator, not a gate). Confirmed by the human on 2026-10-15: D6 (Windows and macOS only on ready pull requests) and D7 (no merging of test binaries, Task 8). The other decisions are proposals until the human confirms them at the Task 1 checkpoint.

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

### Task 6: Each shard builds only its own binaries, in one cargo call; rebalance
Language: ci
Files: .github/workflows/ci.yml, scripts/integration-shards.sh, scripts/run-integration-tests.sh, docs/plans/ci-speed.progress.md
Depends on: Task 5
Acceptance criteria:
- Found while measuring (run `38716596416`, ledger): the "Build integration tests" step built all 35 binaries (593–869 s), and the run step then compiled again for 327–600 s, because it ran one cargo call per package with other targets. The step now runs `scripts/run-integration-tests.sh --shard N --no-run`, and the run step makes the same call, so there is one build of the shard's 6 to 8 binaries and no second one.
- The run script makes one `cargo test -p twicecat-core -p twicecat-app --test … --lib` call per shard instead of one per package. `integration-shards.sh --check` also fails when a test file name exists in both packages (a name selects the target of whichever package has it).
- The ignored-test count across the four shards equals the baseline (`--check` places every binary; the sum of `passed` of the four local runs matches the baseline sum).
- The shard table is rebalanced from the measured per-binary seconds of run `38716596416`, plus 20 s of linking per binary. The table and the sums per shard are in the ledger.
- Locally, with the stack: all four shards pass, and the run step does not compile.
- With CI (**HUMAN**): the per-shard build and run step durations against the 593–869 s and 327–600 s above.
Evidence: **HUMAN:** `gh run view <id> --json jobs` per-shard step durations.

### Task 7: Native setup once, a pinned cargo-deny, the stack started during the build
Language: ci
Files: .github/actions/setup-build/action.yml (new), .github/workflows/ci.yml, .github/workflows/perf.yml, scripts/fetch-cargo-deny.sh (new), scripts/checksums/cargo-deny.sha256 (new)
Depends on: Task 5
Acceptance criteria:
- Measured first (run `38716596416`, steps over 15 s): the Docker-based cargo-deny action took 61 s in `lint`; libwebrtc took 58 s on Windows and 21 s on macOS; the models, CUDA header and Linux libwebrtc were under 15 s; the Windows and macOS rust-cache restores took 72 s and 58 s; the stack start took about 30 s on the shards' critical path. This is why the Docker images and the models are not cached (a restore costs about what a pull or a download does) and the stack is overlapped instead.
- One composite action, `setup-build`, replaces the four copies of the Linux dependencies, clang 22, libwebrtc and CUDA steps (`lint`, `test`, `integration`, `perf`).
- libwebrtc is kept in the Actions cache (`~/.cache/twicecat/libwebrtc`, restore on every run, save only on main), keyed on the checksum file and the fetch script. The script still downloads, verifies the SHA-256 and unpacks on a miss. **Measured on CI, and reverted if it is not faster on Windows and macOS.**
- `scripts/fetch-cargo-deny.sh` downloads the pinned cargo-deny release for x86-64 Linux, checks its SHA-256 (the digest GitHub computed for the asset), and prints the binary; `checks` runs `cargo deny --workspace check` with it. `cargo deny check` passes locally with it.
- In a shard, the stack's `up -d` runs in the background from the first step, and a step after the build waits for it and then runs `up -d --wait` and registers the test users.
- YAML parses; **actionlint is not installed here**.
Evidence: **HUMAN:** step durations of two consecutive runs on the same pins (the first saves on main, the second restores).

### Task 8: Fewer binaries linked (linking is the cost)
Language: ci
Files: scripts/unit-test-targets.sh (new), .github/workflows/ci.yml, docs/plans/ci-speed.progress.md
Depends on: Task 6
Acceptance criteria:
- **Merging test files into fewer binaries is not done** (human to confirm, D7). Measured and read (ledger): a link is 8–20 s of CPU per binary locally; after Task 6 a shard links 6 to 8 of them, so merging would save at most about 15 s per shard; and files cannot share a process safely: `log_hygiene` and `call_log_hygiene` install the global tracing subscriber and scan everything it sees, `tls_provider` installs the process-wide rustls provider, `call_video_fallback` sets an environment variable, `common::E2EE_LOCK`, `DM_LOCK` and per-file mutexes are per binary today, and `reset_to_b0` deletes the other devices of an account, so tests of different files using the same user would interfere (they run one binary after another today). The linker is already LLD on x86 Linux, and the debug-info level of the workspace crates did not change a test binary's size (508 413 400 bytes both ways), so neither is changed.
- What does cost on every OS job is linking 26 integration test files whose tests are all `#[ignore]` ("0 passed; N ignored", about 500 MB each, 13 GB of writes). `scripts/unit-test-targets.sh` prints `--lib --bins --examples --test …` for the files that have a non-ignored test; the `test` job runs that, and the doc tests apart (`--doc` cannot be combined). A file is skipped only when its `#[ignore]` lines are at least as many as its test attributes.
- The count of passed tests is the same as `cargo test --workspace` (1794 passed locally, before and after), and the ignored tests that move out are the ones the shards run.
- **HUMAN:** the `test` job's build and run step durations on the four OSes against run `38716596416` (Windows 634 s, ubuntu 665 s, arm 747 s, macOS 465 s).
Evidence: the counts above; **HUMAN:** step durations.

### Task 9: Windows and macOS only on ready pull requests (D6); `ui_gallery` once
Language: ci
Files: .github/workflows/ci.yml, docs/testing.md; later crates/app/tests/ui_gallery.rs
Depends on: Task 6
Acceptance criteria:
- D6, as the human decided (2026-10-15): the `test` matrix comes from the `changes` job. On `main`, on a manual run and on a pull request that is ready for review it is Linux x86-64, Linux aarch64, Windows and macOS; on a draft pull request it is the two Linux legs. `ready_for_review` is a trigger, so marking a pull request ready starts a full run. The Windows and macOS legs stay required for a merge.
- **Not on `main` yet:** `ui_gallery` (it comes with PR #27). When that branch rebases onto this one: check which of its checks depend on the platform (fonts, layout), keep those on every OS, and run the rest on ubuntu only, with the reason next to the skip.
- YAML parses; **actionlint is not installed here**.
Evidence: **HUMAN:** this draft pull request's next run shows two `test` legs; marking it ready shows four.

### Task 10: Fix the flake the first CI runs found
Language: rust
Files: crates/core/tests/timeline_actions.rs, crates/core/tests/timeline_live.rs, docs/plans/ci-speed.progress.md
Depends on: Task 6
Acceptance criteria:
- Run `38736783282` failed in shard 1 on `reaction_toggles_on_and_off_for_both_sides` and `typing_notices_reach_the_other_user`. Both had failed before (runs `38722052074`, `38636816641`, `38453477513`), so it is not caused by this branch. Reproduced locally by 30 busy loops on 24 cores: the reaction test failed 6 of 8 runs.
- Root cause of the reaction test: the test helper `apply_until` stopped applying the diffs of a batch as soon as its condition held (`break`), dropping the rest of the batch, so the mirrored timeline diverged from the SDK's (one item twice, with and without the reaction). Under load diffs arrive in batches. `timeline_actions.rs` and `timeline_live.rs` now apply every diff and remember that the condition held. The reaction test also finds its message by event ID and names each wait, so a timeout says which wait and shows the list. After the fix: 16 of 16 passes under the same load.
- `typing_notices_reach_the_other_user` did not fail in 8 runs under the same load (8 of 8) and is not changed; it failed in 3 of about 30 CI runs ("carol never saw alice typing" after four attempts of 8 s). Left for the human (AGENTS.md rule 8): it may be a real sync issue with typing notices, not a test one.
- The gate passes.
Evidence: the load runs above (ledger).

### Task 11: After-numbers and the three green runs (checkpoint)
Language: docs
Files: docs/plans/ci-speed.progress.md, PROGRESS.md
Depends on: Task 4, Task 6, Task 7, Task 9, Task 10
Acceptance criteria:
- The ledger holds the before and after of every job from `gh run view --json jobs`, the percentage cut, and the run ids of 3 consecutive green runs with no new flake. **HUMAN:** the runs and the reading.
- The `perf` workflow has run at least once on `main` and its first summary is linked.
Evidence: **HUMAN.**
