# Phase 4 End-to-End Encryption Plan
Status: APPROVED

## Goal
The SDK already encrypts and decrypts. Phase 4 builds the trust and key-recovery flows around it, the places where users get stuck, and proves them against the local homeserver.

The core (behind the additive command/event API, Phase 2 Q6 convention) gains:
- **Encrypted rooms:** the client is built with explicit encryption settings (Q4). Rooms report whether they are encrypted. Messages carry an authenticity shield. Undecryptable messages carry a reason, and they turn into normal messages in place (same item ID) once their keys arrive.
- **Recovery (key backup + secret storage):** the core reports `Recovery::state()`. It can set recovery up (the new recovery key goes to the app once and is never stored), restore with a key or passphrase (`recover()`), and change the key.
- **Cross-signing:** bootstrapped on login (Q4). If that fails, it can be bootstrapped or reset on request, with re-authentication (password UIA, or the OAuth approval URL).
- **Interactive verification:** incoming and outgoing requests, for this account's own devices and for other users. SAS with the 7 emoji (decimals as a fallback), confirm, mismatch and cancel. **QR display (Q3):** when the other device can scan, Twicecat shows a QR code for it to scan, then asks the user to confirm the scan. Twicecat cannot scan codes.
- **Trust badges:** a verified/unverified state for devices and for the peer of a DM, updated live.
- **Session management:** list the account's devices with their trust, rename one, sign out others (re-authentication), verify one.
- **Security review:** a log-capture test and a data-at-rest scan show that no token, store passphrase, recovery key or message body reaches the logs or the disk in clear. The PROGRESS.md security review records the residual risks.

The app gains:
- a security banner, including a persistent "this device is not verified" banner after **Skip for now** (Q6);
- "Confirm your identity" and recovery setup/restore screens;
- the verification dialog with the emoji grid and the QR code;
- shields and trust badges;
- reason-specific UTD placeholders;
- a Sessions screen with a re-authentication prompt.

Rendering, the recovery-key wording, scanning Twicecat's QR code with a phone, and real-device interop (Element X on a phone, a matrix.org OAuth account) stay on the human checkpoint (rule 9).

E2EE integration tests use only the `.env.test` accounts (Q2): alice and carol, reset to a known server-side crypto baseline at the start of every test and serialized within each test binary. bob stays untouched by them.

### Human answers recorded (2026-10-06)
- **Q1:** Phase 3 is signed off (human confirmed 2026-10-06). Task 1 writes the explicit sign-off line in PROGRESS.md.
- **Q2:** no throwaway accounts. E2EE tests use only alice/bob/carol from `.env.test` (rule 6). Q2 below is redesigned around a per-test baseline reset and a per-binary lock.
- **Q3:** add QR **display**. The human approved enabling matrix-sdk `qrcode` and iced `qr_code` (pins unchanged; only features and the resulting new lock packages). Q3 below uses the first and, for a technical reason found while re-planning, recommends **not** enabling the second; this needs the human's confirmation during validation.
- **Q4:** Element X settings, as recommended.
- **Q6:** **Skip for now** is allowed on Confirm-your-identity; skipping leaves the device unverified and a persistent banner keeps reminding the user (wording on the human checkpoint).
- **Q5, Q7-Q19:** the recommendations stand, adjusted below for consistency with Q2, Q3 and Q6.

### Facts verified while planning (2026-10-06)
Registry root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`. Each implementing task re-confirms the APIs it uses (AGENTS.md rule 2) and cites file:line in PROGRESS.md. No code was compiled for this plan except two lock probes (Q3), which ran in scratch copies of the workspace in the session scratchpad, not in the repo.

**Features (matrix-sdk 0.19.1, Cargo.toml `[features]`)**
- `e2e-encryption` is in `default` and is on for `twicecat-core` (`cargo tree -e features -i matrix-sdk` lists `default`, `e2e-encryption`, `automatic-room-key-forwarding`, `sqlite`, `bundled-sqlite`, `local-server`, `markdown`, `rustls-aws-lc-rs`).
- `qrcode` is **off**. It is `["e2e-encryption", "matrix-sdk-base/qrcode"]` (Cargo.toml:121-123) and pulls `matrix-sdk-qrcode` (not in Cargo.lock today).
- Lock probes (sorted name/version diff of Cargo.lock, before vs after):
  - matrix-sdk `qrcode` **alone** adds exactly 2 packages: matrix-sdk-qrcode 0.19.1 and qrcode 0.14.1. No existing version changes.
  - matrix-sdk `qrcode` **plus** iced `qr_code` adds 9: the two above plus qrcode 0.13.0 (a second version), lyon 1.0.19, lyon_algorithms 1.0.21, lyon_geom 1.0.19, lyon_path 1.0.19, lyon_tessellation 1.0.22, float_next_after 1.0.0 (iced_widget `qr_code = ["canvas", "dep:qrcode"]`, qrcode `0.13`, iced_widget 0.14.2 Cargo.toml:52-55/:84-86).

**QR verification (matrix-sdk `src/encryption/verification/`, matrix-sdk-crypto, matrix-sdk-qrcode)**
- With `qrcode` on, the SDK's default methods are `SasV1`, `QrCodeShowV1`, `ReciprocateV1` (matrix-sdk-crypto verification/requests.rs:60-65). `QrCodeScanV1` is **not** advertised, which matches "Twicecat shows, cannot scan". `accept()` and `request_verification()` use that list (requests.rs:236/:270/:546).
- `VerificationRequest::generate_qr_code() -> Result<Option<QrVerification>>` (matrix-sdk requests.rs:172). It returns `None` unless we advertise `QrCodeShowV1` and the other side advertises `QrCodeScanV1` (crypto requests.rs:1224-1228). Works in `Ready` and `Transitioned` (crypto requests.rs:983-995) and moves the request to `Transitioned`. For the own identity it builds `new_self` when we can sign devices, else `new_self_no_master` (a new, unverified device); for another user, `new_cross` (crypto requests.rs:1230-1290).
- `VerificationRequest::start_sas()` also works from `Transitioned` (crypto requests.rs:850-877), so "compare emoji instead" is possible while a QR code is shown.
- `QrVerification` (matrix-sdk verification/qrcode.rs): `to_qr_code() -> Result<QrCode, EncodingError>` :85, `to_bytes` :94, `confirm()` (confirm the other side scanned our code) :99, `cancel` :108, `has_been_scanned` :48, `changes()` :210, `state()` :218, `other_device` :70, `cancel_info` :59. Re-exports (verification/mod.rs:42-48): `QrVerificationState`, `QrVerificationData`, `EncodingError`; `Verification::QrV1` :61.
- `QrVerificationState::{Started, Scanned, Confirmed, Reciprocated, Done { .. }, Cancelled(CancelInfo)}` (crypto verification/qrcode.rs:97-130).
- matrix-sdk-qrcode builds the code as **one byte segment, no ECI, Version 7, EC level L** because "mobile clients seem to have trouble decoding the QR code that gets generated by `QrCode::new()`" with mixed segments (matrix-sdk-qrcode 0.19.1 utils.rs:60-73). So the module grid is 45 x 45. `QrVerificationData::from_bytes` types.rs:134 and `to_qr_code` :163 (for a unit test with spec bytes).
- qrcode 0.14.1: `QrCode::width()` lib.rs:173, `to_colors()` :218, `Color::select(dark, light)` types.rs:73 (lets the core read modules without naming the qrcode type).
- **iced `qr_code` cannot show that grid as built.** iced_widget 0.14.2 `qr_code::Data` has only `new(bytes)`, `with_error_correction(bytes, ..)` and `with_version(bytes, ..)` (qr_code.rs:242-283). All re-encode raw bytes with qrcode 0.13's automatic segmenting (`QrCode::new`/`with_error_correction_level`/`with_version`), which is the form the SDK avoids. There is no constructor from a module grid.
- iced `image` (already enabled through `image-without-codecs`) can show the grid: `image::Handle::from_rgba(width, height, pixels)` (iced_core 0.14.0 image.rs:147), `Handle::id` :161, `FilterMethod::Nearest` :237, `Image::filter_method` (iced_widget image.rs:124). The app already uses `Handle::from_rgba` (crates/app/src/avatars.rs:154).

**Recovery (matrix-sdk `src/encryption/recovery/`)**
- `Recovery` methods (mod.rs):
  - `state()` :144 and `state_stream()` :176 (emits the current value first);
  - `enable()` :235, `enable_backup()` :261, `disable()` :302;
  - `reset_key()` :359, `recover_and_reset(old)` :386, `reset_identity() -> Result<Option<IdentityResetHandle>>` :438;
  - `recover(key_or_passphrase)` :489, `recover_and_fix_backup` :528, `is_last_device()` :566.
- `disable()` (:302-341) deletes the backup, empties 4S **and writes the "backup disabled" account data** (`KeyBackupContent { enabled: false }`, `BackupDisabledContent { disabled: true }`), which stops every later automatic backup (`should_auto_enable_backups`, :601-609). Phase 4 never calls it (Q17), and the test reset (Q2) doesn't use it.
- `reset_identity()` (:438-466): `backups().disable_and_delete()` (deletes **every** backup version on the server in a loop, then disables locally; backups/mod.rs:276-298), writes `SecretStorageDisabledContent` (4S off), then `encryption().reset_cross_signing()`. Backups are re-created only when `should_auto_enable_backups` (auto setting on, none enabled, none on the server, not marked disabled), either directly or in `IdentityResetHandle::reset` after authentication (:791-799).
- `check_recovery_state` (mod.rs:681-691): `Disabled` when secret storage (4S) is off, even if a backup exists; `Enabled` when 4S is on and every known secret is local; `Incomplete` when 4S is on but secrets are missing (a new device before `recover()`).
- `IdentityResetHandle` has `auth_type` :785, `reset(Option<AuthData>)` :791 and `cancel` :802.
- types.rs: `RecoveryState::{Unknown, Enabled, Disabled, Incomplete}` :87, `RecoveryError::{BackupExistsOnServer, Sdk, SecretStorage}` :32, and `EnableProgress::{Starting, CreatingBackup, CreatingRecoveryKey, BackingUp(RoomKeyCounts), RoomKeyUploadError, Done { recovery_key }}` :51.
- futures.rs `Enable` :33 has `subscribe_to_progress` :53, `wait_for_backups_to_upload` :66 and `with_passphrase` :75. Its `IntoFuture` (:82) resolves to `Result<String>`, the recovery key. It fails with `BackupExistsOnServer` **only** when backups are not enabled locally and a backup exists on the server (:90-97); when this device has backups enabled, it goes straight to creating the recovery key.
- `Backups` (backups/mod.rs): `create` :97, `disable` :213, `disable_and_delete` :276, `are_enabled` :398, `fetch_exists_on_server` :412. `SecretStorage::is_enabled` (secret_storage/mod.rs:343).
- `recover()` → `SecretStore::import_secrets` (secret_storage/secret_store.rs):
  - imports the private cross-signing keys, then **signs the own device** (`own_device.verify()`, :437-452) and enables backups (:458);
  - logs `CrossSigningKeyExport` at `info` (:416), but that type's `Debug` prints only booleans (matrix-sdk-crypto store/types.rs:439-446).

**Encryption settings and initialization (matrix-sdk `src/encryption/mod.rs`, `src/client/builder/mod.rs`)**
- `EncryptionSettings { auto_enable_cross_signing :271, backup_download_strategy :277, auto_enable_backups :280 }` :265. `BackupDownloadStrategy::{OneShot, AfterDecryptionFailure, Manual (default)}` :286-307. Set with `ClientBuilder::with_encryption_settings` (builder :549). A raw `Client::builder()` without it gets the defaults: no auto cross-signing, no auto backup.
- `AfterDecryptionFailure` starts a `BackupDownloadTask` (mod.rs:241-246).
- The init task (`spawn_initialization_task` :2133-2167) runs on login **and on restore**, because `MatrixAuth::set_session` is shared (authentication/matrix/mod.rs:775-816). It:
  - bootstraps cross-signing when `auto_enable_cross_signing` is set, passing the login password as UIA (`AuthData::Password`, :801-813; `None` on restore and for OAuth);
  - calls `backups().setup_and_resume()` and `recovery().setup()`, which auto-creates a backup when `should_auto_enable_backups` holds (recovery/mod.rs:601-632).
- `bootstrap_cross_signing_if_needed` (mod.rs:1696-1711) bootstraps only when **the server knows no identity** for the user (after the initial key query). An existing identity is never replaced by a login.
- **Today's `build_client` (crates/core/src/client.rs:189-215) sets no encryption settings.** So no Twicecat login has ever bootstrapped cross-signing or created a backup.
- `ClientBuilder::sqlite_store(path, passphrase)` :301. `build_store_config` (:790-825) opens the state, event cache, media **and crypto** SQLite stores all with that passphrase. Twicecat passes its keychain-held 64-hex passphrase (client.rs:209, session.rs:86/:192).

**Cross-signing and identities (encryption/mod.rs, identities/)**
- `Encryption` methods:
  - `cross_signing_status()` :1121 (`CrossSigningStatus::is_complete`, matrix-sdk-crypto olm/signing/mod.rs:117);
  - `has_devices_to_verify_against()` :1132;
  - `verification_state() -> Subscriber<VerificationState { Unknown, Verified, Unverified }>` :1184/:315;
  - `get_verification(user, flow_id)` :1189, `get_verification_request(user, flow_id)` :1207;
  - `get_device` :1252, `get_own_device` :1269, `get_user_devices` :1301, `get_user_identity` :1349, `request_user_identity` :1397;
  - `devices_stream` :1438, `user_identities_stream` :1476;
  - `bootstrap_cross_signing(Option<AuthData>)` :1527, `reset_cross_signing() -> Option<CrossSigningResetHandle>` :1593, `bootstrap_cross_signing_if_needed` :1696;
  - `secret_storage()` :1931, `backups()` :1936, `recovery()` :1941, `wait_for_e2ee_initialization_tasks()` :2176.
- `CrossSigningResetHandle::auth(Option<AuthData>)` :370. For OAuth it polls every 500 ms for at most 2 minutes. `CrossSigningResetAuthType::{Uiaa(UiaaInfo), OAuth(OAuthCrossSigningResetInfo { approval_url, session })}` :433/:458.
- `Device` (identities/devices.rs) derefs to `DeviceData` (:102). It has:
  - `request_verification` :142, `request_verification_with_methods` :191;
  - `verify` :291, `is_verified` :421, `is_cross_signed_by_owner` :553.
  - From `DeviceData` (matrix-sdk-crypto identities/device.rs): `device_id` :655, `display_name` :660, `is_deleted` :784, `first_time_seen_ts` :1028.
- `UserIdentity` (users.rs):
  - `request_verification` :187 (and `request_verification_with_methods` :244): for the **own identity** it sends to-device requests to all own devices; for **another user** it uses (or creates) a DM;
  - `verify` :358, `is_verified` :407, `was_previously_verified` :415, `withdraw_verification` :424, `has_verification_violation` :429, `pin` :447.
- **No client-server API deletes a cross-signing identity.** Once an account has one, every later login sees it. So with fixed accounts the "brand-new account" state (no identity) exists only on a fresh Synapse volume (CI: `docker compose down -v` after each run; locally the `synapse-data` volume persists).

**Verification (encryption/verification/)**
- mod.rs:
  - re-exports `Emoji { symbol: &'static str, description: &'static str }` (matrix-sdk-crypto verification/mod.rs:76), `CancelInfo` and `SasState`;
  - `Verification::{SasV1, QrV1 (cfg qrcode)}` :55 is `#[non_exhaustive]`.
