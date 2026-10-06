//! Timeline item view models against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! The receiving side is alice and the active side carol, never bob (see
//! `timeline.rs`). Nothing here prints message bodies.

mod common;

use std::sync::Arc;

use common::{
    TestEnv, create_room, expect_event, raw_login, send_content, set_display_name, start_core,
    unique_room_name,
};
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use tessera_core::CoreHandle;
use tessera_core::api::{
    Block, Command, CoreEvent, Inline, ListDiff, MentionTarget, MessageKind, Password, RoomId,
    RoomSummary, StateChange, SyncState, TimelineItem, TimelineItemKind,
};
use tessera_core::secrets::InMemorySecretStore;

/// Carol's display name is account-wide: Synapse copies a change into the
/// member event of every room she is in, asynchronously and with the latest
/// name at that moment. Two tests changing it at the same time can overwrite
/// each other's change before it reaches the other's room, so the tests that
/// change it take this lock (tests of one binary run in parallel).
static CAROL_PROFILE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

/// Position of the first state item after `from` that satisfies `pred`.
fn state_position(
    items: &[TimelineItem],
    from: usize,
    pred: impl Fn(&UserIdStr, &StateChange) -> bool,
) -> Option<usize> {
    items
        .iter()
        .enumerate()
        .skip(from)
        .find_map(|(i, item)| match &item.kind {
            TimelineItemKind::State { sender, change, .. }
                if pred(&sender.as_str().to_owned(), change) =>
            {
                Some(i)
            }
            _ => None,
        })
}

type UserIdStr = String;

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn membership_and_profile_changes_become_state_items() {
    let _profile = CAROL_PROFILE.lock().await;
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let name = unique_room_name();
    let room_id = create_room(&alice_raw, &name, Some(&env.user_id(3))).await;
    let room = room_id.to_string();

    let alice_dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(alice_dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    open_room(&mut alice, &room).await;

    // One change at a time, each seen before the next is made. Until the
    // room subscription made by `SelectRoom` takes effect, a sync response
    // may carry only the latest events of the room and be marked limited; the
    // SDK then clears the timeline and the earlier invite and join are gone
    // (seen in the full ignored run: only a later name change was left).
    // Waiting after each step keeps every response to one new event.
    let carol_id = env.user_id(3);
    let mut items = Vec::new();
    apply_until(&mut alice, &room, &mut items, |items| {
        state_position(items, 0, |_, c| matches!(c, StateChange::Invited { .. })).is_some()
    })
    .await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    apply_until(&mut alice, &room, &mut items, |items| {
        state_position(items, 0, |s, c| {
            s == &carol_id && matches!(c, StateChange::Joined | StateChange::InviteAccepted)
        })
        .is_some()
    })
    .await;
    let new_name = format!("carol-{}", &name[name.len() - 8..]);
    set_display_name(&carol_raw, &new_name).await;
    apply_until(&mut alice, &room, &mut items, |items| {
        let found = |from, pred: &dyn Fn(&UserIdStr, &StateChange) -> bool| {
            state_position(items, from, pred)
        };
        let Some(invited) = found(0, &|_, c| matches!(c, StateChange::Invited { .. })) else {
            return false;
        };
        let Some(joined) = found(invited + 1, &|s, c| {
            s == &carol_id && matches!(c, StateChange::Joined | StateChange::InviteAccepted)
        }) else {
            return false;
        };
        found(joined + 1, &|s, c| {
            s == &carol_id
                && matches!(c, StateChange::DisplayName { new: Some(n), .. } if n == &new_name)
        })
        .is_some()
    })
    .await;
    assert!(
        items
            .iter()
            .any(|i| matches!(i.kind, TimelineItemKind::DateDivider { .. })),
        "a date divider is expected"
    );

    logout(&mut alice).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn notices_and_emotes_become_messages_with_details() {
    let _profile = CAROL_PROFILE.lock().await;
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let name = unique_room_name();
    let room_id = create_room(&alice_raw, &name, Some(&env.user_id(3))).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    let carol_name = format!("carol-{}", &name[name.len() - 8..]);
    set_display_name(&carol_raw, &carol_name).await;
    let room = room_id.to_string();

    let alice_dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(alice_dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    open_room(&mut alice, &room).await;

    // The SDK clears the timeline when a limited sync response leaves a gap,
    // so each kind is remembered when first seen and the messages are sent
    // one after the other.
    let seen = |items: &[TimelineItem], wanted: MessageKind| {
        items.iter().any(|i| match &i.kind {
            TimelineItemKind::Message {
                details, is_own, ..
            } => details.kind == wanted && !is_own && details.sender_name.is_some(),
            _ => false,
        })
    };
    let mut items = Vec::new();
    send_content(
        &carol_raw,
        &room_id,
        &RoomMessageEventContent::notice_plain("a notice"),
    )
    .await;
    apply_until(&mut alice, &room, &mut items, |items| {
        seen(items, MessageKind::Notice)
    })
    .await;
    send_content(
        &carol_raw,
        &room_id,
        &RoomMessageEventContent::emote_plain("waves"),
    )
    .await;
    apply_until(&mut alice, &room, &mut items, |items| {
        seen(items, MessageKind::Emote)
    })
    .await;

    logout(&mut alice).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn formatted_body_becomes_rich_text_without_script_content() {
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

    let alice_dir = tempfile::tempdir().unwrap();
    let mut alice = start_core(alice_dir.path(), Arc::new(InMemorySecretStore::new()));
    login_and_run(&mut alice, &env, 1).await;
    open_room(&mut alice, &room).await;

    let alice_id = env.user_id(1);
    let html = format!(
        "<b>ok</b><script>alert(1)</script><a href=\"https://matrix.to/#/{alice_id}\">Alice</a>"
    );
    send_content(
        &carol_raw,
        &room_id,
        &RoomMessageEventContent::text_html("ok", html),
    )
    .await;

    let content_of = |items: &[TimelineItem]| {
        items.iter().find_map(|i| match &i.kind {
            TimelineItemKind::Message {
                details, is_own, ..
            } if !is_own => Some(details.content.clone()),
            _ => None,
        })
    };
    let mut items = Vec::new();
    apply_until(&mut alice, &room, &mut items, |items| {
        content_of(items).is_some()
    })
    .await;
    let content = content_of(&items).expect("message arrived");

    let mut inlines = Vec::new();
    for block in &content.blocks {
        if let Block::Paragraph(i) = block {
            inlines.extend(i.iter());
        }
    }
    assert!(inlines.iter().any(|i| matches!(
        i,
        Inline::Text { text, style } if text == "ok" && style.bold
    )));
    assert!(inlines.iter().any(|i| matches!(
        i,
        Inline::Mention { target: MentionTarget::User(u), .. } if u.as_str() == alice_id
    )));
    assert!(
        !inlines.iter().any(|i| matches!(
            i,
            Inline::Text { text, .. } if text.contains("alert")
        )),
        "script contents leaked into the rich text"
    );

    logout(&mut alice).await;
}
