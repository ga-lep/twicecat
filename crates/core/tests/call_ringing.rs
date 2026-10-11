//! Ringing and voice rooms against the local stack (`scripts/stack.sh up -d
//! --wait && scripts/create-test-users.sh`): MSC4075 notifications as
//! Element Call sends them, MSC4310 declines, call counts for the room list
//! and MSC3417 call rooms.
//!
//! Uses alice and carol; bob stays untouched. Audio is silent and virtual.

mod common;

use std::sync::Arc;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_dm, create_room, expect_event, login_and_run, raw_login,
    start_core, unique_room_name, wait_for_room,
};
use serde_json::Value;
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    CallIntent, CallState, Command, CoreEvent, ListDiff, RoomId, RoomSummary,
};
use twicecat_core::secrets::InMemorySecretStore;

async fn logged_in(dir: &std::path::Path, n: u8) -> CoreHandle {
    let env = TestEnv::load();
    let mut core = start_core(dir, Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, n).await;
    core
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

/// Lets members at power level 0 publish call memberships (as Element does).
async fn allow_calls(admin: &matrix_sdk::Client, room_id: &str) {
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

/// Waits until each predicate has matched some event, in any order (the
/// call watcher reports counts and rings as they come). Returns the events
/// that matched, in the predicates' order.
async fn expect_all(
    core: &mut CoreHandle,
    preds: &[&dyn Fn(&CoreEvent) -> bool],
) -> Vec<CoreEvent> {
    let mut got: Vec<Option<CoreEvent>> = vec![None; preds.len()];
    let wait = async {
        while got.iter().any(Option::is_none) {
            let Some(e) = core.events.recv().await else {
                panic!("event channel closed");
            };
            if let Some(i) = (0..preds.len()).find(|&i| got[i].is_none() && preds[i](&e)) {
                got[i] = Some(e);
            }
        }
    };
    if tokio::time::timeout(DEFAULT_TIMEOUT, wait).await.is_err() {
        panic!("timed out; matched so far: {got:?}");
    }
    got.into_iter().map(Option::unwrap).collect()
}

fn is_ring_in(e: &CoreEvent, room_id: &str) -> bool {
    matches!(e, CoreEvent::IncomingCall { room_id: r, .. } if r.as_str() == room_id)
}

fn is_stop_in(e: &CoreEvent, room_id: &str) -> bool {
    matches!(e, CoreEvent::RingingStopped { room_id: r } if r.as_str() == room_id)
}

fn is_count(e: &CoreEvent, room_id: &str, n: u32) -> bool {
    matches!(e, CoreEvent::ActiveCall { room_id: r, participants } if r.as_str() == room_id && *participants == n)
}

/// A DM call (encrypted, as the SDK creates DMs) rings the other member;
/// answering stops it, and the room list counts the devices in the call.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_dm_call_rings_and_answering_stops_it() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_dm(&alice_raw, &env.user_id(3)).await.to_string();
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1).await;
    let mut carol = logged_in(dir_c.path(), 3).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;

    join(&mut alice, &room_id).await;
    let mut got = expect_all(
        &mut carol,
        &[&|e| is_ring_in(e, &room_id), &|e| is_count(e, &room_id, 1)],
    )
    .await;
    match got.swap_remove(0) {
        CoreEvent::IncomingCall {
            caller,
            ring,
            intent,
            ..
        } => {
            assert_eq!(caller.as_str(), env.user_id(1));
            assert!(ring, "a DM call rings");
            assert_eq!(intent, CallIntent::Audio);
        }
        other => panic!("{other:?}"),
    }

    // Answering: the ring stops (before `Connected` arrives, so wait for
    // all three together).
    carol
        .commands
        .send(Command::JoinCall {
            room_id: RoomId::from(room_id.as_str()),
            intent: CallIntent::Audio,
        })
        .await
        .unwrap();
    expect_all(
        &mut carol,
        &[
            &|e| matches!(e, CoreEvent::CallState(CallState::Connected { .. })),
            &|e| is_stop_in(e, &room_id),
            &|e| is_count(e, &room_id, 2),
        ],
    )
    .await;

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// Declining stops the ring here and tells the caller who declined.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn declining_stops_the_ring_and_tells_the_caller() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_dm(&alice_raw, &env.user_id(3)).await.to_string();
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1).await;
    let mut carol = logged_in(dir_c.path(), 3).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;

    join(&mut alice, &room_id).await;
    expect_event(&mut carol, |e| is_ring_in(e, &room_id)).await;
    carol
        .commands
        .send(Command::DeclineCall {
            room_id: RoomId::from(room_id.as_str()),
        })
        .await
        .unwrap();
    expect_event(&mut carol, |e| is_stop_in(e, &room_id)).await;
    let declined = expect_event(&mut alice, |e| matches!(e, CoreEvent::CallDeclined { .. })).await;
    match declined {
        CoreEvent::CallDeclined {
            room_id: r,
            user_id,
        } => {
            assert_eq!(r.as_str(), room_id);
            assert_eq!(user_id.as_str(), env.user_id(3));
        }
        other => panic!("{other:?}"),
    }

    // Nothing rings any more: a second decline is refused.
    carol
        .commands
        .send(Command::DeclineCall {
            room_id: RoomId::from(room_id.as_str()),
        })
        .await
        .unwrap();
    expect_event(&mut carol, |e| matches!(e, CoreEvent::Error(_))).await;

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// In a group room the first member's notification is quiet (`ring:
/// false`), and it stops when the call ends.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_group_call_notifies_quietly_until_it_ends() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1).await;
    let mut carol = logged_in(dir_c.path(), 3).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;

    join(&mut alice, &room_id).await;
    let note = expect_event(&mut carol, |e| is_ring_in(e, &room_id)).await;
    assert!(
        matches!(note, CoreEvent::IncomingCall { ring: false, .. }),
        "{note:?}"
    );
    alice.commands.send(Command::LeaveCall).await.unwrap();
    expect_all(
        &mut carol,
        &[&|e| is_stop_in(e, &room_id), &|e| is_count(e, &room_id, 0)],
    )
    .await;

    for core in [&alice, &carol] {
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// An MSC3417 call room is marked in the room list, a voice room.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn call_rooms_are_marked_in_the_room_list() {
    use matrix_sdk::ruma::api::client::room::create_room::v3::{CreationContent, Request};
    use matrix_sdk::ruma::room::RoomType;
    let alice_raw = raw_login(1).await;
    let mut content = CreationContent::new();
    content.room_type = Some(RoomType::from("org.matrix.msc3417.call"));
    let mut request = Request::new();
    request.name = Some(unique_room_name());
    request.creation_content = Some(matrix_sdk::ruma::serde::Raw::new(&content).unwrap());
    let room_id = alice_raw
        .create_room(request)
        .await
        .unwrap()
        .room_id()
        .to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir.path(), 1).await;
    let mut rooms: Vec<RoomSummary> = Vec::new();
    let wait = async {
        loop {
            if let Some(CoreEvent::RoomListChanged(diffs)) = alice.events.recv().await {
                for diff in diffs {
                    ListDiff::apply(&diff, &mut rooms);
                }
                if let Some(r) = rooms.iter().find(|r| r.id.as_str() == room_id) {
                    if r.is_call_room {
                        return r.clone();
                    }
                    eprintln!("listed, not (yet) as a call room: {r:?}");
                }
            }
        }
    };
    let summary = tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("listed as a call room");
    assert!(!summary.is_dm && !summary.is_space);
    alice.commands.send(Command::Shutdown).await.unwrap();
}
