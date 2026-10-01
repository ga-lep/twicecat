//! Timeline subscription and sending against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! The receiving side is carol, not bob: `login::restore_without_logging_in_again`
//! counts bob's devices and needs bob to stay otherwise unused.

mod common;

use std::sync::Arc;

use common::{TestEnv, create_room, expect_event, raw_login, start_core, unique_room_name};
use tessera_core::CoreHandle;
use tessera_core::api::{
    Command, CoreError, CoreEvent, ListDiff, MessageText, Password, RoomId, RoomSummary,
    SendStatus, SyncState, TimelineItem, TimelineItemKind,
};
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

/// Waits until the room list contains `room_id`, so `SelectRoom` can find it.
async fn wait_for_room(core: &mut CoreHandle, room_id: &str) {
    let mut rooms: Vec<RoomSummary> = Vec::new();
    let wait = async {
        while !rooms.iter().any(|r| r.id.as_str() == room_id) {
            match core.events.recv().await {
                Some(CoreEvent::RoomListChanged(diffs)) => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut rooms);
                    }
                }
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(common::DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for the room to sync");
}

/// Applies timeline diffs of `room_id` to `items` until `done` holds.
async fn apply_until(
    core: &mut CoreHandle,
    room_id: &str,
    items: &mut Vec<TimelineItem>,
    done: impl Fn(&[TimelineItem]) -> bool,
) {
    let wait = async {
        while !done(items) {
            match core.events.recv().await {
                Some(CoreEvent::TimelineDiff { room_id: r, diffs }) if r.as_str() == room_id => {
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
        .expect("timed out waiting for the timeline");
}

fn has_message(
    items: &[TimelineItem],
    marker: &str,
    pred: impl Fn(&UserIdStr, bool, SendStatus) -> bool,
) -> bool {
    items.iter().any(|i| match &i.kind {
        TimelineItemKind::Message {
            body,
            sender,
            is_own,
            sent,
            ..
        } => body == marker && pred(&sender.as_str().to_owned(), *is_own, *sent),
        TimelineItemKind::Other => false,
    })
}

type UserIdStr = String;

fn unique_marker() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("tessera-marker-{hex}")
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn message_travels_from_a_to_b_through_the_timeline() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let name = unique_room_name();
    let room_id = create_room(&alice_raw, &name, Some(&env.user_id(3))).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    let room = room_id.to_string();

    let carol_dir = tempfile::tempdir().unwrap();
    let mut carol = start_core(carol_dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut carol, &env, 3).await;
    wait_for_room(&mut carol, &room).await;
    carol
        .commands
        .send(Command::SelectRoom(RoomId::from(room.as_str())))
        .await
        .unwrap();
    // The first timeline event is the initial state as a single `Reset`.
    let first = expect_event(
        &mut carol,
        |e| matches!(e, CoreEvent::TimelineDiff { room_id, .. } if room_id.as_str() == room),
    )
    .await;
    let CoreEvent::TimelineDiff { diffs, .. } = first else {
        unreachable!("filtered above");
    };
    assert!(matches!(diffs.as_slice(), [ListDiff::Reset { .. }]));
    let mut carol_items = Vec::new();
    for diff in &diffs {
        ListDiff::apply(diff, &mut carol_items);
    }

    let alice_dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(alice_dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    wait_for_room(&mut alice, &room).await;
    alice
        .commands
        .send(Command::SelectRoom(RoomId::from(room.as_str())))
        .await
        .unwrap();
    let marker = unique_marker();
    alice
        .commands
        .send(Command::SendMessage {
            text: MessageText::from(marker.as_str()),
        })
        .await
        .unwrap();

    let alice_id = env.user_id(1);
    let mut alice_items = Vec::new();
    apply_until(&mut alice, &room, &mut alice_items, |items| {
        has_message(items, &marker, |_, own, sent| {
            own && sent == SendStatus::Sent
        })
    })
    .await;

    apply_until(&mut carol, &room, &mut carol_items, |items| {
        has_message(items, &marker, |sender, own, _| sender == &alice_id && !own)
    })
    .await;

    for core in [&mut alice, &mut carol] {
        core.commands.send(Command::Logout).await.unwrap();
        expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
    }
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn send_without_a_selected_room_is_an_error() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    core.commands
        .send(Command::SendMessage {
            text: MessageText::from("never sent"),
        })
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(matches!(event, CoreEvent::Error(CoreError::NoRoomSelected)));

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn selecting_an_unknown_room_is_an_error() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut core, &env, 1).await;

    let unknown = RoomId::from("!does-not-exist:localhost");
    core.commands
        .send(Command::SelectRoom(unknown.clone()))
        .await
        .unwrap();
    let event = expect_event(&mut core, |e| matches!(e, CoreEvent::Error(_))).await;
    let CoreEvent::Error(error) = event else {
        unreachable!("filtered above");
    };
    assert_eq!(error, CoreError::UnknownRoom(unknown));

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}
