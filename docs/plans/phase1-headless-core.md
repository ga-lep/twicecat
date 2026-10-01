# Phase 1 Headless Core Plan
Status: APPROVED

## Goal
Build `tessera-core` into a headless Matrix client engine plus a small command-line front end (`tessera-cli`), with no UI. The engine builds an SDK `Client` with a per-session encrypted SQLite store in the OS data directory, logs in with a password or through OAuth 2.0 / MAS, keeps the session secrets only in the OS keychain (behind a trait with an in-memory test implementation), restores the session on startup, runs `SyncService` (sliding sync plus encryption sync), turns the `RoomListService` and a room `Timeline` into SDK-free diff events, sends messages with `timeline.send()`, and supervises sync state with reconnection. All of this sits behind the commands-in / events-out API that Phase 2 will plug Iced into. Integration tests in `crates/core/tests/` run against the local Synapse from `docker-compose.yml` with the `.env.test` accounts.

### Facts verified while planning (2026-10-01)
Each implementing task re-confirms the APIs it uses (AGENTS.md rule 2) and cites file:line in PROGRESS.md. I compiled a scratch crate in the session scratchpad, not in the repo, against all of these:
- Latest releases: `matrix-sdk` 0.19.1 and `matrix-sdk-ui` 0.19.1 (2026-09-18, MSRV 1.96, so OK with our 1.97.1), `keyring` 4.2.0, `directories` 6.0.0, `tracing-subscriber` 0.3.23, `rpassword` 7.5.4. The source lives in `~/.cargo/registry/src/index.crates.io-*/`. The repo's `examples/` and `labs/multiverse` are **not** in the crate tarballs. Read them from a clone of tag `matrix-sdk-0.19.1` (commit b18166c6) in the scratchpad, never inside the repo.
- `ClientBuilder::server_name_or_homeserver_url` (builder/mod.rs:283), `sqlite_store(path, Option<&str>)` (:301), `handle_refresh_tokens` (:534). The default sliding sync version is `Native` (MSC4186) (:157).
- `client.matrix_auth().login_username(id, pw)` returns a `LoginBuilder` with `.initial_device_display_name()` and `.send()` (or `.await`). `Client::restore_session(impl Into<AuthSession>)` (client/mod.rs:1591), `Client::session() -> Option<AuthSession>` (:889), `subscribe_to_session_changes()` (:3542). `AuthSession::{Matrix(MatrixSession), OAuth(Box<OAuthSession>)}`. `MatrixSession` and `oauth::UserSession` are serde, and their `Debug` hides tokens.
- `client.oauth()`: `.login(redirect_uri, None, Some(registration_data), None).build()` returns `OAuthAuthorizationData { url, .. }`. Then `.finish_login(UrlOrQuery)`, `.full_session()`, `.user_session()`, `.server_metadata()`. Client metadata comes from `registration::{ClientMetadata::new, ApplicationType::Native, OAuthGrantType::AuthorizationCode, Localized}`. The loopback redirect uses `utils::local_server::LocalServerBuilder` (feature `local-server`). All of this follows `examples/oauth_cli`.
- `matrix_sdk_ui::sync_service::{SyncService, State}`. `State` = `Idle | Running | Terminated | Error(Arc<Error>) | Offline`. `SyncServiceBuilder::with_offline_mode()` exists, and `start()` restarts the service from `Error`, `Terminated`, or `Offline`. `room_list_service().all_rooms().await?.entries_with_dynamic_adapters(page_size)` returns a `(stream of Vec<VectorDiff<RoomListItem>>, controller)`. The stream yields nothing until `controller.set_filter(Box::new(filters::new_filter_non_left()))` is called. `RoomListItem: Deref<Target = Room>`. `Room::{cached_display_name, num_unread_messages, num_unread_mentions, num_unread_notifications}` exist.
- `matrix_sdk_ui::timeline::RoomExt::{timeline, timeline_builder}`. `Timeline::subscribe()` returns `(Vector<Arc<TimelineItem>>, stream of Vec<VectorDiff<..>>)`. `Timeline::send(AnyMessageLikeEventContent)`, `paginate_backwards(u16)`. `VectorDiff` (eyeball-im 0.9.1) has 11 variants: Append, Clear, PushFront, PushBack, PopFront, PopBack, Insert, Set, Remove, Truncate, Reset.
- Using the timeline APIs needs `#![recursion_limit = "256"]` at the crate root. Without it, rustc fails with "queries overflow the depth limit", the same as in the SDK's `examples/timeline`.
- `keyring` 4.x is now a thin wrapper over `keyring-core` 1.0.0. With the default `v1` feature it exposes `keyring::Entry::{new, set_password, get_password, set_secret, get_secret, delete_credential}` and `Error::NoEntry`, and picks the native store: macOS Keychain, Windows Credential Manager, or Secret Service through zbus, which is pure Rust (no libdbus). The calls are blocking.
- `directories::ProjectDirs::from(qualifier, org, app)` returns `Option`, and has `.data_dir()`.
- Local Synapse v1.151.0 `/versions` advertises `org.matrix.simplified_msc3575: true`, so **SyncService works locally** without a proxy. Login flows are `m.login.password` only. `/_matrix/client/v1/auth_metadata` and the unstable msc2965 path return 404, so **OAuth can't be tested end to end locally** (no MAS).
- matrix-sdk needs no system sqlite with the `bundled-sqlite` feature. Default TLS is rustls with aws-lc-rs: aws-lc-sys built locally without extra packages, but Windows and macOS are only checked by CI.
- `cargo deny check` on the new tree with the current `deny.toml` finds no advisories. It rejects 10 crates under **MPL-2.0** (eyeball, eyeball-im, imbl, as_variant, async-rx, readlock…) and 1 under **CDLA-Permissive-2.0** (webpki-root-certs, via reqwest → rustls-platform-verifier).

