//! Sync state and room list events against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).

mod common;

use std::sync::Arc;

use common::{TestEnv, create_room, expect_event, raw_login, start_core, unique_room_name};
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use tessera_core::CoreHandle;
use tessera_core::api::{Command, CoreEvent, ListDiff, Password, RoomSummary, SyncState};
use tessera_core::secrets::InMemorySecretStore;

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

/// Applies room list events to `rooms` until `done` holds for it.
async fn apply_until(
    core: &mut CoreHandle,
    rooms: &mut Vec<RoomSummary>,
    done: impl Fn(&[RoomSummary]) -> bool,
) {
    let wait = async {
        while !done(rooms) {
            match core.events.recv().await {
                Some(CoreEvent::RoomListChanged(diffs)) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, rooms);
                    }
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(common::DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for the room list");
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn room_appears_in_room_list() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    let alice = raw_login(1).await;
    let name = unique_room_name();
    let room_id = create_room(&alice, &name, None).await;

    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| r.iter().any(|s| s.name == name)).await;
    let summary = rooms.iter().find(|s| s.name == name).unwrap();
    assert_eq!(summary.id.as_str(), room_id.as_str());

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn unread_count_reaches_two() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 2).await;

    let alice = raw_login(1).await;
    let bob = raw_login(2).await;
    let name = unique_room_name();
    let room_id = create_room(&alice, &name, Some(&env.user_id(2))).await;
    bob.join_room_by_id(&room_id).await.expect("bob joins");
    let room = alice.get_room(&room_id).expect("alice knows the room");
    for text in ["first", "second"] {
        room.send(RoomMessageEventContent::text_plain(text))
            .await
            .expect("alice sends");
    }

    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| {
        r.iter().any(|s| s.name == name && s.unread_messages >= 2)
    })
    .await;

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}
