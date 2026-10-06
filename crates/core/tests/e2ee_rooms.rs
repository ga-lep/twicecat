//! Phase 4 baseline: the E2EE test helpers (account reset, seeded recovery,
//! encrypted rooms) against the local homeserver, plus the per-binary lock.
//!
//! Prints nothing but account names and counts.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    TestEnv, create_encrypted_room, e2ee_serial, expect_event, fetch_raw_event_type,
    raw_login_named, reset_e2ee_account, seed_recovery, start_core,
};
use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::state::get_state_events;
use matrix_sdk::ruma::serde::Raw;
use tessera_core::CoreHandle;
use tessera_core::api::{
    Block, Command, CoreEvent, Inline, ListDiff, MessageKind, MessageText, OutgoingMessage,
    Password, RoomId, RoomSummary, Shield, ShieldCode, SyncState, TextFormat, TimelineItem,
    TimelineItemKind,
};
use tessera_core::secrets::InMemorySecretStore;

/// What a fresh raw client (no encryption settings) sees for an account.
struct View {
    backup_on_server: bool,
    secret_storage_on: bool,
    has_identity: bool,
    devices: usize,
    master_key: Option<String>,
}

/// Logs a fresh raw client in as test user `n` and reads the account's crypto
/// state. The caller logs the client out.
async fn fresh_view(n: u8) -> (Client, View) {
    let client = raw_login_named(n, "tessera-e2ee-view").await;
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    let me = client.user_id().expect("logged in").to_owned();
    let identity = client
        .encryption()
        .request_user_identity(&me)
        .await
        .expect("query own identity");
    let view = View {
        backup_on_server: client
            .encryption()
            .backups()
            .fetch_exists_on_server()
            .await
            .expect("fetch backup state"),
        secret_storage_on: client
            .encryption()
            .secret_storage()
            .is_enabled()
            .await
            .expect("fetch 4S state"),
        has_identity: identity.is_some(),
        devices: client.devices().await.expect("list devices").devices.len(),
        master_key: identity.map(|i| {
            i.master_key()
                .get_first_key()
                .expect("master key has a key")
                .to_base64()
        }),
    };
    (client, view)
}

