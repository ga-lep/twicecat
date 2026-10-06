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
        assert_local_homeserver(values.get("HOMESERVER_URL").map_or("", String::as_str));
        Self(values)
    }

    fn get(&self, key: &str) -> &str {
        self.0
            .get(key)
            .unwrap_or_else(|| panic!("{key} missing from .env.test"))
    }
}

/// Whether `url` is plain http on a loopback address with a port, the only
/// kind of homeserver the test accounts may be used with (AGENTS.md rule 6).
fn is_local_homeserver(url: &str) -> bool {
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
fn assert_local_homeserver(url: &str) {
    assert!(
        is_local_homeserver(url),
        "HOMESERVER_URL must be http://127.0.0.1:<port>, http://localhost:<port> or http://[::1]:<port>; \
         the test accounts must never be used against another server"
    );
}

/// One plain-HTTP JSON request to the local homeserver; returns the body.
/// HTTP/1.0 and `Connection: close`, so the body ends where the stream does.
fn http_json(base: &str, method: &str, path: &str, token: Option<&str>, body: &str) -> String {
    assert_local_homeserver(base);
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

/// Types `text` into the composer through `update`, one editor action per
/// character, like the text editor widget publishes them.
fn type_into_composer(app: &mut App, text: &str) {
    use iced::widget::text_editor::{Action, Edit};
    use tessera_app::composer::EditorAction;
    for c in text.chars() {
        let _ = app.update(Message::Composer(EditorAction(Action::Edit(Edit::Insert(
            c,
        )))));
    }
}

/// Whether the applied timeline holds a message whose rich text is exactly
/// the bold word `word`.
fn timeline_has_bold_message(app: &App, word: &str) -> bool {
    use tessera_core::api::{Block, Inline, TimelineItemKind};
    app.timeline.items.iter().any(|item| match &item.kind {
        TimelineItemKind::Message { details, .. } => matches!(
            details.content.blocks.as_slice(),
            [Block::Paragraph(inlines)] if matches!(
                inlines.as_slice(),
                [Inline::Text { text, style }] if text == word && style.bold
            )
        ),
        _ => false,
    })
}

#[test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
fn typing_markdown_and_pressing_enter_shows_a_bold_message_in_the_timeline() {
    use tessera_app::composer::{Key, KeyInput, bind};
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
                Ok(None) | Err(_) => false,
            }
        })
    };
    let deadline = |secs| std::time::Instant::now() + Duration::from_secs(secs);

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
            "not in Main with a room within 60 s"
        );
        next(&mut app, Duration::from_secs(5));
    }

    // Select a room, type markdown and press Enter, all through `update`.
    let room = app.rooms[0].id.clone();
    let _ = app.update(Message::SelectRoom(room));
    type_into_composer(&mut app, "**hi**");
    let enter = KeyInput {
        key: Key::Enter,
        shift: false,
    };
    let message = bind(enter, app.composer.suggestions.is_open())
        .message()
        .expect("Enter sends when no suggestion list is open");
    let _ = app.update(message);
    assert_eq!(app.composer.text(), "", "the composer resets after sending");

    let limit = deadline(30);
    while !timeline_has_bold_message(&app, "hi") {
        assert!(
            std::time::Instant::now() < limit,
            "no bold `hi` message in the timeline within 30 s"
        );
        next(&mut app, Duration::from_secs(1));
    }

    let _ = app.update(Message::CloseRequested);
    let limit = deadline(10);
    while !app.exit_requested {
        assert!(std::time::Instant::now() < limit, "no CoreClosed in 10 s");
        next(&mut app, Duration::from_secs(1));
    }
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

