# Phase 6 Finish Plan
Status: APPROVED

## Goal
Close the agent-doable work left in Phase 6 before Phase 7 (packaging) starts, on a branch `feat/phase6-finish` cut from `main`. What is left was found by reading `PROGRESS.md` ("Phase 6 steps", "known limitations", "human checkpoint"), `docs/plans/phase6-calls.md` (Tasks 15 and 16) and `docs/plans/phase6-calls.progress.md`.

**Step 16** (cleaning other people's voices) is built: the CPU filters, quiet voices resting, the skip signal and its policy, and batched GPU cleaning (commits up to `56b4b66`). One code path has never been tested: the core moving from a GPU bank that gives up to the CPU filters. Today `crates/core/src/call/cleaning/mod.rs` has no tests at all, and the engine side is covered only in `twicecat-audio` (`a_failing_bank_comes_back_and_its_voices_play_as_they_came`). The ledger lists this as "[open] the core's switch from a failing bank to the CPU". Task 1 covers that path, fixes anything the tests turn up, finishes the privacy wording the plan asks for, and closes step 16 for its agent part.

**Step 15** (Steam Frame performance) still has two open items: aarch64 numbers and the Frame's defaults. The defaults are a human decision (checkpoint item "Decide the Frame's call defaults") and the device is needed, so neither can be finished here. What an agent can do without the device:
- Task 2: one command that measures the whole call path per 10 ms frame against the 5 % budget, for the CI runner and later the Frame.
- Task 3: an on-demand aarch64 CI job that runs that command, so the human gets aarch64 numbers with one click.
- Task 4: measure the silence options the checkpoint names ("stopping frames while silent, or a tiny dither"), so the defaults can be decided from data.

Step 15 stays unchecked after this plan; what it still needs is listed under "Human checkpoint".

Rules for every task (AGENTS.md):
- **Rule 2:** every `twicecat-audio`, `livekit`/`libwebrtc` or `ort` API is checked in the registry source of the pinned version, cited file:line in PROGRESS.md.
- **Rule 3:** no pinned version changes, and no new crate or GitHub Action is added. None is needed. If one turns out to be needed, the task stops and asks.
- **Rule 5:**
  - one commit per task, a Conventional Commit of type `feat`, `fix` or `docs` with a `Step: phase6.N` trailer;
  - review fixes are folded in with `--fixup` and `rebase --autosquash`;
  - PROGRESS.md and `docs/plans/phase6-calls.progress.md` are updated in the same commit.
- **Gate before each commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features`, as PROGRESS.md "How to run the checks" requires;
  - `cargo test --workspace`;
  - `scripts/check-app-deps.sh` (and `livekit`), `scripts/check-core-api.sh`, `scripts/check-no-webrtc-download.sh`, `scripts/check-commits.sh main`.
- **Rules 6 and 9:**
  - tests use only the local stack and `.env.test`;
  - the agent never runs `keyring` or `hardware_*` tests;
  - nothing by ear, on hardware the agent lacks, or on the Frame is ticked; those go to the checkpoint list.

### Task 1: Cover the core's switch from a failing GPU bank to the CPU, and close step 16
Language: rust
Files:
- `crates/core/src/call/cleaning/mod.rs`: a test seam for the bank and model loaders, used by unit tests only. `Cleaning::new`'s callers in `crates/core/src/call/mod.rs` stay unchanged.
- `crates/core/src/call/cleaning/tests.rs` (new).
- `crates/audio/src/dfn/mod.rs`: `DeepFilterStatus::new` and `set` are public only behind a `test-fakes` feature of `twicecat-audio` (enabled by core under `[dev-dependencies]` only), so a fake bank in core can report "too slow"; the filter uses `pub(crate)` `new_internal`/`set_internal`. Not a permanent audio API addition, not a core API change.
- `crates/core/src/api/audio.rs`: rustdoc only.
- `PROGRESS.md`, `docs/plans/phase6-calls.md`, `docs/plans/phase6-calls.progress.md`.
Depends on: none
Acceptance criteria:
- A unit test with a fake `BatchCleaner`, plugged in through the seam, drives a real `VoiceEngine` (`VoiceEngine::new` + `VoiceControl`) with two remote voices on rows. After the fake starts failing, within 2 s of (paused) time:
  - the manager's GPU state is `GaveUp(CpuAfterGpuError)`;
  - no voice is left in `State::Row`;
  - a CPU model load was asked for;
  - no voice's published status stays `Cleaned` while its flag is `None`.
- The same test with the fake's status set to `CpuAfterGpuTooSlow` before it fails ends in `GaveUp(CpuAfterGpuTooSlow)`.
- Turning `remote_cleaning.gpu` off and on again after a give-up loads a new bank: the loader seam is called a second time.
- A voice that leaves while on a row frees that row: the next voice gets row 0 again.
- With the models fetched (`#[ignore = "needs scripts/fetch-models.sh"]`, part of the local ignored run), after the give-up both voices end up with CPU filters plugged in (their flags read `Running` after a few engine frames). The backend slot that feeds `AudioStats::remote_cleaning` then reads `Some(EnhancerBackend::CpuAfterGpuError)`.
- Each new test fails under a mutation that is then reverted, recorded in PROGRESS.md. Mutations: the `gave_up` branch in `watch_failures` removed, and `Gpu::GaveUp` not excluded from `use_gpu`.
- If a test finds a defect, it is fixed in the same commit. The commit is then `fix(core)`, because the code is on `main`.
- The rustdoc of `CleaningAnnouncement` says that the attribute is seen by everyone in the call and by the SFU's operator (plan Task 16, "Privacy").
- `scripts/check-core-api.sh` passes: the public core API is unchanged apart from doc text.
- PROGRESS.md:
  - step 16 is ticked, with its human items listed;
  - the stale "batching (Task 16) … not done" in step 13b's line is corrected, and so is plan line 331 ("Not done: batching incoming streams");
  - the `twicecat-audio` API change is noted;
  - the gate output is recorded.
- The gate passes. The commit carries `Step: phase6.16`.
Evidence: `cargo test -p twicecat-core --lib call::cleaning` shows the new tests passing. `cargo test -p twicecat-core --lib -- --ignored call::cleaning` (with `scripts/fetch-models.sh` run first) passes the CPU-takeover test.

### Task 2: One command that measures the call path against the Frame budget
Language: rust
Files:
- `crates/audio/examples/voice_profile.rs` and `crates/core/examples/opus_profile.rs`: a `--tsv` flag that prints one tab-separated line per row (name, mean, median, p99, worst, allocations or packets sent). The current table stays the default.
- `scripts/profile-call-path.sh` (new).
- `PROGRESS.md` ("How to run the checks", step 15 notes), `docs/plans/phase6-calls.md` (Task 15), `docs/plans/phase6-calls.progress.md`.
Depends on: none
Acceptance criteria:
- `voice_profile -- --tsv` and `opus_profile -- --tsv` print only TSV lines, with the same rows and figures as the table mode. A unit test in each example checks the line format of one row.
- `scripts/profile-call-path.sh [--voices N] [--frames N] [--cpu C]`:
  - builds both examples in release (`--locked`) and runs them, pinned to CPU `C` with `taskset` when given;
  - prints, for each noise-suppression mode (off, WebRTC NS, RNNoise, DeepFilterNet3 on the CPU when the models are present, skipped with a note otherwise), the per-10 ms call-path cost at the median and p99. Call path = processing chain + voice engine with N remote voices (default 4) + Opus encode at 64 kb/s complexity 9 (voice in noise, and digital silence) + N Opus decodes;
  - prints each cost as a share of the 5 % budget (500 µs per 10 ms) and marks `OVER` when a row exceeds it;
  - prints the host (`uname -m`, CPU model from `/proc/cpuinfo`, and whether denormals are flushed on the processing thread);
  - writes the same report to `$GITHUB_STEP_SUMMARY` when that variable is set.
- The script is shellcheck-clean, uses `set -euo pipefail`, downloads nothing (models only if already fetched), and exits non-zero if a profile run fails (`OVER` is reported, not an error).
- PROGRESS.md records one run on this x86_64 machine (P-core and E-core), with the numbers next to the existing step 15 table.
- The gate passes. The commit is `feat(scripts): …` with `Step: phase6.15`.
Evidence: `scripts/profile-call-path.sh --voices 4 --frames 1000` prints the per-mode call-path table with budget shares. `voice_profile -- --tsv default 200` prints TSV lines.

### Task 3: On-demand aarch64 profiling job in CI
Language: rust
Files:
- `.github/workflows/profile.yml` (new). Its only trigger is `workflow_dispatch` (see Q3). The job runs on `ubuntu-26.04-arm` and uses the same setup steps as `ci.yml`'s test job: Linux dependencies, clang 22, the pinned libwebrtc through `scripts/fetch-libwebrtc.sh`, and `Swatinem/rust-cache@v2`, which is already used. Then `scripts/fetch-models.sh` (pinned SHA-256) and `scripts/profile-call-path.sh`.
- `PROGRESS.md` (checkpoint item), `docs/plans/phase6-calls.progress.md`.
Depends on: Task 2
Acceptance criteria:
- `actionlint .github/workflows/profile.yml` is clean.
- The workflow uses only actions already used in `ci.yml` (`actions/checkout@v6`, `Swatinem/rust-cache@v2`), `permissions: contents: read`, and `persist-credentials: false`. The report goes to the job summary, not to an artifact, so no new action is needed.
- The libwebrtc fetch is followed by `scripts/check-no-webrtc-download.sh`.
- The job never runs on pull requests or pushes, so it costs nothing unless triggered.
- The same job steps are dry-run locally on x86_64 by running the script (Task 2). Running on the arm64 runner itself is a human item, because pushing and triggering CI is the human's.
- The gate passes. The commit is `feat(ci): …` with `Step: phase6.15`.
Evidence: `actionlint .github/workflows/profile.yml` exits 0. Once the human pushes and runs "Profile (aarch64)" from the Actions tab, the job summary shows the call-path table for `aarch64`.

### Task 4: Measure the silence options for the Opus encoder
Language: rust
Files:
- `crates/core/examples/opus_profile.rs`: new silence signals. Digital silence; a dither at -110 dBFS and at -90 dBFS (i16 ±1 LSB and a few LSB, seeded); "no frames" (encoder not called). For each: encode cost, packets sent (DTX), and bytes per second. Also included in `--tsv`.
- `PROGRESS.md` (step 15 notes, the "Decide the Frame's call defaults" item gets the numbers), `docs/plans/phase6-calls.md` (Task 15 part 4 "Options to measure next"), `docs/plans/phase6-calls.progress.md`.
Depends on: Task 2
Acceptance criteria:
- The libwebrtc 0.3.50 / webrtc-sys 0.3.47 source is read and cited file:line in PROGRESS.md for what happens when `NativeAudioSource::capture_frame` is not called while a track is published: whether the audio source inserts silence (and so the encoder still runs) or the send stream stops. This answers "stopping frames while silent" from the source. No behaviour change in the call path.
- `opus_profile` reports each silence option at 64 kb/s complexity 9 and at 32 kb/s complexity 5, with and without flushed denormals. Run on a P-core and an E-core (`taskset`) on this machine, the results are recorded in PROGRESS.md: encode µs per 10 ms, % packets sent, kb/s on the wire.
- For each dither level, PROGRESS.md states whether DTX still stops sending (the step 15 note says a dither defeats DTX; this confirms or corrects it with numbers).
- A unit test checks that the dither generator stays within its stated level (peak and RMS in dBFS).
- The gate passes. The commit is `feat(core): …` with `Step: phase6.15`.
Evidence: `cargo run --release -p twicecat-core --example opus_profile -- 10` prints the new silence rows (cost, packets sent, kb/s). `-- 10 --tsv` prints them as TSV.

## Human checkpoint (not agent tasks; added to PROGRESS.md "Phase 6 human checkpoint" by the task named)
- (Task 1) Cleaning other voices by ear on Linux, on the CPU (`/clean on`) and on the GPU (`/clean gpu on`). Then load the GPU with a game until the bank gives up: the CLI should print "moved to the CPU after a GPU error" or "…too slow", and the voices should carry on cleaned with no glitch.
- (Task 1) Element Call (browser) in a call with cleaning on: a browser with its own noise suppression should show `Skipped(AlreadyClean)` under the "any suppressor" policy and be cleaned under the default "neural" policy. Needs a real Element Call client.
- (Task 1) The Frame's cleaning limit: whether `CLEANED_VOICES` (4, measured on an i9 P-core) holds on the Frame while a VR game runs, or whether remote cleaning should be hidden there.
- (Task 3) Push the branch and run the "Profile (aarch64)" workflow; check the job summary (aarch64 numbers for step 15).
- (Tasks 2, 4) On the Steam Frame itself: `scripts/profile-call-path.sh` in desktop and game mode, with a standalone VR game running. Plus the existing Frame item: rtkit priority, Flatpak, AEC on built-in speakers, battery.
- (Tasks 2, 4) Decide the Frame's call defaults from the aarch64 and Frame numbers: bitrate; complexity, which LiveKit 0.9.3's `TrackPublishOptions` does not expose (only `audio_encoding.max_bitrate` and `dtx`); what to do about silence; whether a "low power" profile is needed. Step 15 is ticked only after that and its implementation.
- Note the `twicecat-audio` `test-fakes` feature from Task 1 (`DeepFilterStatus::new`/`set` public only with it; test-only, not a permanent API addition).
- Unchanged from before: the Opus upstream report decision, and every other open Phase 6 checkpoint item.

## Questions
- **Q1 (commit type for Task 1).** If the new tests pass without a code change, Task 1 adds tests (plus a doc line) for code already on `main`. Rule 5 folds test-only changes into the commit they belong to, but that commit is on `main`, and a branch allows only `feat`/`fix`/`docs`. Options:
  - (a) `feat(core): cover the switch from a failing GPU bank to the CPU`, with a body that says it is coverage plus the privacy doc. Recommended: it is what the commit check accepts, and the body is honest.
  - (b) `docs(core)` plus the tests, which misstates the change.
  - (c) a rule exception you choose.

  If a test finds a defect, Task 1 is `fix(core)` regardless.
- **Q2 (Frame defaults now or later).** The agent could build a "platform defaults" hook now: SteamOS on aarch64, detected from `/etc/os-release`, picks default settings when no `call_settings.json` exists. Its values would be placeholders until the aarch64 and Frame numbers exist. Recommended: wait. The values are your decision (checkpoint), and a hook without decided values adds code to change later. Not in this plan unless you say so.
- **Q3 (profile job trigger).** Run only on demand (`workflow_dispatch`, recommended: the arm runner's minutes are spent only when you want numbers), or also on every push to `main`?
- **Q4 (out of scope, confirm).** These are left out: the "device buffer size" candidate (plan "Candidates", not scheduled), the x86-64-v3 check on Windows and macOS (Task 15 part 2 leaves them baseline), and NVIDIA Maxine. Confirm they wait for after Phase 7, or say which to add.
