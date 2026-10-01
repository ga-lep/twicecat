# Progress

## Pinned versions
- Rust toolchain 1.97.1 (rust-toolchain.toml, with rustfmt and clippy), edition 2024, resolver 3
- iced =0.14.0
- Synapse matrixdotorg/synapse:v1.151.0 (Task 2, not yet used)
- Cargo.lock is committed

## Phase 0 steps
- [x] 2. Cargo workspace with core, app, ptt crates (phase0.2)
- [ ] 3. Local Synapse homeserver and test users
- [ ] 4. GitHub Actions CI, cargo-deny, boundary check
- [ ] 5. Linux build dependencies in CI
- [ ] 6. License and name check

## Step 2 notes
- Checks run: cargo fmt --all --check, cargo clippy --workspace --all-targets --all-features -- -D warnings, cargo test --workspace, cargo build --workspace (all pass).
- Entry-point API: `iced::application(boot, update, view).title("Tessera").run()`, confirmed in `~/.cargo/registry/src/*/iced-0.14.0/src/application.rs` (`application()` doc example, `Application::title` with `TitleFn for &'static str`, `Application::run`). Empty view uses `iced::widget::space()`.
- Local Linux build succeeded; no missing system libraries.
- Open issues: none.

## Human checkpoint
- [ ] Run `cargo run -p tessera-app` and confirm an empty window titled "Tessera" opens (rendering is unverified by the agent).
- [ ] Approve the workspace layout and AGENTS.md.
