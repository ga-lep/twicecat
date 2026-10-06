//! Phase 4: security status, cross-signing bootstrap, identity reset and
//! re-authentication in the core, against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Uses only alice and carol, reset to a known baseline at the start of every
//! test (plan Q2). Prints nothing but account names and counts.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    EncryptedPair, TestEnv, e2ee_serial, expect_event, expect_security, first_device, hex,
    message_with, outgoing, pump, raw_login_named, reset_e2ee_account, reset_identity_on,
    seed_recovery, start_core,
};
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, Command, CoreError, CoreEvent, CrossSigning, DeviceVerification, Password,
    ReauthAction, RecoveryStatus, TimelineItemKind,
};
use tessera_core::secrets::InMemorySecretStore;

/// The master key's public part, and whether a backup exists on the server,
/// as a fresh raw client (no encryption settings) sees them for test user `n`.
async fn server_view(n: u8) -> (String, bool) {
    let client = raw_login_named(n, "tessera-e2ee-view").await;
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    let me = client.user_id().expect("logged in").to_owned();
    let master = client
        .encryption()
        .request_user_identity(&me)
        .await
        .expect("query own identity")
        .expect("the account has an identity")
        .master_key()
        .get_first_key()
        .expect("master key has a key")
        .to_base64();
    let backup = client
        .encryption()
        .backups()
        .fetch_exists_on_server()
        .await
        .expect("fetch backup state");
    client.logout().await.expect("log the view client out");
    (master, backup)
}

/// Starts a core on a fresh directory and logs it in as test user `n`.
async fn login(n: u8) -> (CoreHandle, tempfile::TempDir, Arc<InMemorySecretStore>) {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    (core, dir, secrets)
}

async fn logout(mut core: CoreHandle) {
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn status_on_a_fresh_baseline_account() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let (mut core, _dir, _secrets) = login(1).await;
    let status = expect_security(&mut core, |s| {
        s.cross_signing == CrossSigning::MissingKeys
            && s.device == DeviceVerification::Unverified
            && s.identity_exists
            && s.recovery == RecoveryStatus::Disabled
    })
    .await;
    assert!(
        !status.has_devices_to_verify_against,
        "B0 has no other device that is cross-signed"
    );
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reset_needs_the_password_and_replaces_the_identity() {
    let _serial = e2ee_serial().await;
    // B1: a backup exists, which a reset deletes before it asks for the
    // password (the core must not start one without it).
    let _recovery_key = seed_recovery(1, None).await;
    let (mut core, _dir, _secrets) = login(1).await;
    expect_security(&mut core, |s| {
        s.cross_signing == CrossSigning::MissingKeys && s.identity_exists
    })
    .await;
    let (master_before, backup_before) = server_view(1).await;
    assert!(backup_before, "the baseline has a backup");

    // No password: the core asks for one.
    core.commands
        .send(Command::ResetIdentity { password: None })
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| {
        matches!(
            e,
            CoreEvent::ReauthRequired { .. } | CoreEvent::ActionFailed { .. }
        )
    })
    .await;
    assert!(
        matches!(
            event,
            CoreEvent::ReauthRequired {
                action: ReauthAction::ResetIdentity
            }
        ),
        "{event:?}"
    );
    // The refusal came before anything was touched: the backup is still there.
    let (master_now, backup_now) = server_view(1).await;
    assert!(backup_now, "a password-less reset deleted the backup");
    assert_eq!(master_before, master_now);

    // A wrong password is refused.
    core.commands
        .send(Command::ResetIdentity {
            password: Some(Password::from("definitely-the-wrong-password")),
        })
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| {
        matches!(
            e,
            CoreEvent::ReauthRequired { .. } | CoreEvent::ActionFailed { .. }
        )
    })
    .await;
    assert!(
        matches!(
            event,
            CoreEvent::ActionFailed {
                action: ActionKind::CrossSigning,
                error: CoreError::ReauthFailed,
                ..
            }
        ),
        "{event:?}"
    );

    // The right password replaces the identity.
    reset_identity_on(&mut core, 1).await;
    // The status can turn complete before the upload ends, so the server is
    // asked until it shows the new key and the automatic backup (Q4).
    let wait = async {
        loop {
            let (master, backup) = server_view(1).await;
            if master != master_before && backup {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    tokio::time::timeout(common::DEFAULT_TIMEOUT, wait)
        .await
        .expect("the server shows a new identity and a backup");
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn bootstrap_is_a_no_op_when_an_identity_exists() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let (mut core, _dir, _secrets) = login(1).await;
    expect_security(&mut core, |s| s.identity_exists).await;
    let (master_before, _) = server_view(1).await;

    core.commands
        .send(Command::BootstrapCrossSigning { password: None })
        .await
        .unwrap();
    // The action has no success event; it must not ask for a password or fail.
    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match core.events.recv().await {
                Some(e @ (CoreEvent::ReauthRequired { .. } | CoreEvent::ActionFailed { .. })) => {
                    return Some(e);
                }
                Some(_) => {}
                None => return None,
            }
        }
    })
    .await;
    assert!(outcome.is_err(), "unexpected answer: {outcome:?}");
    let (master_after, _) = server_view(1).await;
    assert_eq!(master_before, master_after, "the identity was replaced");
    logout(core).await;
}

