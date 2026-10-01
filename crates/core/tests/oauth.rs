//! OAuth login against the local homeserver. The local Synapse has no MAS, so
//! only the "not supported" path can run here; the real browser flow is on the
//! human checkpoint list.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TestEnv, start_core};
use tessera_core::api::{Command, CoreError, CoreEvent};
use tessera_core::secrets::{InMemorySecretStore, SecretStore as _};

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn oauth_against_a_server_without_mas_is_not_supported() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());

    core.commands
        .send(Command::LoginOAuth {
            server: env.homeserver_url().to_owned(),
        })
        .await
        .unwrap();

    // The first event that is either a failure or an OAuthUrl decides.
    let wait = async {
        loop {
            match core.events.recv().await.expect("event channel closed") {
                CoreEvent::LoginFailed(e) => return e,
                CoreEvent::OAuthUrl(_) => panic!("no OAuthUrl may be emitted without MAS"),
                _ => {}
            }
        }
    };
    let err = tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("LoginFailed within 30 s");
    assert_eq!(err, CoreError::OAuthNotSupported);

    assert!(secrets.get(&env.user_id(1)).unwrap().is_none());
    assert!(!dir.path().join("last_account").exists());
    let sessions = dir.path().join("sessions");
    assert!(
        !sessions.exists() || std::fs::read_dir(&sessions).unwrap().count() == 0,
        "half-created store dir must be removed"
    );

    // Cancelling with nothing pending is a no-op.
    core.commands.send(Command::CancelOAuth).await.unwrap();
    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}
}
