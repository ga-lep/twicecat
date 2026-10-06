//! Phase 4: interactive SAS verification in the core, against the local
//! homeserver (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Uses only alice and carol, reset to a known baseline at the start of every
//! test (plan Q2). Prints nothing but account names, counts and phase names;
//! emoji, keys and message bodies are never printed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, EncryptedPair, TestEnv, create_dm, e2ee_serial, expect_event,
    expect_event_within, expect_security, hex, login_and_run, message_with, outgoing, pump,
    pump_recording, pump_with, raw_login_named, reset_e2ee_account, reset_identity_on, start_core,
    try_pump_with,
};
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, CancelReason, Command, CoreError, CoreEvent, CrossSigning, DeviceVerification,
    ListDiff, Password, PlaceholderKind, RecoveryStatus, RoomId, SasEmoji, SendStatus,
    TimelineItem, TimelineItemKind, UserId, UserTrust, VerificationId, VerificationPeer,
    VerificationPhase,
};
use tessera_core::secrets::InMemorySecretStore;

/// Waits for the first `Verification` event of flow `id` whose phase `want`
/// accepts, ignoring everything else, and returns that phase.
async fn phase(
    core: &mut CoreHandle,
    id: &VerificationId,
    want: impl Fn(&VerificationPhase) -> bool,
) -> VerificationPhase {
    let event = expect_event(
        core,
        |e| matches!(e, CoreEvent::Verification { id: i, phase } if i == id && want(phase)),
    )
    .await;
    match event {
        CoreEvent::Verification { phase, .. } => phase,
        _ => unreachable!("the predicate only accepts Verification events"),
    }
}

/// Waits for the next `VerificationRequested`.
async fn requested(core: &mut CoreHandle) -> (VerificationId, VerificationPeer) {
    match expect_event(core, |e| {
        matches!(e, CoreEvent::VerificationRequested { .. })
    })
    .await
    {
        CoreEvent::VerificationRequested { id, peer } => (id, peer),
        _ => unreachable!("the predicate only accepts VerificationRequested"),
    }
}

/// Waits for the emoji of flow `id`.
async fn emojis(core: &mut CoreHandle, id: &VerificationId) -> Vec<SasEmoji> {
    match phase(core, id, |p| matches!(p, VerificationPhase::Emojis(_))).await {
        VerificationPhase::Emojis(e) => e.to_vec(),
        _ => unreachable!("the predicate only accepts Emojis"),
    }
}

/// Both sides reach the emoji of flow `id`; the seven symbols are identical.
async fn both_see_same_emoji(a: &mut CoreHandle, b: &mut CoreHandle, id: &VerificationId) {
    let (ea, eb) = tokio::join!(emojis(a, id), emojis(b, id));
    assert_eq!(ea.len(), 7);
    let sa: Vec<&str> = ea.iter().map(|e| e.symbol.as_str()).collect();
    let sb: Vec<&str> = eb.iter().map(|e| e.symbol.as_str()).collect();
    assert_eq!(sa, sb, "the two sides show different emoji");
    assert!(ea.iter().all(|e| !e.description.is_empty()));
}

async fn send(core: &mut CoreHandle, command: Command) {
    core.commands.send(command).await.unwrap();
}

/// Both sides confirm and both reach `Done`.
async fn confirm_both(a: &mut CoreHandle, b: &mut CoreHandle, id: &VerificationId) {
    send(a, Command::ConfirmSas { id: id.clone() }).await;
    send(b, Command::ConfirmSas { id: id.clone() }).await;
    let done = |p: &VerificationPhase| matches!(p, VerificationPhase::Done);
    tokio::join!(phase(a, id, done), phase(b, id, done));
}

