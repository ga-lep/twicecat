# Progress

## Pinned versions
- Rust toolchain 1.97.1 (rust-toolchain.toml, with rustfmt and clippy), edition 2024, resolver 3
- iced =0.14.0
- Synapse matrixdotorg/synapse:v1.151.0 (docker-compose.yml)
- Cargo.lock is committed
- Phase 1 (core): matrix-sdk =0.19.1 (features bundled-sqlite, local-server), matrix-sdk-ui =0.19.1, keyring =4.2.0, directories =6.0.0; caret ranges (locked by Cargo.lock) for tokio, tracing, tracing-subscriber, futures-util, serde, serde_json, thiserror, url, rand, rpassword; dev-dep tempfile

## Phase 0 steps
- [x] 2. Cargo workspace with core, app, ptt crates (phase0.2)
- [x] 3. Local Synapse homeserver and test users (phase0.3)
- [x] 4. GitHub Actions CI, cargo-deny, boundary check (phase0.4)
- [x] 5. Linux build dependencies in CI (phase0.5)
- [x] 6. License and name check (phase0.6)

Phase 0 agent work is done; the Phase 0 human checkpoint is fully signed off (2026-10-01).

## Phase 1 steps
- [x] 1. Core dependencies, license policy, data paths (phase1.1; roadmap step 1)
- [x] 2. Public command/event API and SDK boundary check (phase1.2; roadmap step 8)
- [ ] 3. Secret store trait, keychain and in-memory backends
- [ ] 4. Engine, client builder, password login, persistence, restore (roadmap steps 2, 3a, 4)
- [ ] 5. Linux CI job for homeserver integration tests
- [ ] 6. SyncService and room list events (roadmap step 5)
- [ ] 7. Timeline, select room, send message (roadmap step 6)
- [ ] 8. Sync supervision, reconnection, tracing (roadmap step 7)
- [ ] 9. OAuth 2.0 / MAS login (roadmap step 3b)
- [ ] 10. tessera-cli binary

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

## Phase 1 / Task 1 notes
- Checks run (all pass): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked` (3 new paths tests), `scripts/check-app-deps.sh` (exit 0), `cargo deny check` (advisories, bans, licenses, sources ok). TDD: the 3 paths tests were first run against `todo!()` stubs and failed, then passed after implementation.
- `cargo tree -p tessera-core --depth 1`: matrix-sdk v0.19.1, matrix-sdk-ui v0.19.1, keyring v4.2.0, directories v6.0.0, rand v0.10.3, rpassword v7.5.4, tokio v1.53.1, tracing-subscriber v0.3.23, tempfile v3.27.0 (dev). `iced` stays 0.14.0 and wgpu 27.0.1 in Cargo.lock; no existing package version removed.
- deny.toml: `MPL-2.0` re-added to the allowlist and a `[[licenses.exceptions]]` entry for `webpki-root-certs` with `CDLA-Permissive-2.0`, each with a reason comment (Q2). The iced advisory ignores are unchanged.
- Reference study (tag `matrix-sdk-0.19.1`, cloned in the session scratchpad, nothing copied into the repo): `labs/multiverse/src/main.rs` (SyncService builder, `entries_with_dynamic_adapters` plus `set_filter(new_filter_non_left())`, per-room timelines), `examples/persist_session/src/main.rs` (`sqlite_store(path, Some(passphrase))`, `restore_session`, session file split), `examples/oauth_cli/src/main.rs` (`LocalServerBuilder::new().spawn()`, `finish_login`, OAuth session restore), `examples/timeline/src/main.rs` (`timeline.subscribe()`), `examples/login/src/main.rs` (password login). Patterns adopted: per-session encrypted sqlite store with a random passphrase kept out of the data dir (keychain instead of the example's plain JSON file), session restore through `restore_session`, dynamic-adapter room list with explicit filter.
- APIs used: `directories::ProjectDirs::from(&str, &str, &str) -> Option<ProjectDirs>` and `.data_dir()` (directories-6.0.0/src/lib.rs:428, :473 for the platform impl; :220 is the BaseDirs doc variant). No matrix-sdk API is called yet.
- `paths.rs`: `DataPaths::{from_os, at, root, session_store_dir, last_account_file}`; session ids that are empty, `.`, or contain `..`, `/`, `\` or NUL are rejected with `PathsError::InvalidSessionId`.
- `lib.rs` starts with `#![recursion_limit = "256"]` plus a comment.
- Open issues: none. Windows and macOS CI with the new SDK tree is on the human list below.

