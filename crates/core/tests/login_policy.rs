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
            max_upload_bytes_override: None,
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

const STACK_DEFAULTS: &str = "\
# comment
TESSERA_ADDRESS=127.0.0.1
TESSERA_SYNAPSE_PORT=8008
TESSERA_LK_JWT_PORT=8080
TESSERA_LIVEKIT_PORT=7880
TESSERA_LK_JWT_PORT_B=8081
TESSERA_LIVEKIT_PORT_B=7890
TEST_USER_1=alice
";

#[test]
fn stack_urls_are_derived_from_the_address_and_ports() {
    let v = common::stack_env::resolve(STACK_DEFAULTS, None, |_| None);
    assert_eq!(v["HOMESERVER_URL"], "http://127.0.0.1:8008");
    assert_eq!(v["LK_JWT_URL"], "http://127.0.0.1:8080");
    assert_eq!(v["LIVEKIT_URL"], "ws://127.0.0.1:7880");
    assert_eq!(v["LK_JWT_URL_B"], "http://127.0.0.1:8081");
    assert_eq!(v["LIVEKIT_URL_B"], "ws://127.0.0.1:7890");
    assert_eq!(v["TEST_USER_1"], "alice");
}

#[test]
fn the_environment_wins_over_local_settings_which_win_over_the_defaults() {
    let local = "TESSERA_LK_JWT_PORT=8090\nTESSERA_SYNAPSE_PORT=8018\n";
    let env = |key: &str| (key == "TESSERA_SYNAPSE_PORT").then(|| "8028".to_owned());
    let v = common::stack_env::resolve(STACK_DEFAULTS, Some(local), env);
    assert_eq!(v["LK_JWT_URL"], "http://127.0.0.1:8090", "from .env.local");
    assert_eq!(
        v["HOMESERVER_URL"], "http://127.0.0.1:8028",
        "from the environment"
    );
    assert_eq!(v["LIVEKIT_URL"], "ws://127.0.0.1:7880", "default");
}

#[test]
fn a_url_set_in_the_environment_is_kept_and_ipv6_is_bracketed() {
    let env = |key: &str| match key {
        "TESSERA_ADDRESS" => Some("::1".to_owned()),
        "LIVEKIT_URL" => Some("ws://localhost:7999".to_owned()),
        _ => None,
    };
    let v = common::stack_env::resolve(STACK_DEFAULTS, None, env);
    assert_eq!(v["HOMESERVER_URL"], "http://[::1]:8008");
    assert!(common::is_local_homeserver(&v["HOMESERVER_URL"]));
    assert_eq!(v["LIVEKIT_URL"], "ws://localhost:7999");
}
