//! Threads against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`): summaries
//! on thread roots, threaded replies hidden from the main timeline, the
//! thread timeline and actions in it.
//!
//! Alice (user 1) and carol (user 3) each run a core, never bob. The tests
//! share the two accounts, so they run one at a time. Nothing here prints
//! message text.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_room, expect_event, login_and_run, outgoing, raw_login,
    send_content, start_core, unique_room_name,
};
use matrix_sdk::ruma::events::relation::Thread;
use matrix_sdk::ruma::events::room::message::{Relation, RoomMessageEventContent};
use matrix_sdk::ruma::{OwnedEventId, OwnedRoomId};
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, AttachmentData, Command, CoreError, CoreEvent, EventId, ListDiff, MediaBytes,
    MediaKind, MessageDetails, OutgoingAttachment, PaginationState, RoomId, RoomSummary,
    TimelineItem, TimelineItemKind,
};
use tessera_core::secrets::InMemorySecretStore;

/// One user's core with the room's main timeline and the open thread's.
struct Side {
    core: CoreHandle,
    main: Vec<TimelineItem>,
    thread: Vec<TimelineItem>,
    /// The latest `ThreadPagination` state.
    pagination: Option<PaginationState>,
}

impl Side {
    async fn new(dir: &std::path::Path, env: &TestEnv, n: u8) -> Self {
        let mut core = start_core(dir, Arc::new(InMemorySecretStore::new()));
        login_and_run(&mut core, env, n).await;
        Self {
            core,
            main: Vec::new(),
            thread: Vec::new(),
            pagination: None,
        }
    }

    async fn send(&self, command: Command) {
        self.core.commands.send(command).await.unwrap();
    }

    /// Waits until the room is listed, then selects it.
    async fn open_room(&mut self, room: &str) {
        let mut rooms: Vec<RoomSummary> = Vec::new();
        let wait = async {
            while !rooms.iter().any(|r| r.id.as_str() == room) {
                match self.core.events.recv().await {
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
        tokio::time::timeout(DEFAULT_TIMEOUT, wait)
            .await
            .expect("timed out waiting for the room to sync");
        self.send(Command::SelectRoom(RoomId::from(room))).await;
    }

    /// Applies events until `done` holds. An `ActionFailed` fails the test.
    async fn wait(&mut self, room: &str, what: &str, done: impl Fn(&Side) -> bool) {
        let wait = async {
            while !done(self) {
                match self.core.events.recv().await {
                    Some(CoreEvent::TimelineDiff { room_id, diffs })
                        if room_id.as_str() == room =>
                    {
                        for diff in diffs {
                            ListDiff::apply(&diff, &mut self.main);
                        }
                    }
                    Some(CoreEvent::ThreadDiff { room_id, diffs, .. })
                        if room_id.as_str() == room =>
                    {
                        for diff in diffs {
                            ListDiff::apply(&diff, &mut self.thread);
                        }
                    }
                    Some(CoreEvent::ThreadPagination { state, .. }) => {
                        self.pagination = Some(state)
                    }
                    Some(CoreEvent::ActionFailed { action, error, .. }) => {
                        panic!("action {action:?} failed: {error:?}")
                    }
                    Some(_) => {}
                    None => panic!("event channel closed"),
                }
            }
        };
        tokio::time::timeout(DEFAULT_TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for: {what}"));
    }

    async fn logout(&mut self) {
        self.send(Command::Logout).await;
        expect_event(&mut self.core, |e| matches!(e, CoreEvent::LoggedOut)).await;
    }
}

/// The message with this plain body, if any.
fn message<'a>(
    items: &'a [TimelineItem],
    body: &str,
) -> Option<(&'a TimelineItem, &'a MessageDetails)> {
    items.iter().find_map(|i| match &i.kind {
        TimelineItemKind::Message {
            body: b, details, ..
        } if b == body => Some((i, &**details)),
        _ => None,
    })
}

fn marker() -> String {
    format!("tessera-marker-{}", common::hex())
}

/// Alice and carol in a fresh plain room, both with it open.
struct Rig {
    room: String,
    room_id: OwnedRoomId,
    alice: Side,
    carol: Side,
    carol_raw: matrix_sdk::Client,
    _dirs: [tempfile::TempDir; 2],
}

async fn rig() -> Rig {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3))).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    let room = room_id.to_string();
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut alice = Side::new(dirs[0].path(), &env, 1).await;
    let mut carol = Side::new(dirs[1].path(), &env, 3).await;
    alice.open_room(&room).await;
    carol.open_room(&room).await;
    Rig {
        room,
        room_id,
        alice,
        carol,
        carol_raw,
        _dirs: dirs,
    }
}