- requests.rs:
  - `VerificationRequestState::{Created, Requested { their_methods, other_device_data }, Ready, Transitioned { verification }, Done, Cancelled(CancelInfo)}` :35;
  - methods: `is_self_verification` :125, `their_supported_methods` :133, `accept` :148, `accept_with_methods` :162, `generate_qr_code` :172, `start_sas` :199, `cancel` :207, `changes()` :250, `state()` :260.
- sas.rs:
  - `accept` :32, `confirm` :81, `mismatch` :97, `cancel` :106;
  - `emoji() -> Option<[Emoji; 7]>` :155, `decimals` :160, `supports_emoji` :166;
  - `other_device` :197, `changes()` :320, `state()` :328.
  - `SasState::{Created, Started, Accepted, KeysExchanged { emojis, decimals }, Confirmed, Done { .. }, Cancelled(CancelInfo)}` (matrix-sdk-crypto verification/sas/mod.rs:118).
- Incoming requests:
  - **to-device** requests arrive as ruma `ToDeviceKeyVerificationRequestEventContent { from_device, transaction_id, methods }` (ruma-events key/verification/request.rs:15-25);
  - **in-room** requests arrive as `MessageType::VerificationRequest` (ruma-events room/message.rs:426).
- Handlers are registered with `Client::add_event_handler` (client/mod.rs:1073) and kept with `event_handler_drop_guard` :1324. Sliding sync runs them for to-device events too: `sliding_sync/client.rs:152/:261` calls `call_sync_response_handlers` (sync.rs:178-188), which includes `handle_sync_to_device_events`.

**Devices, UIA and OAuth**
- `Client` methods:
  - `logout()` :1652;
  - `devices()` :2972 (`get_devices` gives `device_id`, `display_name`, `last_seen_ip`, `last_seen_ts`);
  - `delete_devices(&[OwnedDeviceId], Option<uiaa::AuthData>)` :3030 and `rename_device(&DeviceId, &str)` :3050.
- UIA types:
  - ruma-client-api uiaa/auth_data.rs: `Password::new(UserIdentifier, String)` :219 with `session` :214, and `UserIdentifier::Matrix(..)` :438;
  - `matrix_sdk::Error::as_uiaa_response` (error.rs:135), also on `HttpError` (:443).
- OAuth: `OAuth::server_metadata()` (authentication/oauth/mod.rs:511), then `AuthorizationServerMetadata::account_management_url_with_action(AccountManagementActionData::DeviceDelete { .. })` (ruma-client-api discovery/get_authorization_server_metadata.rs:282/:530). The local Synapse has no OAuth server, so this path can't be tested locally.

**Rooms, timeline, UTD, shields**
- `Room::encryption_state()` :1115 and `latest_encryption_state()` :1124 return `EncryptionState::{Encrypted, NotEncrypted, Unknown}` (matrix-sdk-base room/encryption.rs:35, with `is_encrypted` :55). `enable_encryption` is at :2354.
- **`Client::create_dm` already creates encrypted DMs** with `e2e-encryption` on (client/mod.rs:1965-1972). The test helper `create_dm` therefore makes encrypted rooms today.
- `MsgLikeKind::UnableToDecrypt(EncryptedMessage)` (matrix-sdk-ui timeline/event_item/content/msg_like.rs:41). `EncryptedMessage::MegolmV1AesSha2 { cause: UtdCause, session_id, .. }` content/mod.rs:548-573.
- `UtdCause` (matrix-sdk-crypto types/events/utd_cause.rs:24) has 9 variants:
  - `Unknown`, `SentBeforeWeJoined`, `VerificationViolation`, `UnsignedDevice`, `UnknownDevice`;
  - `HistoricalMessageAndBackupIsDisabled`, `WithheldForUnverifiedOrInsecureDevice`, `WithheldBySender`, `HistoricalMessageAndDeviceIsUnverified`.
- `EventTimelineItem::encryption_info()` event_item/mod.rs:399. `get_shield(strict)` :408 gives `None` for unencrypted rooms, local echoes and UTDs. `TimelineEventShieldState::{Red { code }, Grey { code }, None}` :884, with 7 codes :918 (`AuthenticityNotGuaranteed`, `UnknownDevice`, `UnsignedDevice`, `UnverifiedIdentity`, `VerificationViolation`, `MismatchedSender`, `SentInClear`).
- Non-strict shields (`to_shield_state_lax`, matrix-sdk-common 0.19.1 deserialized_responses.rs:139-185): a verified sender or a merely unverified identity gives **no** shield; a device not signed by its owner gives `Red(UnsignedDevice)`; a key from an insecure source (for example the key backup) gives `Grey(AuthenticityNotGuaranteed)`; a missing device gives `Red(UnknownDevice)`. The tests below assert these exact values.
- Redecryption when keys arrive is the SDK's job (event cache redecryptor, matrix-sdk event_cache/redecryptor.rs). It reaches the app as a `Set` diff on the same `unique_id`, which Phase 3's forwarder already applies.

**iced 0.14 (for the UI tasks)**
- `text_input(..).secure(true)` (iced_widget 0.14.2 text_input.rs:163) is already used by the login screen (crates/app/src/screens/login.rs:220).
- `iced::clipboard::write` (iced_runtime 0.14.0 clipboard.rs:49) and `read` :29.
- `stack`, `opaque` and `center` (helpers.rs:561/:577/:259) for the modal dialog layer. They follow the Phase 3 "fixed widget-tree shape" rule (PROGRESS, composer focus fix).
- `image(handle)` (helpers.rs:1812) with `Handle::from_rgba` and `FilterMethod::Nearest` for the QR code (see above).

**Test accounts and runner (for Q2)**
- `.env.test` defines exactly three accounts: alice (`TEST_USER_1`), bob (`TEST_USER_2`), carol (`TEST_USER_3`). `scripts/create-test-users.sh` registers them idempotently and prunes their rooms.
- bob: `login::restore_without_logging_in_again` counts bob's devices before and after within that one test (login.rs:97-101; timeline.rs:3-5 explains why other timeline tests use carol). `room_list::unread_count_reaches_two` also logs bob in.
- `raw_login` (tests/common/mod.rs:151) never logs out, so every run adds devices to the account it uses.
- Cargo runs test binaries **one at a time**; tests inside one binary run in parallel. A per-binary `static` `tokio::sync::Mutex` already serializes DM creation (`DM_LOCK`, tests/common/mod.rs:199; `Mutex::const_new`, tokio 1.53.1 sync/mutex.rs:395). Tokio's mutex does not poison when a holder panics.
- The app's `tests/bridge.rs` logs in alice only (bridge.rs:158/:271) and can't use matrix-sdk (`check-app-deps.sh` covers dev-dependencies).
- CI's `integration` job: `docker compose up -d --wait`, `create-test-users.sh`, the ignored run, then `docker compose down -v` (fresh accounts every CI run); `timeout-minutes: 20`.

**Current repo state relevant here**
- `PlaceholderKind::UnableToDecrypt` (core timeline.rs:168) renders "Unable to decrypt this message" (timeline_pane.rs:643). No reason is given.
- `RoomSummary` has no encryption flag.
- `CoreEvent`/`Command` have no security variants.
- Tokens and the store passphrase live only in the keychain `SessionRecord` (session.rs:76-110, redacted `Debug`). The `last_account` file (paths.rs:108) holds the user ID in clear.
- Logging: `DEFAULT_FILTER = "warn,twicecat_core=info"` (logging.rs:11).
- Engine: `handle` processes commands one at a time (engine.rs:821). Network work belongs in spawned tasks (Phase 3 Q9).

**No rule-8 blocker.** Every API the roadmap names exists in the pinned versions: `recovery().state()`, `recover()`, interactive SAS, QR display (with the `qrcode` feature), cross-signing bootstrap, device listing, rename and delete.

