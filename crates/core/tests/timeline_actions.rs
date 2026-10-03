//! Sending (markdown, mentions, replies, local echo, failure and discard)
//! against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Alice sends and carol receives, never bob (see `timeline.rs`). Nothing here
//! prints message bodies.

mod common;

use std::sync::Arc;

use common::{TestEnv, create_room, expect_event, raw_login, send_text, start_core};
use common::{set_display_name, unique_room_name};
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, Block, Command, CoreError, CoreEvent, Inline, ListDiff, MentionRef, MentionTarget,
    MessageKind, MessageText, OutgoingMessage, Password, PlaceholderKind, RoomId, RoomSummary,
    SendStatus, SyncState, TimelineItem, TimelineItemKind, UserId,
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

async fn open_room(core: &mut CoreHandle, room: &str) {
    wait_for_room(core, room).await;
    core.commands
        .send(Command::SelectRoom(RoomId::from(room)))
        .await
        .unwrap();
}

/// Applies timeline diffs of `room_id` to `items`, checking `done` after each
/// single diff, until it holds. An `ActionFailed` fails the test.
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

async fn logout(core: &mut CoreHandle) {
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

fn unique_marker() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("tessera-marker-{hex}")
}

fn outgoing(room: &str, text: &str) -> OutgoingMessage {
    OutgoingMessage {
        room_id: RoomId::from(room),
        text: MessageText::from(text),
        mentions: Vec::new(),
        reply_to: None,
        kind: MessageKind::Text,
    }
}

/// Alice and carol in a fresh room, each with a running core that has the
/// room open.
struct Pair {
    env: TestEnv,
    room: String,
    room_id: matrix_sdk::ruma::OwnedRoomId,
    carol_raw: matrix_sdk::Client,
    alice: CoreHandle,
    carol: CoreHandle,
    alice_items: Vec<TimelineItem>,
    carol_items: Vec<TimelineItem>,
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
        env,
        room,
        room_id,
        carol_raw,
        alice,
        carol,
        alice_items: Vec::new(),
        carol_items: Vec::new(),
        _dirs: dirs,
    }
}

impl Pair {
    async fn finish(mut self) {
        logout(&mut self.alice).await;
        logout(&mut self.carol).await;
    }
}

