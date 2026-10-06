//! Phase 4: recovery (key backup plus secret storage) in the core, against the
//! local homeserver (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Uses only alice and carol, reset to a known baseline at the start of every
//! test (plan Q2). Prints nothing but account names, counts and the observed
//! UTD reason; keys and message bodies are never printed.

mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, EncryptedPair, TestEnv, e2ee_serial, expect_event, expect_security,
    first_device, hex, login_and_run, message_with, outgoing, pump, pump_with, raw_login_named,
    reset_e2ee_account, seed_recovery, start_core, try_pump_with,
};
use matrix_sdk::ruma::api::client::backup::get_latest_backup_info;
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, Command, CoreError, CoreEvent, CrossSigning, DeviceVerification, Password,
    PlaceholderKind, RecoveryKey, RecoveryStatus, RoomId, SecurityStatus, Shield, ShieldCode,
    TimelineItem, TimelineItemKind, UtdReason,
};
use tessera_core::secrets::InMemorySecretStore;

const BACKUP_EXISTS: &str = "a key backup already exists; enter your recovery key";
const WRONG_KEY: &str = "that recovery key or passphrase is not correct";

/// Starts a core on a fresh directory, logged in as test user `n`, with sync
/// running.
async fn fresh_core(n: u8) -> (CoreHandle, tempfile::TempDir) {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, n).await;
    (core, dir)
}

async fn logout(mut core: CoreHandle) {
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

/// Sends `EnableRecovery` and waits for the new key and for recovery to turn
/// `Enabled` (either may come first). A `Busy` answer (an identity operation
/// of the same core is still ending) is retried.
async fn enable_recovery_on(core: &mut CoreHandle) -> RecoveryKey {
    let wait = async {
        'retry: loop {
            core.commands.send(Command::EnableRecovery).await.unwrap();
            let mut key: Option<RecoveryKey> = None;
            let mut enabled = false;
            let mut keys_seen = 0;
            while key.is_none() || !enabled {
                match core.events.recv().await.expect("event channel open") {
                    CoreEvent::RecoveryKeyReady(k) => {
                        keys_seen += 1;
                        key = Some(k);
                    }
                    CoreEvent::Security(s) if s.recovery == RecoveryStatus::Enabled => {
                        enabled = true;
                    }
                    CoreEvent::ActionFailed {
                        action: ActionKind::Recovery,
                        error: CoreError::Busy,
                        ..
                    } => {
                        tokio::time::sleep(Duration::from_millis(250)).await;
                        continue 'retry;
                    }
                    CoreEvent::ActionFailed { action, error, .. } => {
                        panic!("enabling recovery failed: {action:?} {error:?}")
                    }
                    _ => {}
                }
            }
            assert_eq!(keys_seen, 1, "the key must be sent exactly once");
            return key.expect("loop ends with a key");
        }
    };
    tokio::time::timeout(Duration::from_secs(60), wait)
        .await
        .expect("timed out enabling recovery")
}

/// Sends `command` (a recovery action) and returns the error of its
/// `ActionFailed`.
async fn recovery_error(core: &mut CoreHandle, command: Command) -> CoreError {
    core.commands.send(command).await.unwrap();
    match expect_event(core, |e| {
        matches!(
            e,
            CoreEvent::ActionFailed { .. } | CoreEvent::Recovered | CoreEvent::RecoveryKeyReady(_)
        )
    })
    .await
    {
        CoreEvent::ActionFailed {
            room_id,
            action,
            error,
        } => {
            assert!(room_id.as_str().is_empty(), "not a room action");
            assert_eq!(action, ActionKind::Recovery);
            error
        }
        other => panic!("expected a failure, got {other:?}"),
    }
}

