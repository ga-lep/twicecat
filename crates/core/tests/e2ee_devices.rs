//! Phase 4: the device list, rename, sign out and user trust in the core,
//! against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Uses only alice and carol, reset to a known baseline at the start of every
//! test (plan Q2), so the device counts below are exact. Prints nothing but
//! account names, counts and phase names; passwords, emoji and device names
//! are never printed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    TestEnv, create_dm, e2ee_serial, expect_event, expect_event_within, expect_security,
    first_device, login_and_run, pump, raw_login_named, reset_e2ee_account, start_core,
};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    ActionKind, Command, CoreError, CoreEvent, DeviceId, DeviceSummary, DeviceTrust,
    DeviceVerification, Password, ReauthAction, RoomId, UserId, UserTrust, VerificationId,
    VerificationPhase,
};
use twicecat_core::secrets::InMemorySecretStore;

async fn send(core: &mut CoreHandle, command: Command) {
    core.commands.send(command).await.unwrap();
}

async fn logout(mut core: CoreHandle) {
    send(&mut core, Command::Logout).await;
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

/// Asks `core` for its devices and waits for the first list `want` accepts.
async fn devices_where(
    core: &mut CoreHandle,
    want: impl Fn(&[DeviceSummary]) -> bool,
) -> Vec<DeviceSummary> {
    send(core, Command::ListDevices).await;
    match expect_event(core, |e| matches!(e, CoreEvent::Devices(d) if want(d))).await {
        CoreEvent::Devices(devices) => devices,
        _ => unreachable!("the predicate only accepts Devices"),
    }
}

/// Like [`devices_where`], asking again every 2 s, for a state that arrives
/// through the sync (a device the store has not learned yet is `NoKeys`).
async fn devices_eventually(
    core: &mut CoreHandle,
    want: impl Fn(&[DeviceSummary]) -> bool,
) -> Vec<DeviceSummary> {
    let wait = async {
        loop {
            send(core, Command::ListDevices).await;
            let round = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    match core.events.recv().await {
                        Some(CoreEvent::Devices(d)) if want(&d) => return d,
                        Some(_) => {}
                        None => panic!("event channel closed"),
                    }
                }
            })
            .await;
            if let Ok(devices) = round {
                return devices;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), wait)
        .await
        .expect("the device list never reached the expected state")
}

/// Alice's first device (core 1, an identity, a verified device) and a second
/// device that has just logged in (core 2).
async fn alice_with_second_device() -> (CoreHandle, CoreHandle, [tempfile::TempDir; 2]) {
    reset_e2ee_account(1).await;
    let env = TestEnv::load();
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut first = start_core(dirs[0].path(), Arc::new(InMemorySecretStore::new()));
    first_device(&mut first, 1).await;
    let mut second = start_core(dirs[1].path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut second, &env, 1).await;
    (first, second, dirs)
}

