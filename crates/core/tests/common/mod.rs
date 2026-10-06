//! Helpers for the homeserver integration tests.
//!
//! Nothing here prints passwords, tokens, or message bodies.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tessera_core::api::{
    Command, CoreEvent, ListDiff, MessageKind, MessageText, OutgoingMessage, Password, RoomId,
    RoomSummary, SecurityStatus, SyncState, TextFormat, TimelineItem, TimelineItemKind,
};
use tessera_core::paths::DataPaths;
use tessera_core::secrets::InMemorySecretStore;
use tessera_core::{Core, CoreConfig, CoreHandle};

/// Default wait for an expected event.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Values from `.env.test`, overridden by environment variables.
pub struct TestEnv {
    values: HashMap<String, String>,
}

impl TestEnv {
    pub fn load() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env.test");
        let text = std::fs::read_to_string(&path).expect("read .env.test at the workspace root");
        let mut values = parse_env(&text);
        for (key, value) in values.iter_mut() {
            if let Ok(over) = std::env::var(key) {
                *value = over;
            }
        }
        let env = Self { values };
        assert_local_homeserver(env.get("HOMESERVER_URL"));
        env
    }

    pub fn get(&self, key: &str) -> &str {
        self.values
            .get(key)
            .unwrap_or_else(|| panic!("{key} missing from .env.test"))
    }

    pub fn homeserver_url(&self) -> &str {
        self.get("HOMESERVER_URL")
    }

    /// Localpart of test user `n` (1-based).
    pub fn user(&self, n: u8) -> &str {
        self.get(&format!("TEST_USER_{n}"))
    }

    /// Password of test user `n` (1-based).
    pub fn password(&self, n: u8) -> &str {
        self.get(&format!("TEST_PASSWORD_{n}"))
    }

    /// Full user ID of test user `n`.
    pub fn user_id(&self, n: u8) -> String {
        format!("@{}:{}", self.user(n), self.get("HOMESERVER_DOMAIN"))
    }
}

