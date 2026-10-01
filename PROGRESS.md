# Progress

## Pinned versions
- Rust toolchain 1.97.1 (rust-toolchain.toml, with rustfmt and clippy), edition 2024, resolver 3
- iced =0.14.0
- Synapse matrixdotorg/synapse:v1.151.0 (docker-compose.yml)
- Cargo.lock is committed

## Phase 0 steps
- [x] 2. Cargo workspace with core, app, ptt crates (phase0.2)
- [x] 3. Local Synapse homeserver and test users (phase0.3)
- [ ] 4. GitHub Actions CI, cargo-deny, boundary check
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

## Human checkpoint
- [ ] Run `cargo run -p tessera-app` and confirm an empty window titled "Tessera" opens (rendering is unverified by the agent).
- [ ] Approve the workspace layout and AGENTS.md.
