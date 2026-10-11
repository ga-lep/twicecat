# Phase 0 Foundation Plan
Status: APPROVED

## Goal
Set up the smallest working base for Twicecat before any Matrix code exists: a Cargo workspace (`crates/core`, `crates/app`, `crates/ptt`) whose app binary opens an empty Iced window, a local Synapse homeserver in Docker with open registration and a script that registers three throwaway test accounts, a GitHub Actions CI (fmt, clippy -D warnings, cargo-deny, the app/matrix-sdk boundary check, build and test on Linux, Windows, and macOS), the Linux build dependencies installed in CI, and the GPL-3.0-or-later license plus a recorded name-availability check. Roadmap step 1 (learn the prerequisites) has no deliverable and is skipped.

Decisions this plan makes (for the user to approve):
- Pinned versions: Rust toolchain `1.97.1` (the local toolchain, pinned in `rust-toolchain.toml` with rustfmt and clippy so CI runs the same lints), edition 2024, `iced = "=0.14.0"` (latest on crates.io), Synapse image `matrixdotorg/synapse:v1.151.0` (the tag matrix-rust-sdk's own integration tests use). `Cargo.lock` is committed. All pins go into PROGRESS.md.
- Crate names: `twicecat-core`, `twicecat-app` (binary `twicecat`), and `twicecat-ptt`, all `publish = false`. The bare name `twicecat` is already taken on crates.io, so we can't publish it under that name anyway.
- Homeserver: Synapse rather than Continuwuity. Continuwuity creates a first-user registration token at startup, which gets in the way of scripted open registration. We copy matrix-rust-sdk's harness pattern: on first start, run `/start.py generate`, then append the registration settings. Server name is `localhost`. The port is bound to `127.0.0.1:8008` only, so open registration can't be reached from the network.
- `.env.test` is **committed**. It holds only throwaway passwords for the loopback homeserver, and a header comment says real credentials must never go there. That lets the Phase 1 integration tests and CI run without secrets setup. We don't add a `.env.test.example`.
- Agents do not push. A green CI on all three OSes, the local `apt install` of the Linux deps, and opening the window are all human checklist items in PROGRESS.md.

Gate for every task (AGENTS.md rule 4), and it must pass before each commit:
`cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`

Each task is one commit, `phase0.<roadmap step>: summary`, and updates PROGRESS.md (step done, checks run, open issues, human items). ROADMAP.md is never committed (it is git-ignored locally; check with `git check-ignore ROADMAP.md`).

### Task 1: Workspace with empty Iced window (roadmap step 2)
Language: rust
Files: Cargo.toml, Cargo.lock, rust-toolchain.toml, .gitignore, crates/core/Cargo.toml, crates/core/src/lib.rs, crates/app/Cargo.toml, crates/app/src/main.rs, crates/ptt/Cargo.toml, crates/ptt/src/lib.rs, PROGRESS.md
Depends on: none
Acceptance criteria:
- Root `Cargo.toml` is a virtual workspace with `resolver = "3"`, `members = ["crates/*"]`, and `[workspace.package]` setting `edition = "2024"` and `repository = "https://github.com/ga-lep/twicecat"`. `rust-toolchain.toml` pins `1.97.1` with the `rustfmt` and `clippy` components.
- `twicecat-core` and `twicecat-ptt` are empty lib crates with no dependencies (`cargo tree -p twicecat-core --depth 1` and `cargo tree -p twicecat-ptt --depth 1` list only the crate itself).
- `twicecat-app` depends on `iced = "=0.14.0"` and `twicecat-core` (path), and has a `[[bin]] name = "twicecat"`. `main.rs` opens one empty window titled "Twicecat". Before coding it, the entry-point API (`iced::application` / `iced::run` or equivalent) is confirmed in `~/.cargo/registry/src/*/iced-0.14.0/` source or examples (rule 2), and PROGRESS.md records which API was used and where it was confirmed.
- `cargo build --workspace` passes, and so does the gate command.
- `.gitignore` contains `/target`. `git status --porcelain` shows no `ROADMAP.md`.
- PROGRESS.md exists and records the pinned versions, the Phase 0 step list with status, and a "Human checkpoint" checklist containing: run `cargo run -p twicecat-app` and confirm an empty window opens; approve the workspace layout and AGENTS.md.
- If the local Linux build fails because system libraries are missing, stop and add the `apt install` item to PROGRESS.md for the human (installing needs sudo). Don't work around it.
- Commit: `phase0.2: cargo workspace with core, app, ptt crates`.

### Task 2: Local Synapse homeserver and test users (roadmap step 3)
Language: rust
Files: docker-compose.yml, dev/synapse/start.sh, scripts/create-test-users.sh, .env.test, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- `docker-compose.yml` defines one `synapse` service. It uses image `matrixdotorg/synapse:v1.151.0`, a named data volume, the port `127.0.0.1:8008:8008`, and an entrypoint that runs the mounted `dev/synapse/start.sh`. That script runs `/start.py generate` (with `SYNAPSE_SERVER_NAME=localhost` and `SYNAPSE_REPORT_STATS=no`) only when `/data/homeserver.yaml` is missing, then appends `enable_registration: true` and `enable_registration_without_verification: true`, and finally runs `exec /start.py run`.
- From a clean state (`docker compose down -v`), `docker compose up -d --wait` exits 0, and `curl -sf http://127.0.0.1:8008/_matrix/client/versions` returns JSON.
- `.env.test` defines `HOMESERVER_URL=http://127.0.0.1:8008`, `HOMESERVER_DOMAIN=localhost`, and three users with throwaway passwords (`TEST_USER_1..3` / `TEST_PASSWORD_1..3`, for alice, bob, carol). Its header comment says the values are localhost-only throwaways and that real credentials must never be put there.
- `scripts/create-test-users.sh` (bash, `set -euo pipefail`, executable) sources `.env.test`, waits for the server to be up, and registers each user through `POST /_matrix/client/v3/register` with `m.login.dummy` auth, completing the UIA session if the server asks for it. It exits 0. Running it a second time also exits 0, because `M_USER_IN_USE` counts as success.
- The script never prints passwords or access tokens. Running `scripts/create-test-users.sh 2>&1 | grep -E "access_token|$TEST_PASSWORD_1"` finds nothing.
- After the script runs, a password login for each of the 3 users via `POST /_matrix/client/v3/login` returns HTTP 200. This check is done manually and recorded in PROGRESS.md, without printing the response.
- `shellcheck` passes on both scripts, if it is available. If it isn't, run it via `docker run --rm koalaman/shellcheck:stable`.
- The gate passes. Commit: `phase0.3: local synapse homeserver and test user script`.

### Task 3: CI on three OSes with fmt, clippy, deny, and boundary check (roadmap step 4)
Language: rust
Files: .github/workflows/ci.yml, deny.toml, scripts/check-app-deps.sh, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- `scripts/check-app-deps.sh [pattern]` (default pattern `matrix-sdk`) runs `cargo tree -p twicecat-app --depth 1 -e normal,build,dev --prefix none`. It exits 1 if any direct dependency's package name matches `^<pattern>`, and 0 otherwise. Because `cargo tree` prints real package names, renamed deps are caught too. On the current tree, `scripts/check-app-deps.sh` exits 0, and `scripts/check-app-deps.sh iced` exits 1 (this negative self-check is recorded in PROGRESS.md).
- `deny.toml` sets up advisories, bans (`multiple-versions = "warn"`), sources (crates.io only), and licenses with `[licenses.private] ignore = true`. The allowlist contains only licenses compatible with GPL-3.0 (for example MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, BSL-1.0, CC0-1.0), trimmed to what the dependency tree actually needs. Every license added beyond this list is justified in PROGRESS.md against the FSF GPL-compatibility list. Locally, `cargo deny check` passes, after `cargo install cargo-deny --locked`. If installing isn't possible, it goes on the human checklist.
- `ci.yml` runs on push and pull_request and has two jobs:
  - `lint` on `ubuntu-latest` runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `EmbarkStudios/cargo-deny-action@v2`, and `scripts/check-app-deps.sh`.
  - `test` is a matrix over `ubuntu-latest`, `windows-latest`, and `macos-latest` that runs `cargo build --workspace` and `cargo test --workspace`. It also runs clippy with `-D warnings` so platform-specific warnings are caught.
  - The toolchain comes from `rust-toolchain.toml`, and caching uses `Swatinem/rust-cache@v2`.
- `actionlint` reports no errors on `.github/workflows/ci.yml`. If it isn't installed, run it via `docker run --rm -v "$PWD":/repo -w /repo rhysd/actionlint:<pinned tag>`.
- PROGRESS.md human checklist gains: push the branch and confirm CI is green on Linux, Windows, and macOS.
- The gate passes. Commit: `phase0.4: github actions ci, cargo-deny, app dependency boundary check`.

### Task 4: Linux build dependencies in CI (roadmap step 5)
Language: rust
Files: .github/workflows/ci.yml, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- Both CI jobs run `sudo apt-get update && sudo apt-get install -y libxkbcommon-dev libwayland-dev libx11-dev libdbus-1-dev libssl-dev pkg-config` before any cargo step, and only when `runner.os == 'Linux'`.
- `actionlint` still reports no errors.
- PROGRESS.md human checklist gains: run the same `apt install` locally (needs sudo).
- The gate passes. Commit: `phase0.5: install linux build dependencies in ci`.

### Task 5: License and name (roadmap step 6)
Language: rust
Files: LICENSE, Cargo.toml, crates/core/Cargo.toml, crates/app/Cargo.toml, crates/ptt/Cargo.toml, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- `LICENSE` is the unmodified GPL-3.0 text from https://www.gnu.org/licenses/gpl-3.0.txt. `[workspace.package]` sets `license = "GPL-3.0-or-later"`, and all three crates inherit it with `license.workspace = true`. `cargo metadata --no-deps --format-version 1 | grep -c '"license":"GPL-3.0-or-later"'` prints 3.
- `cargo deny check licenses` still passes.
- PROGRESS.md records the name check:
  - crates.io: `tessera` is taken (v0.1.0, a 3D Tiles geometric-error library from github.com/sensat/tessera). This is mitigated by `publish = false` and the `twicecat-*` crate names.
  - Flathub: an unrelated GTK QR-mosaic app called "Twicecat" (`fr.ethal.twicecat`) has submission PR flathub/flathub#10239 open and awaiting review, as of 2026-10-01.
  - Flathub IDs are unique by reverse-DNS, so our ID would be something like `io.github.ga_lep.Twicecat`. The shared display name is left to the human to decide. Per the user's decision, we don't rename.
- The gate passes. Commit: `phase0.6: gpl-3.0-or-later license and name availability record`.