/// Whether `url` is plain http on a loopback address with a port, the only
/// kind of homeserver the test accounts may be used with (AGENTS.md rule 6).
pub fn is_local_homeserver(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| {
        rest.strip_prefix(host)
            .and_then(|r| r.strip_prefix(':'))
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// Panics unless `url` is a local homeserver; call before sending credentials.
pub fn assert_local_homeserver(url: &str) {
    assert!(
        is_local_homeserver(url),
        "HOMESERVER_URL must be http://127.0.0.1:<port>, http://localhost:<port> or http://[::1]:<port>; \
         the test accounts must never be used against another server"
    );
}

/// Parses `KEY=VALUE` lines; `#` comments and blank lines are skipped.
pub fn parse_env(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect()
}

/// Starts a core on `dir` against the local homeserver.
pub fn start_core(dir: &Path, secrets: Arc<InMemorySecretStore>) -> CoreHandle {
    start_core_with_page_size(dir, secrets, tessera_core::DEFAULT_ROOM_LIST_PAGE_SIZE)
}

/// Like [`start_core`] with an explicit room list page size.
pub fn start_core_with_page_size(
    dir: &Path,
    secrets: Arc<InMemorySecretStore>,
    room_list_page_size: usize,
) -> CoreHandle {
    let env = TestEnv::load();
    Core::start(
        CoreConfig {
            data: DataPaths::at(dir),
            homeserver_override: Some(env.homeserver_url().to_owned()),
            room_list_page_size,
        },
        secrets,
    )
}

/// Waits for the first event matching `predicate`, ignoring others.
pub async fn expect_event(
    handle: &mut CoreHandle,
    predicate: impl Fn(&CoreEvent) -> bool,
) -> CoreEvent {
    expect_event_within(handle, DEFAULT_TIMEOUT, predicate).await
}

/// Like [`expect_event`] with an explicit timeout.
pub async fn expect_event_within(
    handle: &mut CoreHandle,
    timeout: Duration,
    predicate: impl Fn(&CoreEvent) -> bool,
) -> CoreEvent {
    let wait = async {
        loop {
            match handle.events.recv().await {
                Some(event) if predicate(&event) => return event,
                Some(_) => {}
                None => panic!("event channel closed while waiting for an event"),
            }
        }
    };
    tokio::time::timeout(timeout, wait)
        .await
        .expect("timed out waiting for an event")
}

/// A plain SDK client logged in as test user `n`, for test setup only.
pub async fn raw_login(n: u8) -> matrix_sdk::Client {
    let env = TestEnv::load();
    let client = matrix_sdk::Client::builder()
        .homeserver_url(env.homeserver_url())
        .build()
        .await
        .expect("build raw client");
    client
        .matrix_auth()
        .login_username(env.user(n), env.password(n))
        .initial_device_display_name("tessera-test-setup")
        .await
        .expect("raw login");
    client
}

/// A fresh room name that no other test (or earlier run) uses.
pub fn unique_room_name() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("tessera-test-{hex}")
}

/// Creates a plain (unencrypted) room as `creator` and invites `invitee`
/// when given.
pub async fn create_room(
    creator: &matrix_sdk::Client,
    name: &str,
    invitee: Option<&str>,
) -> matrix_sdk::ruma::OwnedRoomId {
    use matrix_sdk::ruma::api::client::room::create_room::v3::Request;
    let mut request = Request::new();
    request.name = Some(name.to_owned());
    if let Some(user) = invitee {
        request
            .invite
            .push(user.try_into().expect("valid invitee user id"));
    }
    creator
        .create_room(request)
        .await
        .expect("create room")
        .room_id()
        .to_owned()
}

/// Serializes DM creation within a test binary, see [`create_dm`].
static DM_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Creates a DM from `creator` to `with` (a full user ID), using the SDK's
/// own DM helper.
///
/// The SDK marks the room in the `m.direct` account data by reading the
/// client's local copy and writing the whole event back. A test client never
/// syncs, so its copy is empty and every DM it creates would replace the
/// server's `m.direct`, dropping the DMs of tests running at the same time on
/// the same account (those rooms then stay non-DM forever). So the create and
/// a merge with the server's current `m.direct` happen under one lock.
pub async fn create_dm(creator: &matrix_sdk::Client, with: &str) -> matrix_sdk::ruma::OwnedRoomId {
    use matrix_sdk::ruma::events::direct::DirectEventContent;
    let user: matrix_sdk::ruma::OwnedUserId = with.try_into().expect("valid user id");
    let _guard = DM_LOCK.lock().await;
    let account = creator.account();
    let mut direct: DirectEventContent = account
        .fetch_account_data_static::<DirectEventContent>()
        .await
        .expect("fetch m.direct")
        .map(|raw| raw.deserialize().expect("parse m.direct"))
        .unwrap_or_default();
    let room_id = creator
        .create_dm(&user)
        .await
        .expect("create dm")
        .room_id()
        .to_owned();
    let rooms = direct.entry(user.into()).or_default();
    if !rooms.contains(&room_id) {
        rooms.push(room_id.clone());
    }
    account
        .set_account_data(direct)
        .await
        .expect("restore merged m.direct");
    room_id
}

/// Creates a space named `name` as `creator`.
pub async fn create_space(
    creator: &matrix_sdk::Client,
    name: &str,
) -> matrix_sdk::ruma::OwnedRoomId {
    use matrix_sdk::ruma::api::client::room::create_room::v3::{CreationContent, Request};
    use matrix_sdk::ruma::room::RoomType;
    use matrix_sdk::ruma::serde::Raw;
    let mut content = CreationContent::new();
    content.room_type = Some(RoomType::Space);
    let mut request = Request::new();
    request.name = Some(name.to_owned());
    request.creation_content = Some(Raw::new(&content).expect("serialize creation content"));
    creator
        .create_room(request)
        .await
        .expect("create space")
        .room_id()
        .to_owned()
}

/// Sets the global display name of the account `client` is logged in as.
pub async fn set_display_name(client: &matrix_sdk::Client, name: &str) {
    client
        .account()
        .set_display_name(Some(name))
        .await
        .expect("set display name");
}

/// Sends a plain `m.text` message to `room` as `client` and returns the
/// event ID. Works on a client that never synced. Prints nothing.
pub async fn send_text(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::ruma::RoomId,
    body: &str,
) -> matrix_sdk::ruma::OwnedEventId {
    use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
    send_content(client, room, &RoomMessageEventContent::text_plain(body)).await
}

/// Sends `content` to `room` as `client` and returns the event ID.
pub async fn send_content(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::ruma::RoomId,
    content: &matrix_sdk::ruma::events::room::message::RoomMessageEventContent,
) -> matrix_sdk::ruma::OwnedEventId {
    use matrix_sdk::ruma::TransactionId;
    use matrix_sdk::ruma::api::client::message::send_message_event::v3::Request;
    let request = Request::new(room.to_owned(), TransactionId::new(), content)
        .expect("serialize message content");
    client.send(request).await.expect("send message").event_id
}

/// Serializes the E2EE tests of one test binary (Phase 4 Q2).
static E2EE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Takes the per-binary E2EE lock; hold the guard for the whole test.
///
/// E2EE tests reset the crypto state of the fixed accounts, so two of them must
/// never overlap. Cargo runs test binaries one at a time, so one lock per
/// binary is enough. A test that panics while holding the guard does not block
/// the next one (tokio's mutex does not poison).
pub async fn e2ee_serial() -> tokio::sync::MutexGuard<'static, ()> {
    E2EE_LOCK.lock().await
}

