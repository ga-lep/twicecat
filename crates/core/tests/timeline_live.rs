//! Back-pagination, read receipts and typing notices against the local
//! homeserver (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Alice and carol take part, never bob (see `timeline.rs`). Nothing here
//! prints message bodies.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TestEnv, create_room, expect_event, raw_login, send_text, start_core};
use common::{set_display_name, unique_room_name};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    Command, CoreEvent, ListDiff, PaginationState, Password, RoomId, RoomSummary, SyncState,
    TimelineItem, TimelineItemKind,
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

async fn open_room(core: &mut CoreHandle, room: &str) {
    wait_for_room(core, room).await;
    core.commands
        .send(Command::SelectRoom(RoomId::from(room)))
        .await
        .unwrap();
}

async fn logout(core: &mut CoreHandle) {
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

/// Applies timeline diffs of `room_id` to `items` until `done` holds. An
/// `ActionFailed` fails the test.
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
                        if done(items) {
                            break;
                        }
                    }
                }
                Some(CoreEvent::ActionFailed { action, error, .. }) => {
                    panic!("action {action:?} failed: {error:?}")
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

/// The plain bodies of the messages, in timeline order.
fn bodies(items: &[TimelineItem]) -> Vec<&str> {
    items
        .iter()
        .filter_map(|i| match &i.kind {
            TimelineItemKind::Message { body, .. } => Some(body.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn back_pagination_loads_the_whole_room_in_order() {
    const COUNT: usize = 60;
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), None).await;
    let expected: Vec<String> = (1..=COUNT).map(|i| format!("n{i:02}")).collect();
    for body in &expected {
        send_text(&alice_raw, &room_id, body).await;
    }
    let room = room_id.to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    open_room(&mut alice, &room).await;

    let mut items: Vec<TimelineItem> = Vec::new();
    let mut last = None;
    let wait = async {
        loop {
            // A request that arrives while a pagination still holds its slot
            // is dropped (the app asks again later), so ask again when quiet.
            let event =
                match tokio::time::timeout(Duration::from_secs(3), alice.events.recv()).await {
                    Ok(Some(event)) => event,
                    Ok(None) => panic!("event channel closed"),
                    Err(_) => {
                        if last == Some(PaginationState::Idle) {
                            alice
                                .commands
                                .send(Command::PaginateBackwards {
                                    room_id: RoomId::from(room.as_str()),
                                })
                                .await
                                .unwrap();
                        }
                        continue;
                    }
                };
            match event {
                CoreEvent::TimelineDiff { room_id: r, diffs } if r.as_str() == room => {
                    for diff in diffs {
                        ListDiff::apply(&diff, &mut items);
                    }
                }
                CoreEvent::Pagination { room_id: r, state } if r.as_str() == room => {
                    last = Some(state);
                    match state {
                        PaginationState::Idle => {
                            alice
                                .commands
                                .send(Command::PaginateBackwards {
                                    room_id: RoomId::from(room.as_str()),
                                })
                                .await
                                .unwrap();
                        }
                        PaginationState::Loading => {}
                        PaginationState::ReachedStart => {
                            // A limited sync can still clear and refill the
                            // list; stop once all messages are in.
                            if bodies(&items).len() == COUNT {
                                break;
                            }
                        }
                    }
                }
                CoreEvent::ActionFailed { action, error, .. } => {
                    panic!("action {action:?} failed: {error:?}")
                }
                _ => {}
            }
            if last == Some(PaginationState::ReachedStart) && bodies(&items).len() == COUNT {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), wait)
        .await
        .expect("timed out paginating to the start of the room");

    // The status event and the diff that inserts `TimelineStart` come from
    // different forwarders, so the diff may still be on its way.
    apply_until(&mut alice, &room, &mut items, |items| {
        items
            .iter()
            .any(|i| matches!(i.kind, TimelineItemKind::TimelineStart))
    })
    .await;
    assert!(
        matches!(
            items.first().map(|i| &i.kind),
            Some(TimelineItemKind::TimelineStart)
        ),
        "the start of the room comes first"
    );
    let got = bodies(&items);
    assert_eq!(got, expected.iter().map(String::as_str).collect::<Vec<_>>());
    logout(&mut alice).await;
}

/// Alice and carol in a fresh room, each with a running core that has the
/// room open.
struct Pair {
    room: String,
    alice_raw: matrix_sdk::Client,
    alice_id: String,
    alice: CoreHandle,
    carol: CoreHandle,
    alice_items: Vec<TimelineItem>,
    carol_items: Vec<TimelineItem>,
    room_id: matrix_sdk::ruma::OwnedRoomId,
    _dirs: [tempfile::TempDir; 2],
}

async fn pair() -> Pair {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let name = unique_room_name();
    let room_id = create_room(&alice_raw, &name, Some(&env.user_id(3))).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    set_display_name(&carol_raw, &format!("carol-{}", &name[name.len() - 8..])).await;
    let room = room_id.to_string();

    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut alice = start_core(dirs[0].path(), Arc::new(InMemorySecretStore::new()));
    let mut carol = start_core(dirs[1].path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    login_and_run(&mut carol, &env, 3).await;
    open_room(&mut alice, &room).await;
    open_room(&mut carol, &room).await;
    Pair {
        room,
        alice_raw,
        alice_id: env.user_id(1),
        alice,
        carol,
        alice_items: Vec::new(),
        carol_items: Vec::new(),
        room_id,
        _dirs: dirs,
    }
}

impl Pair {
    async fn finish(mut self) {
        logout(&mut self.alice).await;
        logout(&mut self.carol).await;
    }
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_read_receipt_reaches_the_sender() {
    let mut p = pair().await;
    let carol_id = TestEnv::load().user_id(3);
    send_text(&p.alice_raw, &p.room_id, "read me").await;
    let has_message = |items: &[TimelineItem]| bodies(items).contains(&"read me");
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, has_message).await;
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, has_message).await;

    p.carol
        .commands
        .send(Command::MarkRead {
            room_id: RoomId::from(p.room.as_str()),
            fully_read: false,
        })
        .await
        .unwrap();

    let read_by_carol = |items: &[TimelineItem]| {
        items.iter().rev().find_map(|i| match &i.kind {
            TimelineItemKind::Message { .. } => {
                Some(i.read_by.iter().any(|u| u.as_str() == carol_id))
            }
            _ => None,
        }) == Some(true)
    };
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, read_by_carol).await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn typing_notices_reach_the_other_user() {
    let mut p = pair().await;
    let room = p.room.clone();
    let set = |typing| Command::SetTyping {
        room_id: RoomId::from(room.as_str()),
        typing,
    };
    // Carol's room subscription is applied asynchronously, so a notice sent
    // right after selecting can pass before she listens: send it again.
    let mut typing_users = None;
    for _ in 0..4 {
        p.alice.commands.send(set(true)).await.unwrap();
        let wait = async {
            while let Some(event) = p.carol.events.recv().await {
                if let CoreEvent::Typing { users, .. } = event
                    && !users.is_empty()
                {
                    return Some(users);
                }
            }
            None
        };
        if let Ok(Some(users)) = tokio::time::timeout(Duration::from_secs(8), wait).await {
            typing_users = Some(users);
            break;
        }
        p.alice.commands.send(set(false)).await.unwrap();
    }
    let users = typing_users.expect("carol never saw alice typing");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].user_id.as_str(), p.alice_id);

    p.alice.commands.send(set(false)).await.unwrap();
    expect_event(
        &mut p.carol,
        |e| matches!(e, CoreEvent::Typing { users, .. } if users.is_empty()),
    )
    .await;
    // Leave the unused field read.
    let _ = (&p.carol_items, &p.alice_items);
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_draft_is_kept_per_room_and_cleared_on_request() {
    use twicecat_core::api::{Draft, MentionRef, UserId};

    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let first = create_room(&alice_raw, &unique_room_name(), None)
        .await
        .to_string();
    let second = create_room(&alice_raw, &unique_room_name(), None)
        .await
        .to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    // Both rooms, in one pass: they may arrive in the same list update.
    let mut rooms: Vec<RoomSummary> = Vec::new();
    let both = async {
        while ![&first, &second]
            .iter()
            .all(|id| rooms.iter().any(|r| r.id.as_str() == id.as_str()))
        {
            match alice.events.recv().await {
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
    tokio::time::timeout(common::DEFAULT_TIMEOUT, both)
        .await
        .expect("timed out waiting for both rooms to sync");
    alice
        .commands
        .send(Command::SelectRoom(RoomId::from(first.as_str())))
        .await
        .unwrap();

    let draft = Draft {
        text: twicecat_core::api::MessageText::from("half a thought for @Carol"),
        mentions: vec![MentionRef {
            label: "@Carol".into(),
            user_id: UserId::from(env.user_id(3)),
        }],
        reply_to: None,
    };
    let save = |draft: Option<Draft>| Command::SaveDraft {
        room_id: RoomId::from(first.as_str()),
        draft,
    };
    alice
        .commands
        .send(save(Some(draft.clone())))
        .await
        .unwrap();
    alice
        .commands
        .send(Command::SelectRoom(RoomId::from(second.as_str())))
        .await
        .unwrap();
    alice
        .commands
        .send(Command::SelectRoom(RoomId::from(first.as_str())))
        .await
        .unwrap();
    let loaded = expect_event(
        &mut alice,
        |e| matches!(e, CoreEvent::DraftLoaded { room_id, .. } if room_id.as_str() == first),
    )
    .await;
    let CoreEvent::DraftLoaded { draft: back, .. } = loaded else {
        unreachable!()
    };
    assert_eq!(back, draft);

    // Cleared: reopening the room gives its timeline and no draft.
    alice.commands.send(save(None)).await.unwrap();
    alice
        .commands
        .send(Command::SelectRoom(RoomId::from(first.as_str())))
        .await
        .unwrap();
    let mut saw_reset = false;
    let wait = async {
        loop {
            match alice.events.recv().await {
                Some(CoreEvent::TimelineDiff { room_id, diffs }) if room_id.as_str() == first => {
                    if diffs.iter().any(|d| matches!(d, ListDiff::Reset { .. })) {
                        saw_reset = true;
                    }
                }
                Some(CoreEvent::DraftLoaded { .. }) => panic!("a cleared draft came back"),
                Some(_) => {}
                None => break,
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(3), wait).await;
    assert!(saw_reset, "the room reopened");
    logout(&mut alice).await;
}
