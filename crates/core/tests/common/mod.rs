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
    let env = TestEnv::load();
    Core::start(
        CoreConfig {
            data: DataPaths::at(dir),
            homeserver_override: Some(env.homeserver_url().to_owned()),
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
