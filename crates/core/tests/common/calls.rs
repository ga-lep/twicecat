//! Call helpers shared by the call tests: power levels for memberships,
//! reading a membership, and joining the SFU as a made-up device.

use serde_json::Value;

use super::TestEnv;

/// The content of a call membership `user` sent.
pub async fn membership_of(
    reader: &matrix_sdk::Client,
    room_id: &str,
    user: &str,
) -> Option<Value> {
    use matrix_sdk::ruma::api::client::state::get_state_events;
    let reply = reader
        .send(get_state_events::v3::Request::new(
            room_id.try_into().unwrap(),
        ))
        .await
        .ok()?;
    reply.room_state.iter().find_map(|raw| {
        let event: Value = raw.deserialize_as().ok()?;
        (event["type"] == "org.matrix.msc3401.call.member" && event["sender"] == user)
            .then(|| event["content"].clone())
    })
}

/// Lets members at power level 0 publish call memberships (as Element does).
pub async fn allow_calls(admin: &matrix_sdk::Client, room_id: &str) {
    use matrix_sdk::ruma::api::client::state::{get_state_event_for_key, send_state_event};
    use matrix_sdk::ruma::events::StateEventType;
    let room: matrix_sdk::ruma::OwnedRoomId = room_id.try_into().unwrap();
    let reply = admin
        .send(get_state_event_for_key::v3::Request::new(
            room.clone(),
            StateEventType::RoomPowerLevels,
            String::new(),
        ))
        .await
        .unwrap();
    let mut levels: Value = serde_json::from_str(reply.event_or_content.get()).unwrap();
    levels["events"]["org.matrix.msc3401.call.member"] = serde_json::json!(0);
    admin
        .send(send_state_event::v3::Request::new_raw(
            room,
            StateEventType::RoomPowerLevels,
            String::new(),
            matrix_sdk::ruma::serde::Raw::new(&levels)
                .unwrap()
                .cast_unchecked(),
        ))
        .await
        .unwrap();
}

/// Joins the call's LiveKit room as `client`'s user with the made-up
/// `device` (a token from the call service, with or without a call
/// membership).
pub async fn sfu_room(
    client: &matrix_sdk::Client,
    room_id: &str,
    device: &str,
) -> livekit::prelude::Room {
    use livekit::prelude::{Room, RoomOptions};
    use matrix_sdk::ruma::api::client::account::request_openid_token;

    let env = TestEnv::load();
    let openid = client
        .send(request_openid_token::v3::Request::new(
            client.user_id().unwrap().to_owned(),
        ))
        .await
        .unwrap();
    let body = serde_json::json!({
        "room": room_id,
        "openid_token": {
            "access_token": openid.access_token,
            "token_type": openid.token_type,
            "matrix_server_name": openid.matrix_server_name,
            "expires_in": openid.expires_in.as_secs(),
        },
        "device_id": device,
    });
    let reply = matrix_sdk::reqwest::Client::new()
        .post(format!("{}/sfu/get", env.get("LK_JWT_URL")))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let (room, _events) = Room::connect(
        reply["url"].as_str().unwrap(),
        reply["jwt"].as_str().unwrap(),
        RoomOptions::default(),
    )
    .await
    .expect("connects to the SFU");
    room
}

/// Gives `client`'s user a call membership for the made-up `device`, copied
/// from `like`'s (so on the same SFU), as another client of theirs would.
pub async fn add_member_device(
    client: &matrix_sdk::Client,
    room_id: &str,
    like: &str,
    device: &str,
) {
    use matrix_sdk::ruma::api::client::state::send_state_event;
    use matrix_sdk::ruma::events::StateEventType;
    let user = client.user_id().unwrap().to_string();
    let mut membership = membership_of(client, room_id, like)
        .await
        .expect("the membership to copy");
    membership["device_id"] = device.into();
    membership["membershipID"] = format!("{user}:{device}").into();
    client
        .send(send_state_event::v3::Request::new_raw(
            room_id.try_into().unwrap(),
            StateEventType::from("org.matrix.msc3401.call.member"),
            format!("_{user}_{device}_m.call"),
            matrix_sdk::ruma::serde::Raw::new(&membership)
                .unwrap()
                .cast_unchecked(),
        ))
        .await
        .unwrap();
}
