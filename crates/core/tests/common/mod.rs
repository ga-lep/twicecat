//! Helpers for the homeserver integration tests.
//!
//! Nothing here prints passwords, tokens, or message bodies.

// Each test file uses a different subset of these helpers.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tessera_core::api::CoreEvent;
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
        Self { values }
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