### Decisions (human answers to Q1–Q6, 2026-10-01)
1. **Q1, dependency pins: accepted as proposed.** Exact pins `matrix-sdk = "=0.19.1"` (features `bundled-sqlite`, `local-server`, default features kept), `matrix-sdk-ui = "=0.19.1"`, `keyring = "=4.2.0"` (default `v1`), `directories = "=6.0.0"`. Caret ranges, locked by Cargo.lock, for `tokio`, `tracing`, `tracing-subscriber` (`env-filter`), `futures-util`, `serde`, `serde_json`, `thiserror`, `url`, `rand` (the major Cargo.lock already resolves for matrix-sdk), and `rpassword`. Dev-dep `tempfile`. No existing pin changes.
2. **Q2, deny.toml: accepted.** Re-allow `MPL-2.0` in the allowlist. Add `CDLA-Permissive-2.0` as a crate-scoped `[[licenses.exceptions]]` entry for `webpki-root-certs` only.
3. **Q3, integration tests: Option A.** Every homeserver integration test is marked `#[ignore = "needs local homeserver: docker compose up -d --wait"]`; the reason string is required, verbatim, on every such test. Plain `cargo test --workspace` skips them on every OS. The agent runs them locally with `-- --ignored --skip keyring` per the gate below (see Decision 7). The Linux-only `integration` CI job (Task 5) runs `cargo test -p tessera-core --locked -- --ignored --skip keyring` against the compose stack. Rejected: a Cargo feature, `cfg_attr` on non-Linux, a name filter in the `test` job.
4. **Q4, OIDC verification: default.** Unit tests plus the local "OAuth not supported" integration test; the real browser flow goes on the human checkpoint list. No MAS in compose, no wiremock mock server.
5. **Q5, app identity: accepted.** `ProjectDirs::from("io.github", "ga_lep", "Tessera")` and keychain service `io.github.ga_lep.Tessera`.
6. **Q6, scope: default.** Out of scope: E2EE flows (Phase 4; tests use unencrypted rooms, crypto store already set up), room create/invite/join commands (Phase 5; tests create rooms with a plain SDK client in a test helper), the bare call prototype, and multi-account support (one active account).
7. **Real keychain is human-only (2026-10-01).** The agent's local ignored-test run is `cargo test -p tessera-core --locked -- --ignored --skip keyring`, identical to the CI `integration` job, so the agent's gate equals CI. The real-keychain test is run only by the human (`cargo test -p tessera-core --locked -- --ignored keyring`, human checkpoint list). Rationale: the agent never touches the real OS keychain (security; AGENTS.md rules 6 and 9), and that test uses a dedicated service name `io.github.ga_lep.Tessera.test` and cleans up after itself (Task 3). The production service name stays `io.github.ga_lep.Tessera`.