/// The device a core runs on, as it lists itself.
async fn own_device(core: &mut CoreHandle) -> DeviceSummary {
    let list = devices_where(core, |d| d.iter().any(|x| x.current)).await;
    list.into_iter()
        .find(|d| d.current)
        .expect("current device")
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn the_list_shows_both_devices_and_a_rename_shows_in_the_other() {
    let _serial = e2ee_serial().await;
    let (mut first, mut second, _dirs) = alice_with_second_device().await;

    let list = devices_where(&mut first, |d| d.len() == 2).await;
    assert_eq!(list.len(), 2, "exactly the two cores' devices");
    assert!(list[0].current, "the current device comes first");
    assert!(!list[1].current);
    assert_ne!(list[0].device_id, list[1].device_id);

    // Core 2 renames itself (spaces trimmed), and gets the new list at once.
    let own = own_device(&mut second).await;
    send(
        &mut second,
        Command::RenameDevice {
            device_id: own.device_id.clone(),
            name: "  Laptop test  ".into(),
        },
    )
    .await;
    expect_event(&mut second, |e| {
        matches!(e, CoreEvent::Devices(d)
            if d.iter().any(|x| x.current && x.display_name.as_deref() == Some("Laptop test")))
    })
    .await;

    // Core 1's next list shows the name.
    let list = devices_where(&mut first, |d| {
        d.iter()
            .any(|x| x.display_name.as_deref() == Some("Laptop test"))
    })
    .await;
    let renamed = list
        .iter()
        .find(|d| d.display_name.as_deref() == Some("Laptop test"))
        .unwrap();
    assert_eq!(renamed.device_id, own.device_id);
    assert!(!renamed.current);

    // An empty name is refused, and nothing is sent.
    send(
        &mut second,
        Command::RenameDevice {
            device_id: own.device_id,
            name: "  ".into(),
        },
    )
    .await;
    expect_event(&mut second, |e| {
        matches!(
            e,
            CoreEvent::ActionFailed {
                action: ActionKind::Devices,
                error: CoreError::NotAllowed,
                ..
            }
        )
    })
    .await;
    logout(second).await;
    logout(first).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn trust_in_the_list_follows_a_self_verification() {
    let _serial = e2ee_serial().await;
    let (mut first, mut second, _dirs) = alice_with_second_device().await;
    expect_security(&mut second, |s| {
        s.device == DeviceVerification::Unverified && s.has_devices_to_verify_against
    })
    .await;

    let other_unverified = |d: &[DeviceSummary]| {
        d.len() == 2
            && d.iter()
                .any(|x| !x.current && x.trust == DeviceTrust::Unverified)
    };
    let list = devices_eventually(&mut first, other_unverified).await;
    assert_eq!(list[0].trust, DeviceTrust::Verified, "the first device");

    // The Task 6 self-verification flow.
    send(&mut second, Command::RequestSelfVerification).await;
    let id = match expect_event(&mut first, |e| {
        matches!(e, CoreEvent::VerificationRequested { .. })
    })
    .await
    {
        CoreEvent::VerificationRequested { id, .. } => id,
        _ => unreachable!(),
    };
    send(&mut first, Command::AcceptVerification { id: id.clone() }).await;
    let emojis = |id: VerificationId| {
        move |e: &CoreEvent| {
            matches!(e, CoreEvent::Verification { id: i, phase: VerificationPhase::Emojis(_) }
                if *i == id)
        }
    };
    tokio::join!(
        expect_event(&mut first, emojis(id.clone())),
        expect_event(&mut second, emojis(id.clone())),
    );
    send(&mut first, Command::ConfirmSas { id: id.clone() }).await;
    send(&mut second, Command::ConfirmSas { id: id.clone() }).await;
    let done = |id: VerificationId| {
        move |e: &CoreEvent| {
            matches!(e, CoreEvent::Verification { id: i, phase: VerificationPhase::Done }
                if *i == id)
        }
    };
    tokio::join!(
        expect_event(&mut first, done(id.clone())),
        expect_event(&mut second, done(id.clone())),
    );

    // The signature on core 2's device reaches core 1 through the sync.
    let list = devices_eventually(&mut first, |d| {
        d.len() == 2
            && d.iter()
                .any(|x| !x.current && x.trust == DeviceTrust::Verified)
    })
    .await;
    assert!(list.iter().all(|d| d.trust == DeviceTrust::Verified));
    logout(second).await;
    logout(first).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn signing_out_another_device_needs_the_password() {
    let _serial = e2ee_serial().await;
    let (mut first, mut second, _dirs) = alice_with_second_device().await;
    let env = TestEnv::load();

    let list = devices_where(&mut first, |d| d.len() == 2).await;
    let other: DeviceId = list
        .iter()
        .find(|d| !d.current)
        .expect("the other device")
        .device_id
        .clone();

    // No password: the server asks for one.
    send(
        &mut first,
        Command::SignOutDevices {
            device_ids: vec![other.clone()],
            password: None,
        },
    )
    .await;
    expect_event(&mut first, |e| {
        matches!(
            e,
            CoreEvent::ReauthRequired {
                action: ReauthAction::DeleteDevices
            }
        )
    })
    .await;

    // A wrong password is refused, and the answer does not echo it.
    send(
        &mut first,
        Command::SignOutDevices {
            device_ids: vec![other.clone()],
            password: Some(Password::from("secret-pw-wrong-x")),
        },
    )
    .await;
    let failed = expect_event(&mut first, |e| matches!(e, CoreEvent::ActionFailed { .. })).await;
    assert!(
        matches!(
            &failed,
            CoreEvent::ActionFailed {
                action: ActionKind::SignOutDevices,
                error: CoreError::ReauthFailed,
                ..
            }
        ),
        "{failed:?}"
    );
    assert!(!format!("{failed:?}").contains("secret-pw-wrong-x"));
    // Still two devices.
    let list = devices_where(&mut first, |d| !d.is_empty()).await;
    assert_eq!(list.len(), 2);

    // The right password signs it out: core 2 learns, core 1's list shrinks.
    send(
        &mut first,
        Command::SignOutDevices {
            device_ids: vec![other],
            password: Some(Password::from(env.password(1))),
        },
    )
    .await;
    expect_event_within(&mut second, Duration::from_secs(30), |e| {
        matches!(e, CoreEvent::SessionExpired)
    })
    .await;
    let list = expect_event(
        &mut first,
        |e| matches!(e, CoreEvent::Devices(d) if d.len() == 1),
    )
    .await;
    assert!(matches!(&list, CoreEvent::Devices(d) if d[0].current));
    logout(first).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn the_current_device_cannot_be_signed_out_from_the_list() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    first_device(&mut core, 1).await;
    let own = own_device(&mut core).await;
    send(
        &mut core,
        Command::SignOutDevices {
            device_ids: vec![own.device_id.clone()],
            password: Some(Password::from(TestEnv::load().password(1))),
        },
    )
    .await;
    expect_event(&mut core, |e| {
        matches!(
            e,
            CoreEvent::ActionFailed {
                action: ActionKind::SignOutDevices,
                error: CoreError::NotAllowed,
                ..
            }
        )
    })
    .await;
    // Still signed in, and still listed.
    let list = devices_where(&mut core, |d| !d.is_empty()).await;
    assert_eq!(list.len(), 1);
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn selecting_a_dm_reports_the_trust_in_its_peer() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    reset_e2ee_account(3).await;
    let env = TestEnv::load();
    let alice_raw = raw_login_named(1, "twicecat-e2ee-setup").await;
    let carol_raw = raw_login_named(3, "twicecat-e2ee-setup").await;
    let room_id = create_dm(&alice_raw, &env.user_id(3)).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins the dm");
    alice_raw.logout().await.expect("alice setup logout");
    carol_raw.logout().await.expect("carol setup logout");
    let room = room_id.to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    let (mut rooms, mut items) = (Vec::new(), Vec::new());
    pump(&mut alice, &room, &mut rooms, &mut items, |r, _| {
        r.iter().any(|x| x.id.as_str() == room)
    })
    .await;
    send(&mut alice, Command::SelectRoom(RoomId::from(room.as_str()))).await;
    let carol = UserId::from(env.user_id(3).as_str());
    let event = expect_event(
        &mut alice,
        |e| matches!(e, CoreEvent::UserTrust { user_id, .. } if *user_id == carol),
    )
    .await;
    // Carol has no identity after the reset, or one alice has not verified.
    assert!(
        matches!(
            event,
            CoreEvent::UserTrust {
                trust: UserTrust::NoIdentity | UserTrust::Unverified,
                ..
            }
        ),
        "{event:?}"
    );
    logout(alice).await;
}