/// Resets the identity and expects the whole identity here, and no loss of the
/// private keys once the sync is back (a sync-driven keys query answered with
/// the old identity drops them; the reset stops the sync to prevent it).
async fn reset_and_expect_complete(core: &mut CoreHandle, n: u8) {
    reset_identity_on(core, n).await;
    common::assert_identity_stays_complete(core, Duration::from_secs(5)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_reset_right_after_login_keeps_the_new_private_keys() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    // No settle time: the reset starts while the first syncs query the keys.
    let (mut core, _dir, _secrets) = login(1).await;
    reset_and_expect_complete(&mut core, 1).await;
    // A second reset in a row, also right away.
    reset_and_expect_complete(&mut core, 1).await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_restored_session_keeps_the_complete_identity() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(InMemorySecretStore::new());
    let mut core = start_core(dir.path(), secrets.clone());
    first_device(&mut core, 1).await;
    core.commands.send(Command::Shutdown).await.unwrap();
    while core.events.recv().await.is_some() {}

    let mut core = start_core(dir.path(), secrets);
    core.commands.send(Command::RestoreSession).await.unwrap();
    let event = expect_event(&mut core, |e| {
        matches!(e, CoreEvent::LoggedIn { .. } | CoreEvent::NoSession)
    })
    .await;
    assert!(
        matches!(event, CoreEvent::LoggedIn { restored: true, .. }),
        "{event:?}"
    );
    expect_security(&mut core, |s| {
        s.cross_signing == CrossSigning::Complete && s.device == DeviceVerification::Verified
    })
    .await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn messages_after_a_reset_carry_no_shield() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    reset_identity_on(&mut p.alice, 1).await;
    // The status can turn complete before the upload ends; wait for the server.
    let before = server_view(1).await.0;
    let wait = async {
        loop {
            if server_view(1).await.0 != before {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), wait).await;

    // carol learns alice's new keys through sync; until then a message can
    // still show the old red shield, so send until one arrives without it.
    let room = p.room();
    let mut shield = None;
    for _ in 0..6 {
        let text = format!("secret-body-{}", hex());
        p.alice
            .commands
            .send(Command::Send(outgoing(&room, &text)))
            .await
            .unwrap();
        pump(
            &mut p.carol,
            &room,
            &mut p.carol_rooms,
            &mut p.carol_items,
            |_, i| message_with(i, &text, false).is_some(),
        )
        .await;
        let item = message_with(&p.carol_items, &text, false).expect("arrived");
        let TimelineItemKind::Message { details, .. } = &item.kind else {
            unreachable!()
        };
        shield = details.shield;
        if shield.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    assert_eq!(shield, None, "the shield never went away");
    p.finish().await;
}

/// A message sent right after the reset's `Security` event goes out promptly
/// (regression, Task 9): the "Sent" item for it is in the list within 10 s.
/// A stale local echo next to it is a separate check (Task 9, step B).
#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_message_sent_right_after_a_reset_is_sent_promptly() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
    reset_identity_on(&mut p.alice, 1).await;
    let body = format!("after-reset-{}", hex());
    p.alice
        .commands
        .send(Command::Send(outgoing(&room, &body)))
        .await
        .unwrap();
    let sent = |items: &[tessera_core::api::TimelineItem]| {
        items.iter().any(|i| {
            matches!(&i.kind, TimelineItemKind::Message { body: b, is_own: true,
                sent: tessera_core::api::SendStatus::Sent, .. } if *b == body)
        })
    };
    let reached = common::try_pump_with(
        &mut p.alice,
        &room,
        Duration::from_secs(10),
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_| {},
        |_, items| sent(items),
    )
    .await;
    assert!(reached, "the message was not sent within 10 s of the reset");
    p.finish().await;
}