### Conventions for every task
- **Gate (rule 4), required before each commit:** `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --locked`, plus `scripts/check-app-deps.sh` and `cargo deny check`. From Task 4 on, also run (Q3, Decision 7) `docker compose up -d --wait && scripts/create-test-users.sh && cargo test -p tessera-core --locked -- --ignored --skip keyring` (the same command as the CI `integration` job), then `docker compose stop`. The agent never runs the real-keychain test.
- **One commit per task**, message `phase1.<task number>: <summary>`, attribution trailer as instructed by the session. ROADMAP.md is never committed (`git check-ignore ROADMAP.md`).
- **PROGRESS.md (rule 5).** Task 1 adds a "Phase 1 steps" checklist (one line per task, with the roadmap steps it covers), new pinned versions under "Pinned versions", and a "Phase 1 human checkpoint" list. Each task ticks its line and adds a "Phase 1 / Task N notes" block with:
  - the checks run and their results (including the ignored-test run),
  - every SDK, keyring, or directories API used, with the registry file:line or example file where it was confirmed,
  - open issues,
  - new human items.

  Unverifiable items (real keychain, browser OIDC, real account, Windows/macOS CI) go on the human list, never ticked by the agent (rule 9).
- **Secrets hygiene (rule 6).** No token, password, store passphrase, recovery key, or message body ever goes to `tracing` or to a plain file. `Debug` on every public type that holds one is redacted. Default log filter: `warn,tessera_core=info`. The SDK's own `debug`/`trace` levels can log request details, which is noted in the CLI help. The CLI printing message bodies to stdout is user-facing output, not logging.
- **Boundary (rule 7).** `crates/app` is untouched in this phase. The public API lives in `crates/core/src/api/` and names no `matrix_sdk`, `matrix_sdk_ui`, `ruma`, `eyeball*`, or `imbl` type. SDK→API conversions are private functions in the engine, not public `From` impls. Task 2 adds the check for this.
- **Stop and ask (rule 8)** if any API above behaves differently from what this plan assumes, if an integration test is flaky twice in a row, or if a step needs a real account.