#[tokio::test]
async fn serial_lock_orders_holders_and_survives_a_panic() {
    let first = e2ee_serial().await;
    let second = tokio::spawn(async {
        let _guard = e2ee_serial().await;
    });
    // The second task cannot finish while the first guard is held.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!second.is_finished(), "second holder got the lock early");
    drop(first);
    tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("second holder gets the lock after the first drops it")
        .expect("second task");

    // A holder that panics does not poison the lock.
    let panicker = tokio::spawn(async {
        let _guard = e2ee_serial().await;
        panic!("holder panics on purpose");
    });
    assert!(panicker.await.expect_err("task panicked").is_panic());
    let _next = tokio::time::timeout(Duration::from_secs(5), e2ee_serial())
        .await
        .expect("lock is free after a panicking holder");
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reset_reaches_the_empty_baseline_and_repeats() {
    let _serial = e2ee_serial().await;
    let mut master_keys = Vec::new();
    for _ in 0..2 {
        reset_e2ee_account(1).await;
        let (client, view) = fresh_view(1).await;
        assert!(!view.backup_on_server, "a backup survived the reset");
        assert!(!view.secret_storage_on, "4S is on after the reset");
        assert!(view.has_identity, "no own identity after the reset");
        assert_eq!(view.devices, 1, "only the view client's device is left");
        master_keys.push(view.master_key.expect("identity has a master key"));
        client.logout().await.expect("log the view client out");
    }
    assert_ne!(
        master_keys[0], master_keys[1],
        "the reset did not replace the identity"
    );
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn seeded_recovery_reaches_the_key_baseline() {
    let _serial = e2ee_serial().await;
    let key = seed_recovery(1, None).await;
    let (client, view) = fresh_view(1).await;
    assert!(view.secret_storage_on, "4S is off after seeding");
    assert!(view.backup_on_server, "no backup after seeding");
    assert_eq!(view.devices, 1);
    client
        .encryption()
        .recovery()
        .recover(&key)
        .await
        .expect("the seeded key recovers on a fresh client");
    client.logout().await.expect("log the view client out");
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn encrypted_room_state_is_readable_by_the_invitee() {
    let _serial = e2ee_serial().await;
    reset_e2ee_account(1).await;
    reset_e2ee_account(3).await;
    let env = TestEnv::load();
    let alice = raw_login_named(1, "tessera-e2ee-room").await;
    let carol = raw_login_named(3, "tessera-e2ee-room").await;
    let room_id =
        create_encrypted_room(&alice, &common::unique_room_name(), Some(&env.user_id(3))).await;
    carol
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins the room");

    let state = carol
        .send(get_state_events::v3::Request::new(room_id.clone()))
        .await
        .expect("carol reads the room state")
        .room_state;
    let encryption_event: Raw<_> = state
        .into_iter()
        .find(|event| {
            event
                .get_field::<String>("type")
                .ok()
                .flatten()
                .is_some_and(|t| t == "m.room.encryption")
        })
        .expect("the room has an m.room.encryption state event");
    let event_id: matrix_sdk::ruma::OwnedEventId = encryption_event
        .get_field::<String>("event_id")
        .expect("event_id parses")
        .expect("state event has an event_id")
        .try_into()
        .expect("valid event id");
    let event_type = fetch_raw_event_type(&carol, &room_id, &event_id).await;
    assert_eq!(event_type, "m.room.encryption");
    let algorithm = encryption_event
        .get_field::<serde_json::Value>("content")
        .expect("content parses")
        .and_then(|c| {
            c.get("algorithm")
                .and_then(|a| a.as_str().map(str::to_owned))
        });
    assert_eq!(algorithm.as_deref(), Some("m.megolm.v1.aes-sha2"));

    alice.logout().await.expect("alice logs out");
    carol.logout().await.expect("carol logs out");
}

/// Alice and carol (both reset to B0) in a fresh encrypted room, each with a
/// running core that has the room open.
struct EncryptedPair {
    room_id: matrix_sdk::ruma::OwnedRoomId,
    alice: CoreHandle,
    carol: CoreHandle,
    alice_rooms: Vec<RoomSummary>,
    carol_rooms: Vec<RoomSummary>,
    alice_items: Vec<TimelineItem>,
    carol_items: Vec<TimelineItem>,
    _dirs: [tempfile::TempDir; 2],
}

fn hex() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn outgoing(room: &str, text: &str) -> OutgoingMessage {
    OutgoingMessage {
        room_id: RoomId::from(room),
        text: MessageText::from(text),
        mentions: Vec::new(),
        reply_to: None,
        kind: MessageKind::Text,
        format: TextFormat::Markdown,
    }
}

async fn login_and_run(core: &mut CoreHandle, env: &TestEnv, n: u8) {
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    expect_event(core, |e| {
        matches!(e, CoreEvent::SyncState(SyncState::Running))
    })
    .await;
}

/// Applies room list and timeline events until `done` holds.
async fn pump(
    core: &mut CoreHandle,
    room: &str,
    rooms: &mut Vec<RoomSummary>,
    items: &mut Vec<TimelineItem>,
    done: impl Fn(&[RoomSummary], &[TimelineItem]) -> bool,
) {
    let wait = async {
        while !done(rooms, items) {
            match core.events.recv().await {
                Some(CoreEvent::RoomListChanged(diffs)) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, rooms);
                    }
                }
                Some(CoreEvent::TimelineDiff { room_id, diffs }) if room_id.as_str() == room => {
                    for diff in diffs {
                        ListDiff::apply(&diff, items);
                    }
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(common::DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for the core");
}

impl EncryptedPair {
    async fn new() -> Self {
        reset_e2ee_account(1).await;
        reset_e2ee_account(3).await;
        let env = TestEnv::load();
        let alice_raw = raw_login_named(1, "tessera-e2ee-setup").await;
        let carol_raw = raw_login_named(3, "tessera-e2ee-setup").await;
        let room_id = create_encrypted_room(
            &alice_raw,
            &common::unique_room_name(),
            Some(&env.user_id(3)),
        )
        .await;
        carol_raw
            .join_room_by_id(&room_id)
            .await
            .expect("carol joins the room");
        alice_raw.logout().await.expect("alice setup logout");
        carol_raw.logout().await.expect("carol setup logout");

        let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let mut alice = start_core(dirs[0].path(), Arc::new(InMemorySecretStore::new()));
        let mut carol = start_core(dirs[1].path(), Arc::new(InMemorySecretStore::new()));
        login_and_run(&mut alice, &env, 1).await;
        login_and_run(&mut carol, &env, 3).await;
        let room = room_id.to_string();
        let mut pair = Self {
            room_id,
            alice,
            carol,
            alice_rooms: Vec::new(),
            carol_rooms: Vec::new(),
            alice_items: Vec::new(),
            carol_items: Vec::new(),
            _dirs: dirs,
        };
        pump(
            &mut pair.alice,
            &room,
            &mut pair.alice_rooms,
            &mut pair.alice_items,
            |r, _| r.iter().any(|x| x.id.as_str() == room),
        )
        .await;
        pump(
            &mut pair.carol,
            &room,
            &mut pair.carol_rooms,
            &mut pair.carol_items,
            |r, _| r.iter().any(|x| x.id.as_str() == room),
        )
        .await;
        for core in [&mut pair.alice, &mut pair.carol] {
            core.commands
                .send(Command::SelectRoom(RoomId::from(room.as_str())))
                .await
                .unwrap();
        }
        pair
    }

    fn room(&self) -> String {
        self.room_id.to_string()
    }

    async fn finish(mut self) {
        for core in [&mut self.alice, &mut self.carol] {
            core.commands.send(Command::Logout).await.unwrap();
            expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
        }
    }
}

fn message_with<'a>(items: &'a [TimelineItem], text: &str, own: bool) -> Option<&'a TimelineItem> {
    items.iter().find(|i| {
        matches!(&i.kind, TimelineItemKind::Message { body, is_own, .. }
            if body == text && *is_own == own)
    })
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn encrypted_dm_works_both_ways() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
    let (from_alice, from_carol) = (
        format!("secret-body-{}", hex()),
        format!("secret-body-{}", hex()),
    );

    p.alice
        .commands
        .send(Command::Send(outgoing(&room, &from_alice)))
        .await
        .unwrap();
    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| message_with(i, &from_alice, false).is_some(),
    )
    .await;

    p.carol
        .commands
        .send(Command::Send(outgoing(&room, &from_carol)))
        .await
        .unwrap();
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| message_with(i, &from_carol, false).is_some(),
    )
    .await;

    // Both events are encrypted on the server.
    let carol_event = message_with(&p.carol_items, &from_alice, false)
        .and_then(|i| i.event_id.clone())
        .expect("carol's item has an event id");
    let alice_event = message_with(&p.alice_items, &from_carol, false)
        .and_then(|i| i.event_id.clone())
        .expect("alice's item has an event id");
    for room_summary in [&p.alice_rooms, &p.carol_rooms] {
        let r = room_summary.iter().find(|r| r.id.as_str() == room).unwrap();
        assert!(r.is_encrypted, "the room is not reported as encrypted");
    }
    let checker = raw_login_named(3, "tessera-e2ee-check").await;
    for id in [carol_event, alice_event] {
        let id: matrix_sdk::ruma::OwnedEventId = id.as_str().try_into().expect("event id");
        assert_eq!(
            fetch_raw_event_type(&checker, &p.room_id, &id).await,
            "m.room.encrypted"
        );
    }
    checker.logout().await.expect("checker logout");
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn formatted_body_survives_encryption() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
    let source = "**bold**";
    p.alice
        .commands
        .send(Command::Send(outgoing(&room, source)))
        .await
        .unwrap();
    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| message_with(i, source, false).is_some(),
    )
    .await;
    let item = message_with(&p.carol_items, source, false).expect("arrived");
    let TimelineItemKind::Message { details, .. } = &item.kind else {
        unreachable!()
    };
    let bold = details.content.blocks.iter().any(|b| match b {
        Block::Paragraph(inlines) => inlines
            .iter()
            .any(|i| matches!(i, Inline::Text { text, style } if text == "bold" && style.bold)),
        _ => false,
    });
    assert!(bold, "the decrypted formatted body has no bold inline");
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn messages_from_an_unsigned_device_get_a_red_shield() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
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
    assert_eq!(
        details.shield,
        Some(Shield::Red(ShieldCode::UnsignedDevice))
    );
    p.finish().await;
}