### Decisions (RECOMMENDED unless marked answered; awaiting the human's validation through the controller)
1. **Q1: precondition (rule 1). ANSWERED.** Phase 3 is signed off (human confirmed 2026-10-06). The Phase 3 checkpoint has every item ticked except IME, which the human deferred on 2026-10-06; PR #4 and PR #6 are merged to `main`. Task 1 adds to PROGRESS.md the line "Phase 3 signed off by the human on 2026-10-06 (merge of PR #4, #6); IME deferred, tracked under Phase 3 open issues" and leaves the IME item and the Phase 3 open issues as they are.
2. **Q2: test accounts for E2EE tests (rule 6). ANSWERED: fixed `.env.test` accounts only; the design below follows from that.**
   - **The problem.** Encryption state is per account and lives on the server (cross-signing identity, backup versions, secret storage). An identity can never be deleted. Reused naively, "set up recovery" fails with `BackupExistsOnServer` from the second run on, parallel tests race each other's resets, and leftover devices pile up.
   - **Accounts.** alice is user A (own-account flows, several devices = several cores with their own data directories). carol is user B (the other side of DMs and user verification). bob is not used by any Phase 4 test, so the login device-count test and the room-list test are unaffected. Two accounts cover every planned test, so **no new account and no change to `.env.test` or `create-test-users.sh`**. The app's headless E2EE test uses carol, because the existing bridge tests use alice.
   - **Baseline reset, `reset_e2ee_account(n)`** in `tests/common/mod.rs`, run at the start of every E2EE test for every account the test uses. It uses a raw SDK client built **without** encryption settings (so nothing runs automatically):
     1. `assert_local_homeserver`, then log in as device "twicecat-e2ee-reset" and `wait_for_e2ee_initialization_tasks()`;
     2. sign out every other device of the account: `delete_devices(others, None)`, then once more with `uiaa::Password` and the UIA session from `as_uiaa_response`. This removes the devices left by `raw_login`, earlier cores and earlier failed runs;
     3. `recovery().reset_identity()` and, on `Uiaa`, `reset(Some(password))`. This deletes every backup version on the server, turns 4S off and replaces the cross-signing identity. With auto backups off on this client, no new backup is created;
     4. assert the postconditions on the same client: `backups().fetch_exists_on_server() == false`, `secret_storage().is_enabled() == false`, exactly one device, an own identity exists;
     5. `logout()`, so the reset device is gone.

     **Baseline B0:** a fresh identity whose private keys nobody holds, no backup, 4S off, no devices. A Twicecat core that logs in on B0 is an unverified device (`cross_signing: MissingKeys`, `device: Unverified`, `recovery: Disabled`); with Q4 settings it auto-creates a backup of its own.
   - **Seeded recovery, `seed_recovery(n, passphrase: Option<&str>) -> Zeroizing<String>`:** steps 1-4, then, on the same raw client (which now holds the new identity's private keys), `recovery().enable()` (`with_passphrase` when given) and `logout()`. **Baseline B1** (the returning user): identity, 4S on with the returned key, a matching backup, no devices. A core on B1 is `Unverified`/`Incomplete` until `Recover { key }`.
   - **First device, `first_device(core, n)`** (from Task 4): log a core in on B0 and send `ResetIdentity { password: Some(..) }`, then wait for `Security { cross_signing: Complete, device: Verified, recovery: Disabled }`. The reset deletes every backup and, with Q4 settings, the SDK creates a fresh one enabled on this device. This is the closest reachable state to a brand-new account (identities can't be deleted), and it is exactly what the user's own "Reset your identity" does.
   - **No accidental `BackupExistsOnServer`.** Recovery is set up only on a `first_device` core, whose backup is enabled locally, so `enable()` skips the server check (futures.rs:90-97). One test hits the error **on purpose** (a core on B1 sends `EnableRecovery`) to prove the Q5 error mapping.
   - **No races.** Every E2EE test takes `let _serial = e2ee_serial().await;` as its first statement: a `static E2EE_LOCK: tokio::sync::Mutex<()> = Mutex::const_new(())` guard held for the whole test (the `DM_LOCK` pattern). Cargo runs binaries one at a time, so one lock per binary is enough. A failed test doesn't block later ones (no poisoning), and its cores die with its runtime. `log_hygiene.rs` holds one test and takes the lock too.
   - **Repeat runs.** Every core E2EE task's acceptance includes running its E2EE test binary **twice in a row** without `create-test-users.sh` in between, both passing.
   - **Side effect, on the human checkpoint.** The ignored suite signs out every alice and carol session on the local Synapse and resets their identities. Local human checks with Element Web should use bob (or the human's own account). CI is unaffected (fresh volume).
   - **The never-had-an-identity case** (first-time upload, `None` auth on a restored pre-Phase-4 session) can't be produced on demand with fixed accounts. Task 4 records Synapse v1.151.0's first-upload rule from the image's installed `synapse/rest/client/keys.py` (read through `docker compose exec synapse`), and notes that CI's fresh volume runs a first-time bootstrap (with the login password) on every run.
   - Rejected:
     - fresh throwaway accounts (declined by the human, 2026-10-06);
     - `recovery().disable()` as the reset (it marks backups disabled, which stops every later automatic backup);
     - serializing the whole suite with `--test-threads=1` (slows the unrelated tests; the gate command stays as it is);
     - a Synapse admin-API reset (no endpoint deletes a cross-signing identity, and the raw-client reset is enough);
     - using bob as a third E2EE account (not needed; it would couple the E2EE resets to bob's device-count test).
3. **Q3: QR display and dependencies (rule 3). ANSWERED: add QR display; matrix-sdk `qrcode` and iced `qr_code` approved (pins unchanged).**
   - **Scope:** Twicecat **shows** a QR code for the other device to scan. It never scans (no camera). When the other device doesn't advertise `QrCodeScanV1` (for example another Twicecat), the flow is SAS as before.
   - **matrix-sdk `qrcode`: enabled** in Task 7 (`crates/core/Cargo.toml`, `=0.19.1` unchanged). Lock: exactly matrix-sdk-qrcode 0.19.1 and qrcode 0.14.1 (measured). The SDK's default method list then becomes SAS + show QR + reciprocate (no scan), which is what Twicecat wants, so no method list is hand-built.
   - **iced `qr_code`: RECOMMENDED not to enable it (needs the human's confirmation).** Its `Data` only takes raw bytes and re-encodes them with qrcode 0.13's automatic segmenting, the form the SDK says mobile scanners have trouble with; it can't take the SDK's ready-made 45 x 45 grid. It would also add 7 more packages (a second qrcode version, lyon and canvas). Instead, the core sends the module grid (`QrImage`), and the app draws it with the already-enabled `image` widget: black modules on white with a 4-module quiet zone, scaled with nearest-neighbour filtering. If the human prefers the iced widget anyway, it would show a different, re-encoded code, and scanning by Element X would have to be proven on the checkpoint before relying on it.
   - **What crosses the API:** only the grid (`QrImage { size, dark }`), never the payload bytes. The grid encodes the flow's one-time shared secret, so its `Debug` prints the size only and it is never logged.
   - **Testing:** a unit test builds a code from spec bytes (`QrVerificationData::from_bytes` → `to_qr_code`) and checks the grid; an integration test proves the code is shown, and that "compare emoji instead" and cancel work, against a raw SDK client advertising `QrCodeScanV1`. The scan itself (camera) and the reciprocate/confirm round trip are proven on the human checkpoint with Element X (rule 9).
   - Rejected: a test-only QR decoder to automate the reciprocate step (a sizeable piece of test code with its own bugs, for a step that starts with a camera anyway); exposing the payload bytes for tests (puts the shared secret in the API).
4. **Q4: encryption settings. ANSWERED (as recommended).** `build_client` adds `.with_encryption_settings(EncryptionSettings { auto_enable_cross_signing: true, auto_enable_backups: true, backup_download_strategy: AfterDecryptionFailure })`. This is the combination Element X uses: cross-signing on every login, a backup that starts collecting keys at once, and per-event key download for UTDs.
   - Effect on existing accounts:
     - on its next Twicecat login, an account with no identity gets one, using the typed password;
     - an account that already has an identity gets an **unverified** new device, which the Q6 flow handles;
     - alice, bob and carol each get an identity and a backup the first time a core logs them in. bob's device-count test is unaffected; alice's and carol's state is reset by the E2EE tests (Q2).
   - Restored pre-Phase-4 sessions bootstrap with `None` auth. If Synapse demands UIA, the core reports `CrossSigning::NotSetUp` and the app asks for the password (Q13). Task 4 records what Synapse v1.151.0 does (Q2).
   - Rejected:
     - `OneShot` (downloads the whole backup on every recovery);
     - `Manual` (history would never decrypt without extra commands);
     - no auto backup (a new user who never opens the recovery screen would lose every key).
5. **Q5: recovery UX.** RECOMMENDED (stands):
   - **Set up:** a recovery key only, no passphrase, as in Element X today. `enable().wait_for_backups_to_upload()`, with progress (`RecoveryProgress`).
   - **Showing the key:** it is shown **once** in a monospace box, in the SDK's grouped form, with **Copy** and the warning "Store this key somewhere safe, like a password manager. Without it you can't read your encrypted messages on a new device if you're signed out everywhere." The screen can only be left by ticking "I've saved my recovery key" and pressing **Done**.
   - **Clearing the key:** the app drops it on **Done**, on logout and on window close. A copy is cleared from the clipboard after 60 s, but only if the clipboard still holds exactly the key (`clipboard::read`, then `write("")`).
   - **Restore:** one secure field that accepts a recovery key **or** a passphrase (accounts set up in Element Web may use one). It goes through `recover()` (Q15).
   - **Change key:** `reset_key()` from the security screen, shown once like set-up.
   - **"A backup already exists":** when `EnableRecovery` fails with `BackupExistsOnServer` (this device isn't connected to the existing backup), the app switches to the restore field with that message.
   - The final wording is a human checkpoint item.
   - Rejected:
     - showing the key again later (the SDK does not keep it, and we never store it);
     - a passphrase on set-up (a weaker secret, and Element X has dropped it).
6. **Q6: confirming a new device. ANSWERED: Skip allowed.** When a fresh login lands on an account that already has a cross-signing identity and `verification_state` is `Unverified`, the app shows a **Confirm your identity** screen before the main view, offering:
   - **Use another device** (self-verification request, Q9; the other device may scan Twicecat's QR code or compare emoji), shown when `has_devices_to_verify_against`;
   - **Use recovery key** (Q5 restore);
   - **Can't confirm? Reset your identity** (Q14);
   - **Skip for now**.

   **Skip** closes the screen and leaves the device unverified. A **persistent banner** then stays at the top of the main view for as long as `device` is `Unverified`: "This device isn't verified. Verify it to read your encrypted history and so others can trust it." with a **Verify** button that reopens the Confirm-your-identity screen. This banner cannot be dismissed; it reappears after a restart (a restored session shows the banner, not the screen); it goes away only when the device becomes verified. The final wording is on the human checkpoint. Recovery banners (Disabled, Incomplete) can be dismissed for the session.

   Rejected: confirming automatically from a stored key (we never store the recovery key); a dismissible unverified banner (the human asked for a persistent reminder).
7. **Q7: additive core API (Phase 2 Q6 convention).** RECOMMENDED (stands, plus QR): no existing field or variant is removed or renamed. New types go in a new file, `crates/core/src/api/security.rs`, re-exported from `api/mod.rs` (`check-core-api.sh` already scans `api/`).
   - **Types:**
     - `RecoveryKey`: a `Zeroizing<String>` newtype. `Debug` prints `RecoveryKey(<redacted>)`, `Clone` zeroizes each copy, and `expose()` returns the text.
     - `RecoveryStatus { Unknown, Enabled, Disabled, Incomplete }`;
     - `RecoveryProgress { Starting, CreatingBackup, CreatingKey, BackingUp { done: u64, total: u64 }, UploadFailed }`;
     - `DeviceVerification { Unknown, Verified, Unverified }`;
     - `CrossSigning { Unknown, NotSetUp, Complete, MissingKeys }`;
     - `SecurityStatus { recovery, device, cross_signing, has_devices_to_verify_against: bool, identity_exists: bool }`;
     - `VerificationId(String)` (the flow ID; not secret);
     - `VerificationPeer { user_id, device_id: Option<DeviceId>, device_name: Option<String>, own_account: bool }`;
     - `SasEmoji { symbol: String, description: String }`;
     - `QrImage { size: u16, dark: Vec<bool> }`: row-major modules without the quiet zone (`dark.len() == size * size`). Manual `Debug` prints `QrImage { size, .. }` only;
     - `VerificationPhase { Requested { incoming }, Ready, Waiting, ShowQr(QrImage), QrScanned, Emojis([SasEmoji; 7]), Decimals([u16; 3]), Confirmed, Done, Cancelled { reason: CancelReason } }`;
     - `CancelReason { ByUs, ByThem, Mismatch, Timeout, AcceptedElsewhere, Other }`;
     - `DeviceSummary { device_id, display_name: Option<String>, last_seen_ms: Option<u64>, current: bool, trust: DeviceTrust }` with `DeviceTrust { Verified, Unverified, NoKeys }`;
     - `UserTrust { Verified, Unverified, NoIdentity, IdentityChanged }`;
     - `Shield { Red(ShieldCode), Grey(ShieldCode) }` with the 7 codes;
     - `UtdReason` (the 9 causes, plus `Unknown` for non-Megolm);
     - `ReauthAction { DeleteDevices, ResetIdentity, BootstrapCrossSigning }`.
   - **Changes to existing types:**
     - `RoomSummary.is_encrypted: bool` (`RoomSummary::new` sets `false`);
     - `MessageDetails.shield: Option<Shield>` (`Default` gives `None`);
     - `TimelineItemKind::Placeholder` gains `utd: Option<UtdReason>`. This is the one semantic adjustment: struct-literal sites add `utd: None`, and `..` patterns are unaffected;
     - `CoreError` gains `ReauthFailed` and `Busy`;
     - `ActionKind` gains `Recovery`, `CrossSigning`, `Verification` and `Devices`.
   - **Commands:**
     - recovery: `EnableRecovery`, `Recover { key: RecoveryKey }`, `ChangeRecoveryKey`;
     - identity: `BootstrapCrossSigning { password: Option<Password> }`, `ResetIdentity { password: Option<Password> }`, `CancelIdentityReset`;
     - starting verification: `RequestSelfVerification`, `RequestDeviceVerification { device_id }`, `RequestUserVerification { user_id }`;
     - during verification: `AcceptVerification { id }`, `StartSas { id }` ("compare emoji instead" while a QR code shows), `ConfirmQrScanned { id }`, `ConfirmSas { id }`, `SasMismatch { id }`, `CancelVerification { id }`;
     - devices: `ListDevices`, `RenameDevice { device_id, name: String }`, `SignOutDevices { device_ids: Vec<DeviceId>, password: Option<Password> }`;
     - trust: `WatchUserTrust { user_id }` (any user; the selected DM's peer is watched automatically, Q10).
   - **Events:**
     - recovery and status: `Security(SecurityStatus)` (sent once after login/restore and on every change), `RecoveryProgress(RecoveryProgress)`, `RecoveryKeyReady(RecoveryKey)`, `Recovered`;
     - re-authentication: `ReauthRequired { action: ReauthAction }`, `OpenAccountManagement(AuthUrl)` (OAuth device deletion and identity-reset approval);
     - verification: `VerificationRequested { id, peer }`, `Verification { id, phase }`;
     - devices and trust: `Devices(Vec<DeviceSummary>)`, `UserTrust { user_id, trust }`.
   - **`Debug`:** every new command and event has a redacted or derived `Debug`. `RecoveryKey`, `Password` and `QrImage` modules are never printed, and device names (user-chosen) are printed like room names (Phase 3 policy).
   - Rejected:
     - SDK types or flow objects across the API;
     - QR payload bytes across the API (Q3);
     - `#[non_exhaustive]` (changes the approved contract);
     - putting the recovery key inside `SecurityStatus`.
8. **Q8: engine structure and concurrency.** RECOMMENDED (stands):
   - **New private modules:** E2EE logic lives in `crates/core/src/security.rs` (status, recovery, cross-signing, devices, trust) and `crates/core/src/verification.rs` (SAS and QR flows). This keeps `engine.rs` (2,096 lines) a dispatcher.
   - **`Active` gains:**
     - `security: SecurityTasks`, which holds a `JoinSet` for network actions;
     - a status forwarder (it merges `recovery().state_stream()`, `verification_state()`, `devices_stream()` and `user_identities_stream()`, recomputes `SecurityStatus`, and emits it only when it changed);
     - the verification handlers' `EventHandlerDropGuard`s.

     All of these end in `Active::stop_tasks` before the client is dropped, so logout still deletes the store safely.
   - **One verification at a time.** A second incoming request while one is active is cancelled through the SDK (`cancel()`, which the other side sees as cancelled by the user). Outgoing requests while one is active get `ActionFailed { error: Busy }`.
   - `handle` never awaits network work. Every new command runs in the security `JoinSet` and reports failures as `ActionFailed` with a fixed message. The one exception is `Recover`, whose success is also reported as `Recovered`.
   - Rejected: awaiting inside `handle`, and one task per verification event.
9. **Q9: verification protocol.** RECOMMENDED (adjusted for QR):
   - **Methods:** the SDK defaults with `qrcode` on: `m.sas.v1`, `m.qr_code.show.v1`, `m.reciprocate.v1` (never `m.qr_code.scan.v1`). Emoji are shown when both sides support them (`supports_emoji`), and the decimals otherwise.
   - **Incoming:**
     - a to-device request (own devices) and an in-room request (from another user in a DM) both become `VerificationRequested`;
     - the user accepts (`accept()`, which sends ready) or declines (`cancel()`);
     - once `Ready`, the core calls `generate_qr_code()`. `Some` gives `ShowQr(grid)`; `None` (the peer can't scan) leaves the phase at `Waiting`;
     - when the other side starts SAS (also while a QR code shows), the core accepts it (`SasVerification::accept()`) and forwards `KeysExchanged` as `Emojis` or `Decimals`.
   - **Outgoing:**
     - `RequestSelfVerification` uses the own identity's `request_verification()`, sent to every own device;
     - `RequestDeviceVerification` uses `Device::request_verification()`;
     - `RequestUserVerification` uses `UserIdentity::request_verification()` (in a DM);
     - once the request is `Ready`, the core tries `generate_qr_code()` first: `Some` gives `ShowQr`, `None` makes the core call `start_sas()` (the Phase 4 SAS behaviour, unchanged for Twicecat-to-Twicecat).
   - **QR display:** the dialog offers **Compare emoji instead** (`StartSas`, `start_sas()` from `Transitioned`) and **Cancel**. When the peer scans the code, `QrVerificationState::Scanned` becomes `QrScanned`; the user confirms that the other device shows success (`ConfirmQrScanned` → `QrVerification::confirm()`) or cancels. `Confirmed`, `Done` and `Cancelled` map as for SAS; `Reciprocated` (only reached by a scanning side) maps to `Waiting`.
   - **Finishing SAS:** "They match" calls `confirm()`, "They don't match" calls `mismatch()`, and the end is `Done` or `Cancelled { reason }`. The SDK's own timeouts apply, and `CancelInfo`'s code maps to `CancelReason`.
   - **After a self-verification:** the SDK gossips the cross-signing keys and the backup key. With `AfterDecryptionFailure`, history UTDs then resolve. That path is proven in Task 6.
   - Rejected: auto-accepting incoming requests; showing the emoji description in any language but English (i18n is out of scope); hand-built method lists (the defaults already exclude scanning).
10. **Q10: trust and badges.** RECOMMENDED (stands):
    - **Message shield:** `get_shield(false)` (Element X's non-strict mode). `Red` shows a red shield icon and `Grey` a grey one next to the time, each with a tooltip per code. Wording follows Element X: "Encrypted by a device not verified by its owner", "Encrypted by an unknown or deleted device", "The authenticity of this encrypted message can't be guaranteed on this device", "Sent in clear in an encrypted room", "Sender's identity changed" (red).
    - **Room:** a lock mark in the room header and room list row when `is_encrypted`.
    - **DM peer:** on `SelectRoom` of a DM whose `Room::direct_targets()` (matrix-sdk-base room/mod.rs:335) names exactly one other user, the core watches that user and emits `UserTrust` (a DM with several targets gets no badge). The header shows **Verified** (green) or **Not verified** with a **Verify** button (Q9 user verification). It shows **Identity changed** (red) when `has_verification_violation`. Updates come from `user_identities_stream`.
    - **Devices:** a badge in the Sessions screen (Q12).
    - Rejected:
      - strict shields (red for every unverified sender floods the timeline);
      - identity-change acknowledgement (`pin`/`withdraw_verification`) UI (Q17).
11. **Q11: UTD placeholders.** RECOMMENDED (stands): each `UtdReason` has one muted line, worded as in Element X:
    - `SentBeforeWeJoined`: "You don't have access to this message";
    - `HistoricalMessageAndDeviceIsUnverified`: "You need to verify this device for access to historical messages";
    - `HistoricalMessageAndBackupIsDisabled`: "Historical messages are not available on this device";
    - `WithheldForUnverifiedOrInsecureDevice`: "Your device doesn't meet the sender's security requirements";
    - `WithheldBySender`, `UnknownDevice` and `UnsignedDevice`: "The sender didn't share the key for this message";
    - `VerificationViolation`: "Sender's verified identity was reset";
    - otherwise: "Waiting for this message" (the key may still arrive).

    The row keeps its item ID, so the existing `Set` diff replaces it in place once decrypted, and the Phase 3 sensor re-measures the height. A unit test drives exactly that `Set` through `update`.

    Rejected: a "retry decryption" button (the SDK retries on its own when keys arrive, and `AfterDecryptionFailure` fetches from the backup).
12. **Q12: session management.** RECOMMENDED (stands):
    - **Listing:** `ListDevices` joins `client.devices()` (names, last seen) with `get_user_devices(own)` (trust). The current device comes first, then the rest by `last_seen_ms` descending. `last_seen_ip` is **not** exposed (personal data, and not needed).
    - **Actions:**
      - **Rename** calls `rename_device`;
      - **Sign out** (others, one or a selection) calls `delete_devices(ids, None)` first. A UIA answer gives `ReauthRequired { DeleteDevices }`, and the app asks for the password and resends with it (Q13). On an OAuth session the core emits `OpenAccountManagement(url)` for `DeviceDelete` instead, and the app opens it through the Phase 2 browser path (`check_browser_url`);
      - **Verify** (another own device that is unverified) sends `RequestDeviceVerification`.
    - **Signing out this device** is the existing Logout. When recovery is not `Enabled` and this is the only device, the app first warns "You'll lose access to your encrypted messages", with **Set up recovery** and **Sign out anyway**.
    - Device names come from the server and are shown through `one_line_capped`.
    - Rejected: showing IPs, and signing out the current device from the list (Logout already does it).
13. **Q13: re-authentication.** RECOMMENDED (stands): the core never stores the password.
    - `SignOutDevices`, `ResetIdentity` and `BootstrapCrossSigning` take `password: Option<Password>`. Without one, a UIA answer gives `ReauthRequired { action }`.
    - With one, the core builds `uiaa::Password::new(UserIdentifier::Matrix(own user), password)` with the UIA session from the first answer and retries once.
    - A rejected password gives `ActionFailed { error: ReauthFailed }`. The app re-prompts and keeps the field secure.
    - The password lives in a `Password` (zeroized) and is dropped after the call.
    - Rejected: keeping the login password in memory for later UIA.
14. **Q14: identity reset.** RECOMMENDED (stands): offered only from the Confirm-identity screen and the security screen, behind a red confirmation ("Your verified contacts will see a warning; messages you can't decrypt now stay unreadable"). `ResetIdentity` calls `recovery().reset_identity()`:
    - `Uiaa`: follows Q13;
    - `OAuth`: emits `OpenAccountManagement(approval_url)` and calls `handle.reset(None)`, which polls for 2 minutes. `CancelIdentityReset` calls `cancel()`;
    - success: the device becomes verified, every old backup is gone and the SDK creates a new one (Q4 settings), recovery is `Disabled`, and the app goes on to Q5 set-up.

    Rejected: hiding reset entirely (the only way out for a user with no key and no other device).
15. **Q15: secret hygiene in flows.** RECOMMENDED (stands, plus QR):
    - **Never logged or stored:** the recovery key (new or typed), the passphrase and the UIA password never reach `tracing`, a file or a `Debug` string. The core never writes the recovery key anywhere, and the SDK keeps it only inside 4S, encrypted. The QR grid is never logged (`Debug` prints its size only).
    - **Typed key:** whitespace is trimmed, and the typed string is passed to `recover()` as is. The SDK accepts both the key (with spaces) and a passphrase; Task 5 tests both.
    - **App:** the `RecoveryKey` sits in the setup state only while the "show once" screen is open. The secure input's `String` is zeroized on submit and screen exit (via `Password`/`RecoveryKey` wrappers). The QR image handle is dropped when the flow ends.
    - **Residuals,** recorded in PROGRESS: iced's text and paragraph buffers, the clipboard history of OS clipboard managers, and the QR code on screen (anyone who can see the screen during the flow sees the one-time code, as in every client).
    - Rejected: a "show key again" feature.
16. **Q16: security review (roadmap step 7).** RECOMMENDED (stands, accounts per Q2): three automated checks plus a written review.
    - **(a) Log capture.**
      - `crates/core/tests/log_hygiene.rs` is its own test binary with a single test. It installs a global `tracing_subscriber::fmt` subscriber writing to an in-memory buffer, with filter `info` (everything at info, stricter than `DEFAULT_FILTER`), then runs a full flow with alice and carol after `reset_e2ee_account`:
        - password login, sync, an encrypted DM with sentinel bodies (`secret-body-<hex>`) both ways;
        - identity reset on alice's first core (`first_device`), recovery enable (capturing the key), a second alice core that recovers with the key;
        - a SAS verification between the two alice devices;
        - device rename, then logout.
      - It then reads the stored `SessionRecord` JSON from the `InMemorySecretStore` (access token, refresh token if any, store passphrase) before logout, and the test passwords, recovery key and bodies.
      - It asserts that **none** of them appears in the captured logs. A second pass repeats the check with `DEFAULT_FILTER`.
      - SDK `debug`/`trace` stay documented as "local debugging only" (logging.rs). They are not asserted.
    - **(b) Data at rest.** After the same flow, before logout, every file under the test's data root (SQLite main, `-wal` and `-shm` files included) is read as bytes and searched for the sentinel bodies, the passphrase, the tokens and the recovery key. None may appear. This is the "database encrypted at rest" proof. The SDK encrypts values with the passphrase-derived store cipher and hashes keys; table layout and sizes stay visible, a residual risk.
    - **(c) Keychain-only audit,** a unit test in the core: `SessionRecord`'s fields are the only place tokens and the passphrase exist. `paths.rs` writes only `last_account` (the user ID) and the store directory, checked by listing the data root after login.
    - **(d) Written review** in PROGRESS.md: what is stored where, what is encrypted with what, the residuals (user ID in `last_account`, SQLite metadata, editor/text buffers not zeroized, clipboard, SDK debug logs, the iced-rendered recovery key, the on-screen QR code), and the open Phase 3 security items carried over.
    - Rejected: asserting at SDK `debug` (the SDK logs request details at debug by design), and a SQLCipher-style whole-file encryption claim (not what matrix-sdk-sqlite does).
17. **Q17: scope cuts.** RECOMMENDED, out of Phase 4 (QR display removed from this list per Q3):
    - **scanning** QR codes (no camera) and QR login (MSC4108);
    - identity-change acknowledgement and pinning UI (badge only);
    - turning recovery or backup off (`disable()`);
    - key export/import files;
    - dehydrated devices;
    - history sharing on invite (`with_enable_share_history_on_invite`);
    - "send only to verified devices" settings (`DecryptionSettings`/collect strategy);
    - encrypted state events;
    - the cross-process store lock (one process);
    - creating encrypted rooms from the UI (Phase 5 room creation; tests use helpers, and DMs from Element are already encrypted);
    - i18n.

    No pin changes. The only dependency change is the matrix-sdk `qrcode` feature (Q3).
18. **Q18: integration tests and CI.** RECOMMENDED (adjusted for Q2):
    - New ignored test files: `crates/core/tests/e2ee_rooms.rs`, `e2ee_identity.rs`, `e2ee_recovery.rs`, `e2ee_verification.rs`, `e2ee_devices.rs` and `log_hygiene.rs`. They use only alice and carol, start with `e2ee_serial()` and `reset_e2ee_account` (or `seed_recovery`), and use the existing ignore-reason convention.
    - Helpers in `tests/common/mod.rs`:
      - `e2ee_serial() -> tokio::sync::MutexGuard<'static, ()>`;
      - `reset_e2ee_account(n)` and `seed_recovery(n, passphrase) -> Zeroizing<String>` (Q2);
      - `raw_login_named(n, device_name)` (a raw login with a chosen device name, logged out by the caller);
      - `first_device(core, n)` (added in Task 4);
      - `create_encrypted_room(creator, name, invitee)` (initial `m.room.encryption` state, `m.megolm.v1.aes-sha2`);
      - `fetch_raw_event_type(client, room, event)` through `room::get_room_event::v3` (ruma-client-api room/get_room_event.rs:50).
    - Helpers print nothing but account names and counts; passwords and keys stay in `Zeroizing` values and are never printed.
    - The `integration` CI job keeps its 20-minute timeout. Each task records the ignored-run duration and the per-test reset cost. If the run passes 15 minutes, stop and ask (rule 8).
    - A test that fails twice in a row is a rule-8 stop, including the Phase 3 `typing_notices_reach_the_other_user` watch.
    - Rejected: mock-server (`matrix-sdk-test`) E2EE tests. They need the `testing` feature, a new dev-dependency surface, and prove less than the real Synapse.
19. **Q19: CLI.** RECOMMENDED (stands): `twicecat-cli` matches the new events exhaustively and prints one non-secret line each. It prints `Security` as recovery/device/cross-signing states, never `RecoveryKeyReady` (it prints "recovery key received, not shown"). Verification events (including `ShowQr`) are ignored. It gains no new commands. Rejected: CLI verification (the human checkpoint uses the window).

### Conventions for every task
- **Gate (rule 4)**, required before each commit:
  - `cargo fmt --all --check && cargo clippy --workspace --all-targets --all-features --locked -- -D warnings && cargo test --workspace --locked`, plus `scripts/check-app-deps.sh`, `scripts/check-core-api.sh` and `cargo deny check`.
  - Tasks that touch the core or the bridge also run `docker compose up -d --wait && scripts/create-test-users.sh && cargo test -p twicecat-core -p twicecat-app --locked -- --ignored --skip keyring`, then `docker compose stop`.
  - Core tasks that add E2EE tests also run their E2EE test binary a second time right after (`cargo test -p twicecat-core --locked --test <binary> -- --ignored`), without `create-test-users.sh` in between (Q2 repeat runs).
  - The agent never runs the real-keychain test.
- **One commit per task**, message `phase4.<task number>: <summary>`, with the attribution trailer as instructed by the session. ROADMAP.md is never committed.
- **PROGRESS.md (rule 5).**
  - Task 1 adds a "Phase 4 steps" checklist (one line per task, with the roadmap steps it covers) and a "Phase 4 human checkpoint" list.
  - Each task ticks its line and adds a "Phase 4 / Task N notes" block with:
    - the checks run and their results;
    - the ignored-run duration (and the repeat-run result for E2EE binaries);
    - every matrix-sdk(-ui/-crypto/-qrcode), ruma or iced API used, with the registry file:line where it was confirmed;
    - open issues and new human items, which also go on the checkpoint list unasked.
  - Rendering, wording, clipboard, browser opening, QR scanning and real-device interop go on the human list, never ticked by the agent (rule 9).
- **Boundary (rule 7).**
  - `crates/app` gains no dependency and no feature, and imports only `twicecat_core::{api, Core, CoreConfig, CoreHandle, DEFAULT_ROOM_LIST_PAGE_SIZE, paths, secrets, logging}`.
  - `api/` (including `api/security.rs`) stays SDK-free. `security.rs` and `verification.rs` are private core modules.
- **No network or blocking in `update`.** `update` only mutates state, calls `try_send`, and returns `Task`s (clipboard, timers, browser). Unit tests drive `update` with a plain `mpsc` sink.
- **Secrets and content hygiene (rule 6).**
  - No token, password, store passphrase, recovery key or passphrase, message body, UIA session ID or QR grid reaches `tracing`, a file, or a `Debug` string.
  - Every new public type that holds one has a redacted `Debug`, unit-tested with a sentinel value (`secret-key-x`, `secret-pw-x`).
  - Untrusted strings (device names, display names, SAS descriptions are static but still go through the sanitizer) pass through `text_safety`.
- **Stop and ask (rule 8)** if:
  - an API above behaves differently from this plan (for example Synapse refuses first-time cross-signing upload without UIA, `recover()` rejects the spaced key, the reset helper can't reach B0, or `generate_qr_code()` returns `None` for a peer that advertises scanning);
  - a test is flaky twice in a row, or an E2EE binary fails on its repeat run;
  - the integration run passes 15 minutes;
  - `cargo deny check` flags matrix-sdk-qrcode or qrcode;
  - a step needs a real account or a phone.

### Task 1: Phase 4 ledger, sign-off record and fixed-account E2EE test helpers
Language: rust
Files: crates/core/tests/common/mod.rs, crates/core/tests/e2ee_rooms.rs, PROGRESS.md
Depends on: none (Q1: Phase 3 signed off by the human on 2026-10-06)
Acceptance criteria:
- PROGRESS.md contains the line "Phase 3 signed off by the human on 2026-10-06 (merge of PR #4, #6); IME deferred, tracked under Phase 3 open issues", and the IME checkpoint item stays unticked as deferred.
- PROGRESS.md records the human's Phase 4 answers of 2026-10-06 (Q1 sign-off; Q2 fixed `.env.test` accounts only; Q3 QR display with matrix-sdk `qrcode`, iced `qr_code` per the validated plan; Q4 Element X settings; Q6 Skip with a persistent banner; Q5, Q7-Q19 as recommended).
- PROGRESS.md gains the "Phase 4 steps" list (Tasks 1-14 with roadmap steps) and a "Phase 4 human checkpoint" list seeded with the items at the end of this plan.
- `tests/common/mod.rs` gains the Q18 helpers `e2ee_serial`, `reset_e2ee_account`, `seed_recovery`, `raw_login_named`, `create_encrypted_room` and `fetch_raw_event_type`:
  - `reset_e2ee_account` and `seed_recovery` call `assert_local_homeserver` before any request, follow the Q2 steps, assert the Q2 postconditions, and log their own device out;
  - they print nothing but the account name and the number of devices removed; `seed_recovery` returns the key in `Zeroizing<String>`.
- `e2ee_rooms.rs` holds:
  - one **non-ignored** test (no network): two tasks contend for `e2ee_serial()`; the second gets the guard only after the first drops it, and a task that panics while holding it doesn't block the next (no poisoning);
  - ignored tests (verbatim ignore reason), each starting with `e2ee_serial()`:
    - **reset reaches B0 and repeats:** `reset_e2ee_account(1)` twice in one test; after each, a fresh raw client sees no backup, 4S off, an own identity, and only its own device; the master key's public part differs between the two resets;
    - **seeded recovery reaches B1:** `seed_recovery(1, None)`; a fresh raw client sees 4S on and a backup on the server, and `recovery().recover(key)` succeeds on it;
    - **encrypted room smoke:** after resetting alice and carol, `create_encrypted_room` succeeds, and the room's `m.room.encryption` state is readable by carol's raw client.
- `.env.test`, `scripts/create-test-users.sh` and bob's tests are unchanged.
- Gate, the ignored run and the repeat run of `e2ee_rooms` pass. Commit: `phase4.1: phase 4 ledger, sign-off and e2ee test baseline helpers`.

### Task 2: Additive core API for Phase 4
Language: rust
Files: crates/core/src/api/security.rs, crates/core/src/api/mod.rs, crates/core/src/api/types.rs, crates/core/src/engine.rs, crates/core/src/timeline.rs, crates/core/src/room_list.rs, crates/core/src/bin/twicecat-cli.rs, crates/app/src/screens/timeline_pane.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 1
Acceptance criteria:
- The API has exactly the Q7 additions (including `QrImage`, `ShowQr`, `QrScanned`, `StartSas` and `ConfirmQrScanned`), all `Clone + Debug + PartialEq` (`RecoveryKey` compares by value without printing it), SDK-free and documented. `scripts/check-core-api.sh` exits 0.
- `RoomSummary::new` gives `is_encrypted == false`, and `MessageDetails::default()` gives `shield == None`. Every existing `Placeholder { .. }` literal gains `utd: None`, and every existing test passes unchanged in meaning.
- Redaction unit tests with sentinels: no sentinel appears in `format!("{:?}")` of:
  - `RecoveryKey`, `Command::Recover`;
  - `CoreEvent::RecoveryKeyReady`;
  - `Command::SignOutDevices { password }`, `Command::ResetIdentity { password }`, `Command::BootstrapCrossSigning { password }`.
- `format!("{:?}", QrImage)` prints the size and no module data (checked with a 45 x 45 grid).
- Cloning a `RecoveryKey` and dropping both copies zeroizes each (checked through the `Zeroizing` wrapper type, as `MessageText` does).
- Before the real handlers land, the engine answers every new command without panicking:
  - `NotLoggedIn` with no session;
  - `ActionFailed { action, error: Internal("not implemented yet") }` with a session.

  Unit-tested with the engine test helper (no network).
- The CLI matches the new events exhaustively per Q19. Its tests pass.
- Gate and the ignored run pass. Commit: `phase4.2: additive core api for encryption, recovery, verification and devices`.

### Task 3: Encryption settings, encrypted rooms, shields and UTD reasons in the core (roadmap steps 1 and 5, core side)
Language: rust
Files: crates/core/src/client.rs, crates/core/src/room_list.rs, crates/core/src/timeline.rs, crates/core/tests/e2ee_rooms.rs, PROGRESS.md
Depends on: Task 2
Acceptance criteria:
- `build_client` sets the Q4 `EncryptionSettings`, unit-tested through a pure `encryption_settings()` that returns them. Every existing ignored test still passes.
- `summarize` sets `RoomSummary.is_encrypted` from `Room::encryption_state().is_encrypted()`.
- `map_event` behaviour:
  - sets `MessageDetails.shield` from `get_shield(false)`, through a pure, table-tested `map_shield` (every `TimelineEventShieldState`/code pair);
  - maps `UnableToDecrypt` to `Placeholder { what: UnableToDecrypt, utd: Some(reason) }`, through a pure, table-tested `map_utd_cause` (all 9 `UtdCause` variants plus non-Megolm to `Unknown`).
- Integration tests in `e2ee_rooms.rs` (verbatim ignore reason; `e2ee_serial`, then `reset_e2ee_account` for alice and carol; both run a core on B0):
  - **encrypted DM both ways:** alice creates an encrypted room and invites carol, and carol joins. alice sends `Send { text: "secret-body-<hex1>" }`, and carol's timeline shows it as a `Message` with that body within 30 s. carol replies `secret-body-<hex2>`, and alice sees it. On the server both events are `m.room.encrypted` (`fetch_raw_event_type`). Both cores report the room with `is_encrypted == true`;
  - **formatted body in an encrypted room:** alice sends `**bold**`, and carol's item has a bold inline. This proves the Phase 3 `latest_json` path works on decrypted events;
  - **shield:** carol's items from alice's B0 device (not signed by alice's identity) have `shield == Some(Red(UnsignedDevice))`. The plain-room messages in the existing tests keep `shield == None`.
- Gate, the ignored run and the repeat run of `e2ee_rooms` pass. Commit: `phase4.3: encryption settings, encrypted rooms, shields and utd reasons in the core`.

### Task 4: Security status, cross-signing bootstrap, identity reset and re-authentication in the core (roadmap step 4, core side)
Language: rust
Files: crates/core/src/security.rs, crates/core/src/lib.rs, crates/core/src/engine.rs, crates/core/tests/common/mod.rs, crates/core/tests/e2ee_identity.rs, PROGRESS.md
Depends on: Task 3
Acceptance criteria:
- `security.rs` (private) starts the Q8 status forwarder after login and restore. It emits `Security(SecurityStatus)` once at start and then only on change, through a pure `compute_status(..)` that is table-tested (including identity present but no private keys → `MissingKeys`).
- `BootstrapCrossSigning { password }` calls `bootstrap_cross_signing_if_needed(auth)`, and `ResetIdentity { password }` calls `recovery().reset_identity()`. Both follow Q13 and Q14:
  - a UIA answer without a password gives `ReauthRequired { action }`;
  - with a password, the core retries once with the UIA session;
  - a rejected password gives `ActionFailed { error: ReauthFailed }`;
  - OAuth gives `OpenAccountManagement(approval_url)` and then polling;
  - `CancelIdentityReset` cancels.
- Pure helpers are unit-tested: `uia_password(user_id, password, session)` builds the right `AuthData`, and `reset_outcome(..)` covers the Q14 branches. The OAuth branch is tested with a hand-built `CrossSigningResetAuthType::OAuth` value, with no network.
- `Active::stop_tasks` ends the forwarder and the security `JoinSet` before the sync service. The existing logout and session tests pass.
- `tests/common/mod.rs` gains `first_device(core, n)` (Q2).
- Integration tests in `e2ee_identity.rs` (verbatim ignore reason; `e2ee_serial`, then `reset_e2ee_account(1)`, and carol too where used):
  - **status on B0:** right after alice's core logs in, `Security` reaches `cross_signing: MissingKeys`, `device: Unverified`, `identity_exists: true`, `recovery: Disabled` within 30 s;
  - **reset needs the password:** `ResetIdentity { password: None }` gives `ReauthRequired { ResetIdentity }`. A wrong password gives `ReauthFailed`. The right password gives a new identity: the master key's public part differs (read by a raw client through `request_user_identity`), `Security { cross_signing: Complete, device: Verified, recovery: Disabled }`, and a backup exists on the server (raw client `fetch_exists_on_server`, proving the Q4 auto backup after reset);
  - **shield after reset:** in an encrypted room with carol, a message alice sends after the reset reaches carol with `shield == None` (signed device, identity not verified by carol: no shield in non-strict mode);
  - **bootstrap is a no-op when an identity exists:** `BootstrapCrossSigning { password: None }` after login succeeds without `ReauthRequired`;
  - **restored session:** after `first_device` and a core restart (`RestoreSession`), `Security` reports `cross_signing: Complete` and `device: Verified` again (the restore path of Q4).
- PROGRESS records Synapse v1.151.0's rule for a first-time cross-signing upload without UIA, read from the image's installed `synapse/rest/client/keys.py` (file and line cited), and that CI's fresh volume runs the first-time bootstrap on each run. If Synapse requires UIA for a first upload, restored pre-Phase-4 sessions need the Q13 password prompt; if that differs from what Q4 assumes, stop and ask.
- Gate, the ignored run and the repeat run of `e2ee_identity` pass. Commit: `phase4.4: security status, cross-signing bootstrap, identity reset and re-authentication in the core`.

### Task 5: Recovery in the core (roadmap steps 2 and 5, core side)
Language: rust
Files: crates/core/src/security.rs, crates/core/src/engine.rs, crates/core/tests/e2ee_recovery.rs, PROGRESS.md
Depends on: Task 4
Acceptance criteria:
- `security.rs`:
  - handles `EnableRecovery`: it runs `enable().wait_for_backups_to_upload()` in the security `JoinSet`, forwards `subscribe_to_progress` as `RecoveryProgress` (never the `Done { recovery_key }` payload), and emits `RecoveryKeyReady(RecoveryKey)` once;
  - handles `Recover { key }` (`recover`, then `Recovered`) and `ChangeRecoveryKey` (`reset_key()`, then `RecoveryKeyReady`).
- Errors map to fixed messages through a pure, unit-tested `map_recovery_error`:
  - `BackupExistsOnServer` gives "a key backup already exists; enter your recovery key";
  - a wrong key gives "that recovery key or passphrase is not correct".
- Integration tests in `e2ee_recovery.rs` (verbatim ignore reason; `e2ee_serial`, then `reset_e2ee_account` or `seed_recovery` as stated):
  - **set up, then restore on a fresh store reads earlier history (roadmap acceptance):**
    - reset alice and carol; alice's core 1 is `first_device`; alice creates an encrypted room with carol, carol (core on B0) sends 3 messages (`secret-body-<hex>`), and alice's timeline shows them;
    - alice sends `EnableRecovery` and gets `RecoveryKeyReady` and then `Security { recovery: Enabled }`, with no `BackupExistsOnServer`;
    - a **second alice core on a new data directory** logs in and opens the room, and the 3 items are `Placeholder { utd: Some(HistoricalMessageAndDeviceIsUnverified | HistoricalMessageAndBackupIsDisabled | Unknown) }` (the observed reason is recorded);
    - it sends `Recover { key }` and gets `Recovered` and `Security { device: Verified, recovery: Enabled }`;
    - within 60 s the same item IDs become `Message`s with the original bodies (roadmap step 5: the update arrives as `Set` diffs), with `shield == Some(Grey(AuthenticityNotGuaranteed))` (keys from the backup);
  - **returning user and `BackupExistsOnServer` on purpose:** `seed_recovery(1, None)` gives K0. A core on B1 reports `device: Unverified`, `recovery: Incomplete`. `EnableRecovery` gives `ActionFailed { Recovery, .. }` with the fixed "a key backup already exists" message. `Recover { K0 }` then gives `Security { device: Verified, recovery: Enabled, cross_signing: Complete }`;
  - **spaced key, wrong key and passphrase input:** the key with its spaces is accepted (as above). A key with one changed character gives `ActionFailed { Recovery, .. }` with the fixed message, and `recovery` stays `Incomplete`. After `seed_recovery(1, Some(<random passphrase>))`, `Recover { <passphrase> }` succeeds;
  - **change key:** on a `first_device` with recovery enabled, `ChangeRecoveryKey` gives a new key that differs from the first. A fresh-store core recovers with the new key, and another fresh-store core fails with the old one;
  - **reset clears recovery:** after recovery is `Enabled`, `ResetIdentity { password }` leaves `recovery: Disabled` and a new backup on the server.
- Gate, the ignored run and the repeat run of `e2ee_recovery` pass. Commit: `phase4.5: recovery in the core`.

### Task 6: Interactive SAS verification in the core (roadmap steps 3 and 4, core side)
Language: rust
Files: crates/core/src/verification.rs, crates/core/src/lib.rs, crates/core/src/engine.rs, crates/core/tests/e2ee_verification.rs, PROGRESS.md
Depends on: Task 5
Acceptance criteria:
- `verification.rs` (private):
  - registers to-device and in-room request handlers (guards kept in `Active`);
  - emits `VerificationRequested { id, peer }` (the peer device name comes from `other_device_data`);
  - drives `VerificationRequest::changes()` and then `SasVerification::changes()` into `Verification { id, phase }` per Q9 (SAS part);
  - handles `AcceptVerification`, `ConfirmSas`, `SasMismatch`, `CancelVerification` and the three `Request*` commands. `StartSas` and `ConfirmQrScanned` still answer "not implemented yet" (Task 7).
- At most one flow is active. A second incoming request is cancelled, and a second outgoing one gets `ActionFailed { Verification, Busy }` (Q8).
- Pure, table-tested helpers: `map_request_state`, `map_sas_state` (the emoji array is copied as `SasEmoji` strings, decimals as `[u16; 3]`) and `map_cancel_code` (`m.user`, `m.mismatched_sas`, `m.timeout`, `m.accepted`, other).
- Integration tests in `e2ee_verification.rs` (verbatim ignore reason; `e2ee_serial`, resets of alice and carol; roadmap acceptance "SAS verification between two SDK clients completes"):
  - **self-verification:**
    - setup: alice's core 1 is `first_device`. alice sends 2 encrypted messages in a room with carol. alice logs in on core 2 (new data dir): `device: Unverified` and the 2 items are UTD;
    - flow: core 2 sends `RequestSelfVerification`, and core 1 gets `VerificationRequested { peer.own_account: true }` and accepts. Both reach `Emojis(e)` with **identical** 7 symbols, and both confirm;
    - outcome: both reach `Done`, core 2 reports `device: Verified` within 30 s, and its 2 UTD items become the original messages within 60 s (the backup key arrives by secret gossip);
  - **user verification in a DM:** alice and carol are each `first_device`. alice sends `RequestUserVerification { carol }` in their encrypted DM, and carol accepts. The emojis match and both confirm. Then `WatchUserTrust { carol }` on alice gives `UserTrust { carol, Verified }`, and the reverse gives carol `Verified` for alice;
  - **mismatch:** in a new flow, one side sends `SasMismatch`, and both sides reach `Cancelled { reason: Mismatch }`. Trust is unchanged;
  - **busy:** a second `RequestSelfVerification` while a flow is open gives `ActionFailed { Verification, Busy }`.
- Gate, the ignored run and the repeat run of `e2ee_verification` pass. Commit: `phase4.6: interactive sas verification in the core`.

### Task 7: QR code display in the core (Q3)
Language: rust
Files: crates/core/Cargo.toml, Cargo.lock, crates/core/src/verification.rs, crates/core/tests/e2ee_verification.rs, PROGRESS.md
Depends on: Task 6
Acceptance criteria:
- `crates/core/Cargo.toml`: the matrix-sdk dependency keeps `version = "=0.19.1"` and gains only the `qrcode` feature. `crates/app/Cargo.toml` (iced features) is unchanged unless the human's validation of Q3 says otherwise.
- `Cargo.lock` adds exactly matrix-sdk-qrcode 0.19.1 and qrcode 0.14.1: a sorted name/version comparison before vs after differs only in those two entries, and no existing version changes. `cargo tree -p twicecat-core -e features -i matrix-sdk` lists `qrcode`. `cargo deny check` passes with `deny.toml` unchanged (else stop and ask).
- `verification.rs`:
  - once a request is `Ready` (incoming accepted or outgoing ready), calls `generate_qr_code()`. `Some` emits `ShowQr(grid)` and follows `QrVerification::changes()`; `None` keeps the Task 6 behaviour (outgoing: `start_sas()`; incoming: `Waiting`);
  - `StartSas { id }` calls `start_sas()` while a QR code shows, and the flow continues as SAS. If the peer starts SAS while the code shows, the flow follows SAS too;
  - `ConfirmQrScanned { id }` calls `QrVerification::confirm()` only in `QrScanned`; in any other phase it gives `ActionFailed { Verification, NotAllowed }`.
- Pure, table-tested helpers:
  - `qr_image(size, modules)` builds a `QrImage` from the code's width and its `to_colors()` read through `Color::select(true, false)`; it rejects a module count that isn't `size * size`;
  - `map_qr_state` covers every `QrVerificationState` (`Started` → `ShowQr` already sent, `Scanned` → `QrScanned`, `Confirmed` → `Confirmed`, `Reciprocated` → `Waiting`, `Done` → `Done`, `Cancelled` → `Cancelled { reason }` through `map_cancel_code`).
- A unit test (no network) builds spec-conformant QR bytes (`MATRIX`, version 0x02, mode 0x02, flow ID, two 32-byte keys, a shared secret), parses them with `QrVerificationData::from_bytes`, calls `to_qr_code()`, and gets a `QrImage` of size 45 whose three 7 x 7 finder patterns are in place.
- Integration tests added to `e2ee_verification.rs` (verbatim ignore reason; `e2ee_serial`, reset of alice):
  - **QR shown to a scanning peer:** alice's core is `first_device`. A raw client logged in as alice (`raw_login_named(1, "twicecat-test-qr")`) requests verification of the core's device with `request_verification_with_methods([SasV1, QrCodeScanV1, ReciprocateV1])`. The core gets `VerificationRequested { own_account: true }` and accepts, then emits `ShowQr(img)` with `img.size == 45`. The raw client's request reports the core's methods with `QrCodeShowV1` and without `QrCodeScanV1`;
  - **compare emoji instead:** in that state the core sends `StartSas`; the raw client accepts the SAS; both sides' emoji are identical; both confirm; the core reaches `Done` and the raw client's own device is verified;
  - **cancel while showing:** in a new flow the core sends `CancelVerification` while `ShowQr` is shown; the raw client sees the request cancelled with code `m.user`;
  - **Twicecat to Twicecat stays SAS:** the Task 6 self-verification test still passes and never emits `ShowQr` (neither side advertises scanning).
- The raw client logs out at the end of each test. The scan and reciprocate/confirm round trip is on the human checkpoint (Element X), per Q3.
- Gate, the ignored run and the repeat run of `e2ee_verification` pass. Commit: `phase4.7: qr code display for verification in the core`.

### Task 8: Devices, session management and user trust in the core (roadmap steps 4 and 6, core side)
Language: rust
Files: crates/core/src/security.rs, crates/core/src/engine.rs, crates/core/tests/e2ee_devices.rs, PROGRESS.md
Depends on: Task 7
Acceptance criteria:
- `ListDevices` emits `Devices` per Q12 (current first, no IP), through a pure `merge_devices(server_list, crypto_trust, current)` that is unit-tested for ordering, missing keys (`NoKeys`) and missing names. It is re-emitted on `devices_stream` updates while the Sessions screen has asked at least once since login.
- `RenameDevice` trims the name, caps it at 64 chars and refuses an empty name (`NotAllowed`).
- `SignOutDevices` follows Q12 and Q13, and refuses the current device (`NotAllowed`). On an OAuth session, a pure `account_management_target(metadata, device_id)` (unit-tested with hand-built metadata JSON) gives the `DeviceDelete` URL, emitted as `OpenAccountManagement`. A missing URL gives `ActionFailed` "manage your devices on your account page".
- `WatchUserTrust { user_id }` emits `UserTrust` at once and on each `user_identities_stream` change for that user (one watched user at a time, replaced on each call), through a pure `map_user_trust` (verified, unverified, no identity, violation) that is table-tested. `SelectRoom` of a DM with exactly one `direct_targets()` user watches that user automatically (Q10); unit-tested through a pure `dm_peer(targets, own)`.
- Integration tests in `e2ee_devices.rs` (verbatim ignore reason; `e2ee_serial`, then `reset_e2ee_account(1)`, so the device counts below are exact):
  - **list and rename:** alice on cores 1 (`first_device`) and 2. `ListDevices` on core 1 lists exactly 2 devices, its own first with `current: true`. Core 2 renames itself to "Laptop test", and core 1's next `Devices` shows that name;
  - **trust in the list:** before a self-verification (the Task 6 flow), core 1 lists core 2 as `Unverified`; after it, `Verified`;
  - **sign out another device:** core 1 sends `SignOutDevices { [core 2 device], password: None }` and gets `ReauthRequired { DeleteDevices }`. A wrong password gives `ReauthFailed`. The right password succeeds: core 2 receives `SessionExpired` within 30 s, and the next `Devices` on core 1 lists exactly 1 device;
  - **current device refused:** `SignOutDevices { [own device] }` gives `ActionFailed { Devices, NotAllowed }`.
- Gate, the ignored run and the repeat run of `e2ee_devices` pass. Commit: `phase4.8: device list, rename, sign out and user trust in the core`.

### Task 9: Log-capture and data-at-rest tests (roadmap step 7, core side)
Language: rust
Files: crates/core/tests/log_hygiene.rs, crates/core/tests/common/mod.rs, crates/core/src/session.rs, crates/core/src/paths.rs, PROGRESS.md
Depends on: Task 8
Acceptance criteria:
- `log_hygiene.rs` is a separate test binary with exactly one ignored test (verbatim ignore reason) implementing Q16 (a) and (b):
  - it takes `e2ee_serial()` and resets alice and carol first;
  - a global capturing subscriber at `info`;
  - the full flow from Q16 (its verification is SAS between two Twicecat cores, so no QR code is involved);
  - the sentinel set: access token, refresh token if present, store passphrase (all read from the stored record JSON), alice's and carol's passwords, the recovery key, and every `secret-body-<hex>` sent;
  - it asserts that no sentinel appears in the captured log text, at `info` and again with `DEFAULT_FILTER`;
  - it asserts that no sentinel's bytes (UTF-8 and the JSON-escaped form) appear in any file under the data root, `-wal`/`-shm` included, while the session is still live;
  - it prints only counts and file names, never contents.
- Self-check: the test first proves the scanner works by finding a sentinel it deliberately wrote to a scratch file and logged through `tracing::info!` in a separate, discarded buffer (so a broken capture can't pass silently).
- **Q16 (c):** a unit test in `paths.rs` lists the data root after `create_session_store_dir` and `write_last_account`, and finds only `last_account` and the store directory. A test in `session.rs` shows `SessionRecord::to_bytes` is the only serializer of tokens and passphrase (its `Debug` and error strings carry no sentinel; the test extends the existing one).
- If any check finds a leak, the task stops and reports it (rule 8). It doesn't hide the leak, and it doesn't filter the SDK's output.
- Gate, the ignored run and the repeat run of `log_hygiene` pass, with the duration recorded. Commit: `phase4.9: log capture and data at rest tests`.

### Task 10: Security state, banners, Confirm-identity and recovery screens in the app (roadmap steps 2 and 4, UI)
Language: rust
Files: crates/app/src/security.rs, crates/app/src/lib.rs, crates/app/src/state.rs, crates/app/src/screens/security.rs, crates/app/src/screens/mod.rs, crates/app/src/screens/main_view.rs, crates/app/tests/bridge.rs, PROGRESS.md
Depends on: Task 9
Acceptance criteria:
- `security.rs` holds a pure `SecurityModel`. Its fields: `status`, `progress`, `setup: Option<RecoverySetup { key: RecoveryKey, saved_ticked: bool }>`, `restore_input` (secure), `identity_screen_skipped`, `dismissed_recovery_banner` and `reauth: Option<ReauthPrompt>`. Its manual `Debug` prints no key or input.
- Pure functions, table-tested:
  - `banner(&SecurityStatus, dismissed_recovery) -> Option<Banner>`: device `Unverified` always gives the persistent unverified banner (Q6), whatever the dismissal flag; recovery `Disabled` and `Incomplete` give dismissible banners unless dismissed; none when everything is fine or the status is `Unknown`;
  - `needs_identity_screen(status, fresh_login, skipped)`, per Q6 (fresh login only, not after Skip, never on a restored session);
  - `can_finish_setup(&RecoverySetup)` (only when ticked).
- `update` wiring:
  - `Security` updates the model; the unverified banner disappears as soon as `device` becomes `Verified`;
  - **Set up recovery** sends `EnableRecovery`, and `RecoveryKeyReady` opens the show-once screen;
  - **Copy** returns `clipboard::write` and arms a 60 s `ClipboardDue` timer. Its handler reads the clipboard and clears it only if it still equals the key (the comparison is a pure, tested helper);
  - **Done** drops the key;
  - **Restore** sends `Recover { key }` from the trimmed input, then zeroizes the input; an `ActionFailed` with the "backup already exists" message from `EnableRecovery` opens the restore field (Q5);
  - the Confirm-identity screen offers the Q6 choices; **Skip for now** sets `identity_screen_skipped` and shows the main view with the unverified banner; the banner's **Verify** reopens the Confirm-identity screen;
  - logout and window close drop the key and inputs.
- Unit tests through `update` with a plain `mpsc` sink:
  - each command is sent once;
  - Done is refused until ticked;
  - `ActionFailed { Recovery }` shows its fixed message and keeps the input;
  - Skip leaves the unverified banner visible, and a dismiss message doesn't remove it;
  - a sentinel key never appears in `format!("{:?}", app)`.
- Widget-tree shape: the security screens and the banner sit in the existing fixed `stack` layer pattern, so opening and closing them doesn't reset the timeline scroll or the editor focus. A tag test, as in Phase 3, covers this.
- `tests/bridge.rs` gains a headless ignored test (verbatim ignore reason) **as carol** (the other bridge tests use alice): through `update` only, carol logs in and sees the Confirm-identity screen (or the unverified banner); chooses Reset; answers the re-authentication prompt with carol's password; then presses Set up recovery, receives the key on the show-once screen, ticks and presses Done; then logs out. The model ends with `setup == None` and `status.recovery == Enabled` before logout. This path is deterministic whatever state earlier runs left carol in (the reset deletes every backup).
- Human checklist gains:
  - the recovery-key screen wording and layout in both themes (roadmap item);
  - the key is legible and copyable, and the clipboard clears after 60 s;
  - the Confirm-identity screen reads well; **Skip for now** leaves the unverified banner, whose wording reads well, which stays after a restart and can't be dismissed, and whose Verify button reopens the screen;
  - restoring with a key from Element works.
- Gate and the ignored run pass. Commit: `phase4.10: security banner, confirm identity and recovery screens`.

### Task 11: Verification dialog with the emoji grid and the QR code (roadmap step 3, UI)
Language: rust
Files: crates/app/src/verification.rs, crates/app/src/lib.rs, crates/app/src/state.rs, crates/app/src/screens/verification.rs, crates/app/src/screens/mod.rs, crates/app/src/screens/main_view.rs, PROGRESS.md
Depends on: Task 10
Acceptance criteria:
- A pure `VerificationModel` follows one flow (`id`, `peer`, `phase`) and ignores events with another `id`. Its transitions are table-tested: Requested(incoming) → Ready → Emojis → Confirmed → Done; Ready → ShowQr → QrScanned → Confirmed → Done; ShowQr → Emojis (compare emoji instead, or the peer started SAS); Cancelled from any phase; a stale ID ignored.
- A pure `qr_rgba(&QrImage, scale, quiet) -> (u32, u32, Vec<u8>)` is unit-tested: the output is `(size + 2 * quiet) * scale` square; the quiet zone is white; module (0, 0) (a finder corner) is black; colours are black on white in both themes. On `ShowQr` the model builds one `image::Handle::from_rgba` (scale 6, quiet zone 4) and keeps it until the flow ends; a test shows `Handle::id()` stays the same across unrelated `update` calls and the handle is dropped on `Done`/`Cancelled`.
- The dialog is a modal in the fixed stack layer, with these states:
  - an incoming request shows "<sanitized name or user ID> wants to verify" (for own devices, "Your other device <name>") with **Accept** and **Decline**;
  - the waiting states show a spinner and **Cancel**;
  - `ShowQr` shows "Scan this code with your other device" (own account) or "Ask <name> to scan this code", the QR image with `FilterMethod::Nearest` at a fixed size, **Compare emoji instead** and **Cancel**;
  - `QrScanned` shows "Does your other device show that the code was scanned successfully?" (own account; for another user, "Does <name> see a confirmation?") with **Yes** (`ConfirmQrScanned`) and **No** (`CancelVerification`);
  - `Emojis` shows the 7 symbols in a grid with their English descriptions under each, then **They match** and **They don't match**;
  - `Decimals` shows the 3 numbers;
  - `Done` shows "Verified" and **Close**;
  - `Cancelled` shows the reason in one line.
- Each button sends exactly one command (`update` tests with an `mpsc` sink), including `StartSas` and `ConfirmQrScanned`. Peer names and descriptions go through `one_line_capped`. `format!("{:?}", app)` with a QR flow open shows no module data.
- **Entry points:** "Use another device" on the Confirm-identity screen sends `RequestSelfVerification`. **Verify** buttons come in Tasks 12 and 13.
- Human checklist gains:
  - the emoji render (font coverage on each OS) and match Element X and Element Web character for character;
  - the dialog layout in both themes, including the QR code (crisp, black on white, with its quiet zone, in light and dark themes);
  - **scan Twicecat's QR code with Element X on a phone**: (a) a new Twicecat device (Confirm your identity → Use another device) shows the code and Element X scans it; Twicecat then asks to confirm, **Yes** completes, and Twicecat becomes verified; (b) Element X as the new device (Twicecat already verified) scans the code Twicecat shows; (c) **Compare emoji instead** from the QR screen completes by emoji;
  - **verify this device against Element X on a phone (roadmap checkpoint)** and against the local Element Web (`docker compose --profile element up -d`, signed in as bob, Q2).
- Gate passes. Commit: `phase4.11: verification dialog with the sas emoji grid and qr code`.

### Task 12: Shields, trust badges, encrypted-room marks and UTD placeholders (roadmap steps 4 and 5, UI)
Language: rust
Files: crates/app/src/screens/timeline_pane.rs, crates/app/src/screens/room_list_pane.rs, crates/app/src/screens/main_view.rs, crates/app/src/state.rs, crates/app/src/timeline.rs, PROGRESS.md
Depends on: Task 11
Acceptance criteria:
- Pure helpers, each unit-tested:
  - `shield_text(Shield) -> (Look, &'static str)`, covering the 7 codes with the Q10 wording;
  - `utd_text(Option<UtdReason>) -> &'static str`, with the Q11 wording for every variant;
  - `trust_badge(UserTrust) -> Option<Badge>`.
- In the timeline:
  - a message with `shield: Some(..)` shows a small shield after the time, with a tooltip of the text;
  - UTD placeholders show `utd_text`;
  - row estimates account for neither (same height).
- An `update` test applies a `Set` that turns a UTD `Placeholder` into a `Message` with the same ID. The row's text changes, and the height is re-measured (the measured entry is invalidated as Phase 3 does for `Set`). The Phase 3 timing test (16 ms) and layout test still pass.
- Room list rows and the room header show a lock mark when `is_encrypted`.
- On selecting a DM, the core's `UserTrust` for its peer (Q10) is kept per room in app state; the header shows the badge, and **Verify** sends `RequestUserVerification { user_id }`. Unit-tested through `update`; a `UserTrust` for a user who is not the selected DM's peer is ignored by the header.
- Human checklist gains:
  - shields and tooltips look right in both themes and on Element-sent messages;
  - the lock and DM trust badges look right;
  - UTD placeholders read well and turn into messages in place, with no jump, after recovery or verification.
- Gate passes. Commit: `phase4.12: shields, trust badges, encrypted room marks and utd placeholders`.

### Task 13: Sessions screen and re-authentication prompt (roadmap step 6, UI)
Language: rust
Files: crates/app/src/sessions.rs, crates/app/src/lib.rs, crates/app/src/state.rs, crates/app/src/screens/sessions.rs, crates/app/src/screens/mod.rs, crates/app/src/screens/main_view.rs, PROGRESS.md
Depends on: Task 12
Acceptance criteria:
- A **Sessions** entry in the main view's account area opens the screen, which sends `ListDevices`.
- The list: the current device first ("This device"), then the others with:
  - the sanitized name or device ID, "last active" (`dates.rs` labels) and a Verified/Not verified badge;
  - **Rename** (inline field, Enter saves);
  - **Verify** (unverified others only), which sends `RequestDeviceVerification`;
  - **Sign out**, after an inline confirmation;
  - a "Sign out of all other sessions" action.
- The re-authentication modal opens on `ReauthRequired`. It has a secure password field and repeats the pending command with `password: Some(..)`. On `ReauthFailed` it shows "Wrong password" and keeps the modal. On close it zeroizes the field. The pending command keeps device IDs only, never the password after sending.
- `OpenAccountManagement(url)` goes through the Phase 2 `check_browser_url` path inside a `Task`, behind the Phase 3 "confirm every link" bar showing the host.
- Logout warning per Q12: a pure `logout_needs_warning(status, devices_known)` (table-tested), plus a confirmation with **Set up recovery** and **Sign out anyway**.
- `update` tests with an `mpsc` sink cover: rename, verify, sign out one and all, the reauth round trip (no password in `Debug` of the app state, sentinel), the OAuth URL path, and the logout warning.
- Human checklist gains:
  - the Sessions screen layout;
  - renaming shows in Element;
  - signing out another session with the password works, and a wrong password re-prompts;
  - on a matrix.org OAuth account (real account, human only), "Sign out" opens the account page for that device;
  - the logout warning appears without recovery.
- Gate passes. Commit: `phase4.13: sessions screen with rename, verify, sign out and re-authentication`.

### Task 14: Security review and Phase 4 wrap-up (roadmap step 7)
Language: rust
Files: crates/app/src/state.rs, crates/app/src/security.rs, crates/app/src/sessions.rs, crates/app/src/verification.rs, PROGRESS.md
Depends on: Task 13
Acceptance criteria:
- App-side redaction sweep: one test builds an `App` in each of these states with sentinel values and asserts no sentinel in `format!("{:?}", app)` or in any `Message` `Debug`:
  - the show-once key screen;
  - a restore input;
  - the reauth modal with a typed password;
  - a verification in `Emojis`, and one in `ShowQr` (no module data printed);
  - the Sessions screen.
- A grep-style test checks that `crates/app/src` never calls `tracing`, `println!`, `eprintln!` or `dbg!` with a field from those types (the patterns are listed in the test).
- PROGRESS.md gains a "Phase 4 security review" section per Q16 (d):
  - each secret and where it lives (keychain record: tokens, passphrase; SDK stores: room keys, cross-signing private keys, backup key, all encrypted with the passphrase cipher; never stored: recovery key, passwords; shown only on screen during a flow: the QR code);
  - the evidence (Task 9 tests, with their sentinels and filters);
  - the residuals listed in Q15 and Q16;
  - the Phase 3 security items carried over (editor buffer, name ambiguity, Copy pastejacking).
- PROGRESS.md marks the Phase 4 agent work done (the human checkpoint stays open), and consolidates "Phase 4 open issues (summary)": the Q17 scope cuts, the observed Synapse first-upload rule (Task 4), the Q3 iced-widget decision, the per-test reset cost and any test-duration notes, and every Low or Medium finding from the task reviews.
- The full ignored suite passes after `create-test-users.sh`, and again right after without it (Q2 repeat runs), with both durations recorded (under 15 minutes each, else stop and ask).
- Gate passes. Commit: `phase4.14: security review and phase 4 wrap-up`.

### Task 15: Change the recovery key from the app (Q5 "Change key", added 2026-10-07)
Language: rust
Files: crates/app/src/security.rs, crates/app/src/screens/security.rs, crates/app/src/sessions.rs, crates/app/src/screens/sessions.rs, crates/app/src/state.rs, PROGRESS.md
Depends on: Task 14
Why: Task 10 built no screen for Q5 "Change key"; the human decided on 2026-10-07 to add it before the Phase 4 sign-off. The core already does the work (Task 5: `Command::ChangeRecoveryKey` runs `Recovery::reset_key()`, matrix-sdk 0.19.1 `encryption/recovery/mod.rs:359`, which "will rotate the secret storage key and re-upload all the secrets"; `change_key_precheck` allows it only when recovery is `Enabled`; success is `RecoveryKeyReady`, failure is `ActionFailed { Recovery, .. }`). This task is app-only: no core, API or dependency change.
Acceptance criteria:
- **Entry point (Q-T15a, RECOMMENDED):** a **Recovery** line at the top of the Sessions page, above the device list, driven by the `Security` state the app already holds:
  - `Enabled`: "Recovery is on." with a **Change recovery key** button;
  - `Disabled`: "Recovery is off." with **Set up recovery** (the existing `SetupRecovery` flow);
  - `Incomplete`: "This device isn't connected to your recovery." with **Enter recovery key** (the existing `OpenRestore`);
  - `Unknown`: no button.
  - Rejected: a new settings screen (one button does not justify it); a banner (recovery is fine in this state, nothing needs attention).
- **Warning first (new `Overlay::ChangeKeyConfirm`).** **Change recovery key** opens a warning card and sends nothing yet. Draft wording (final wording is a human item):
  - title "Change your recovery key?";
  - "Your current recovery key will stop working. Twicecat will show you a new one, once. Other apps and devices that ask for your recovery key will need the new one. Anything another app saved in your secure storage that this device doesn't have will be lost.";
  - **Change key** (sends `Command::ChangeRecoveryKey` once and sets `Busy::ChangingKey`) and **Cancel** (closes, sends nothing).
- **While busy:** the card shows "Changing your recovery key…" and no buttons; a second press, or **Set up recovery**, sends nothing while any `Busy` is set.
- **Success:** `RecoveryKeyReady` while `Busy::ChangingKey` opens the existing show-once key screen with the title "Your new recovery key" (set-up keeps "Your recovery key"). Everything else is unchanged from Task 10: the "I've saved my recovery key" box gates **Done**, Copy and the 60 s clipboard clear, and the key is dropped on Done, logout and window close.
- **Failure:** `ActionFailed { action: Recovery, .. }` while `Busy::ChangingKey` clears the busy state, closes the card and shows the core's fixed message on the Sessions page notice line. The old key still works (the core changed nothing it could not finish; this is the SDK's behaviour and is not claimed beyond that).
- **Hygiene:** `Debug` of every new message and of the model in each new state holds no key (sentinel `secret-key-x`), and the Task 14 redaction sweep gains the "new key" screen and `ChangeKeyConfirm`.
- **Tests (red first):**
  - `security.rs` / `state.rs` unit tests through `update` with a plain `mpsc` sink: the button only opens the card; **Cancel** sends nothing; **Change key** sends exactly one `ChangeRecoveryKey`; a second press while busy sends nothing; `RecoveryKeyReady` shows the key screen titled "Your new recovery key" and Done needs the box; `ActionFailed` clears busy and sets the notice; the Recovery line shows the right label and button for each of the four states;
  - no new integration test: the E2EE reset helpers (`e2ee_serial`, `reset_e2ee_account`) live in `crates/core/tests/common` and app tests can't reach them, and the core's Task 5 `change key` test already proves a new key that differs, opens a fresh store, and makes the old key fail. The app side is covered by the unit tests above.
- **Human checkpoint items** (added to PROGRESS.md and the checklist page unasked):
  - the Recovery line on the Sessions page in each state, in both themes;
  - the warning wording, and the "Your new recovery key" screen;
  - after changing bob's key in Twicecat, the new key restores in Element Web (bob on a new session) and the old one is refused.
- Gate passes (app only, no core or bridge code: no ignored run needed). Commit: `phase4.15: change the recovery key from the sessions page`.

### Phase 4 human checkpoint (collected in PROGRESS.md)
- [ ] Phase 3 sign-off recorded (Q1, 2026-10-06). Confirm Q3 as planned: matrix-sdk `qrcode` enabled, iced `qr_code` **not** enabled (the QR code is drawn with the existing `image` widget from the SDK's own grid); and revisit any of Q1-Q19.
- [ ] CI green on Linux, Windows and macOS; the `integration` job stays under its 20-minute timeout.
- [ ] Aware that the ignored suite signs out every alice and carol session on the local Synapse and resets their identities (Q2); local Element Web checks use bob.
- [ ] **On a fresh install, verify this device against Element X on your phone (emoji match), and confirm encrypted history loads** (roadmap checkpoint).
- [ ] **Scan Twicecat's QR code with Element X:** a new Twicecat device shows the code and Element X scans it, Twicecat asks for confirmation, **Yes** completes, and Twicecat becomes verified; the reverse (Element X as the new device scanning Twicecat's code); **Compare emoji instead** from the QR screen works; the code is crisp, black on white with a quiet zone, in both themes.
- [ ] **Review the recovery-key screen wording** (roadmap checkpoint); the key is shown once, Copy works, and the clipboard clears after 60 s.
- [ ] Recovery restore with a key (and a passphrase, if your account has one) made in Element; history decrypts in place.
- [ ] Confirm-identity screen after a fresh login: each choice works. **Skip for now** leaves the device unverified and shows the persistent banner; review its wording ("This device isn't verified. Verify it to read your encrypted history and so others can trust it."); it can't be dismissed, it is back after a restart, its Verify button reopens the screen, and it disappears once the device is verified.
- [ ] Verification dialog: incoming from Element X/Web, outgoing to another device, user verification in a DM, mismatch and cancel; emoji draw on each OS.
- [ ] Shields, lock marks and DM trust badges look right in both themes; UTD placeholders turn into messages without a jump.
- [ ] Sessions screen: rename shows in Element; sign out another session with the password; wrong password re-prompts; logout warning without recovery.
- [ ] OAuth account (matrix.org): cross-signing set up after login, identity reset approval page, device sign-out via the account page.
- [ ] Keychain on each OS still holds the session after these flows (no new keychain prompts beyond Phase 1's).
- [ ] Read the Phase 4 security review and accept or reject its residuals.
