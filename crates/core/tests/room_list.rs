//! Sync state and room list events against the local homeserver
//! (`scripts/stack.sh up -d --wait && scripts/create-test-users.sh`).

mod common;

use std::sync::Arc;

use common::{
    TestEnv, create_dm, create_room, create_space, expect_event, raw_login, start_core,
    start_core_with_page_size, unique_room_name,
};
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    Command, CoreEvent, ListDiff, Password, RoomListQuery, RoomListTab, RoomSummary, SyncState,
};
use twicecat_core::secrets::InMemorySecretStore;

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
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
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
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
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

    // The room list asks the server for one timeline event per room. A room
    // that first reaches the core together with both messages would show only
    // the latest of them (one unread), so on a slow machine the count could
    // never reach two. Send each message only once the core has caught up with
    // the previous state.
    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| r.iter().any(|s| s.name == name)).await;
    for (text, expected) in [("first", 1), ("second", 2)] {
        room.send(RoomMessageEventContent::text_plain(text))
            .await
            .expect("alice sends");
        apply_until(&mut core, &mut rooms, |r| {
            r.iter()
                .any(|s| s.name == name && s.unread_messages >= expected)
        })
        .await;
    }

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn last_message_preview_shows_own_message() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    let alice = raw_login(1).await;
    let name = unique_room_name();
    let room_id = create_room(&alice, &name, None).await;
    let room = alice.get_room(&room_id).expect("alice knows the room");

    // The core must know the room before the message, so the preview comes
    // from a latest event it registered for (and so that it is not only the
    // initial sync state).
    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| r.iter().any(|s| s.name == name)).await;
    let marker = unique_room_name();
    room.send(RoomMessageEventContent::text_plain(marker.clone()))
        .await
        .expect("alice sends");
    apply_until(&mut core, &mut rooms, |r| {
        r.iter().any(|s| {
            s.name == name
                && s.last_message
                    .as_ref()
                    .is_some_and(|m| m.body == marker && m.is_own)
        })
    })
    .await;

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn dm_and_space_flags() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    let alice = raw_login(1).await;
    let dm_id = create_dm(&alice, &env.user_id(2)).await;
    let space_name = unique_room_name();
    let space_id = create_space(&alice, &space_name).await;

    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| {
        r.iter().any(|s| s.id.as_str() == dm_id.as_str() && s.is_dm)
            && r.iter()
                .any(|s| s.id.as_str() == space_id.as_str() && s.is_space)
    })
    .await;
    let dm = rooms.iter().find(|s| s.id.as_str() == dm_id.as_str());
    assert!(!dm.expect("dm listed").is_space);
    let space = rooms.iter().find(|s| s.id.as_str() == space_id.as_str());
    assert!(!space.expect("space listed").is_dm);

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn room_list_loaded_is_signalled() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    core.commands
        .send(Command::LoginPassword {
            username: env.user(1).to_owned(),
            password: Password::from(env.password(1)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    // The signal is not ordered against `Running`, so wait for it from the
    // moment sync starts; the 30 s default timeout bounds the wait.
    let event = expect_event(&mut core, |e| matches!(e, CoreEvent::RoomListLoaded { .. })).await;
    assert!(matches!(
        event,
        CoreEvent::RoomListLoaded {
            total_rooms: Some(_)
        }
    ));

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

fn has(rooms: &[RoomSummary], id: &matrix_sdk::ruma::OwnedRoomId) -> bool {
    rooms.iter().any(|s| s.id.as_str() == id.as_str())
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn search_query_filters_the_list() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    let alice = raw_login(1).await;
    let suffix = unique_room_name();
    let name_a = format!("twicecat-a-{suffix}");
    let name_b = format!("twicecat-b-{suffix}");
    create_room(&alice, &name_a, None).await;
    create_room(&alice, &name_b, None).await;

    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| {
        r.iter().any(|s| s.name == name_a) && r.iter().any(|s| s.name == name_b)
    })
    .await;

    core.commands
        .send(Command::SetRoomListQuery(RoomListQuery {
            tab: RoomListTab::All,
            search: name_a.clone(),
            space: None,
        }))
        .await
        .unwrap();
    // The new filter arrives as a `Reset`, so the list ends up with only the
    // first room.
    apply_until(&mut core, &mut rooms, |r| {
        r.len() == 1 && r[0].name == name_a
    })
    .await;

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn tab_queries_split_dms_rooms_and_spaces() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    let alice = raw_login(1).await;
    let dm = create_dm(&alice, &env.user_id(2)).await;
    let space = create_space(&alice, &unique_room_name()).await;
    let room = create_room(&alice, &unique_room_name(), None).await;

    let mut rooms = Vec::new();
    apply_until(&mut core, &mut rooms, |r| {
        has(r, &dm) && has(r, &space) && has(r, &room)
    })
    .await;

    // The old state still holds everything until the new filter's `Reset`
    // arrives, so each wait names a state the previous tab cannot satisfy.
    let set_tab = |tab| {
        Command::SetRoomListQuery(RoomListQuery {
            tab,
            search: String::new(),
            space: None,
        })
    };
    core.commands
        .send(set_tab(RoomListTab::People))
        .await
        .unwrap();
    apply_until(&mut core, &mut rooms, |r| {
        has(r, &dm) && !has(r, &space) && !has(r, &room)
    })
    .await;

    core.commands
        .send(set_tab(RoomListTab::Rooms))
        .await
        .unwrap();
    apply_until(&mut core, &mut rooms, |r| {
        has(r, &room) && !has(r, &dm) && !has(r, &space)
    })
    .await;

    core.commands
        .send(set_tab(RoomListTab::Spaces))
        .await
        .unwrap();
    apply_until(&mut core, &mut rooms, |r| {
        has(r, &space) && !has(r, &dm) && !has(r, &room)
    })
    .await;

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn load_more_rooms_adds_a_page() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let alice = raw_login(1).await;
    for _ in 0..3 {
        create_room(&alice, &unique_room_name(), None).await;
    }

    let mut core = start_core_with_page_size(dir.path(), Arc::new(InMemorySecretStore::new()), 2);
    login_and_run(&mut core, &env, 1).await;

    // Paging needs the total, so wait for the page and for the signal.
    let mut rooms: Vec<RoomSummary> = Vec::new();
    let mut loaded = false;
    let wait = async {
        while !(loaded && rooms.len() == 2) {
            match core.events.recv().await {
                Some(CoreEvent::RoomListChanged(diffs)) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut rooms);
                    }
                }
                Some(CoreEvent::RoomListLoaded { .. }) => loaded = true,
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(common::DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for the first page");
    assert_eq!(rooms.len(), 2, "only one page is listed");

    core.commands.send(Command::LoadMoreRooms).await.unwrap();
    apply_until(&mut core, &mut rooms, |r| r.len() >= 3).await;

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}
