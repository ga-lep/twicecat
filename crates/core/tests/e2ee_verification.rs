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
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    ActionKind, CancelReason, Command, CoreError, CoreEvent, CrossSigning, DeviceVerification,
    ListDiff, Password, PlaceholderKind, RecoveryStatus, RoomId, SasEmoji, SendStatus,
    TimelineItem, TimelineItemKind, UserId, UserTrust, VerificationId, VerificationPeer,
    VerificationPhase,
};
use twicecat_core::secrets::InMemorySecretStore;

/// Set by the tests that talk to a peer that can scan, so `phase` lets
/// `ShowQr` through. Tests run one at a time under `e2ee_serial`.
static QR_EXPECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Waits for the first `Verification` event of flow `id` whose phase `want`
/// accepts, ignoring everything else, and returns that phase.
async fn phase(
    core: &mut CoreHandle,
    id: &VerificationId,
    want: impl Fn(&VerificationPhase) -> bool,
) -> VerificationPhase {
    let event = expect_event(core, |e| {
        // Two Twicecat cores never offer a QR code to each other (neither
        // advertises scanning); tests that expect one use `qr_phase`.
        if let CoreEvent::Verification {
            phase: VerificationPhase::ShowQr(_),
            ..
        } = e
            && !QR_EXPECTED.load(std::sync::atomic::Ordering::SeqCst)
        {
            panic!("a QR code was shown between two Twicecat cores");
        }
        matches!(e, CoreEvent::Verification { id: i, phase } if i == id && want(phase))
    })
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
    let client = raw_login_named(n, "twicecat-e2ee-view").await;
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
                assert!(
                    !matches!(
                        e,
                        CoreEvent::Verification {
                            phase: VerificationPhase::ShowQr(_),
                            ..
                        }
                    ),
                    "a QR code was shown between two Twicecat cores"
                );
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
    let alice_raw = raw_login_named(1, "twicecat-e2ee-setup").await;
    let carol_raw = raw_login_named(3, "twicecat-e2ee-setup").await;
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

// ---- QR code display (plan Q3) ----

/// A second login of alice that can scan QR codes, syncing in the background.
struct ScanningPeer {
    client: matrix_sdk::Client,
    sync: tokio::task::JoinHandle<()>,
}

impl ScanningPeer {
    async fn login() -> Self {
        let client = raw_login_named(1, "twicecat-test-qr").await;
        // The first sync uploads the device keys.
        client
            .sync_once(matrix_sdk::config::SyncSettings::default())
            .await
            .expect("first sync of the scanning peer");
        let background = client.clone();
        let sync = tokio::spawn(async move {
            // `Client::sync` does not pass the compiler's `Send` check here
            // (E0275), so the loop is written out.
            let settings =
                matrix_sdk::config::SyncSettings::default().timeout(Duration::from_secs(5));
            loop {
                let _ = background.sync_once(settings.clone()).await;
            }
        });
        Self { client, sync }
    }

    /// Asks the core's device for verification, advertising that it can scan.
    async fn request_verification(
        &self,
    ) -> matrix_sdk::encryption::verification::VerificationRequest {
        use matrix_sdk::ruma::events::key::verification::VerificationMethod;
        let me = self.client.user_id().expect("logged in").to_owned();
        let own = self.client.device_id().expect("logged in").to_owned();
        let found = async {
            loop {
                // Ask the server, so the core's device is known.
                let _ = self.client.encryption().request_user_identity(&me).await;
                if let Ok(devices) = self.client.encryption().get_user_devices(&me).await
                    && let Some(device) = devices.devices().find(|d| d.device_id() != own)
                {
                    return device;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        };
        let device = tokio::time::timeout(DEFAULT_TIMEOUT, found)
            .await
            .expect("the core's device never showed up");
        device
            .request_verification_with_methods(vec![
                VerificationMethod::SasV1,
                VerificationMethod::QrCodeScanV1,
                VerificationMethod::ReciprocateV1,
            ])
            .await
            .expect("request verification")
    }

    async fn logout(self) {
        self.sync.abort();
        let _ = self.sync.await;
        self.client.logout().await.expect("log the peer out");
    }
}

/// Waits for the first state of `request` that `want` accepts.
async fn raw_state(
    request: &matrix_sdk::encryption::verification::VerificationRequest,
    want: impl Fn(&matrix_sdk::encryption::verification::VerificationRequestState) -> bool,
) -> matrix_sdk::encryption::verification::VerificationRequestState {
    use futures_util::StreamExt as _;
    // Subscribe before reading, so no change is missed.
    let mut changes = request.changes().boxed();
    let current = request.state();
    if want(&current) {
        return current;
    }
    tokio::time::timeout(DEFAULT_TIMEOUT, async {
        while let Some(state) = changes.next().await {
            if want(&state) {
                return state;
            }
        }
        panic!("the request's changes ended");
    })
    .await
    .expect("the request never reached the expected state")
}

/// Alice's only device (a core, an identity, a verified device) and a peer
/// that can scan, which has asked the core to verify. The core has accepted
/// and shows a QR code.
async fn core_showing_qr() -> (
    CoreHandle,
    tempfile::TempDir,
    ScanningPeer,
    matrix_sdk::encryption::verification::VerificationRequest,
    VerificationId,
    twicecat_core::api::QrImage,
) {
    reset_e2ee_account(1).await;
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;
    reset_identity_on(&mut core, 1).await;
    let peer = ScanningPeer::login().await;
    know_user(&mut core, &UserId::from(env.user_id(1).as_str())).await;
    let request = peer.request_verification().await;
    let (id, who) = requested(&mut core).await;
    assert!(who.own_account, "{who:?}");
    send(&mut core, Command::AcceptVerification { id: id.clone() }).await;
    QR_EXPECTED.store(true, std::sync::atomic::Ordering::SeqCst);
    let shown = phase(&mut core, &id, |p| {
        matches!(p, VerificationPhase::ShowQr(_))
    })
    .await;
    let VerificationPhase::ShowQr(image) = shown else {
        unreachable!("the predicate only accepts ShowQr");
    };
    (core, dir, peer, request, id, image)
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_qr_code_is_shown_to_a_peer_that_can_scan() {
    use matrix_sdk::encryption::verification::VerificationRequestState as Raw;
    use matrix_sdk::ruma::events::key::verification::VerificationMethod;
    let _serial = e2ee_serial().await;
    let (core, _dir, peer, request, _id, image) = core_showing_qr().await;
    QR_EXPECTED.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(image.size, 45);
    assert_eq!(image.dark.len(), 45 * 45);
    assert!(image.dark.iter().any(|d| *d) && image.dark.iter().any(|d| !*d));
    match raw_state(&request, |s| matches!(s, Raw::Ready { .. })).await {
        Raw::Ready { their_methods, .. } => {
            assert!(their_methods.contains(&VerificationMethod::QrCodeShowV1));
            assert!(!their_methods.contains(&VerificationMethod::QrCodeScanV1));
        }
        _ => unreachable!("the predicate only accepts Ready"),
    }
    peer.logout().await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn compare_emoji_instead_of_the_qr_code_verifies_the_peer() {
    use futures_util::StreamExt as _;
    use matrix_sdk::encryption::verification::VerificationRequestState as Raw;
    let _serial = e2ee_serial().await;
    let (mut core, _dir, peer, request, id, _image) = core_showing_qr().await;
    send(&mut core, Command::StartSas { id: id.clone() }).await;
    QR_EXPECTED.store(false, std::sync::atomic::Ordering::SeqCst);

    // The peer gets the SAS flow and accepts it.
    let state = raw_state(
        &request,
        |s| matches!(s, Raw::Transitioned { verification } if verification.clone().sas().is_some()),
    )
    .await;
    let Raw::Transitioned { verification } = state else {
        unreachable!("the predicate only accepts Transitioned");
    };
    let sas = verification.sas().expect("a SAS flow");
    let mut changes = sas.changes().boxed();
    sas.accept().await.expect("the peer accepts the SAS flow");
    let peer_emoji = tokio::time::timeout(DEFAULT_TIMEOUT, async {
        loop {
            if let Some(emoji) = sas.emoji() {
                return emoji.map(|e| e.symbol);
            }
            assert!(changes.next().await.is_some(), "the SAS changes ended");
        }
    })
    .await
    .expect("the peer never got the emoji");
    let core_emoji = emojis(&mut core, &id).await;
    assert_eq!(core_emoji.len(), 7);
    assert!(
        core_emoji
            .iter()
            .map(|e| e.symbol.as_str())
            .eq(peer_emoji.iter().copied()),
        "the two sides show different emoji"
    );

    sas.confirm().await.expect("the peer confirms");
    send(&mut core, Command::ConfirmSas { id: id.clone() }).await;
    phase(&mut core, &id, |p| matches!(p, VerificationPhase::Done)).await;

    // The core signed the peer's device (the peer sees it after its next
    // keys query).
    let me = peer.client.user_id().expect("logged in").to_owned();
    let verified = async {
        loop {
            let _ = peer.client.encryption().request_user_identity(&me).await;
            if let Ok(Some(device)) = peer.client.encryption().get_own_device().await
                && device.is_verified()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(60), verified)
        .await
        .expect("the peer's device never became verified");
    peer.logout().await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn cancelling_while_the_qr_code_shows_cancels_the_peer_request() {
    use matrix_sdk::encryption::verification::VerificationRequestState as Raw;
    use matrix_sdk::ruma::events::key::verification::cancel::CancelCode;
    let _serial = e2ee_serial().await;
    let (mut core, _dir, peer, request, id, _image) = core_showing_qr().await;
    QR_EXPECTED.store(false, std::sync::atomic::Ordering::SeqCst);
    send(&mut core, Command::CancelVerification { id: id.clone() }).await;
    let ended = phase(&mut core, &id, |p| {
        matches!(p, VerificationPhase::Cancelled { .. })
    })
    .await;
    assert_eq!(
        ended,
        VerificationPhase::Cancelled {
            reason: CancelReason::ByUs
        }
    );
    match raw_state(&request, |s| matches!(s, Raw::Cancelled(_))).await {
        Raw::Cancelled(info) => assert_eq!(*info.cancel_code(), CancelCode::User),
        _ => unreachable!("the predicate only accepts Cancelled"),
    }
    peer.logout().await;
    logout(core).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn qr_confirmation_and_sas_need_the_right_phase() {
    let _serial = e2ee_serial().await;
    let (mut core, _dir, peer, _request, id, _image) = core_showing_qr().await;
    QR_EXPECTED.store(false, std::sync::atomic::Ordering::SeqCst);
    // Nobody scanned the code, so there is nothing to confirm.
    send(&mut core, Command::ConfirmQrScanned { id: id.clone() }).await;
    match expect_event(&mut core, |e| matches!(e, CoreEvent::ActionFailed { .. })).await {
        CoreEvent::ActionFailed { action, error, .. } => {
            assert_eq!(action, ActionKind::Verification);
            assert_eq!(error, CoreError::NotAllowed);
        }
        _ => unreachable!(),
    }
    peer.logout().await;
    logout(core).await;
}
