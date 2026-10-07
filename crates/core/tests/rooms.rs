//! Smoke tests for the Phase 5 raw-client test helpers in `common`
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).

mod common;

use common::{
    TestEnv, create_room_raw, create_space_raw, fetch_raw_event_json, leave_all_created, raw_login,
    reset_notification_baseline, send_attachment_raw, unique_room_name,
};
use matrix_sdk::ruma::RoomId;
use matrix_sdk::ruma::api::client::state::get_state_events;
use serde_json::Value;

/// All state events of `room` as seen by `client`, as JSON.
async fn state_events(client: &matrix_sdk::Client, room: &RoomId) -> Vec<Value> {
    client
        .send(get_state_events::v3::Request::new(room.to_owned()))
        .await
        .expect("fetch room state")
        .room_state
        .iter()
        .map(|raw| serde_json::from_str(raw.json().get()).expect("state event is JSON"))
        .collect()
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn create_room_raw_encrypted_is_readable_by_the_invitee() {
    let env = TestEnv::load();
    let carol_id = env.user_id(2);
    let room = create_room_raw(1, &unique_room_name(), true, &[carol_id.as_str()]).await;

    let carol = raw_login(2).await;
    carol.join_room_by_id(&room).await.expect("carol joins");
    let state = state_events(&carol, &room).await;
    let encryption = state
        .iter()
        .find(|e| e["type"] == "m.room.encryption")
        .expect("carol can read m.room.encryption");
    assert_eq!(encryption["content"]["algorithm"], "m.megolm.v1.aes-sha2");

    leave_all_created(2, std::slice::from_ref(&room)).await;
    leave_all_created(1, &[room]).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn create_room_raw_plain_has_no_encryption() {
    let room = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let alice = raw_login(1).await;
    let state = state_events(&alice, &room).await;
    assert!(state.iter().all(|e| e["type"] != "m.room.encryption"));
    leave_all_created(1, &[room]).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn create_space_raw_has_the_space_type_and_one_child() {
    let child = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let space = create_space_raw(1, &unique_room_name(), std::slice::from_ref(&child)).await;

    let alice = raw_login(1).await;
    let state = state_events(&alice, &space).await;
    let create = state
        .iter()
        .find(|e| e["type"] == "m.room.create")
        .expect("create event");
    assert_eq!(create["content"]["type"], "m.space");
    let children: Vec<&Value> = state
        .iter()
        .filter(|e| e["type"] == "m.space.child")
        .collect();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["state_key"], child.as_str());

    leave_all_created(1, &[space, child]).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reset_notification_baseline_leaves_no_test_keyword() {
    let alice = raw_login(1).await;
    let settings = alice.notification_settings().await;
    let keyword = format!("tessera-kw-{}", common::hex());
    settings
        .add_keyword(keyword.clone())
        .await
        .expect("add keyword");
    assert!(settings.enabled_keywords().await.contains(&keyword));

    reset_notification_baseline(1).await;

    let fresh = raw_login(1).await.notification_settings().await;
    let left: Vec<String> = fresh
        .enabled_keywords()
        .await
        .into_iter()
        .filter(|k| k.starts_with("tessera-kw-"))
        .collect();
    assert!(left.is_empty(), "{} test keywords left", left.len());
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn send_attachment_raw_posts_an_image_event() {
    let room = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let bytes = include_bytes!("fixtures/small-64x48.png");
    let event = send_attachment_raw(1, &room, "small.png", "image/png", bytes.to_vec()).await;

    let alice = raw_login(1).await;
    let json = fetch_raw_event_json(&alice, &room, &event).await;
    assert_eq!(json["type"], "m.room.message");
    assert_eq!(json["content"]["msgtype"], "m.image");
    assert_eq!(json["content"]["info"]["mimetype"], "image/png");
    leave_all_created(1, &[room]).await;
}
