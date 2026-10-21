//! A space's rooms in `SpaceView.rooms` against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Alice (user 1) runs a core. The rooms and spaces are made up and left at
//! the end. Nothing here prints names or IDs.

mod common;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_room_raw, create_space_raw, leave_all_created, start_core,
    unique_room_name,
};
use matrix_sdk::ruma::OwnedRoomId;
use std::sync::Arc;
use twicecat_core::api::{Command, CoreEvent, Password, RoomId, SpaceView};
use twicecat_core::secrets::InMemorySecretStore;

fn ids(view: &SpaceView) -> Vec<&str> {
    let mut ids: Vec<&str> = view.rooms.iter().map(RoomId::as_str).collect();
    ids.sort_unstable();
    ids
}

fn sorted(rooms: &[&OwnedRoomId]) -> Vec<String> {
    let mut ids: Vec<String> = rooms.iter().map(|r| r.to_string()).collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_space_view_lists_its_rooms_by_level() {
    let env = TestEnv::load();
    let room_a = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let room_b = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let room_c = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let sub = create_space_raw(1, &unique_room_name(), std::slice::from_ref(&room_c)).await;
    let top = create_space_raw(
        1,
        &unique_room_name(),
        &[room_a.clone(), room_b.clone(), sub.clone()],
    )
    .await;

    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    // Login by hand: a `Spaces` event can arrive before sync reports Running,
    // and `login_and_run` would drop it.
    core.commands
        .send(Command::LoginPassword {
            username: env.user(1).to_owned(),
            password: Password::from(env.password(1)),
            server: None,
        })
        .await
        .unwrap();

    let (top_id, sub_id) = (top.to_string(), sub.to_string());
    let mut latest: Vec<SpaceView> = Vec::new();
    let ready = |list: &[SpaceView]| {
        list.iter()
            .any(|v| v.id.as_str() == top_id && v.level == 0 && v.rooms.len() == 3)
            && list
                .iter()
                .any(|v| v.id.as_str() == sub_id && v.level == 1 && v.rooms.len() == 2)
    };
    let wait = async {
        while !ready(&latest) {
            match core.events.recv().await {
                Some(CoreEvent::Spaces(list)) => latest = list,
                Some(_) => {}
                None => panic!("event channel closed while waiting for the spaces"),
            }
        }
    };
    if tokio::time::timeout(DEFAULT_TIMEOUT, wait).await.is_err() {
        // Shapes only: levels and counts, never IDs or names.
        let shapes: Vec<(u32, u64, usize)> = latest
            .iter()
            .map(|v| (v.level, v.children, v.rooms.len()))
            .collect();
        panic!(
            "timed out waiting for both spaces with their rooms; last (level, children, rooms): {shapes:?}"
        );
    }
    let list = latest;

    // Level 0: the direct children, the sub-space included.
    let level0 = list.iter().find(|v| v.id.as_str() == top_id).unwrap();
    assert_eq!(ids(level0), sorted(&[&room_a, &room_b, &sub]));
    assert_eq!(level0.children, 3);
    // Level 1: all the sub-space's descendants, and, as the SDK builds them
    // (`flattened_bottom_up_subtree`), the sub-space itself, last.
    let level1 = list.iter().find(|v| v.id.as_str() == sub_id).unwrap();
    assert_eq!(ids(level1), sorted(&[&room_c, &sub]));
    assert_eq!(level1.rooms.last().unwrap().as_str(), sub.as_str());
    // Debug shows the count only.
    let text = format!("{level0:?}");
    assert!(text.contains("rooms: 3"));
    assert!(!text.contains(room_a.as_str()) && !text.contains(sub.as_str()));

    core.commands.send(Command::Logout).await.unwrap();
    common::expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
    leave_all_created(1, &[top, sub, room_a, room_b, room_c]).await;
}