/// The inlines of all paragraphs.
fn paragraph_inlines(blocks: &[Block]) -> Vec<&Inline> {
    blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(i) => Some(i.iter()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// The first message of another user whose plain body is `body`.
fn received<'a>(
    items: &'a [TimelineItem],
    body: &str,
) -> Option<(&'a TimelineItem, &'a tessera_core::api::MessageDetails)> {
    items.iter().find_map(|i| match &i.kind {
        TimelineItemKind::Message {
            body: b,
            is_own: false,
            details,
            ..
        } if b == body => Some((i, &**details)),
        _ => None,
    })
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn markdown_round_trip() {
    let mut p = pair().await;
    let source = "**hi** `x`";
    p.alice
        .commands
        .send(Command::Send(outgoing(&p.room, source)))
        .await
        .unwrap();

    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        received(items, source).is_some()
    })
    .await;
    let (_, details) = received(&p.carol_items, source).expect("arrived");
    let inlines = paragraph_inlines(&details.content.blocks);
    assert!(inlines.iter().any(|i| matches!(
        i,
        Inline::Text { text, style } if text == "hi" && style.bold
    )));
    assert!(inlines.iter().any(|i| matches!(
        i,
        Inline::Text { text, style } if text == "x" && style.code
    )));
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn raw_html_typed_in_the_composer_is_not_sent_as_html() {
    let mut p = pair().await;
    let marker = unique_marker();
    let source = format!("**b** <img src=x onerror=alert(1)><script>alert(2)</script> {marker}");
    p.alice
        .commands
        .send(Command::Send(outgoing(&p.room, &source)))
        .await
        .unwrap();
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        received(items, &source).is_some()
    })
    .await;
    // Look at the event the server holds, not at our own mapping of it.
    let room = p.carol_raw.get_room(&p.room_id).expect("room");
    let options = matrix_sdk::room::MessagesOptions::backward();
    let events = room.messages(options).await.expect("messages").chunk;
    let contents: Vec<serde_json::Value> = events
        .iter()
        .filter_map(|e| serde_json::from_str::<serde_json::Value>(e.raw().json().get()).ok())
        .map(|v| v["content"].clone())
        .filter(|c| c["body"].as_str().is_some_and(|b| b.contains(&marker)))
        .collect();
    let [content] = contents.as_slice() else {
        panic!("expected one event, found {}", contents.len());
    };
    let html = content["formatted_body"]
        .as_str()
        .expect("a formatted body")
        .to_lowercase();
    assert!(html.contains("<strong>b</strong>"), "{html}");
    for bad in ["<img", "<script", "onerror"] {
        assert!(!html.contains(bad), "{bad} survived in the formatted body");
    }
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reply_round_trip() {
    let mut p = pair().await;
    let carol_id = p.env.user_id(3);
    let marker = unique_marker();
    let original = format!("{marker} first line\nsecond line");
    send_text(&p.carol_raw, &p.room_id, &original).await;

    // Alice sees carol's message and replies to it.
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        received(items, &original).is_some()
    })
    .await;
    let (item, details) = received(&p.alice_items, &original).expect("arrived");
    assert!(details.actions.can_reply);
    let item_id = item.id.clone();
    let mut reply = outgoing(&p.room, "the answer");
    reply.reply_to = Some(item_id);
    p.alice.commands.send(Command::Send(reply)).await.unwrap();

    // Carol's copy of alice's reply quotes carol's first line.
    let first_line = format!("{marker} first line");
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        received(items, "the answer").is_some_and(|(_, d)| {
            d.reply_to
                .as_ref()
                .is_some_and(|r| r.sender.as_str() == carol_id && r.body == first_line)
        })
    })
    .await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn mention_highlights_for_the_mentioned_user() {
    let mut p = pair().await;
    let carol_id = p.env.user_id(3);
    let marker = unique_marker();
    let text = format!("hello Carol Test {marker}");
    let mut message = outgoing(&p.room, &text);
    message.mentions = vec![MentionRef {
        label: "Carol Test".to_owned(),
        user_id: UserId::from(carol_id.clone()),
    }];
    p.alice.commands.send(Command::Send(message)).await.unwrap();

    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        received(items, &text).is_some()
    })
    .await;
    let (_, details) = received(&p.carol_items, &text).expect("arrived");
    assert!(details.highlighted, "the mention did not highlight");
    assert!(
        paragraph_inlines(&details.content.blocks)
            .iter()
            .any(|i| matches!(
                i,
                Inline::Mention { target: MentionTarget::User(u), .. } if u.as_str() == carol_id
            )),
        "no mention pill"
    );
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn local_echo_goes_from_sending_to_sent_under_one_id() {
    let mut p = pair().await;
    let marker = unique_marker();
    p.alice
        .commands
        .send(Command::Send(outgoing(&p.room, &marker)))
        .await
        .unwrap();

    let own = |items: &[TimelineItem]| {
        items.iter().find_map(|i| match &i.kind {
            TimelineItemKind::Message {
                body,
                is_own: true,
                sent,
                ..
            } if *body == marker => Some((i.id.clone(), *sent)),
            _ => None,
        })
    };
    let mut seen: Vec<(String, SendStatus)> = Vec::new();
    // `apply_until` checks after every diff, so the closure sees each state.
    let record = std::cell::RefCell::new(&mut seen);
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        if let Some(state) = own(items) {
            let mut seen = record.borrow_mut();
            if seen.last() != Some(&state) {
                seen.push(state.clone());
            }
            return state.1 == SendStatus::Sent;
        }
        false
    })
    .await;
    let first = seen.first().expect("an item appeared");
    let last = seen.last().expect("an item appeared");
    assert_eq!(first.1, SendStatus::Sending, "no Sending state: {seen:?}");
    assert_eq!(last.1, SendStatus::Sent);
    assert!(
        seen.iter().all(|(id, _)| *id == first.0),
        "the item ID changed: {seen:?}"
    );
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn too_large_message_fails_and_can_be_discarded() {
    let mut p = pair().await;
    let big = "a".repeat(70_000);
    p.alice
        .commands
        .send(Command::Send(outgoing(&p.room, &big)))
        .await
        .unwrap();

    let failed = |items: &[TimelineItem]| {
        items.iter().find_map(|i| match &i.kind {
            TimelineItemKind::Message {
                is_own: true,
                sent: SendStatus::Failed,
                details,
                ..
            } => Some((i.id.clone(), details.send_failure)),
            _ => None,
        })
    };
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        failed(items).is_some_and(|(_, f)| f.is_some())
    })
    .await;
    let (item_id, failure) = failed(&p.alice_items).expect("failed item");
    assert_eq!(
        failure.map(|f| f.recoverable),
        Some(false),
        "M_TOO_LARGE should not be recoverable"
    );

    p.alice
        .commands
        .send(Command::DiscardSend {
            room_id: RoomId::from(p.room.as_str()),
            item_id: item_id.clone(),
        })
        .await
        .unwrap();
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        items.iter().all(|i| i.id != item_id)
    })
    .await;

    // The queue works again.
    let marker = unique_marker();
    p.alice
        .commands
        .send(Command::Send(outgoing(&p.room, &marker)))
        .await
        .unwrap();
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        received(items, &marker).is_some()
    })
    .await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reply_to_an_unknown_item_or_action_in_another_room_fails() {
    let mut p = pair().await;
    let mut reply = outgoing(&p.room, "never sent");
    reply.reply_to = Some("no-such-item".to_owned());
    p.alice.commands.send(Command::Send(reply)).await.unwrap();
    let event = expect_event(&mut p.alice, |e| {
        matches!(e, CoreEvent::ActionFailed { .. })
    })
    .await;
    assert!(matches!(
        event,
        CoreEvent::ActionFailed {
            action: ActionKind::Send,
            error: CoreError::ItemNotFound,
            ..
        }
    ));

    // A room other than the selected one.
    let other = outgoing("!other:localhost", "never sent");
    p.alice.commands.send(Command::Send(other)).await.unwrap();
    let event = expect_event(&mut p.alice, |e| {
        matches!(e, CoreEvent::ActionFailed { .. })
    })
    .await;
    assert!(matches!(
        event,
        CoreEvent::ActionFailed {
            error: CoreError::NoRoomSelected,
            ..
        }
    ));
    p.finish().await;
}

