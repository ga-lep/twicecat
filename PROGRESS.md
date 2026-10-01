# Progress

## Pinned versions
- Rust toolchain 1.97.1 (rust-toolchain.toml, with rustfmt and clippy), edition 2024, resolver 3
- iced =0.14.0
- Synapse matrixdotorg/synapse:v1.151.0 (docker-compose.yml)
- Cargo.lock is committed
- Phase 1 (core): matrix-sdk =0.19.1 (features bundled-sqlite, local-server), matrix-sdk-ui =0.19.1, keyring =4.2.0, directories =6.0.0; caret ranges (locked by Cargo.lock) for tokio, tracing, tracing-subscriber, futures-util, serde, serde_json, thiserror, url, rand, rpassword; dev-dep tempfile
- Phase 1 Task 4 (human-approved, rule 3): zeroize 1.9.0 (`zeroize = { version = "1.9", features = ["serde"] }`, already in Cargo.lock; lock diff only adds the edge and zeroize's serde edge)

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
- [x] 3. Secret store trait, keychain and in-memory backends (phase1.3)
- [x] 4. Engine, client builder, password login, persistence, restore (roadmap steps 2, 3a, 4)
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

## Phase 1 / Task 3 notes
- Checks run (all pass): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked` (34 core unit tests, 1 ignored), `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, `cargo deny check`. No homeserver tests exist yet, so no ignored-test run. The real-keychain test was NOT run (Decision 7); only `cargo test -p tessera-core -- --ignored --list` was used, which shows exactly one ignored test, `secrets::tests::keyring_real_keychain_round_trip`, and `--list` shows exactly one test name containing `keyring`.
- TDD: tests were first run against `todo!()` stubs (4 secrets tests and 7 session tests failed), then passed after implementation.
- APIs used: `keyring::Entry::{new (keyring-4.2.0/src/v1.rs:47), set_secret (:81), get_secret (:95), delete_credential (:102)}`, `keyring::Error` re-exported from keyring-core (v1.rs:~38; `Error::NoEntry` at keyring-core-1.0.0/src/error.rs:40). `matrix_sdk::authentication::matrix::MatrixSession` (matrix-sdk-0.19.1/src/authentication/matrix/mod.rs:844, Debug at :855), `authentication::oauth::UserSession` (oauth/mod.rs:1773), `matrix_sdk::SessionTokens` (lib.rs:71; authentication/mod.rs:35, Debug hides tokens), `matrix_sdk::SessionMeta` (lib.rs:28). `rand` 0.10.3: `rand::fill` (rand-0.10.3/src/lib.rs:314), which uses the thread-local ChaCha CSPRNG seeded from the OS (`thread_rng` is a default feature).
- Design:
  - `secrets.rs`: `SecretStore`, `InMemorySecretStore`, `KeyringSecretStore::new(service)`, constants `KEYCHAIN_SERVICE` and `TEST_KEYCHAIN_SERVICE`, `SecretError`. The trait is blocking; Task 4 must call it through `spawn_blocking` (not yet wired, no engine exists).
  - `session.rs` (private module, with a module-level `#[allow(dead_code)]` until Task 4 consumes it): `SessionRecord` and `SessionAuth` have fully manual redacted `Debug` (no tokens, passphrase, or OAuth client id). `to_bytes` returns `CoreError::Storage` over 2560 bytes. Serde error text is never forwarded, because serde can quote values. `from_bytes` rejects versions other than 1. Passphrase: 32 bytes, lowercase hex (64 chars), no new dependency (no base64 crate in core).
  - `SessionAuth::OAuth.client_id` is a `String` (the SDK's `oauth2::ClientId` is converted in Task 9).
- Open issues: none.
- New human item: the real-keychain test (already on the checkpoint list), now covering Linux Secret Service, macOS, and Windows.

## Phase 1 / Task 4 notes
- Checks run (all pass): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, `cargo test --workspace --locked` (40 core unit tests; 4 login tests reported as ignored), `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, `cargo deny check` (zeroize licence ok), shellcheck (docker) on check-core-api.sh. Ignored-test run: `docker compose up -d --wait && scripts/create-test-users.sh && cargo test -p tessera-core --locked -- --ignored --skip keyring` passed 3 times in a row (4/4), then `docker compose stop`. The real-keychain test was never run (Decision 7). `grep -c` of the ignore string in tests/login.rs = 4 = number of `#[tokio::test]`.
- TDD: login tests were first run before `Core` existed (compile error, unresolved imports). Session-record validation and path tests were added with their code in one step (not run red separately for paths).
- Integration failures found and fixed (not flakes): the first runs timed out because Synapse's default login rate limits (per-address burst 5, failed-attempt limits) returned 429 with a retry_after of minutes. Fix: `dev/synapse/start.sh` now relaxes `rc_login` (dev only); it applies only to a fresh volume, so run `docker compose down -v` once locally. The engine also bounds password login with a 20 s timeout (`LOGIN_TIMEOUT`), because the SDK waits out `retry_after`.
- SDK APIs used (matrix-sdk 0.19.1, registry src/): `ClientBuilder::{server_name_or_homeserver_url client/builder/mod.rs:283, homeserver_url :221, sqlite_store :301, handle_refresh_tokens :534, build :656}`; `ClientBuildError` (:945); `MatrixAuth::login_username` (authentication/matrix/mod.rs:175), `LoginBuilder::{initial_device_display_name login_builder.rs:135, send :174}` (send uses `RequestConfig::short_retry()`); `Client::{restore_session client/mod.rs:1591, session :889, logout :1652, subscribe_to_session_changes :3542, user_id :818, device_id :823, homeserver :711}`; `SessionChange::{UnknownToken, TokensRefreshed}` (client/mod.rs:167); `AuthSession` and `From<MatrixSession>` (authentication/mod.rs:193, :235); `matrix_sdk::Error::{Http, client_api_error_kind}` (error.rs:250, :428) and `HttpError::Reqwest` (:66); `ErrorKind::{Forbidden, UserDeactivated, LimitExceeded}` (ruma-common-0.20.0/src/api/error/kind.rs:124, :357, :168); `Client::devices` (client/mod.rs:2972, test helper only). zeroize 1.9.0: `Zeroizing` (src/lib.rs:604; `serde` feature for Serialize/Deserialize :706).
- Design: `engine.rs` (private; only `Core`, `CoreConfig`, `CoreHandle` are re-exported from lib.rs), `client.rs` (builder + error mapping to fixed messages). Server choice: `homeserver_override`, else the command's `server`, else the MXID's server name; a bare localpart with neither gives a `Login` error. Discovery note: with `@alice:localhost` and no override, discovery tries `https://localhost` and fails, which is why tests pass the override (`HOMESERVER_URL`). Restore failures (missing store dir, invalid record, keychain error) emit `Error`, not `NoSession`, so a transient keychain failure does not push the user into a duplicate login. `LoginOAuth`, `SelectRoom`, `SendMessage` are stubs until Tasks 7 and 9. OAuth records are refused on restore until Task 9. On `UnknownToken` (hard or soft logout) the record, marker and store dir are deleted and `SessionExpired` is emitted.
- Review carry-overs: (a) `session_store_dir` is an allowlist (1..=128 of `[A-Za-z0-9_-]`, Windows reserved names rejected; tests include "C:x", "C:", "a:b", "CON", "x."). (b) data root, `sessions/` and the session dir are created 0o700, `last_account` is 0o600 and tightened if older (Unix test). (c) `SessionRecord::from_bytes` checks session_id, homeserver URL (https, or http only for localhost/127.0.0.0/8/::1) and user_id == auth user id, with non-secret `CoreError::Storage`. (d) every `SecretStore` and file-system call goes through `spawn_blocking`. (e) the module-wide `allow(dead_code)` in session.rs is gone. (f) SDK errors map to constant messages; no SDK error text is forwarded or logged. (g) `.expose()` is used only at the login call; nothing logs exposed values.
- (h) `scripts/check-core-api.sh` new rules, each with a negative self-check (temporary edit, exit 1, reverted, exit 0): multi-line `pub use matrix_sdk::{ Client, };` in lib.rs fails; `pub mod engine;` in lib.rs fails; `pub(crate) mod foo;` in lib.rs fails; `pub use crate::engine::Core as X;` in api/mod.rs fails; the same split across lines (`pub use crate::` newline `session::..`) fails; `pub use crate::api::CoreError as Y;` in api/ is allowed (exit 0). Allow-list: api, paths, secrets. shellcheck clean.
- Zeroize (item i): `Password` and `MessageText` hold `Zeroizing<String>` (clones zeroize separately); `SessionRecord.store_passphrase` is `Zeroizing<String>`; `SessionRecord::to_bytes` returns `Zeroizing<Vec<u8>>`; the passphrase generator zeroizes its 32 raw bytes and the hex string wipes on drop; `InMemorySecretStore` stores `Zeroizing<Vec<u8>>`; `SecretStore::get` returns `Option<Zeroizing<Vec<u8>>>` (both backends). Known limitation: the SDK-owned `MatrixSession`/`UserSession` tokens (and the SDK's own copies inside `Client`) are not zeroized by us, and `AuthUrl` is not wiped; no SDK type is wrapped or forked. Residual copies can also exist in serde_json intermediate buffers and the keyring backend.
- Open issues: none. Parallel integration tests share the three accounts: bob is used only by the restore test (it counts devices).
- New human items: none beyond the existing list (real keychain restore on each OS is still covered by the keyring item); `docker compose down -v` once to pick up the relaxed rc_login.

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