### Task 1: Dependencies, license policy, and data paths
Language: rust
Files: crates/core/Cargo.toml, Cargo.lock, deny.toml, crates/core/src/lib.rs, crates/core/src/paths.rs, PROGRESS.md
Depends on: none
Acceptance criteria:
- Roadmap step 1 (study the reference): PROGRESS.md "Task 1 notes" lists the multiverse and example files read (`labs/multiverse/src/main.rs`, `examples/{login,persist_session,oauth_cli,timeline}`) at tag `matrix-sdk-0.19.1`, and the patterns adopted. No SDK source is copied into the repo.
- `tessera-core` depends on the crates and versions approved in Q1. `cargo tree -p tessera-core --depth 1` shows `matrix-sdk v0.19.1`, `matrix-sdk-ui v0.19.1`, `keyring v4.2.0`, `directories v6.0.0`. No version of an existing dependency changes (`git diff Cargo.lock` adds packages only; `iced` stays 0.14.0).
- `deny.toml` changes exactly as approved in Q2, and each addition has a reason comment. `cargo deny check` passes. If the iced advisories stop applying, they stay as-is: the iced tree is unchanged.
- `crates/core/src/lib.rs` starts with `#![recursion_limit = "256"]`, with a comment explaining why.
- `paths.rs`:
  - `DataPaths::from_os()` uses `ProjectDirs::from(<Q5 ids>)`; `DataPaths::at(root)` is for tests and `--data-dir`.
  - `session_store_dir(session_id)` returns `<root>/sessions/<session_id>/`.
  - `last_account_file()` returns `<root>/last_account`, which holds only a user ID, never a secret.
  - Unit tests: paths are under the root; a `session_id` with `/`, `..` or `\` is rejected; `from_os()` returns `Some` on the test machine.
- `scripts/check-app-deps.sh` still exits 0. The gate passes on Linux. Windows and macOS CI green goes on the human list.
- Commit: `phase1.1: core dependencies, license policy, data paths`.

### Task 2: Public command/event API and the SDK boundary check (roadmap step 8)
Language: rust
Files: crates/core/src/api/mod.rs, crates/core/src/api/diff.rs, crates/core/src/api/types.rs, crates/core/src/lib.rs, scripts/check-core-api.sh, .github/workflows/ci.yml, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- `api` defines SDK-free types, all `Clone + Debug`:
  - `Command`:
    - `RestoreSession`
    - `LoginPassword { username, password: Password, server: Option<String> }`. `username` is a localpart or full MXID; `server` is a server name or homeserver URL, and when it is `None` it is taken from the MXID.
    - `LoginOAuth { server }`, `CancelOAuth`
    - `SelectRoom(RoomId)`, `SendMessage { text }` (to the selected room)
    - `Logout`, `Shutdown`
  - `CoreEvent`:
    - `NoSession`, `LoggedIn { user_id, device_id, restored: bool }`, `LoginFailed(CoreError)`
    - `OAuthUrl(String)`, `LoggedOut`, `SessionExpired`
    - `SyncState(SyncState)`
    - `RoomListChanged(Vec<ListDiff<RoomSummary>>)`
    - `TimelineDiff { room_id, diffs: Vec<ListDiff<TimelineItem>> }`
    - `SendFailed { room_id, error }`, `Error(CoreError)`
  - `SyncState { Idle, Running, Offline, Reconnecting { attempt: u32 }, Error(String), Terminated }`
  - `RoomSummary { id, name, unread_messages, unread_mentions, unread_notifications }`
  - `TimelineItem { id: String, kind: TimelineItemKind }`, where `TimelineItemKind::Message { sender, body, is_own, timestamp_ms, sent: SendStatus }` or `Other`
  - newtypes `RoomId`, `UserId`, `DeviceId`, `Password`
  - `CoreError`: `thiserror`, with non-secret messages.
- `ListDiff<T>` mirrors the 11 `VectorDiff` variants and has `apply(&self, &mut Vec<T>)`. A unit test covers each variant against a hand-computed expected `Vec`.
- Redaction:
  - `format!("{:?}", Password::from("hunter2-x"))` doesn't contain `hunter2-x`.
  - `format!("{:?}", <message TimelineItem with body "secret-body-x">)` doesn't contain `secret-body-x`.
  - Both are unit-tested. A `Command::LoginPassword` Debug check is unit-tested the same way.
- `scripts/check-core-api.sh` (bash, `set -euo pipefail`, fails closed like `check-app-deps.sh`) exits 1 if:
  - `crates/core/src/api/` mentions `matrix_sdk`, `ruma`, `eyeball`, or `imbl`, or
  - `crates/core/src/lib.rs` has a `pub use` of those crates.

  It exits 0 on the tree. Negative self-check: a temporary `use matrix_sdk::Client;` in `api/mod.rs` makes it exit 1, and the result is recorded in PROGRESS.md. shellcheck is clean.
- The `lint` CI job runs `scripts/check-core-api.sh`. actionlint is clean.
- Gate passes. Commit: `phase1.2: sdk-free command and event api with boundary check`.

### Task 3: Secret store trait, keychain and in-memory implementations, session record
Language: rust
Files: crates/core/src/secrets.rs, crates/core/src/session.rs, crates/core/src/lib.rs, PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- `pub trait SecretStore: Send + Sync` has `get(&self, account: &str) -> Result<Option<Vec<u8>>, SecretError>`, `set(&self, account: &str, secret: &[u8])`, and `delete(&self, account: &str)` (deleting a missing entry is `Ok`).
  - `InMemorySecretStore` uses `Mutex<HashMap>` and is public, so CLI `--ephemeral` and tests can use it.
  - `KeyringSecretStore` uses `keyring::Entry::new(<Q5 service>, account)` with `set_secret`, `get_secret`, and `delete_credential`, and maps `Error::NoEntry` to `Ok(None)`.
  - The engine calls the store through `tokio::task::spawn_blocking`, because keyring calls block.
- `SessionRecord` (serde JSON, `version: 1`) holds:
  - `user_id`, `homeserver_url`, `session_id` (the store dir name), `store_passphrase`
  - `auth`: `Matrix(MatrixSession)` or `OAuth { client_id, user: UserSession }`. These SDK types are allowed here because `session.rs` is private, not in `api`.

  `Debug` is redacted: no token or passphrase appears in `format!("{:?}")`, unit-tested with sentinel values.
- The store passphrase is 32 random bytes, base64 or hex encoded, from `rand`. A unit test checks the length and that two calls differ.
- Unit tests:
  - round-trip through `InMemorySecretStore`: set, get, delete, then get returns `None`;
  - `SessionRecord` JSON round-trip;
  - a realistic record (Synapse-style and MAS-style tokens) serializes to under 2560 bytes, the Windows Credential Manager blob limit. The code returns `CoreError` instead of writing an oversize blob.
- An `#[ignore = "touches the real OS keychain"]` test, whose name contains `keyring`, does a set/get/delete round-trip with `KeyringSecretStore` under a throwaway account name:
  - it uses the dedicated service name `io.github.ga_lep.Tessera.test`, distinct from the production `io.github.ga_lep.Tessera` (`KeyringSecretStore` takes the service name as a constructor argument; production code passes the Q5 constant). A unit test checks the test constant differs from the production constant;
  - it deletes its entry on completion, including on failure or panic, through a drop guard that calls `delete_credential` (a missing entry is ignored);
  - no other ignored test has `keyring` in its name, so `--skip keyring` skips exactly this test and `--ignored keyring` selects exactly it (checked with `cargo test -p tessera-core -- --ignored --list`).

  The agent does not run this test (Decision 7). Human checklist gains: run `cargo test -p tessera-core --locked -- --ignored keyring` on Linux (Secret Service), macOS, and Windows, and confirm no `io.github.ga_lep.Tessera.test` entry remains in the keychain afterwards.