impl Rig {
    async fn finish(mut self) {
        self.alice.logout().await;
        self.carol.logout().await;
    }

    /// Alice sends a root message; both mains show it. Returns its text and
    /// event ID.
    async fn root(&mut self) -> (String, EventId) {
        let text = marker();
        self.alice
            .send(Command::Send(outgoing(&self.room, &text)))
            .await;
        let room = self.room.clone();
        self.alice
            .wait(&room, "alice's root is sent", |s| {
                message(&s.main, &text).is_some_and(|(i, _)| i.event_id.is_some())
            })
            .await;
        self.carol
            .wait(&room, "carol sees the root", |s| {
                message(&s.main, &text).is_some()
            })
            .await;
        let id = message(&self.alice.main, &text)
            .and_then(|(i, _)| i.event_id.clone())
            .expect("the root has an event ID");
        (text, id)
    }

    /// Carol opens the thread of `root` and sends `text` into it.
    async fn carol_replies(&mut self, root: &EventId, text: &str) {
        let room = self.room.clone();
        self.carol
            .send(Command::OpenThread {
                room_id: RoomId::from(room.as_str()),
                root: root.clone(),
            })
            .await;
        self.carol
            .wait(&room, "carol's thread opens with its root", |s| {
                s.pagination.is_some() && !s.thread.is_empty()
            })
            .await;
        let mut message = outgoing(&room, text);
        message.thread_root = Some(root.clone());
        self.carol.send(Command::Send(message)).await;
    }
}