async fn logout(mut core: CoreHandle) {
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

/// Alice's first device (core 1, an identity, a verified device) and a second
/// device that has just logged in (core 2, unverified), no room.
async fn alice_with_second_device() -> (CoreHandle, CoreHandle, [tempfile::TempDir; 2]) {
    reset_e2ee_account(1).await;
    let env = TestEnv::load();
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut first = start_core(dirs[0].path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut first, &env, 1).await;
    reset_identity_on(&mut first, 1).await;
    let mut second = start_core(dirs[1].path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut second, &env, 1).await;
    expect_security(&mut second, |s| {
        s.device == DeviceVerification::Unverified && s.has_devices_to_verify_against
    })
    .await;
    (first, second, dirs)
}

/// Waits until the key backup of test user `n` on the server holds at least
/// one room key. Alice's core backs its room keys up in the background; a
/// device that verifies and asks for the history before the upload ended finds
/// nothing, and the SDK does not ask again.
async fn wait_for_backed_up_key(n: u8) {
    use matrix_sdk::ruma::api::client::backup::get_latest_backup_info;
    let client = raw_login_named(n, "tessera-e2ee-view").await;
    let wait = async {
        loop {
            if let Ok(info) = client
                .send(get_latest_backup_info::v3::Request::new())
                .await
                && u64::from(info.count) >= 1
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    let waited = tokio::time::timeout(Duration::from_secs(60), wait).await;
    client.logout().await.expect("log the view client out");
    waited.expect("the room key was never backed up");
}

/// Has `core` look up `user` (a trust watch queries the user's keys), and
/// waits for the answer. The SDK ignores a verification request from a device
/// it does not know yet ("Could not retrieve the device data for the incoming
/// verification request"), and two accounts that share only a new DM have not
/// queried each other when the first request arrives. The app does this when a
/// DM opens (plan Q10).
async fn know_user(core: &mut CoreHandle, user: &UserId) {
    send(
        core,
        Command::WatchUserTrust {
            user_id: user.clone(),
        },
    )
    .await;
    expect_event(
        core,
        |e| matches!(e, CoreEvent::UserTrust { user_id, .. } if user_id == user),
    )
    .await;
}

fn is_utd(item: &TimelineItem) -> bool {
    matches!(
        &item.kind,
        TimelineItemKind::Placeholder {
            what: PlaceholderKind::UnableToDecrypt,
            ..
        }
    )
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn self_verification_verifies_the_device_and_decrypts_history() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
    // Alice's core 1 is the account's first device. The reset runs through a
    // pump, so the open room's timeline events that arrive meanwhile are not
    // lost (the item list would go out of step with them).
    let env = TestEnv::load();
    p.alice
        .commands
        .send(Command::ResetIdentity {
            password: Some(Password::from(env.password(1))),
        })
        .await
        .unwrap();
    let reset_done = std::cell::Cell::new(false);
    pump_with(
        &mut p.alice,
        &room,
        DEFAULT_TIMEOUT,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |e| {
            if let CoreEvent::Security(s) = e
                && s.cross_signing == CrossSigning::Complete
                && s.device == DeviceVerification::Verified
                && s.recovery == RecoveryStatus::Disabled
            {
                reset_done.set(true);
            }
        },
        |_, _| reset_done.get(),
    )
    .await;

    let bodies: Vec<String> = (0..2).map(|_| format!("secret-body-{}", hex())).collect();
    for body in &bodies {
        send(&mut p.alice, Command::Send(outgoing(&room, body))).await;
    }
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| {
            bodies.iter().all(|b| {
                matches!(
                    message_with(i, b, true).map(|m| &m.kind),
                    Some(TimelineItemKind::Message {
                        sent: SendStatus::Sent,
                        ..
                    })
                )
            })
        },
    )
    .await;

    wait_for_backed_up_key(1).await;

    // Alice's second device sees two undecryptable messages.
    let dir2 = tempfile::tempdir().unwrap();
    let mut second = start_core(dir2.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut second, &env, 1).await;
    let (mut rooms, mut items): (Vec<_>, Vec<TimelineItem>) = (Vec::new(), Vec::new());
    pump_with(
        &mut second,
        &room,
        DEFAULT_TIMEOUT,
        &mut rooms,
        &mut items,
        |_| {},
        |r, _| r.iter().any(|x| x.id.as_str() == room),
    )
    .await;
    send(
        &mut second,
        Command::SelectRoom(RoomId::from(room.as_str())),
    )
    .await;
    let mut shown = false;
    for _ in 0..6 {
        send(
            &mut second,
            Command::PaginateBackwards {
                room_id: RoomId::from(room.as_str()),
            },
        )
        .await;
        shown = try_pump_with(
            &mut second,
            &room,
            Duration::from_secs(5),
            &mut rooms,
            &mut items,
            |_| {},
            |_, i| i.iter().filter(|x| is_utd(x)).count() >= 2,
        )
        .await;
        if shown {
            break;
        }
    }
    assert!(shown, "the two historical messages never showed");
    let ids: Vec<String> = items
        .iter()
        .filter(|i| is_utd(i))
        .map(|i| i.id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    // Core 2 asks, core 1 gets the request from one of its own devices.
    send(&mut second, Command::RequestSelfVerification).await;
    let (id, peer) = requested(&mut p.alice).await;
    assert!(peer.own_account, "{peer:?}");
    assert_eq!(peer.user_id.as_str(), env.user_id(1));
    assert!(peer.device_id.is_some());
    send(&mut p.alice, Command::AcceptVerification { id: id.clone() }).await;
    // Core 2 is followed through the pump from here on: waiting for its events
    // any other way would drop timeline diffs (the item list would go out of
    // step) and the status events (Verified can come before or after Done).
    let found: std::cell::RefCell<Option<Vec<SasEmoji>>> = std::cell::RefCell::new(None);
    let (alice_emoji, ()) = tokio::join!(emojis(&mut p.alice, &id), async {
        pump_with(
            &mut second,
            &room,
            DEFAULT_TIMEOUT,
            &mut rooms,
            &mut items,
            |e| {
                if let CoreEvent::Verification {
                    id: i,
                    phase: VerificationPhase::Emojis(x),
                } = e
                    && *i == id
                {
                    *found.borrow_mut() = Some(x.to_vec());
                }
            },
            |_, _| found.borrow().is_some(),
        )
        .await;
    });
    let second_emoji = found.take().expect("core 2 showed emoji");
    let symbols = |e: &[SasEmoji]| e.iter().map(|x| x.symbol.clone()).collect::<Vec<_>>();
    assert_eq!(symbols(&alice_emoji), symbols(&second_emoji));
    assert_eq!(second_emoji.len(), 7);

    send(&mut p.alice, Command::ConfirmSas { id: id.clone() }).await;
    send(&mut second, Command::ConfirmSas { id: id.clone() }).await;

    // The flow ends, the device is verified, and the history decrypts in
    // place.
    let done = std::cell::Cell::new(false);
    let verified = std::cell::Cell::new(false);
    let mut diffs: Vec<ListDiff<TimelineItem>> = Vec::new();
    let (_, ()) = tokio::join!(
        phase(&mut p.alice, &id, |p| matches!(p, VerificationPhase::Done)),
        pump_recording(
            &mut second,
            &room,
            Duration::from_secs(60),
            &mut items,
            &mut diffs,
            |e| match e {
                CoreEvent::Security(s) if s.device == DeviceVerification::Verified => {
                    verified.set(true);
                }
                CoreEvent::Verification {
                    id: i,
                    phase: VerificationPhase::Done,
                } if *i == id => done.set(true),
                _ => {}
            },
            |i| {
                done.get()
                    && verified.get()
                    && ids.iter().all(|id| {
                        i.iter().any(|x| {
                            &x.id == id && matches!(x.kind, TimelineItemKind::Message { .. })
                        })
                    })
            },
        ),
    );
    let mut got: Vec<String> = ids
        .iter()
        .map(|id| {
            let item = items.iter().find(|x| &x.id == id).expect("same item id");
            let TimelineItemKind::Message { body, .. } = &item.kind else {
                panic!("not a message");
            };
            body.clone()
        })
        .collect();
    got.sort();
    let mut want = bodies.clone();
    want.sort();
    assert_eq!(got, want);

    logout(second).await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn user_verification_in_a_dm_verifies_both_users() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    reset_e2ee_account(3).await;
    let env = TestEnv::load();
    let alice_raw = raw_login_named(1, "tessera-e2ee-setup").await;
    let carol_raw = raw_login_named(3, "tessera-e2ee-setup").await;
    let room_id = create_dm(&alice_raw, &env.user_id(3)).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins the dm");
    alice_raw.logout().await.expect("alice setup logout");
    carol_raw.logout().await.expect("carol setup logout");
    let room = room_id.to_string();

    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut alice = start_core(dirs[0].path(), Arc::new(InMemorySecretStore::new()));
    let mut carol = start_core(dirs[1].path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    login_and_run(&mut carol, &env, 3).await;
    // Each side has the DM before the request (the SDK finds it through
    // `m.direct`).
    for core in [&mut alice, &mut carol] {
        let (mut rooms, mut items) = (Vec::new(), Vec::new());
        pump(core, &room, &mut rooms, &mut items, |r, _| {
            r.iter().any(|x| x.id.as_str() == room)
        })
        .await;
    }
    reset_identity_on(&mut alice, 1).await;
    reset_identity_on(&mut carol, 3).await;
    know_user(&mut alice, &UserId::from(env.user_id(3).as_str())).await;
    know_user(&mut carol, &UserId::from(env.user_id(1).as_str())).await;

    let carol_id = UserId::from(env.user_id(3).as_str());
    let alice_id = UserId::from(env.user_id(1).as_str());
    send(
        &mut alice,
        Command::RequestUserVerification {
            user_id: carol_id.clone(),
        },
    )
    .await;
    let (id, peer) = requested(&mut carol).await;
    assert!(!peer.own_account, "{peer:?}");
    assert_eq!(peer.user_id, alice_id);
    send(&mut carol, Command::AcceptVerification { id: id.clone() }).await;
    both_see_same_emoji(&mut alice, &mut carol, &id).await;
    confirm_both(&mut alice, &mut carol, &id).await;

    // Each side now trusts the other user.
    send(
        &mut alice,
        Command::WatchUserTrust {
            user_id: carol_id.clone(),
        },
    )
    .await;
    send(
        &mut carol,
        Command::WatchUserTrust {
            user_id: alice_id.clone(),
        },
    )
    .await;
    let trust = |who: UserId| {
        move |e: &CoreEvent| {
            matches!(e, CoreEvent::UserTrust { user_id, trust: UserTrust::Verified }
                if *user_id == who)
        }
    };
    // The SDK uploads the signature on the other's identity with its next
    // sync request, which can wait for the end of a long poll (30 s); the
    // other side sees it after that.
    let slow = Duration::from_secs(75);
    let (a, c) = tokio::join!(
        expect_event_within(&mut alice, slow, trust(carol_id.clone())),
        expect_event_within(&mut carol, slow, trust(alice_id.clone())),
    );
    drop((a, c));
    logout(alice).await;
    logout(carol).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn mismatch_cancels_both_sides_and_trust_is_unchanged() {
    let _serial = e2ee_serial().await;
    let (mut first, mut second, _dirs) = alice_with_second_device().await;
    send(&mut second, Command::RequestSelfVerification).await;
    let (id, _peer) = requested(&mut first).await;
    send(&mut first, Command::AcceptVerification { id: id.clone() }).await;
    both_see_same_emoji(&mut first, &mut second, &id).await;

    send(&mut first, Command::SasMismatch { id: id.clone() }).await;
    let cancelled = |p: &VerificationPhase| {
        matches!(
            p,
            VerificationPhase::Cancelled {
                reason: CancelReason::Mismatch
            }
        )
    };
    tokio::join!(
        phase(&mut first, &id, cancelled),
        phase(&mut second, &id, cancelled),
    );

    // Core 2 never turns verified.
    let watch = async {
        loop {
            match second.events.recv().await {
                Some(CoreEvent::Security(s)) => assert_ne!(
                    s.device,
                    DeviceVerification::Verified,
                    "a mismatch verified the device"
                ),
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    // Running out of time is the expected end.
    let _ = tokio::time::timeout(Duration::from_secs(4), watch).await;
    logout(second).await;
    logout(first).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn only_one_verification_is_open_at_a_time() {
    let _serial = e2ee_serial().await;
    let (mut first, mut second, _dirs) = alice_with_second_device().await;
    send(&mut second, Command::RequestSelfVerification).await;
    let (id, _peer) = requested(&mut first).await;
    phase(&mut second, &id, |p| {
        matches!(p, VerificationPhase::Requested { incoming: false })
    })
    .await;

    // A second outgoing request while the flow is open is refused.
    send(&mut second, Command::RequestSelfVerification).await;
    match expect_event(&mut second, |e| matches!(e, CoreEvent::ActionFailed { .. })).await {
        CoreEvent::ActionFailed { action, error, .. } => {
            assert_eq!(action, ActionKind::Verification);
            assert_eq!(error, CoreError::Busy);
        }
        _ => unreachable!(),
    }

    // Declining the first request ends it on both sides.
    send(&mut first, Command::CancelVerification { id: id.clone() }).await;
    let ended = |p: &VerificationPhase| matches!(p, VerificationPhase::Cancelled { .. });
    let a = phase(&mut first, &id, ended).await;
    let b = phase(&mut second, &id, ended).await;
    assert_eq!(
        a,
        VerificationPhase::Cancelled {
            reason: CancelReason::ByUs
        }
    );
    assert_eq!(
        b,
        VerificationPhase::Cancelled {
            reason: CancelReason::ByThem
        }
    );
    logout(second).await;
    logout(first).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_second_incoming_request_from_another_user_is_cancelled() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(3).await;
    let env = TestEnv::load();
    // A DM between alice and carol (the SDK's own check cancels two requests
    // of one user, so the core's check is exercised by a different user).
    let (mut first, mut second, _dirs) = alice_with_second_device().await;
    let alice_raw = raw_login_named(1, "tessera-e2ee-setup").await;
    let carol_raw = raw_login_named(3, "tessera-e2ee-setup").await;
    // Carol creates the DM, so that her `m.direct` has it and her request
    // goes there.
    let room_id = create_dm(&carol_raw, &env.user_id(1)).await;
    alice_raw
        .join_room_by_id(&room_id)
        .await
        .expect("alice joins the dm");
    alice_raw.logout().await.expect("alice setup logout");
    carol_raw.logout().await.expect("carol setup logout");
    let room = room_id.to_string();
    let dir3 = tempfile::tempdir().unwrap();
    let mut carol = start_core(dir3.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut carol, &env, 3).await;
    let (mut rooms, mut items) = (Vec::new(), Vec::new());
    pump(&mut carol, &room, &mut rooms, &mut items, |r, _| {
        r.iter().any(|x| x.id.as_str() == room)
    })
    .await;
    reset_identity_on(&mut carol, 3).await;
    let carol_user = UserId::from(env.user_id(3).as_str());
    know_user(&mut first, &carol_user).await;
    know_user(&mut second, &carol_user).await;
    know_user(&mut carol, &UserId::from(env.user_id(1).as_str())).await;

    // Alice's core 1 has a flow open with her second device.
    send(&mut second, Command::RequestSelfVerification).await;
    let (id, _peer) = requested(&mut first).await;

    // Carol asks to verify alice: core 1 cancels it, carol sees the cancel.
    let alice_id = UserId::from(env.user_id(1).as_str());
    send(
        &mut carol,
        Command::RequestUserVerification { user_id: alice_id },
    )
    .await;
    let ended = expect_event_within(&mut carol, Duration::from_secs(30), |e| {
        matches!(
            e,
            CoreEvent::Verification {
                phase: VerificationPhase::Cancelled { .. },
                ..
            }
        )
    })
    .await;
    let CoreEvent::Verification {
        id: carols_id,
        phase,
    } = ended
    else {
        unreachable!()
    };
    assert_ne!(carols_id, id);
    assert_eq!(
        phase,
        VerificationPhase::Cancelled {
            reason: CancelReason::ByThem
        }
    );
    // Core 1 never announced carol's request, and still has its own flow.
    let stray = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match first.events.recv().await {
                Some(CoreEvent::VerificationRequested { id: other, .. }) if other != id => {
                    return other;
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    })
    .await;
    assert!(
        stray.is_err(),
        "the second request was announced: {stray:?}"
    );
    send(&mut first, Command::CancelVerification { id: id.clone() }).await;
    phase_cancelled(&mut first, &id).await;
    logout(carol).await;
    logout(second).await;
    logout(first).await;
}

/// Waits for the end of flow `id`.
async fn phase_cancelled(core: &mut CoreHandle, id: &VerificationId) {
    phase(core, id, |p| {
        matches!(p, VerificationPhase::Cancelled { .. })
    })
    .await;
}
