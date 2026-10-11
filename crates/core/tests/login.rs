//! Password login, session persistence, restore, and logout against the local
//! homeserver (`scripts/stack.sh up -d --wait && scripts/create-test-users.sh`).

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{TestEnv, expect_event, raw_login, start_core};
use twicecat_core::api::{Command, CoreEvent, Password};
use twicecat_core::secrets::{InMemorySecretStore, SecretStore as _};

fn login_cmd(env: &TestEnv, n: u8, password: &str) -> Command {
    Command::LoginPassword {
        username: env.user(n).to_owned(),
        password: Password::from(password),
        server: None,
    }
}

fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    // Only a vanished entry (SQLite WAL churn) is tolerated; anything else
    // would hide files from the no-token scan.
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => panic!("read_dir {}: {e}", dir.display()),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => panic!("dir entry in {}: {e}", dir.display()),
        };
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn password_login_and_wrong_password() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());

    core.commands
        .send(login_cmd(&env, 1, "definitely-the-wrong-password"))
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| matches!(e, CoreEvent::LoginFailed(_))).await;
    assert!(matches!(event, CoreEvent::LoginFailed(_)));
    assert!(secrets.get(&env.user_id(1)).unwrap().is_none());
    assert!(!dir.path().join("last_account").exists());
    assert!(
        !dir.path().join("sessions").exists()
            || std::fs::read_dir(dir.path().join("sessions"))
                .unwrap()
                .count()
                == 0,
        "half-created store dir must be removed"
    );

    core.commands
        .send(login_cmd(&env, 1, env.password(1)))
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    let CoreEvent::LoggedIn {
        user_id, restored, ..
    } = event
    else {
        unreachable!()
    };
    assert_eq!(user_id.as_str(), "@alice:localhost");
    assert!(!restored);
    assert!(secrets.get(&env.user_id(1)).unwrap().is_some());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("last_account")).unwrap(),
        "@alice:localhost"
    );

    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn restore_without_logging_in_again() {
    let env = TestEnv::load();
    // Bob is used only by this test, so the device count is stable.
    let admin = raw_login(2).await;
    let before = admin.devices().await.unwrap().devices.len();

    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());
    core.commands
        .send(login_cmd(&env, 2, env.password(2)))
        .await
        .unwrap();
    let CoreEvent::LoggedIn {
        device_id: first_device,
        restored: false,
        ..
    } = expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await
    else {
        panic!("expected a fresh login");
    };
    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}

    let mut core = start_core(dir.path(), secrets);
    core.commands.send(Command::RestoreSession).await.unwrap();
    let CoreEvent::LoggedIn {
        device_id,
        restored,
        user_id,
    } = expect_event(&mut core, |e| {
        matches!(e, CoreEvent::LoggedIn { .. } | CoreEvent::NoSession)
    })
    .await
    else {
        panic!("expected the session to be restored");
    };
    assert!(restored);
    assert_eq!(user_id.as_str(), env.user_id(2));
    assert_eq!(device_id, first_device);

    let after = admin.devices().await.unwrap().devices.len();
    assert_eq!(after, before + 1, "only the first login created a device");

    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn no_token_on_disk() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());
    core.commands
        .send(login_cmd(&env, 3, env.password(3)))
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}

    let record = secrets
        .get(&env.user_id(3))
        .unwrap()
        .expect("record stored");
    let json: serde_json::Value = serde_json::from_slice(&record).unwrap();
    let token = json["auth"]["Matrix"]["access_token"]
        .as_str()
        .expect("access token in record");
    let passphrase = json["store_passphrase"].as_str().expect("passphrase");
    assert!(token.len() > 8);

    let mut files = Vec::new();
    files_under(dir.path(), &mut files);
    assert!(!files.is_empty());
    for file in files {
        // SQLite journal/WAL files can vanish while the SDK closes the store
        // in the background; a file that is gone holds nothing to find.
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => panic!("read {}: {e}", file.display()),
        };
        assert!(!contains(&bytes, token.as_bytes()), "token found on disk");
        assert!(
            !contains(&bytes, passphrase.as_bytes()),
            "passphrase on disk"
        );
    }
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn logout_removes_everything() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());
    core.commands
        .send(login_cmd(&env, 3, env.password(3)))
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    let sessions = dir.path().join("sessions");
    assert_eq!(std::fs::read_dir(&sessions).unwrap().count(), 1);

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
    assert!(secrets.get(&env.user_id(3)).unwrap().is_none());
    assert!(!dir.path().join("last_account").exists());
    assert!(
        !sessions.exists() || std::fs::read_dir(&sessions).unwrap().count() == 0,
        "store dir must be gone"
    );

    core.commands.send(Command::RestoreSession).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::NoSession)).await;
    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}
}
