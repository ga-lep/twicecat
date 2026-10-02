//! MatrixRTC signalling against the local stack (`docker compose up -d
//! --wait && scripts/create-test-users.sh`): our membership event in
//! Element Call's format, the delayed leave kept alive, a lost membership
//! restored, leave, and two devices seeing each other. No media yet.
//!
//! Uses alice and carol; bob stays untouched (`login::restore_without_
//! logging_in_again` counts his devices).

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_room, expect_event, raw_login, start_core, unique_room_name,
    wait_for_room,
};
use matrix_sdk::ruma::api::client::state::get_state_event_for_key;
use matrix_sdk::ruma::events::StateEventType;
use serde_json::Value;
use tessera_core::CoreHandle;
use tessera_core::api::{
    CallEndReason, CallIntent, CallState, Command, CoreEvent, ListDiff, Participant, RoomId,
};
use tessera_core::secrets::InMemorySecretStore;

const MEMBER_EVENT: &str = "org.matrix.msc3401.call.member";

async fn logged_in_core(dir: &std::path::Path, n: u8) -> (CoreHandle, String) {
    let env = TestEnv::load();
    let mut core = start_core(dir, Arc::new(InMemorySecretStore::new()));
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: tessera_core::api::Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    let device = match expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await {
        CoreEvent::LoggedIn { device_id, .. } => device_id.as_str().to_owned(),
        _ => unreachable!(),
    };
    expect_event(&mut core, |e| {
        matches!(
            e,
            CoreEvent::SyncState(tessera_core::api::SyncState::Running)
        )
    })
    .await;
    (core, device)
}

async fn join(core: &mut CoreHandle, room_id: &str) {
    core.commands
        .send(Command::JoinCall {
            room_id: RoomId::from(room_id),
            intent: CallIntent::Audio,
        })
        .await
        .unwrap();
    let state = expect_event(core, |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Connected { .. } | CallState::Left { .. })
        )
    })
    .await;
    assert!(
        matches!(state, CoreEvent::CallState(CallState::Connected { .. })),
        "{state:?}"
    );
}

/// The content of `state_key`'s membership, read by a separate client.
async fn membership_content(
    reader: &matrix_sdk::Client,
    room_id: &str,
    state_key: &str,
) -> Option<Value> {
    let reply = reader
        .send(get_state_event_for_key::v3::Request::new(
            room_id.try_into().unwrap(),
            StateEventType::from(MEMBER_EVENT),
            state_key.to_owned(),
        ))
        .await
        .ok()?;
    serde_json::from_str(reply.event_or_content.get()).ok()
}

/// Lets members at power level 0 publish call memberships, as Element does
/// for the rooms it creates.
async fn allow_calls_for_everyone(admin: &matrix_sdk::Client, room_id: &str) {
    use matrix_sdk::ruma::api::client::state::send_state_event;
    let room: matrix_sdk::ruma::OwnedRoomId = room_id.try_into().unwrap();
    let mut levels: Value = serde_json::from_str(
        admin
            .send(get_state_event_for_key::v3::Request::new(
                room.clone(),
                StateEventType::RoomPowerLevels,
                String::new(),
            ))
            .await
            .expect("read power levels")
            .event_or_content
            .get(),
    )
    .unwrap();
    levels["events"][MEMBER_EVENT] = serde_json::json!(0);
    let raw = matrix_sdk::ruma::serde::Raw::new(&levels)
        .unwrap()
        .cast_unchecked();
    admin
        .send(send_state_event::v3::Request::new_raw(
            room,
            StateEventType::RoomPowerLevels,
            String::new(),
            raw,
        ))
        .await
        .expect("allow call memberships at level 0");
}

/// Waits for a `CallParticipants` event whose list satisfies `done`.
async fn participants_until(
    core: &mut CoreHandle,
    done: impl Fn(&[Participant]) -> bool,
) -> Vec<Participant> {
    let mut list = Vec::new();
    let wait = async {
        loop {
            match core.events.recv().await {
                Some(CoreEvent::CallParticipants { diffs, .. }) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut list);
                    }
                    if done(&list) {
                        return list;
                    }
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for participants")
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn membership_is_element_compatible_survives_the_delay_and_leaves() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let alice_raw = raw_login(1).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), None)
        .await
        .to_string();
    let (mut core, device) = logged_in_core(dir.path(), 1).await;
    wait_for_room(&mut core, &room_id).await;

    join(&mut core, &room_id).await;
    let user = env.user_id(1);
    let key = format!("_{user}_{device}_m.call");
    let content = membership_content(&alice_raw, &room_id, &key)
        .await
        .expect("membership published");
    assert_eq!(content["application"], "m.call");
    assert_eq!(content["call_id"], "");
    assert_eq!(content["scope"], "m.room");
    assert_eq!(content["device_id"], device.as_str());
    assert_eq!(content["membershipID"], format!("{user}:{device}"));
    assert_eq!(content["focus_active"]["focus_selection"], "multi_sfu");
    assert_eq!(
        content["foci_preferred"][0]["livekit_service_url"],
        env.get("LK_JWT_URL")
    );
    assert_eq!(
        content["foci_preferred"][0]["livekit_alias"],
        room_id.as_str()
    );

    let me = participants_until(&mut core, |p| p.iter().any(|p| p.is_local)).await;
    assert_eq!(me.len(), 1);
    assert_eq!(me[0].id.as_str(), format!("{user}:{device}"));

    // Past the 18 s delayed leave: the restarts keep the membership.
    tokio::time::sleep(Duration::from_secs(25)).await;
    let still = membership_content(&alice_raw, &room_id, &key).await;
    assert!(
        still.as_ref().is_some_and(|c| c.get("device_id").is_some()),
        "membership still published after 25 s: {still:?}"
    );

    core.commands.send(Command::LeaveCall).await.unwrap();
    let left = expect_event(&mut core, |e| {
        matches!(e, CoreEvent::CallState(CallState::Left { .. }))
    })
    .await;
    assert!(
        matches!(
            left,
            CoreEvent::CallState(CallState::Left {
                reason: CallEndReason::Hangup,
                ..
            })
        ),
        "{left:?}"
    );
    let after = membership_content(&alice_raw, &room_id, &key).await;
    assert_eq!(after, Some(serde_json::json!({})), "membership removed");
    core.commands.send(Command::Shutdown).await.unwrap();
}

