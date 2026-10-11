//! Checks the local MatrixRTC stack from docker-compose.yml: transport
//! discovery (MSC4143), the lk-jwt-service OpenID-to-JWT exchange (MSC4195),
//! a LiveKit connection with that JWT, and delayed events (MSC4140).
//!
//! These use plain SDK clients: they check the server setup, not the core.
//! Nothing here prints tokens or JWTs.

mod common;

use std::time::Duration;

use common::{TestEnv, create_room, raw_login, unique_room_name};
use matrix_sdk::ruma::api::client::account::request_openid_token;
use matrix_sdk::ruma::api::client::delayed_events::{DelayParameters, delayed_state_event};
use matrix_sdk::ruma::api::client::rtc::RtcTransport;
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::serde::Raw;
use serde_json::json;

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn rtc_transports_point_at_the_local_jwt_service() {
    let env = TestEnv::load();
    let alice = raw_login(1).await;
    let transports = alice
        .discover_rtc_transports()
        .await
        .expect("discover rtc transports")
        .expect("the homeserver advertises rtc transports");
    let urls: Vec<_> = transports
        .iter()
        .filter_map(|t| match t {
            RtcTransport::LiveKit(lk) => Some(lk.service_url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(urls, [env.get("LK_JWT_URL")]);
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn openid_token_buys_a_livekit_jwt_that_connects() {
    let env = TestEnv::load();
    let alice = raw_login(1).await;
    let room_id = create_room(&alice, &unique_room_name(), None).await;
    let user_id = alice.user_id().expect("logged in").to_owned();
    let device_id = alice.device_id().expect("logged in").to_owned();

    let openid = alice
        .send(request_openid_token::v3::Request::new(user_id.clone()))
        .await
        .expect("request an OpenID token");
    let body = json!({
        "room_id": room_id,
        "slot_id": "m.call#ROOM",
        "openid_token": {
            "access_token": openid.access_token,
            "token_type": openid.token_type,
            "matrix_server_name": openid.matrix_server_name,
            "expires_in": openid.expires_in.as_secs(),
        },
        "member": {
            "id": format!("{user_id}:{device_id}"),
            "claimed_user_id": user_id,
            "claimed_device_id": device_id,
        },
    });
    let response = matrix_sdk::reqwest::Client::new()
        .post(format!("{}/get_token", env.get("LK_JWT_URL")))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .expect("reach lk-jwt-service");
    assert!(
        response.status().is_success(),
        "get_token: {}",
        response.status()
    );
    let reply: serde_json::Value =
        serde_json::from_str(&response.text().await.expect("read get_token reply"))
            .expect("get_token reply is JSON");
    assert_eq!(reply["url"], env.get("LIVEKIT_URL"));
    let jwt = reply["jwt"].as_str().expect("get_token returns a jwt");

    let (room, _events) = tokio::time::timeout(
        Duration::from_secs(30),
        livekit::Room::connect(env.get("LIVEKIT_URL"), jwt, livekit::RoomOptions::default()),
    )
    .await
    .expect("LiveKit connect timed out")
    .expect("connect to the local LiveKit SFU");
    // lk-jwt-service hands out a pseudonymous identity:
    // base64_nopad(sha256(json([user_id, device_id, member_id]))), 43 chars.
    let identity = room.local_participant().identity().as_str().to_owned();
    assert_eq!(identity.len(), 43, "sha256 in unpadded base64");
    assert!(
        identity
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/'),
        "identity is base64"
    );
    assert!(
        !identity.contains(user_id.localpart()),
        "identity is pseudonymous"
    );
    room.close().await.expect("leave the LiveKit room");
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn delayed_state_event_is_sent_by_the_server() {
    let alice = raw_login(1).await;
    let room_id = create_room(&alice, &unique_room_name(), None).await;
    let event_type = StateEventType::from("io.github.ga_lep.twicecat.test");
    let content = Raw::new(&json!({ "sent": "by the server" }))
        .expect("serialise content")
        .cast_unchecked();

    let reply = alice
        .send(delayed_state_event::unstable::Request::new_raw(
            room_id.clone(),
            String::new(),
            event_type.clone(),
            DelayParameters::Timeout {
                timeout: Duration::from_secs(2),
            },
            content,
        ))
        .await
        .expect("schedule a delayed state event");
    assert!(!reply.delay_id.is_empty());

    let get = || {
        alice.send(get_state_event_for_key::v3::Request::new(
            room_id.clone(),
            event_type.clone(),
            String::new(),
        ))
    };
    assert!(get().await.is_err(), "not sent before the delay");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if get().await.is_ok() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the server never sent the delayed event"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
