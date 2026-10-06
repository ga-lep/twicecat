//! Phase 4 baseline: the E2EE test helpers (account reset, seeded recovery,
//! encrypted rooms) against the local homeserver, plus the per-binary lock.
//!
//! Prints nothing but account names and counts.

mod common;

use std::time::Duration;

use common::{
    TestEnv, create_encrypted_room, e2ee_serial, fetch_raw_event_type, raw_login_named,
    reset_e2ee_account, seed_recovery,
};
use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::state::get_state_events;
use matrix_sdk::ruma::serde::Raw;

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