## Phase 1 / Task 2 notes
- Checks run (all pass): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked` (15 new unit tests plus 2 doctests), `scripts/check-app-deps.sh`, `scripts/check-core-api.sh` (exit 0), `cargo deny check`. shellcheck (docker koalaman/shellcheck:stable) clean on `scripts/check-core-api.sh`; actionlint 1.7.7 (docker) clean. No homeserver tests exist yet, so no ignored-test run.
- TDD: tests were written with the types; a mutation run (Truncate made a no-op, `Password` Debug printing the value) failed 3 tests, then the real code passed.
- APIs used: no SDK API. `api/` is SDK-free (std, `thiserror`). `ListDiff` mirrors the 11 `VectorDiff` variants named in the plan (Append, Clear, PushFront, PushBack, PopFront, PopBack, Insert, Set, Remove, Truncate, Reset); the SDK mapping is Task 6/7 and will be confirmed there.
- Design: `ListDiff::apply` ignores out-of-range indices instead of panicking. `TimelineItemKind` has a manual redacted `Debug` (body shown only as byte length), so any event holding a timeline item is safe to log. `SendStatus` is `Sending | Sent | Failed`.
- Negative self-check of `scripts/check-core-api.sh`: with a temporary `use matrix_sdk::Client;` as the first line of `api/mod.rs` it exits 1 (reports the line); a temporary `pub use matrix_sdk::Client;` in `lib.rs` also exits 1; reverted, it exits 0. A missing `api/` dir exits 2 (fail closed).
- CI: `lint` job runs `scripts/check-core-api.sh` after `check-app-deps.sh`.
- Review fix: `Command::SendMessage.text` is now `MessageText` and `CoreEvent::OAuthUrl` holds `AuthUrl`; both have redacted `Debug` (text shows byte length, URL shows scheme+host only) with `expose()` accessors. Tests added for both, plus `ListDiff::Insert` at `index == len`.
- Open issues: none. New human items: none (the `api` approval is already on the checkpoint list).

## Phase 1 human checkpoint
- [ ] CI green on Linux, Windows, and macOS with the new SDK tree (aws-lc-sys and bundled sqlite builds), and the `integration` job green (Task 5).
- [ ] Real-keychain test `cargo test -p tessera-core --locked -- --ignored keyring` passes on each OS you use, and no `io.github.ga_lep.Tessera.test` entry is left in the keychain.
- [ ] CLI against your real account: password login, room list with unread counts, open a room, send and receive, restart restores without a prompt.
- [ ] OIDC login on matrix.org in a browser; restore after restart; restore still works after more than 10 minutes (token refresh).
- [ ] Approve the `api` module as the Phase 2 contract.

## Human checkpoint
- [x] Push the branch and confirm CI is green on Linux, Windows, and macOS (confirmed by human 2026-10-01).
- [x] Run `cargo run -p tessera-app` and confirm an empty window titled "Tessera" opens (rendering is unverified by the agent) (confirmed by human 2026-10-01).
- [x] Approve the workspace layout and AGENTS.md (confirmed by human 2026-10-01).
- [x] Run locally (needs sudo): `sudo apt-get update && sudo apt-get install -y libxkbcommon-dev libwayland-dev libx11-dev libdbus-1-dev libssl-dev pkg-config` (confirmed by human 2026-10-01).