/// Sends `Recover { key }` and waits for `Recovered` and for the status of a
/// restored device (either may come first).
async fn recover_with(core: &mut CoreHandle, key: &str) {
    core.commands
        .send(Command::Recover {
            key: RecoveryKey::from(key),
        })
        .await
        .unwrap();
    let wait = async {
        let (mut recovered, mut status) = (false, false);
        while !(recovered && status) {
            match core.events.recv().await.expect("event channel open") {
                CoreEvent::Recovered => recovered = true,
                CoreEvent::Security(s) if restored(&s) => status = true,
                e @ CoreEvent::ActionFailed { .. } => panic!("recovery failed: {e:?}"),
                _ => {}
            }
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out restoring with the key");
}

/// A status that says recovery restored this device.
fn restored(s: &SecurityStatus) -> bool {
    s.device == DeviceVerification::Verified
        && s.recovery == RecoveryStatus::Enabled
        && s.cross_signing == CrossSigning::Complete
}

/// The version of the backup on the server for test user `n`, and whether
/// secret storage is on, as a fresh raw client sees them.
async fn server_backup(n: u8) -> (Option<String>, bool) {
    let client = raw_login_named(n, "tessera-e2ee-view").await;
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    let version = client
        .send(get_latest_backup_info::v3::Request::new())
        .await
        .ok()
        .map(|r| r.version);
    let secret_storage = client
        .encryption()
        .secret_storage()
        .is_enabled()
        .await
        .expect("fetch 4S state");
    client.logout().await.expect("log the view client out");
    (version, secret_storage)
}

/// Changes one character of `key` to another base58 character.
fn mistyped(key: &str) -> String {
    let mut chars: Vec<char> = key.chars().collect();
    let i = chars
        .iter()
        .position(|c| c.is_alphanumeric())
        .expect("the key has a character");
    chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
    chars.into_iter().collect()
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn set_up_then_restore_on_a_fresh_store_reads_earlier_history() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
    // Alice's core is the account's first device (the pair logged in on B0).
    common::reset_identity_on(&mut p.alice, 1).await;

    let bodies: Vec<String> = (0..3).map(|_| format!("secret-body-{}", hex())).collect();
    for body in &bodies {
        p.carol
            .commands
            .send(Command::Send(outgoing(&room, body)))
            .await
            .unwrap();
    }
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| bodies.iter().all(|b| message_with(i, b, false).is_some()),
    )
    .await;

    let key = enable_recovery_on(&mut p.alice).await;
    assert!(!key.expose().is_empty());

    // A second alice core on a new data directory.
    let env = TestEnv::load();
    let dir2 = tempfile::tempdir().unwrap();
    let mut second = start_core(dir2.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut second, &env, 1).await;
    let (mut rooms, mut items): (Vec<_>, Vec<TimelineItem>) = (Vec::new(), Vec::new());
    // `Security` arrives between timeline events; keep what the pumps see.
    let mut statuses: Vec<SecurityStatus> = Vec::new();
    let mut note = |e: &CoreEvent| {
        if let CoreEvent::Security(s) = e {
            statuses.push(*s);
        }
    };
    pump_with(
        &mut second,
        &room,
        DEFAULT_TIMEOUT,
        &mut rooms,
        &mut items,
        &mut note,
        |r, _| r.iter().any(|x| x.id.as_str() == room),
    )
    .await;
    second
        .commands
        .send(Command::SelectRoom(RoomId::from(room.as_str())))
        .await
        .unwrap();
    let is_utd = |i: &TimelineItem| {
        matches!(
            &i.kind,
            TimelineItemKind::Placeholder {
                what: PlaceholderKind::UnableToDecrypt,
                ..
            }
        )
    };
    // A fresh store gets only the newest event at first; page back for the
    // rest until all three show.
    let mut shown = false;
    for _ in 0..6 {
        second
            .commands
            .send(Command::PaginateBackwards {
                room_id: RoomId::from(room.as_str()),
            })
            .await
            .unwrap();
        shown = try_pump_with(
            &mut second,
            &room,
            Duration::from_secs(5),
            &mut rooms,
            &mut items,
            &mut note,
            |_, i| i.iter().filter(|x| is_utd(x)).count() >= 3,
        )
        .await;
        if shown {
            break;
        }
    }
    assert!(shown, "the three historical messages never showed");
    let placeholders: Vec<&TimelineItem> = items.iter().filter(|i| is_utd(i)).collect();
    assert_eq!(placeholders.len(), 3);
    for item in &placeholders {
        let TimelineItemKind::Placeholder { utd, .. } = &item.kind else {
            unreachable!()
        };
        // The observed reason is recorded in PROGRESS.md.
        println!("observed utd reason before recovery: {utd:?}");
        assert!(
            matches!(
                utd,
                Some(
                    UtdReason::HistoricalMessageAndDeviceIsUnverified
                        | UtdReason::HistoricalMessageAndBackupIsDisabled
                        | UtdReason::Unknown
                )
            ),
            "{utd:?}"
        );
    }
    let ids: Vec<String> = placeholders.iter().map(|i| i.id.clone()).collect();

    // The status first shows an unrecovered device.
    if !statuses.iter().any(|s| {
        s.device == DeviceVerification::Unverified && s.recovery == RecoveryStatus::Incomplete
    }) {
        expect_security(&mut second, |s| {
            s.device == DeviceVerification::Unverified && s.recovery == RecoveryStatus::Incomplete
        })
        .await;
    }

    // The key as the SDK groups it (with spaces) is accepted as is.
    second
        .commands
        .send(Command::Recover { key: key.clone() })
        .await
        .unwrap();
    let mut recovered = false;
    let mut restored_status = false;
    pump_with(
        &mut second,
        &room,
        Duration::from_secs(60),
        &mut rooms,
        &mut items,
        |e| match e {
            CoreEvent::Recovered => recovered = true,
            CoreEvent::Security(s) if restored(s) => restored_status = true,
            CoreEvent::ActionFailed { action, error, .. } => {
                panic!("recovery failed: {action:?} {error:?}")
            }
            _ => {}
        },
        |_, i| {
            ids.iter().all(|id| {
                i.iter()
                    .any(|x| &x.id == id && matches!(&x.kind, TimelineItemKind::Message { .. }))
            })
        },
    )
    .await;
    // Same item IDs, now messages with the original bodies.
    let mut by_id: HashMap<&str, &TimelineItem> = HashMap::new();
    for item in &items {
        by_id.insert(item.id.as_str(), item);
    }
    let mut got = Vec::new();
    for id in &ids {
        let item = by_id.get(id.as_str()).expect("the item is still there");
        let TimelineItemKind::Message { body, details, .. } = &item.kind else {
            panic!("not a message");
        };
        assert_eq!(
            details.shield,
            Some(Shield::Grey(ShieldCode::AuthenticityNotGuaranteed)),
            "keys from the backup"
        );
        got.push(body.clone());
    }
    got.sort();
    let mut want = bodies.clone();
    want.sort();
    assert_eq!(got, want);
    if !(recovered && restored_status) {
        // Both can arrive after the timeline caught up.
        if !recovered {
            expect_event(&mut second, |e| matches!(e, CoreEvent::Recovered)).await;
        }
        if !restored_status {
            expect_security(&mut second, restored).await;
        }
    }
    logout(second).await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_returning_user_gets_the_backup_message_and_recovers_with_the_key() {
    let _serial = e2ee_serial().await;
    let k0 = seed_recovery(1, None).await;
    let (mut core, _dir) = fresh_core(1).await;
    expect_security(&mut core, |s| {
        s.device == DeviceVerification::Unverified && s.recovery == RecoveryStatus::Incomplete
    })
    .await;
    let error = recovery_error(&mut core, Command::EnableRecovery).await;
    assert_eq!(error, CoreError::Internal(BACKUP_EXISTS.into()));
    // The refusal changed nothing on the server.
    assert!(server_backup(1).await.0.is_some());

    recover_with(&mut core, &k0).await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_backup_this_device_is_not_connected_to_is_never_replaced() {
    // Recovery state `Disabled` (4S off) but a backup on the server: the SDK's
    // own check answers, and the core maps it to the same message.
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let raw = raw_login_named(1, "tessera-e2ee-backup").await;
    raw.encryption().wait_for_e2ee_initialization_tasks().await;
    raw.encryption()
        .recovery()
        .enable_backup()
        .await
        .expect("create a backup without secret storage");
    let (version_before, secret_storage) = server_backup(1).await;
    assert!(version_before.is_some() && !secret_storage);

    let (mut core, _dir) = fresh_core(1).await;
    expect_security(&mut core, |s| s.recovery == RecoveryStatus::Disabled).await;
    let error = recovery_error(&mut core, Command::EnableRecovery).await;
    assert_eq!(error, CoreError::Internal(BACKUP_EXISTS.into()));
    let (version_after, secret_storage) = server_backup(1).await;
    assert_eq!(version_before, version_after, "the backup was replaced");
    assert!(!secret_storage, "secret storage was turned on");
    raw.logout().await.expect("log the raw client out");
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_wrong_key_is_refused_and_a_passphrase_is_accepted() {
    let _serial = e2ee_serial().await;
    let k0 = seed_recovery(1, None).await;
    let (mut core, _dir) = fresh_core(1).await;
    expect_security(&mut core, |s| s.recovery == RecoveryStatus::Incomplete).await;

    let error = recovery_error(
        &mut core,
        Command::Recover {
            key: RecoveryKey::from(mistyped(&k0)),
        },
    )
    .await;
    assert_eq!(error, CoreError::Internal(WRONG_KEY.into()));
    // Not a key at all.
    let error = recovery_error(
        &mut core,
        Command::Recover {
            key: RecoveryKey::from("definitely not a recovery key"),
        },
    )
    .await;
    assert_eq!(error, CoreError::Internal(WRONG_KEY.into()));
    // Recovery stays incomplete: no `Enabled` status follows.
    let status = tokio::time::timeout(Duration::from_secs(3), async {
        expect_security(&mut core, |s| s.recovery != RecoveryStatus::Incomplete).await
    })
    .await;
    assert!(status.is_err(), "the status changed: {status:?}");
    logout(core).await;

    // An account whose secret storage is protected by a passphrase.
    let passphrase = format!("pass-{}", hex());
    let _key = seed_recovery(1, Some(&passphrase)).await;
    let (mut core, _dir) = fresh_core(1).await;
    expect_security(&mut core, |s| s.recovery == RecoveryStatus::Incomplete).await;
    recover_with(&mut core, &passphrase).await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn changing_the_key_makes_the_old_one_useless() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let dir = tempfile::tempdir().unwrap();
    let mut first = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    first_device(&mut first, 1).await;
    let k1 = enable_recovery_on(&mut first).await;

    first
        .commands
        .send(Command::ChangeRecoveryKey)
        .await
        .unwrap();
    let k2 = match expect_event(&mut first, |e| {
        matches!(
            e,
            CoreEvent::RecoveryKeyReady(_) | CoreEvent::ActionFailed { .. }
        )
    })
    .await
    {
        CoreEvent::RecoveryKeyReady(key) => key,
        other => panic!("changing the key failed: {other:?}"),
    };
    assert_ne!(k1, k2, "the key did not change");

    let (mut with_new, _d1) = fresh_core(1).await;
    expect_security(&mut with_new, |s| s.recovery == RecoveryStatus::Incomplete).await;
    recover_with(&mut with_new, k2.expose()).await;
    logout(with_new).await;

    let (mut with_old, _d2) = fresh_core(1).await;
    expect_security(&mut with_old, |s| s.recovery == RecoveryStatus::Incomplete).await;
    let error = recovery_error(&mut with_old, Command::Recover { key: k1.clone() }).await;
    assert_eq!(error, CoreError::Internal(WRONG_KEY.into()));
    logout(with_old).await;
    logout(first).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn changing_the_key_needs_a_restored_device() {
    // A device that lacks the secrets must not replace the key: it would
    // upload only what it holds.
    let _serial = e2ee_serial().await;
    let k0 = seed_recovery(1, None).await;
    let (mut core, _dir) = fresh_core(1).await;
    expect_security(&mut core, |s| s.recovery == RecoveryStatus::Incomplete).await;
    let (version_before, _) = server_backup(1).await;
    let error = recovery_error(&mut core, Command::ChangeRecoveryKey).await;
    assert!(matches!(error, CoreError::Internal(_)), "{error:?}");
    assert_eq!(server_backup(1).await.0, version_before);
    // The old key still works: nothing was replaced.
    recover_with(&mut core, &k0).await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn resetting_the_identity_clears_recovery_and_makes_a_new_backup() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    first_device(&mut core, 1).await;
    let _key = enable_recovery_on(&mut core).await;
    let (version_before, secret_storage) = server_backup(1).await;
    assert!(version_before.is_some() && secret_storage);

    let env = TestEnv::load();
    core.commands
        .send(Command::ResetIdentity {
            password: Some(Password::from(env.password(1))),
        })
        .await
        .unwrap();
    // Recovery is off from the moment secret storage is turned off. The
    // cross-signing state is not asserted here: a sync-driven keys query can
    // drop the new private keys during a reset (see the Task 5 notes).
    expect_security(&mut core, |s| s.recovery == RecoveryStatus::Disabled).await;
    // The new backup is made at the end of the reset; poll for it.
    let wait = async {
        loop {
            let (version, secret_storage) = server_backup(1).await;
            if version.is_some() && version != version_before {
                assert!(!secret_storage, "secret storage is still on");
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("the server shows a new backup");
    logout(core).await;
}