- Gate passes. Commit: `phase1.3: secret store trait with keychain and in-memory backends`.

### Task 4: Engine, client builder, password login, session persistence and restore (roadmap steps 2, 3a, 4)
Language: rust
Files: crates/core/src/engine.rs, crates/core/src/client.rs, crates/core/src/lib.rs, crates/core/tests/common/mod.rs, crates/core/tests/login.rs, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- `Core::start(CoreConfig { data: DataPaths, homeserver_override: Option<String> }, Arc<dyn SecretStore>) -> CoreHandle { commands: tokio mpsc Sender<Command>, events: tokio mpsc Receiver<CoreEvent> }`.
  - It must be called inside a tokio runtime, and spawns one engine task. Phase 2 will provide the runtime.
  - `Shutdown` stops the task cleanly, and the events channel then closes.
- The client is built with:
  - `Client::builder().server_name_or_homeserver_url(server)`: server from the command, else from the MXID's server name;
  - `.sqlite_store(session_store_dir(new random session_id), Some(passphrase))`;
  - `.handle_refresh_tokens()`.
- `LoginPassword` runs `matrix_auth().login_username(..).initial_device_display_name("Tessera")`. On success:
  - the `SessionRecord` goes into the `SecretStore` under the user ID;
  - the user ID goes into `last_account`;
  - `LoggedIn { restored: false }` is emitted.

  On failure, the core emits `LoginFailed` and removes the half-created store dir.
- `RestoreSession` reads `last_account`, then the `SecretStore` record. It rebuilds the client with `homeserver_url(record.homeserver_url)` and the same store dir and passphrase, calls `client.restore_session(..)`, and emits `LoggedIn { restored: true }`. If no record exists, it emits `NoSession`.
- The engine subscribes to `subscribe_to_session_changes()`. On `TokensRefreshed` it rewrites the record. On `UnknownToken` it emits `SessionExpired` and deletes the record.
- `Logout` calls the server logout (best effort), deletes the record, `last_account`, and the store dir, and emits `LoggedOut`.
- `tests/common/mod.rs` (test helper):
  - reads `.env.test` from the workspace root with a tiny `KEY=VALUE` parser (no new dep), and environment variables override it;
  - provides `start_core(tempdir, Arc<InMemorySecretStore>)`, `expect_event(timeout, predicate)` (30 s default), and a plain-SDK `raw_login(user)` helper for test setup;
  - never prints passwords, tokens, or bodies.
- Integration tests, each marked exactly `#[ignore = "needs local homeserver: docker compose up -d --wait"]` (Q3 Option A; `grep -c` of that string in `crates/core/tests/login.rs` equals the number of `#[tokio::test]` functions there), homeserver override `HOMESERVER_URL` from `.env.test`. Plain `cargo test --workspace --locked` reports them as ignored, not run:
  - **password login**: alice logs in and gets `LoggedIn { user_id: "@alice:localhost", restored: false }`. A wrong password gives `LoginFailed`, and the secret store stays empty.
  - **session restore without logging in again**: log in, `Shutdown`, then start a new `Core` on the same tempdir and the same `InMemorySecretStore`, and send `RestoreSession`. That gives `LoggedIn { restored: true }` with the **same device_id** as the first login. The first login's device is the only new device, which proves no second login happened.
  - **no token on disk**: after login, the access token from the in-memory record is searched for as bytes in every file under the tempdir. It is not found.
  - **logout**: after `Logout` then `RestoreSession`, the result is `NoSession`, and the store dir is gone.