/// Like [`raw_login`] with a chosen device display name. The caller logs the
/// client out, so no device is left behind.
pub async fn raw_login_named(n: u8, device_name: &str) -> matrix_sdk::Client {
    let env = TestEnv::load();
    assert_local_homeserver(env.homeserver_url());
    let client = matrix_sdk::Client::builder()
        .homeserver_url(env.homeserver_url())
        .build()
        .await
        .expect("build raw client");
    client
        .matrix_auth()
        .login_username(env.user(n), env.password(n))
        .initial_device_display_name(device_name)
        .await
        .expect("raw login");
    client
}

/// Password UIA data for test user `n`, for the session in `info`.
fn password_auth(
    env: &TestEnv,
    n: u8,
    session: Option<String>,
) -> matrix_sdk::ruma::api::client::uiaa::AuthData {
    use matrix_sdk::ruma::api::client::uiaa::{AuthData, MatrixUserIdentifier, Password};
    let mut password = Password::new(
        MatrixUserIdentifier::new(env.user(n).to_owned()).into(),
        env.password(n).to_owned(),
    );
    password.session = session;
    AuthData::Password(password)
}

/// Resets test user `n` to the Q2 baseline B0 and returns the client used, still
/// logged in (the caller must log it out), with the number of other devices
/// removed.
///
/// B0: a fresh cross-signing identity whose private keys nobody else holds, no
/// backup on the server, secret storage (4S) off, and no device but the
/// returned client's. The client is built without encryption settings, so
/// nothing runs automatically.
async fn reset_to_b0(n: u8) -> (matrix_sdk::Client, usize) {
    use matrix_sdk::encryption::CrossSigningResetAuthType;
    let env = TestEnv::load();
    assert_local_homeserver(env.homeserver_url());
    let client = raw_login_named(n, "tessera-e2ee-reset").await;
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;

    // 1. Sign out every other device.
    let own = client.device_id().expect("logged in").to_owned();
    let others: Vec<_> = client
        .devices()
        .await
        .expect("list devices")
        .devices
        .into_iter()
        .map(|d| d.device_id)
        .filter(|id| *id != own)
        .collect();
    let removed = others.len();
    if !others.is_empty()
        && let Err(error) = client.delete_devices(&others, None).await
    {
        let session = error
            .as_uiaa_response()
            .expect("device deletion fails only for user-interactive auth")
            .session
            .clone();
        client
            .delete_devices(&others, Some(password_auth(&env, n, session)))
            .await
            .expect("delete other devices with the password");
    }

    // 2. Delete every backup, turn 4S off, replace the identity.
    if let Some(handle) = client
        .encryption()
        .recovery()
        .reset_identity()
        .await
        .expect("reset identity")
    {
        match handle.auth_type() {
            CrossSigningResetAuthType::Uiaa(info) => handle
                .reset(Some(password_auth(&env, n, info.session.clone())))
                .await
                .expect("reset identity with the password"),
            _ => panic!("the local homeserver asked for a non-password reset"),
        }
    }

    // 3. Postconditions, on the same client.
    let me = client.user_id().expect("logged in").to_owned();
    assert!(
        !client
            .encryption()
            .backups()
            .fetch_exists_on_server()
            .await
            .expect("fetch backup state"),
        "a backup survived the reset"
    );
    assert!(
        !client
            .encryption()
            .secret_storage()
            .is_enabled()
            .await
            .expect("fetch 4S state"),
        "4S is still on after the reset"
    );
    assert!(
        client
            .encryption()
            .request_user_identity(&me)
            .await
            .expect("query own identity")
            .is_some(),
        "no own identity after the reset"
    );
    assert_eq!(
        client.devices().await.expect("list devices").devices.len(),
        1,
        "other devices survived the reset"
    );
    (client, removed)
}

