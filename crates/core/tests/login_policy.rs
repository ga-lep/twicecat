//! Login policy checks that need no homeserver (not `#[ignore]`d).

mod common;

use std::sync::Arc;

use common::expect_event;
use tessera_core::api::{Command, CoreError, CoreEvent, Password};
use tessera_core::paths::DataPaths;
use tessera_core::secrets::{InMemorySecretStore, SecretStore as _};
use tessera_core::{Core, CoreConfig};

/// Needs no homeserver: the policy check must reject the URL before any
/// request is made (the host cannot resolve, so a network attempt would
/// produce a different error message).
#[tokio::test]
async fn http_non_loopback_homeserver_is_refused_before_login() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = Core::start(
        CoreConfig {
            data: DataPaths::at(dir.path()),
            homeserver_override: Some("http://matrix.example.invalid".to_owned()),
            room_list_page_size: tessera_core::DEFAULT_ROOM_LIST_PAGE_SIZE,
            audio: Default::default(),
            call_service_override: None,
        },
        secrets.clone(),
    );
    core.commands
        .send(Command::LoginPassword {
            username: "@alice:example.invalid".to_owned(),
            password: Password::from("never-sent"),
            server: None,
        })
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| matches!(e, CoreEvent::LoginFailed(_))).await;
    let CoreEvent::LoginFailed(err) = event else {
        unreachable!()
    };
    assert!(
        matches!(&err, CoreError::Login(m) if m == "the homeserver must use https"),
        "unexpected error: {err}"
    );
    assert!(secrets.get("@alice:example.invalid").unwrap().is_none());
    assert!(!dir.path().join("last_account").exists());
    assert!(
        !dir.path().join("sessions").exists()
            || std::fs::read_dir(dir.path().join("sessions"))
                .unwrap()
                .count()
                == 0,
        "half-created store dir must be removed"
    );
    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}
}

#[test]
fn only_loopback_http_urls_count_as_local() {
    for ok in [
        "http://127.0.0.1:8008",
        "http://localhost:8008",
        "http://[::1]:8008",
        "http://127.0.0.1:8008/",
    ] {
        assert!(common::is_local_homeserver(ok), "{ok}");
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
        assert!(!common::is_local_homeserver(bad), "{bad}");
    }
}