/// The message item with this ID, if it is one.
fn message_by_id<'a>(
    items: &'a [TimelineItem],
    id: &str,
) -> Option<&'a tessera_core::api::MessageDetails> {
    items
        .iter()
        .find(|i| i.id == id)
        .and_then(|i| match &i.kind {
            TimelineItemKind::Message { details, .. } => Some(&**details),
            _ => None,
        })
}

/// Alice's own sent message with this body: its item ID.
fn own_sent(items: &[TimelineItem], body: &str) -> Option<String> {
    items.iter().find_map(|i| match &i.kind {
        TimelineItemKind::Message {
            body: b,
            is_own: true,
            sent: SendStatus::Sent,
            ..
        } if b == body => Some(i.id.clone()),
        _ => None,
    })
}

/// Alice sends `body`; both sides have it. Returns (alice's item ID,
/// carol's item ID).
async fn send_and_receive(p: &mut Pair, body: &str) -> (String, String) {
    p.alice
        .commands
        .send(Command::Send(outgoing(&p.room, body)))
        .await
        .unwrap();
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        own_sent(items, body).is_some()
    })
    .await;
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        received(items, body).is_some()
    })
    .await;
    let alice_id = own_sent(&p.alice_items, body).expect("sent");
    let carol_id = received(&p.carol_items, body)
        .expect("arrived")
        .0
        .id
        .clone();
    (alice_id, carol_id)
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn edit_keeps_the_item_id_and_marks_it_edited() {
    let mut p = pair().await;
    let marker = unique_marker();
    let (alice_item, carol_item) = send_and_receive(&mut p, &marker).await;

    p.alice
        .commands
        .send(Command::Edit {
            item_id: alice_item,
            message: outgoing(&p.room, "**new**"),
        })
        .await
        .unwrap();
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        message_by_id(items, &carol_item).is_some_and(|d| d.edited)
    })
    .await;
    let details = message_by_id(&p.carol_items, &carol_item).expect("same item");
    assert!(
        paragraph_inlines(&details.content.blocks)
            .iter()
            .any(|i| matches!(
                i,
                Inline::Text { text, style } if text == "new" && style.bold
            )),
        "no bold `new`"
    );
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn edit_of_a_message_that_is_not_the_users_is_refused() {
    let mut p = pair().await;
    let marker = unique_marker();
    let (_, carol_item) = send_and_receive(&mut p, &marker).await;
    p.carol
        .commands
        .send(Command::Edit {
            item_id: carol_item,
            message: outgoing(&p.room, "mine now"),
        })
        .await
        .unwrap();
    let event = expect_event(&mut p.carol, |e| {
        matches!(e, CoreEvent::ActionFailed { .. })
    })
    .await;
    assert!(matches!(
        event,
        CoreEvent::ActionFailed {
            action: ActionKind::Edit,
            error: CoreError::NotAllowed,
            ..
        }
    ));
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn redact_turns_the_item_into_a_placeholder() {
    let mut p = pair().await;
    let marker = unique_marker();
    let (alice_item, carol_item) = send_and_receive(&mut p, &marker).await;

    p.alice
        .commands
        .send(Command::Redact {
            room_id: RoomId::from(p.room.as_str()),
            item_id: alice_item,
            reason: Some(MessageText::from("spam")),
        })
        .await
        .unwrap();
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        items.iter().any(|i| {
            i.id == carol_item
                && matches!(
                    i.kind,
                    TimelineItemKind::Placeholder {
                        what: PlaceholderKind::Redacted,
                        ..
                    }
                )
        })
    })
    .await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reaction_toggles_on_and_off_for_both_sides() {
    let mut p = pair().await;
    let marker = unique_marker();
    let (alice_item, carol_item) = send_and_receive(&mut p, &marker).await;
    let toggle = || Command::ToggleReaction {
        room_id: RoomId::from(p.room.as_str()),
        item_id: carol_item.clone(),
        key: " 👍 ".to_owned(),
    };

    p.carol.commands.send(toggle()).await.unwrap();
    let thumbs = |own: bool| move |d: &tessera_core::api::MessageDetails| matches!(d.reactions.as_slice(), [g] if g.key == "👍" && g.count == 1 && g.own == own);
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        message_by_id(items, &alice_item).is_some_and(thumbs(false))
    })
    .await;
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        message_by_id(items, &carol_item).is_some_and(thumbs(true))
    })
    .await;

    p.carol.commands.send(toggle()).await.unwrap();
    apply_until(&mut p.alice, &p.room, &mut p.alice_items, |items| {
        message_by_id(items, &alice_item).is_some_and(|d| d.reactions.is_empty())
    })
    .await;
    apply_until(&mut p.carol, &p.room, &mut p.carol_items, |items| {
        message_by_id(items, &carol_item).is_some_and(|d| d.reactions.is_empty())
    })
    .await;

    // An empty key is refused.
    p.carol
        .commands
        .send(Command::ToggleReaction {
            room_id: RoomId::from(p.room.as_str()),
            item_id: carol_item.clone(),
            key: "  ".to_owned(),
        })
        .await
        .unwrap();
    let event = expect_event(&mut p.carol, |e| {
        matches!(e, CoreEvent::ActionFailed { .. })
    })
    .await;
    assert!(matches!(
        event,
        CoreEvent::ActionFailed {
            action: ActionKind::React,
            error: CoreError::NotAllowed,
            ..
        }
    ));
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn member_search_finds_carol_for_alice() {
    let mut p = pair().await;
    let carol_id = p.env.user_id(3);
    p.alice
        .commands
        .send(Command::SearchMembers {
            room_id: RoomId::from(p.room.as_str()),
            query: "car".to_owned(),
        })
        .await
        .unwrap();
    let event = expect_event(&mut p.alice, |e| {
        matches!(
            e,
            CoreEvent::Members { .. } | CoreEvent::ActionFailed { .. }
        )
    })
    .await;
    let CoreEvent::Members { members, .. } = event else {
        panic!("unexpected event: {event:?}");
    };
    assert!(
        members.iter().any(|m| m.user_id.as_str() == carol_id),
        "carol not found"
    );
    assert!(members.len() <= 8);
    p.finish().await;
}