- The gate passes, and `cargo test -p tessera-core --locked -- --ignored --skip keyring` passes against the local compose stack. PROGRESS.md notes the server-discovery behaviour: with `@alice:localhost` and no override, discovery tries `https://localhost` and fails, which is why tests pass the override. Commit: `phase1.4: client builder, password login, keychain session persistence and restore`.

### Task 5: Linux CI job for homeserver integration tests
Language: rust
Files: .github/workflows/ci.yml, PROGRESS.md
Depends on: Task 4
Acceptance criteria:
- A new `integration` job on `ubuntu-latest` with the same hardening as the existing jobs:
  - `permissions: contents: read`, `persist-credentials: false`;
  - apt deps, rust-cache;
  - runs `docker compose up -d --wait`, `scripts/create-test-users.sh`, and `cargo test -p tessera-core --locked -- --ignored --skip keyring` (Q3 Option A: this job is the only CI place the `#[ignore = "needs local homeserver: docker compose up -d --wait"]` tests run). The keyring test needs a desktop session, hence `--skip keyring`.
  - It ends with `docker compose down -v` under `if: always()`.
- actionlint is clean. The existing `lint` and `test` jobs are unchanged apart from Task 2's line; the `test` job keeps plain `cargo test --workspace` (no feature, no `cfg_attr`, no name filter), so homeserver tests stay ignored there on every OS.
- Human checklist gains: push, and confirm the `integration` job is green.
- Gate passes. Commit: `phase1.5: ci job running core integration tests against local synapse`.

### Task 6: SyncService and room list events (roadmap step 5)
Language: rust
Files: crates/core/src/engine.rs, crates/core/src/sync.rs, crates/core/src/room_list.rs, crates/core/tests/room_list.rs, PROGRESS.md
Depends on: Task 5
Acceptance criteria:
- After `LoggedIn`, the engine builds `SyncService::builder(client).with_offline_mode().build()` and calls `start()`. It forwards `state()` changes as `CoreEvent::SyncState`, with an explicit mapping table in code and a unit test over a fake state sequence.
- `room_list_service().all_rooms()` with `entries_with_dynamic_adapters(page size from config, default 500)` and `set_filter(new_filter_non_left())`. Each diff batch is mapped privately to `ListDiff<RoomSummary>`:
  - `name` from `cached_display_name()`, falling back to the room ID;
  - unread counts from `num_unread_messages`, `num_unread_mentions`, and `num_unread_notifications`.

  Each batch is emitted as one `RoomListChanged`.
- `Logout` and `Shutdown` call `sync_service.stop()` before tearing down.
- Integration test **room appears**: alice's core logs in and reaches `SyncState(Running)`. A plain-SDK alice client (test helper) creates a room named `tessera-test-<uuid or random>`. Applying the `RoomListChanged` diffs to a `Vec` then yields a `RoomSummary` with that name within 30 s.
- Integration test **unread count**: bob is invited and joins via the helper, and alice sends 2 messages via the helper. Bob's core shows that room with `unread_messages >= 2`.
- The gate and the ignored integration run (`--ignored --skip keyring`) pass. Commit: `phase1.6: sync service and room list diff events`.

### Task 7: Timeline, select room, send message (roadmap step 6)
Language: rust
Files: crates/core/src/engine.rs, crates/core/src/timeline.rs, crates/core/tests/timeline.rs, PROGRESS.md
Depends on: Task 6
Acceptance criteria:
- `SelectRoom(id)`:
  - drops any previous room's timeline task;
  - builds `room.timeline()` (via `RoomExt`) and calls `subscribe()`;
  - emits the initial items as one `TimelineDiff` with a single `ListDiff::Reset`, then forwards each diff batch mapped to `ListDiff<TimelineItem>`.

  Text messages map to `Message { sender, body, is_own, timestamp_ms, sent }`, and everything else maps to `Other`. An unknown room ID gives `Error(CoreError::UnknownRoom)`.
