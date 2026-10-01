# Progress

## Pinned versions
- Rust toolchain 1.97.1 (rust-toolchain.toml, with rustfmt and clippy), edition 2024, resolver 3
- iced =0.14.0
- Synapse matrixdotorg/synapse:v1.151.0 (docker-compose.yml)
- Cargo.lock is committed

## Phase 0 steps
- [x] 2. Cargo workspace with core, app, ptt crates (phase0.2)
- [x] 3. Local Synapse homeserver and test users (phase0.3)
- [x] 4. GitHub Actions CI, cargo-deny, boundary check
- [ ] 5. Linux build dependencies in CI
- [ ] 6. License and name check

## Step 2 notes
- Checks run: cargo fmt --all --check, cargo clippy --workspace --all-targets --all-features -- -D warnings, cargo test --workspace, cargo build --workspace (all pass).
- Entry-point API: `iced::application(boot, update, view).title("Tessera").run()`, confirmed in `~/.cargo/registry/src/*/iced-0.14.0/src/application.rs` (`application()` doc example, `Application::title` with `TitleFn for &'static str`, `Application::run`). Empty view uses `iced::widget::space()`.
- Local Linux build succeeded; no missing system libraries.
- Open issues: none.

## Step 3 notes
- Checks run: from `docker compose down -v`, `docker compose up -d --wait` exit 0 and `/_matrix/client/versions` returns JSON; `scripts/create-test-users.sh` exit 0 three times in a row (second and later runs hit M_USER_IN_USE); `grep -E "access_token|$TEST_PASSWORD_1"` on its output finds nothing; password login via `POST /_matrix/client/v3/login` returned HTTP 200 for alice, bob, carol (status only, body not printed); shellcheck (via `docker run koalaman/shellcheck:stable`) passes on both scripts; full gate passes.
- Gotchas fixed in `dev/synapse/start.sh`: the generated homeserver.yaml has no trailing newline (appended settings were silently commented out), and Synapse's default registration rate limit returned 429 on a quick second run, so `rc_registration` is relaxed (dev only).
- Containers stopped with `docker compose down`; data lives in the named volume only.
- Open issues: none.

## Step 4 notes
- Checks run: `scripts/check-app-deps.sh` exits 0; negative self-check `scripts/check-app-deps.sh iced` exits 1 (prints `iced`); before the script existed, running it failed (no such file). `cargo deny check` (cargo-deny 0.20.2, installed with --locked) passes: advisories, bans, licenses, sources ok. actionlint 1.7.7 (docker) reports no errors on ci.yml. Full gate passes.
- Licenses: allowlist is the plan's list minus MPL-2.0 (unused by the tree); nothing added beyond the list. Crates offering GPL-2.0-only / LGPL / Unlicense do so as OR alternatives with allowed licenses.
- Advisories: RUSTSEC-2024-0436 (paste) and RUSTSEC-2026-0192 (ttf-parser) are "unmaintained" notices on transitive deps of the pinned iced 0.14.0; ignored in deny.toml with reasons. Revisit when iced is bumped.
- `multiple-versions = "warn"` yields many duplicate warnings from iced's tree; expected.
- Open issues: none.

## Human checkpoint
- [ ] Push the branch and confirm CI is green on Linux, Windows, and macOS (Linux apt deps arrive in step 5, so Linux may fail until then).
- [ ] Run `cargo run -p tessera-app` and confirm an empty window titled "Tessera" opens (rendering is unverified by the agent).
- [ ] Approve the workspace layout and AGENTS.md.
