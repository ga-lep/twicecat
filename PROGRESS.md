# Progress

## Pinned versions
- Rust toolchain 1.97.1 (rust-toolchain.toml, with rustfmt and clippy), edition 2024, resolver 3
- iced =0.14.0
- Synapse matrixdotorg/synapse:v1.151.0 (docker-compose.yml)
- Cargo.lock is committed

## Phase 0 steps
- [x] 2. Cargo workspace with core, app, ptt crates (phase0.2)
- [x] 3. Local Synapse homeserver and test users (phase0.3)
- [x] 4. GitHub Actions CI, cargo-deny, boundary check (phase0.4)
- [x] 5. Linux build dependencies in CI (phase0.5)
- [x] 6. License and name check (phase0.6)

Phase 0 agent work is done; only the human checkpoint remains.

## Step 2 notes
- Checks run: cargo fmt --all --check, cargo clippy --workspace --all-targets --all-features -- -D warnings, cargo test --workspace, cargo build --workspace (all pass).
- Entry-point API: `iced::application(boot, update, view).title("Tessera").run()`, confirmed in `~/.cargo/registry/src/*/iced-0.14.0/src/application.rs` (`application()` doc example, `Application::title` with `TitleFn for &'static str`, `Application::run`). Empty view uses `iced::widget::space()`.
- `iced::widget::space()` confirmed in `~/.cargo/registry/src/*/iced_widget-0.14.2/src/helpers.rs:1748` (`pub fn space() -> Space`, locked version per Cargo.lock); `iced-0.14.0/src/lib.rs:634-637` has `pub mod widget` with `pub use iced_widget::*`, and `iced_widget-0.14.2/src/lib.rs:48` has `pub use helpers::*`.
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

- Hardening fixes: `check-app-deps.sh` now captures `cargo tree` output before grepping, so a cargo failure exits non-zero (before: a fake failing cargo gave exit 0; after: 1), and passes `--all-features --target all` so optional and target-specific deps are caught (a temporary `cfg(windows)` optional `matrix-sdk-x` dep was missed before, exit 0, and caught after, exit 1; manifest reverted). ci.yml: top-level `permissions: contents: read`, `persist-credentials: false` on both checkouts, `--locked` on cargo build/clippy/test. Action tags unchanged (SHA pinning left to the human). actionlint and shellcheck clean.
- Pipeline fix: `check-app-deps.sh` now assigns the `tail`/`awk` output as a plain statement (set -e applies) and runs `grep` separately, so only a match exits 1, no match exits 0, and any other failure fails closed (before: a failing fake `awk` gave exit 0; after: 1; invalid pattern, grep status 2, exits 2). shellcheck clean.
- actions/checkout bumped v4 -> v6 (node 24) for the GitHub Node 20 deprecation warning; approved by the human. v6 action.yml confirms `persist-credentials` input and `using: node24`. actionlint 1.7.7 clean.

## Step 5 notes
- Both CI jobs install the apt packages (libxkbcommon-dev libwayland-dev libx11-dev libdbus-1-dev libssl-dev pkg-config) under `if: runner.os == 'Linux'`, right after checkout and before any cargo step. Before the edit the grep for the package list found 0 matches; after, 2. actionlint 1.7.7 (docker) reports no errors. Full gate passes.
- Open issues: none.

## Step 6 notes
- LICENSE is the unmodified GPL-3.0 text fetched from https://www.gnu.org/licenses/gpl-3.0.txt. `[workspace.package] license = "GPL-3.0-or-later"`, inherited by all three crates. Before the edit the `cargo metadata ... | grep -c` check printed 0; after, 3.
- Name check (as of 2026-10-01):
  - crates.io: `tessera` is taken (v0.1.0, a 3D Tiles geometric-error library from github.com/sensat/tessera). Mitigated by `publish = false` and the `tessera-*` crate names.
  - Flathub: an unrelated GTK QR-mosaic app called "Tessera" (`fr.ethal.tessera`) has submission PR flathub/flathub#10239 open and awaiting review.
  - Flathub IDs are unique by reverse-DNS, so our ID would be something like `io.github.ga_lep.Tessera`. The shared display name is left to the human to decide. Per the user's decision, we don't rename.
- Open issues: none.

## Human checkpoint
- [x] Push the branch and confirm CI is green on Linux, Windows, and macOS (confirmed by human 2026-10-01).
- [ ] Run `cargo run -p tessera-app` and confirm an empty window titled "Tessera" opens (rendering is unverified by the agent).
- [ ] Approve the workspace layout and AGENTS.md.
- [ ] Run locally (needs sudo): `sudo apt-get update && sudo apt-get install -y libxkbcommon-dev libwayland-dev libx11-dev libdbus-1-dev libssl-dev pkg-config`.