/// Resets test user `n` to baseline B0 (see [`reset_to_b0`]) and logs the
/// resetting device out. Prints the account name and the number of devices
/// removed.
pub async fn reset_e2ee_account(n: u8) {
    let env = TestEnv::load();
    assert_local_homeserver(env.homeserver_url());
    let (client, removed) = reset_to_b0(n).await;
    client.logout().await.expect("log the reset device out");
    println!("e2ee reset {}: {removed} devices removed", env.user(n));
}

/// Resets test user `n` to baseline B1: like B0, then recovery is enabled
/// (4S on, a matching backup), `passphrase` protecting the key when given. No
/// device is left. Returns the recovery key; prints the account name and the
/// number of devices removed.
pub async fn seed_recovery(n: u8, passphrase: Option<&str>) -> zeroize::Zeroizing<String> {
    let env = TestEnv::load();
    assert_local_homeserver(env.homeserver_url());
    let (client, removed) = reset_to_b0(n).await;
    let recovery = client.encryption().recovery();
    let enable = recovery.enable();
    let key = match passphrase {
        Some(passphrase) => enable.with_passphrase(passphrase).await,
        None => enable.await,
    }
    .expect("enable recovery");
    client.logout().await.expect("log the reset device out");
    println!("e2ee seed {}: {removed} devices removed", env.user(n));
    zeroize::Zeroizing::new(key)
}

/// Creates an encrypted room (initial `m.room.encryption` state,
/// `m.megolm.v1.aes-sha2`) as `creator` and invites `invitee` when given.
pub async fn create_encrypted_room(
    creator: &matrix_sdk::Client,
    name: &str,
    invitee: Option<&str>,
) -> matrix_sdk::ruma::OwnedRoomId {
    use matrix_sdk::ruma::api::client::room::create_room::v3::Request;
    use matrix_sdk::ruma::events::InitialStateEvent;
    use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
    let mut request = Request::new();
    request.name = Some(name.to_owned());
    request.initial_state = vec![
        InitialStateEvent::with_empty_state_key(
            RoomEncryptionEventContent::with_recommended_defaults(),
        )
        .to_raw_any(),
    ];
    if let Some(user) = invitee {
        request
            .invite
            .push(user.try_into().expect("valid invitee user id"));
    }
    creator
        .create_room(request)
        .await
        .expect("create encrypted room")
        .room_id()
        .to_owned()
}

/// The `type` of the event `event_id` in `room`, read raw from the server
/// (works on a client that never synced).
pub async fn fetch_raw_event_type(
    client: &matrix_sdk::Client,
    room: &matrix_sdk::ruma::RoomId,
    event_id: &matrix_sdk::ruma::EventId,
) -> String {
    use matrix_sdk::ruma::api::client::room::get_room_event::v3::Request;
    let event = client
        .send(Request::new(room.to_owned(), event_id.to_owned()))
        .await
        .expect("fetch raw event")
        .event;
    event
        .get_field::<String>("type")
        .expect("event type parses")
        .expect("event has a type")
}

/// Alice and carol (both reset to B0) in a fresh encrypted room, each with a
/// running core that has the room open.
pub struct EncryptedPair {
    pub room_id: matrix_sdk::ruma::OwnedRoomId,
    pub alice: CoreHandle,
    pub carol: CoreHandle,
    pub alice_rooms: Vec<RoomSummary>,
    pub carol_rooms: Vec<RoomSummary>,
    pub alice_items: Vec<TimelineItem>,
    pub carol_items: Vec<TimelineItem>,
    pub _dirs: [tempfile::TempDir; 2],
}

pub fn hex() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn outgoing(room: &str, text: &str) -> OutgoingMessage {
    OutgoingMessage {
        room_id: RoomId::from(room),
        text: MessageText::from(text),
        mentions: Vec::new(),
        reply_to: None,
        kind: MessageKind::Text,
        format: TextFormat::Markdown,
    }
}

pub async fn login_and_run(core: &mut CoreHandle, env: &TestEnv, n: u8) {
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    expect_event(core, |e| {
        matches!(e, CoreEvent::SyncState(SyncState::Running))
    })
    .await;
}