- `SendMessage { text }` calls `timeline.send(RoomMessageEventContent::text_plain(text).into())` on the selected timeline. With no room selected the result is `Error(NoRoomSelected)`, and a send failure gives `SendFailed`.
- Integration test **A to B through the timeline stream**:
  - setup: alice (helper) creates a room and invites bob, and bob (helper) joins;
  - bob's core runs `SelectRoom` on it, and alice's core runs `SelectRoom` on it and sends `SendMessage` with a unique marker text;
  - bob's applied `TimelineDiff`s contain a `Message` with that body, `sender == "@alice:localhost"`, and `is_own == false`, within 30 s;
  - alice's own timeline shows the item with `is_own == true` and `sent` reaching `Sent`.
- Integration test: `SendMessage` with no room selected gives `Error(NoRoomSelected)`.
- The gate and the ignored integration run (`--ignored --skip keyring`) pass. Commit: `phase1.7: room timeline subscription and message sending`.

### Task 8: Sync supervision, reconnection, and tracing (roadmap step 7)
Language: rust
Files: crates/core/src/sync.rs, crates/core/src/logging.rs, crates/core/src/lib.rs, PROGRESS.md
Depends on: Task 7
Acceptance criteria:
- Supervisor: on `State::Error` or an unexpected `Terminated`, the core emits `SyncState(Reconnecting { attempt })` and calls `sync_service.start()` again after an exponential backoff: 1 s, 2 s, 4 s … capped at 60 s, with ±20% jitter. The attempt count resets only after `Running` has held for 30 s without a failure (`STABLE_RESET`), because the SDK publishes `Running` before any request succeeds.
  - `Offline` is left to the SDK's offline mode, which polls `/versions` and resumes by itself, and is reported as `SyncState(Offline)`. Offline-cycle throttle: a `Running` that does not reach `STABLE_RESET` before the next `Offline` is an unstable cycle; after 3 consecutive unstable cycles (`OFFLINE_CYCLE_LIMIT`) the supervisor emits `Reconnecting { attempt }`, calls `stop()`, waits the backoff delay (attempt counter shared with the `Error` path), then calls `start()`.
  - The backoff policy is a pure function, unit-tested for the sequence, the cap, the jitter bounds, and the reset.
  - The supervisor loop is unit-tested with a fake state stream (no network).
- `logging::init(default_filter)` sets up `tracing-subscriber` `fmt` on stderr with `EnvFilter` (`RUST_LOG` overrides the default `warn,tessera_core=info`). Tracing calls in the core never format tokens, passwords, passphrases, or bodies. `grep -nE 'body|token|password|passphrase' crates/core/src` is reviewed, and any hits inside `tracing::` macros are justified in PROGRESS.md.
- An agent-run reconnection check, recorded in PROGRESS.md with the observed event sequence. It needs the Task 10 CLI, or a temporary test binary if Task 10 isn't done yet; otherwise run it as part of Task 10 and note that here.
  1. CLI logged in as alice against the local server, with sync running.
  2. `docker compose stop synapse`: the core reports `Offline` or `Reconnecting`.
  3. `docker compose start synapse`: within 90 s the core reports `Running` again, and a message sent afterwards is delivered.
- Gate passes. Commit: `phase1.8: sync supervision with backoff and tracing setup`.

### Task 9: OAuth 2.0 / MAS login (roadmap step 3b)
Language: rust
Files: crates/core/src/oauth.rs, crates/core/src/engine.rs, crates/core/tests/oauth.rs, PROGRESS.md
Depends on: Task 8
Acceptance criteria:
- `LoginOAuth { server }`:
  1. Builds a client (new session dir and passphrase).
  2. Spawns `LocalServerBuilder::new().spawn()` to get the loopback redirect URI.
  3. Calls `oauth().login(redirect_uri, None, Some(client_metadata().into()), None).build()` and emits `OAuthUrl(url)`.
  4. Awaits the redirect, then calls `finish_login(query.into())`.
  5. Stores `SessionRecord { auth: OAuth { client_id, user } }` from `full_session()` and emits `LoggedIn { restored: false }`.

  `CancelOAuth` calls `abort_login` and emits `LoginFailed(Cancelled)`. A server without OAuth support gives `LoginFailed(CoreError::OAuthNotSupported)`, and so does a dynamic-registration `NotSupported` error.
