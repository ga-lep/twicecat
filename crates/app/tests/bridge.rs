//! Drives the real bridge and `App::update` headlessly against the local
//! homeserver (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Nothing here prints passwords, tokens, or message bodies.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iced::futures::StreamExt as _;
use tessera_app::bridge::{BridgeEvent, events_stream};
use tessera_app::{App, Bridge, Message, Screen, Startup};
use tessera_core::CoreConfig;
use tessera_core::paths::DataPaths;
use tessera_core::secrets::InMemorySecretStore;

struct TestEnv(HashMap<String, String>);

impl TestEnv {
    fn load() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env.test");
        let text = std::fs::read_to_string(path).expect("read .env.test at the workspace root");
        let mut values: HashMap<String, String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .collect();
        for (key, value) in values.iter_mut() {
            if let Ok(over) = std::env::var(key) {
                *value = over;
            }
        }
        Self(values)
    }

    fn get(&self, key: &str) -> &str {
        self.0
            .get(key)
            .unwrap_or_else(|| panic!("{key} missing from .env.test"))
    }
}

/// One plain-HTTP JSON request to the local homeserver; returns the body.
/// HTTP/1.0 and `Connection: close`, so the body ends where the stream does.
fn http_json(base: &str, method: &str, path: &str, token: Option<&str>, body: &str) -> String {
    let authority = base
        .strip_prefix("http://")
        .expect("the test homeserver is plain http")
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(authority).expect("connect to the local homeserver");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("set read timeout");
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.0\r\nHost: {authority}\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("send the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");
    assert!(
        response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),
        "{method} {path} failed: {}",
        response.lines().next().unwrap_or("")
    );
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default()
}

/// Makes sure the account has a room, using a short-lived raw login.
fn ensure_one_room(base: &str, user: &str, password: &str) {
    let login = login_body(user, password);
    let body = http_json(base, "POST", "/_matrix/client/v3/login", None, &login);
    let token = body
        .split("\"access_token\"")
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .expect("login response has an access token")
        .to_owned();
    http_json(
        base,
        "POST",
        "/_matrix/client/v3/createRoom",
        Some(&token),
        r#"{"name":"app bridge test"}"#,
    );
    http_json(
        base,
        "POST",
        "/_matrix/client/v3/logout",
        Some(&token),
        "{}",
    );
}

/// The values come from `.env.test` (local throwaway accounts: no quotes or
/// backslashes), so plain formatting is enough.
fn login_body(user: &str, password: &str) -> String {
    assert!(!user.contains(['"', '\\']) && !password.contains(['"', '\\']));
    format!(
        r#"{{"type":"m.login.password","identifier":{{"type":"m.id.user","user":"{user}"}},"password":"{password}"}}"#
    )
}

fn scratch_dir() -> PathBuf {
    use std::hash::{BuildHasher as _, Hasher as _};
    let id = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    std::env::temp_dir().join(format!("tessera-app-bridge-test-{id:016x}"))
}

#[test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
fn login_reaches_main_with_rooms_and_shutdown_closes_the_bridge() {
    let env = TestEnv::load();
    let base = env.get("HOMESERVER_URL").to_owned();
    let user = env.get("TEST_USER_1").to_owned();
    let password = env.get("TEST_PASSWORD_1").to_owned();
    ensure_one_room(&base, &user, &password);

    let dir = scratch_dir();
    let (bridge, slot) = Bridge::start(
        CoreConfig {
            data: DataPaths::at(&dir),
            homeserver_override: Some(base),
            room_list_page_size: tessera_core::DEFAULT_ROOM_LIST_PAGE_SIZE,
        },
        Arc::new(InMemorySecretStore::new()),
    )
    .expect("start the bridge");
    let (mut app, _boot) = App::new(Startup {
        sink: Box::new(bridge),
        events: slot.clone(),
    });

    // The test thread only pumps events; the core runs on the bridge's runtime.
    let pump = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("pump runtime");
    let mut events = events_stream(&slot).boxed();
    let mut next = |app: &mut App, wait: Duration| {
        pump.block_on(async {
            match tokio::time::timeout(wait, events.next()).await {
                Ok(Some(BridgeEvent::Core(event))) => {
                    let _ = app.update(Message::Core(Box::new(event)));
                    true
                }
                Ok(Some(BridgeEvent::Closed)) => {
                    let _ = app.update(Message::CoreClosed);
                    true
                }
                Ok(None) => false,
                Err(_) => false,
            }
        })
    };
    let deadline = |secs| std::time::Instant::now() + Duration::from_secs(secs);

    // Restoring finds no session; log in as alice.
    let limit = deadline(60);
    while app.screen != Screen::Login {
        assert!(std::time::Instant::now() < limit, "no login screen in 60 s");
        next(&mut app, Duration::from_secs(5));
    }
    let _ = app.update(Message::UsernameChanged(user));
    let _ = app.update(Message::PasswordChanged(tessera_core::api::Password::from(
        password,
    )));
    let _ = app.update(Message::LoginSubmitted);

    let limit = deadline(60);
    while !(app.screen == Screen::Main && !app.rooms.is_empty()) {
        assert!(
            std::time::Instant::now() < limit,
            "not in Main with a room within 60 s (screen {:?}, {} rooms)",
            app.screen,
            app.rooms.len()
        );
        next(&mut app, Duration::from_secs(5));
    }

    // Closing the window shuts the core down; its channel closes.
    let _ = app.update(Message::CloseRequested);
    assert_eq!(app.screen, Screen::ShuttingDown);
    let limit = deadline(10);
    while !app.exit_requested {
        assert!(std::time::Instant::now() < limit, "no CoreClosed in 10 s");
        next(&mut app, Duration::from_secs(1));
    }

    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}