fn summary_of<'a>(
    side: &'a Side,
    root_text: &str,
) -> Option<&'a tessera_core::api::ThreadSummaryView> {
    message(&side.main, root_text).and_then(|(_, d)| d.thread.as_ref())
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn summary_appears_on_the_root_and_the_reply_leaves_the_main_timeline() {
    let mut rig = rig().await;
    let (root_text, root) = rig.root().await;
    let reply = marker();
    rig.carol_replies(&root, &reply).await;

    let room = rig.room.clone();
    rig.alice
        .wait(&room, "alice sees the thread summary", |s| {
            summary_of(s, &root_text).is_some_and(|t| t.replies == 1)
        })
        .await;
    let summary = summary_of(&rig.alice, &root_text).unwrap();
    assert_eq!(
        summary.latest_sender.as_ref().map(|u| u.as_str()),
        Some(TestEnv::load().user_id(3).as_str())
    );
    // Give the main timeline time to (wrongly) show the reply.
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        rig.alice.wait(&room, "never", |_| false),
    )
    .await;
    assert!(
        message(&rig.alice.main, &reply).is_none(),
        "alice's main shows the reply"
    );
    assert!(
        message(&rig.carol.main, &reply).is_none(),
        "carol's main shows the reply"
    );
    rig.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn thread_timeline_has_prefixed_ids_and_sending_in_it_updates_the_summary() {
    let mut rig = rig().await;
    let (root_text, root) = rig.root().await;
    let reply = marker();
    rig.carol_replies(&root, &reply).await;

    let room = rig.room.clone();
    rig.alice
        .send(Command::OpenThread {
            room_id: RoomId::from(room.as_str()),
            root: root.clone(),
        })
        .await;
    rig.alice
        .wait(&room, "alice's thread shows the root and the reply", |s| {
            message(&s.thread, &root_text).is_some() && message(&s.thread, &reply).is_some()
        })
        .await;
    let prefix = format!("thread:{}:", root.as_str());
    for item in &rig.alice.thread {
        assert!(item.id.starts_with(&prefix), "thread item id {:?}", item.id);
    }
    for item in &rig.alice.main {
        assert!(!item.id.starts_with("thread:"));
    }

    let own = marker();
    let mut message_in_thread = outgoing(&room, &own);
    message_in_thread.thread_root = Some(root.clone());
    rig.alice.send(Command::Send(message_in_thread)).await;
    rig.alice
        .wait(&room, "the summary counts both replies", |s| {
            summary_of(s, &root_text).is_some_and(|t| t.replies == 2)
        })
        .await;
    // The reply is in the thread, not the main timeline.
    assert!(message(&rig.alice.thread, &own).is_some());
    assert!(message(&rig.alice.main, &own).is_none());
    rig.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn actions_in_a_thread_use_its_item_ids() {
    let mut rig = rig().await;
    let (_, root) = rig.root().await;
    let reply = marker();
    rig.carol_replies(&root, &reply).await;

    let room = rig.room.clone();
    rig.alice
        .send(Command::OpenThread {
            room_id: RoomId::from(room.as_str()),
            root: root.clone(),
        })
        .await;
    rig.alice
        .wait(&room, "alice's thread shows carol's reply", |s| {
            message(&s.thread, &reply).is_some_and(|(i, _)| i.event_id.is_some())
        })
        .await;
    let item_id = message(&rig.alice.thread, &reply).unwrap().0.id.clone();
    assert!(item_id.starts_with(&format!("thread:{}:", root.as_str())));
    rig.alice
        .send(Command::ToggleReaction {
            room_id: RoomId::from(room.as_str()),
            item_id,
            key: "👍".into(),
        })
        .await;
    rig.carol
        .wait(&room, "carol's thread shows the reaction", |s| {
            message(&s.thread, &reply)
                .is_some_and(|(_, d)| matches!(d.reactions.as_slice(), [g] if g.key == "👍" && g.count == 1 && !g.own))
        })
        .await;
    rig.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_thread_item_id_of_another_thread_is_not_found() {
    let mut rig = rig().await;
    let (_, root) = rig.root().await;
    let reply = marker();
    rig.carol_replies(&root, &reply).await;

    let room = rig.room.clone();
    rig.alice
        .send(Command::OpenThread {
            room_id: RoomId::from(room.as_str()),
            root: root.clone(),
        })
        .await;
    rig.alice
        .wait(&room, "alice's thread shows carol's reply", |s| {
            message(&s.thread, &reply).is_some_and(|(i, _)| i.event_id.is_some())
        })
        .await;
    let item_id = message(&rig.alice.thread, &reply).unwrap().0.id.clone();
    let counter = item_id.rsplit(':').next().unwrap().to_owned();
    // The same counter in another thread, and the old unbound form.
    for foreign in [
        format!("thread:$other:{counter}"),
        format!("thread:{counter}"),
    ] {
        rig.alice
            .send(Command::ToggleReaction {
                room_id: RoomId::from(room.as_str()),
                item_id: foreign,
                key: "👍".into(),
            })
            .await;
        let event = expect_event(&mut rig.alice.core, |e| {
            matches!(e, CoreEvent::ActionFailed { .. })
        })
        .await;
        let CoreEvent::ActionFailed { error, .. } = event else {
            unreachable!()
        };
        assert_eq!(error, CoreError::ItemNotFound);
    }
    rig.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn element_style_thread_reply_from_a_raw_client_is_counted() {
    let mut rig = rig().await;
    let (root_text, root) = rig.root().await;
    let root_id = OwnedEventId::try_from(root.as_str()).unwrap();
    let mut content = RoomMessageEventContent::text_plain(marker());
    content.relates_to = Some(Relation::Thread(Thread::plain(root_id.clone(), root_id)));
    send_content(&rig.carol_raw, &rig.room_id, &content).await;

    let room = rig.room.clone();
    rig.alice
        .wait(&room, "alice sees the raw client's reply counted", |s| {
            summary_of(s, &root_text).is_some_and(|t| t.replies == 1)
        })
        .await;
    rig.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_closed_thread_takes_no_sends_and_selecting_another_room_closes_it() {
    let mut rig = rig().await;
    let (_, root) = rig.root().await;
    let room = rig.room.clone();
    let open = Command::OpenThread {
        room_id: RoomId::from(room.as_str()),
        root: root.clone(),
    };
    let mut into_thread = outgoing(&room, &marker());
    into_thread.thread_root = Some(root.clone());

    // Never opened: refused.
    rig.alice.send(Command::Send(into_thread.clone())).await;
    expect_refused(&mut rig.alice, ActionKind::Send).await;

    rig.alice.send(open.clone()).await;
    rig.alice
        .wait(&room, "the thread opens", |s| !s.thread.is_empty())
        .await;
    rig.alice.send(Command::CloseThread).await;
    rig.alice.send(Command::Send(into_thread.clone())).await;
    expect_refused(&mut rig.alice, ActionKind::Send).await;

    // Another room closes it too.
    let env = TestEnv::load();
    let raw = raw_login(1).await;
    let other = create_room(&raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    rig.alice.send(open).await;
    rig.alice
        .wait(&room, "the thread reopens", |s| !s.thread.is_empty())
        .await;
    rig.alice.open_room(&other).await;
    rig.alice
        .send(Command::PaginateThread {
            room_id: RoomId::from(room.as_str()),
            root,
        })
        .await;
    expect_refused(&mut rig.alice, ActionKind::Thread).await;
    rig.finish().await;
}

fn thread_file(room: &str, name: &str, root: &EventId) -> OutgoingAttachment {
    OutgoingAttachment {
        room_id: RoomId::from(room),
        filename: name.to_owned(),
        mimetype: "application/octet-stream".into(),
        data: AttachmentData::Bytes(MediaBytes::from(vec![7; 2048])),
        kind: MediaKind::File,
        width: None,
        height: None,
        thumbnail: None,
        reply_to: None,
        thread_root: Some(root.clone()),
    }
}

/// The message whose media has this file name.
fn named_file<'a>(items: &'a [TimelineItem], name: &str) -> Option<&'a TimelineItem> {
    items.iter().find(|i| match &i.kind {
        TimelineItemKind::Message { details, .. } => {
            details.media.as_deref().is_some_and(|m| m.filename == name)
        }
        _ => false,
    })
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn an_attachment_sent_into_an_open_thread_lands_in_the_thread() {
    let mut rig = rig().await;
    let (root_text, root) = rig.root().await;
    let room = rig.room.clone();
    rig.carol
        .send(Command::OpenThread {
            room_id: RoomId::from(room.as_str()),
            root: root.clone(),
        })
        .await;
    rig.carol
        .wait(&room, "carol's thread opens", |s| !s.thread.is_empty())
        .await;
    let name = format!("thread-file-{}.bin", common::hex());
    rig.carol
        .send(Command::SendAttachment(thread_file(&room, &name, &root)))
        .await;
    rig.carol
        .wait(&room, "carol's thread shows the file", |s| {
            named_file(&s.thread, &name).is_some()
        })
        .await;
    rig.alice
        .wait(&room, "alice's summary counts the file", |s| {
            summary_of(s, &root_text).is_some_and(|t| t.replies == 1)
        })
        .await;
    // It is a thread reply: it never shows in either main timeline.
    let _ = tokio::time::timeout(
        Duration::from_secs(3),
        rig.alice.wait(&room, "never", |_| false),
    )
    .await;
    assert!(named_file(&rig.alice.main, &name).is_none());
    assert!(named_file(&rig.carol.main, &name).is_none());
    // Alice opens the thread and finds it there.
    rig.alice
        .send(Command::OpenThread {
            room_id: RoomId::from(room.as_str()),
            root: root.clone(),
        })
        .await;
    rig.alice
        .wait(&room, "alice's thread shows the file", |s| {
            named_file(&s.thread, &name).is_some()
        })
        .await;
    rig.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn an_attachment_for_a_thread_that_is_not_open_is_refused() {
    let mut rig = rig().await;
    let (_, root) = rig.root().await;
    let room = rig.room.clone();
    let name = format!("thread-file-{}.bin", common::hex());
    rig.alice
        .send(Command::SendAttachment(thread_file(&room, &name, &root)))
        .await;
    let event = expect_event(&mut rig.alice.core, |e| {
        matches!(
            e,
            CoreEvent::ActionFailed {
                action: ActionKind::Upload,
                ..
            }
        )
    })
    .await;
    let CoreEvent::ActionFailed { error, .. } = event else {
        unreachable!()
    };
    assert_eq!(error, CoreError::NotAllowed);
    rig.finish().await;
}

/// Waits for an `ActionFailed` of `kind` that is not an internal error.
async fn expect_refused(side: &mut Side, kind: ActionKind) {
    let event = expect_event(
        &mut side.core,
        |e| matches!(e, CoreEvent::ActionFailed { action, .. } if *action == kind),
    )
    .await;
    let CoreEvent::ActionFailed { error, .. } = event else {
        unreachable!()
    };
    assert!(
        !matches!(error, CoreError::Internal(_)),
        "refused with {error:?}"
    );
}