- `client_metadata()`: `ApplicationType::Native`, `AuthorizationCode` with loopback redirect URIs `http://127.0.0.1/` and `http://[::1]/`, `client_name` "Tessera", and `client_uri` the repository URL. A unit test checks those fields in the serialized JSON.
- `RestoreSession` handles OAuth records with `client.restore_session(OAuthSession { client_id, user })`. Unit test: an OAuth `SessionRecord` round-trips and is selected for the OAuth restore path.
- Integration test (`#[ignore = "needs local homeserver: docker compose up -d --wait"]`, local server): `LoginOAuth { server: HOMESERVER_URL }` gives `LoginFailed(OAuthNotSupported)` within 30 s, with no `OAuthUrl` emitted and no record stored. Per Q4 there is no MAS in compose and no mock server; the real flow is on the human list.
- Human checklist gains:
  - OIDC login on matrix.org in a browser via the CLI;
  - the session survives a CLI restart;
  - keep the CLI running for over 10 minutes so MAS refreshes the access token, then restart and confirm restore still works (refresh persistence can't be verified locally).
- The gate and the ignored integration run (`--ignored --skip keyring`) pass. Commit: `phase1.9: oauth 2.0 login via loopback redirect`.

### Task 10: `tessera-cli` binary (roadmap steps 5–7, user-facing)
Language: rust
Files: crates/core/Cargo.toml, crates/core/src/bin/tessera-cli.rs, crates/core/tests/cli.rs, PROGRESS.md
Depends on: Task 9
Acceptance criteria:
- `[[bin]] name = "tessera-cli"` in `tessera-core`. It talks to the engine **only** through `Core::start`, `Command`, and `CoreEvent`: no `matrix_sdk` import in the bin file (checked by grep in the test or the script).
- Flags: `--data-dir <path>`, `--homeserver <url>` (override), `--ephemeral` (`InMemorySecretStore` plus a temporary data dir), `--password-stdin`, `--help`.
- On start it sends `RestoreSession`. On `NoSession` it prompts for the username, then the password via `rpassword`, or reads the password from stdin with `--password-stdin`.
- REPL commands: `/login`, `/oauth <server>` (prints the URL to open), `/rooms` (prints index, name, unread counts), `/select <index|room id>`, `/send <text>`, `/logout`, `/quit`.
- Incoming timeline messages print as `[room] sender: body` on stdout. Sync state changes print as one status line. Logs go to stderr via `logging::init`.
- Integration test (`#[ignore = "needs local homeserver: docker compose up -d --wait"]`) runs the binary via `env!("CARGO_BIN_EXE_tessera-cli")` with `--ephemeral --homeserver $HOMESERVER_URL --password-stdin`:
  - stdin carries the alice username, the password, then `/rooms` and `/quit`;
  - the process exits 0 within 60 s, and stdout contains `Logged in as @alice:localhost`;
  - neither stdout nor stderr contains the password.
- If not done in Task 8, the reconnection check from Task 8 is run with this CLI and recorded.
- Human checklist gains:
  - `cargo run -p tessera-core --bin tessera-cli` against your real account: password login, list rooms, open a room, send a message;
  - restart and confirm it restores without a password prompt;
  - check the OS keychain viewer shows the `io.github.ga_lep.Tessera` entry, and that no token sits in the data dir;
  - plus the OIDC items from Task 9.
- PROGRESS.md marks the Phase 1 agent work done, and the human checkpoint stays open.
- The gate and the ignored integration run (`--ignored --skip keyring`) pass. Commit: `phase1.10: headless tessera-cli on the core api`.

### Phase 1 human checkpoint (collected in PROGRESS.md)
- [x] Q1–Q6 answered 2026-10-01 (see Decisions).
- [ ] CI green on Linux, Windows, and macOS with the new SDK tree (aws-lc-sys and bundled sqlite builds), and the `integration` job green.
- [ ] Real-keychain test `cargo test -p tessera-core --locked -- --ignored keyring` passes on each OS you use, and no `io.github.ga_lep.Tessera.test` entry is left in the keychain.
- [ ] CLI against your real account: password login, room list with unread counts, open a room, send and receive, restart restores without a prompt.
- [ ] OIDC login on matrix.org in a browser; restore after restart; restore still works after more than 10 minutes (token refresh).
- [ ] Approve the `api` module as the Phase 2 contract.