/// Applies room list and timeline events until `done` holds.
pub async fn pump(
    core: &mut CoreHandle,
    room: &str,
    rooms: &mut Vec<RoomSummary>,
    items: &mut Vec<TimelineItem>,
    done: impl Fn(&[RoomSummary], &[TimelineItem]) -> bool,
) {
    let wait = async {
        while !done(rooms, items) {
            match core.events.recv().await {
                Some(CoreEvent::RoomListChanged(diffs)) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, rooms);
                    }
                }
                Some(CoreEvent::TimelineDiff { room_id, diffs }) if room_id.as_str() == room => {
                    for diff in diffs {
                        ListDiff::apply(&diff, items);
                    }
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for the core");
}

impl EncryptedPair {
    pub async fn new() -> Self {
        reset_e2ee_account(1).await;
        reset_e2ee_account(3).await;
        let env = TestEnv::load();
        let alice_raw = raw_login_named(1, "tessera-e2ee-setup").await;
        let carol_raw = raw_login_named(3, "tessera-e2ee-setup").await;
        let room_id =
            create_encrypted_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3))).await;
        carol_raw
            .join_room_by_id(&room_id)
            .await
            .expect("carol joins the room");
        alice_raw.logout().await.expect("alice setup logout");
        carol_raw.logout().await.expect("carol setup logout");

        let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let mut alice = start_core(dirs[0].path(), Arc::new(InMemorySecretStore::new()));
        let mut carol = start_core(dirs[1].path(), Arc::new(InMemorySecretStore::new()));
        login_and_run(&mut alice, &env, 1).await;
        login_and_run(&mut carol, &env, 3).await;
        let room = room_id.to_string();
        let mut pair = Self {
            room_id,
            alice,
            carol,
            alice_rooms: Vec::new(),
            carol_rooms: Vec::new(),
            alice_items: Vec::new(),
            carol_items: Vec::new(),
            _dirs: dirs,
        };
        pump(
            &mut pair.alice,
            &room,
            &mut pair.alice_rooms,
            &mut pair.alice_items,
            |r, _| r.iter().any(|x| x.id.as_str() == room),
        )
        .await;
        pump(
            &mut pair.carol,
            &room,
            &mut pair.carol_rooms,
            &mut pair.carol_items,
            |r, _| r.iter().any(|x| x.id.as_str() == room),
        )
        .await;
        for core in [&mut pair.alice, &mut pair.carol] {
            core.commands
                .send(Command::SelectRoom(RoomId::from(room.as_str())))
                .await
                .unwrap();
        }
        pair
    }

    pub fn room(&self) -> String {
        self.room_id.to_string()
    }

    pub async fn finish(mut self) {
        for core in [&mut self.alice, &mut self.carol] {
            core.commands.send(Command::Logout).await.unwrap();
            expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
        }
    }
}

pub fn message_with<'a>(
    items: &'a [TimelineItem],
    text: &str,
    own: bool,
) -> Option<&'a TimelineItem> {
    items.iter().find(|i| {
        matches!(&i.kind, TimelineItemKind::Message { body, is_own, .. }
            if body == text && *is_own == own)
    })
}

/// Waits for the first `Security` event whose status satisfies `predicate`,
/// ignoring every other event.
pub async fn expect_security(
    core: &mut CoreHandle,
    predicate: impl Fn(&SecurityStatus) -> bool,
) -> SecurityStatus {
    match expect_event(
        core,
        |e| matches!(e, CoreEvent::Security(s) if predicate(s)),
    )
    .await
    {
        CoreEvent::Security(status) => status,
        _ => unreachable!("the predicate only accepts Security events"),
    }
}

/// On a logged-in core, resets the identity with test user `n`'s password and
/// waits for the first-device status: the identity is complete on this
/// device, the device is verified, and recovery is off. The reset deletes
/// every backup and the SDK creates a fresh one (Q4 settings).
pub async fn reset_identity_on(core: &mut CoreHandle, n: u8) {
    use tessera_core::api::{CrossSigning, DeviceVerification, RecoveryStatus};
    let env = TestEnv::load();
    core.commands
        .send(Command::ResetIdentity {
            password: Some(Password::from(env.password(n))),
        })
        .await
        .unwrap();
    expect_security(core, |s| {
        s.cross_signing == CrossSigning::Complete
            && s.device == DeviceVerification::Verified
            && s.recovery == RecoveryStatus::Disabled
    })
    .await;
}

/// Logs `core` in as test user `n` on baseline B0 and makes it the account's
/// first device (Q2): see [`reset_identity_on`]. Nothing is synced yet.
pub async fn first_device(core: &mut CoreHandle, n: u8) {
    let env = TestEnv::load();
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    reset_identity_on(core, n).await;
}