/// When our membership disappears mid-call (the server sends our delayed
/// leave after a stall longer than its delay), we join again, and stay:
/// the old delayed leave is cancelled, not left to fire later.
#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_removed_membership_is_restored_and_stays() {
    use matrix_sdk::ruma::api::client::state::send_state_event;
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let alice_raw = raw_login(1).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), None)
        .await
        .to_string();
    let (mut core, device) = logged_in_core(dir.path(), 1).await;
    wait_for_room(&mut core, &room_id).await;
    join(&mut core, &room_id).await;
    participants_until(&mut core, |p| p.iter().any(|p| p.is_local)).await;

    // What the server's delayed leave sends: an empty membership.
    let key = format!("_{}_{device}_m.call", env.user_id(1));
    let empty = matrix_sdk::ruma::serde::Raw::new(&serde_json::json!({}))
        .unwrap()
        .cast_unchecked();
    alice_raw
        .send(send_state_event::v3::Request::new_raw(
            room_id.as_str().try_into().unwrap(),
            StateEventType::from(MEMBER_EVENT),
            key.clone(),
            empty,
        ))
        .await
        .expect("remove the membership");
    participants_until(&mut core, |p| p.is_empty()).await;
    participants_until(&mut core, |p| p.iter().any(|p| p.is_local)).await;
    let back = membership_content(&alice_raw, &room_id, &key).await;
    assert!(
        back.as_ref().is_some_and(|c| c.get("device_id").is_some()),
        "membership published again: {back:?}"
    );

    // Past the 18 s delay: no old delayed leave removes it again.
    tokio::time::sleep(Duration::from_secs(25)).await;
    let still = membership_content(&alice_raw, &room_id, &key).await;
    assert!(
        still.as_ref().is_some_and(|c| c.get("device_id").is_some()),
        "membership still published after 25 s: {still:?}"
    );

    core.commands.send(Command::LeaveCall).await.unwrap();
    expect_event(&mut core, |e| {
        matches!(e, CoreEvent::CallState(CallState::Left { .. }))
    })
    .await;
    let after = membership_content(&alice_raw, &room_id, &key).await;
    assert_eq!(after, Some(serde_json::json!({})), "membership removed");
    core.commands.send(Command::Shutdown).await.unwrap();
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn two_devices_in_one_call_see_each_other() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let carol_id = env.user_id(3);
    let room_id = create_room(&alice_raw, &unique_room_name(), Some(&carol_id))
        .await
        .to_string();
    allow_calls_for_everyone(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .expect("carol joins");

    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (mut alice, _) = logged_in_core(dir_a.path(), 1).await;
    let (mut carol, _) = logged_in_core(dir_c.path(), 3).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;

    join(&mut alice, &room_id).await;
    join(&mut carol, &room_id).await;
    let users = |p: &[Participant]| {
        let mut u: Vec<String> = p.iter().map(|p| p.user_id.as_str().to_owned()).collect();
        u.sort();
        u
    };
    let mut both = vec![env.user_id(1), env.user_id(3)];
    both.sort();
    let seen_by_alice = participants_until(&mut alice, |p| p.len() == 2).await;
    assert_eq!(users(&seen_by_alice), both);
    assert_eq!(seen_by_alice.iter().filter(|p| p.is_local).count(), 1);
    let seen_by_carol = participants_until(&mut carol, |p| p.len() == 2).await;
    assert_eq!(users(&seen_by_carol), both);

    carol.commands.send(Command::LeaveCall).await.unwrap();
    let alone = participants_until(&mut alice, |p| p.len() == 1).await;
    assert_eq!(alone[0].user_id.as_str(), env.user_id(1));
    for core in [&alice, &carol] {
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}