/// Carol (not alice): confirm-identity, reset with the password, set up
/// recovery, log out, all through `update` only. Nothing is printed.
#[test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
fn carol_resets_her_identity_sets_up_recovery_and_logs_out() {
    use tessera_app::security::{Overlay, SecurityMessage, Visible};
    use tessera_core::api::{DeviceVerification, Password, RecoveryStatus};
    let env = TestEnv::load();
    let base = env.get("HOMESERVER_URL").to_owned();
    let user = env.get("TEST_USER_3").to_owned();
    let password = env.get("TEST_PASSWORD_3").to_owned();

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
                }
                Ok(Some(BridgeEvent::Closed)) => {
                    let _ = app.update(Message::CoreClosed);
                }
                Ok(None) | Err(_) => {}
            }
        })
    };
    // Pumps events until the condition holds; panics after `secs`.
    macro_rules! until {
        ($secs:expr, $what:expr, $cond:expr) => {{
            let limit = std::time::Instant::now() + Duration::from_secs($secs);
            while !($cond) {
                assert!(std::time::Instant::now() < limit, "{}", $what);
                next(&mut app, Duration::from_secs(2));
            }
        }};
    }

    until!(60, "no login screen in 60 s", app.screen == Screen::Login);
    let _ = app.update(Message::UsernameChanged(user));
    let _ = app.update(Message::PasswordChanged(Password::from(password.clone())));
    let _ = app.update(Message::LoginSubmitted);
    until!(
        90,
        "no security status in 90 s",
        app.security.status.device != DeviceVerification::Unknown
    );

    // A new device of an account with an identity sees the screen and the
    // banner. Whatever earlier runs left, the reset below works.
    if app.security.status.device == DeviceVerification::Unverified
        && app.security.status.identity_exists
    {
        assert_eq!(
            app.security.visible(),
            Visible::Overlay(Overlay::ConfirmIdentity)
        );
        assert!(app.security.banner().is_some());
    }

    // Reset: the warning first, then the password when the core asks.
    let _ = app.update(Message::Security(SecurityMessage::ResetRequested));
    let _ = app.update(Message::Security(SecurityMessage::ResetConfirmed));
    let mut answered = false;
    let limit = std::time::Instant::now() + Duration::from_secs(240);
    while app.security.busy.is_some() || app.security.reauth.is_some() {
        assert!(
            std::time::Instant::now() < limit,
            "the reset did not end in 240 s"
        );
        if app.security.visible() == Visible::Reauth && !answered {
            answered = true;
            let _ = app.update(Message::Security(SecurityMessage::ReauthInput(
                Password::from(password.clone()),
            )));
            let _ = app.update(Message::Security(SecurityMessage::ReauthSubmitted));
        }
        next(&mut app, Duration::from_secs(2));
    }
    assert_eq!(app.security.notice, None, "the reset failed");
    assert_eq!(app.security.status.device, DeviceVerification::Verified);

    // Set up recovery: the key arrives on the show-once screen.
    let _ = app.update(Message::Security(SecurityMessage::SetupRecovery));
    until!(
        180,
        "no recovery key in 180 s",
        app.security.visible() == Visible::Key
    );
    let _ = app.update(Message::Security(SecurityMessage::Done));
    assert_eq!(app.security.visible(), Visible::Key, "refused until ticked");
    let _ = app.update(Message::Security(SecurityMessage::SavedToggled(true)));
    let _ = app.update(Message::Security(SecurityMessage::Done));
    assert!(app.security.setup.is_none());
    until!(
        60,
        "recovery not enabled in 60 s",
        app.security.status.recovery == RecoveryStatus::Enabled
    );
    assert!(app.security.setup.is_none());

    let _ = app.update(Message::LogoutRequested);
    until!(60, "no logout in 60 s", app.screen == Screen::Login);
    drop(app);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_loopback_http_urls_count_as_local() {
    for ok in [
        "http://127.0.0.1:8008",
        "http://localhost:8008",
        "http://[::1]:8008",
        "http://127.0.0.1:8008/",
    ] {
        assert!(is_local_homeserver(ok), "{ok}");
    }
    for bad in [
        "https://127.0.0.1:8008",
        "https://matrix.org",
        "http://example.com:8008",
        "http://127.0.0.1",
        "http://127.0.0.1.evil.com:8008",
        "http://localhost:8008@evil.com",
        "http://evil.com:80/@localhost:8008",
        "http://localhost:80/path",
        "http://localhost:",
        "",
    ] {
        assert!(!is_local_homeserver(bad), "{bad}");
    }
}
