//! Notifications, notification settings and unread totals against the local
//! homeserver (`docker compose up -d --wait && scripts/create-test-users.sh`).
//!
//! Alice (raw SDK client, never syncing) sends; carol runs the core under
//! test. Bob is left alone. Push rules are account data and shared by every
//! test here, so the tests run one at a time and each starts from
//! `reset_notification_baseline`. A test that expects *no* notification is
//! bounded without sleeps (Phase 5 Q19): the event that must not notify is
//! followed by two sentinels in the DM; the second sentinel arrives in a later
//! sync than the first, so everything the first sync produced is in by then.
//! Nothing here prints message bodies or keywords.

mod common;

use std::sync::Arc;

use common::{
    DEFAULT_TIMEOUT, TestEnv, assert_local_homeserver, create_dm, create_room, hex, login_and_run,
    outgoing, raw_login, reset_notification_baseline, send_content, send_text, start_core,
};
use matrix_sdk::ruma::api::client::membership::joined_rooms;
use matrix_sdk::ruma::events::Mentions;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId};
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, Command, CoreEvent, ListDiff, NotificationKind, NotificationSettingsView,
    NotificationView, RoomId, RoomNotificationModeView, RoomSummary,
};
use tessera_core::secrets::InMemorySecretStore;

/// All tests in this binary share two accounts' push rules.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What the core has reported so far.
#[derive(Default)]
struct Seen {
    notifications: Vec<NotificationView>,
    settings: Option<NotificationSettingsView>,
    settings_events: usize,
    totals: Vec<(u64, u64)>,
    rooms: Vec<RoomSummary>,
    failures: Vec<(ActionKind, RoomId)>,
}

impl Seen {
    fn apply(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::Notification(n) => self.notifications.push(n),
            CoreEvent::NotificationSettings(s) => {
                self.settings = Some(s);
                self.settings_events += 1;
            }
            CoreEvent::UnreadTotals {
                notifications,
                highlights,
            } => self.totals.push((notifications, highlights)),
            CoreEvent::RoomListChanged(diffs) => {
                for diff in diffs {
                    ListDiff::apply(&diff, &mut self.rooms);
                }
            }
            CoreEvent::ActionFailed {
                room_id, action, ..
            } => self.failures.push((action, room_id)),
            _ => {}
        }
    }

    fn room(&self, id: &OwnedRoomId) -> Option<&RoomSummary> {
        self.rooms.iter().find(|r| r.id.as_str() == id.as_str())
    }

    fn has_notification(&self, event_text: &str) -> bool {
        self.notifications
            .iter()
            .any(|n| n.preview.as_ref().is_some_and(|p| p.expose() == event_text))
    }
}

struct Fixture {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    alice: matrix_sdk::Client,
    carol_raw: matrix_sdk::Client,
    alice_id: String,
    carol_id: String,
    core: CoreHandle,
    seen: Seen,
    /// A DM of alice and carol, for notifying sentinels.
    dm: OwnedRoomId,
    created: Vec<OwnedRoomId>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// Resets both accounts' notification settings, creates the DM (carol
    /// creates it, so her `m.direct` is right), and logs carol's core in.
    async fn new() -> Self {
        Self::start(false).await
    }

    /// Like `new`; `empty_carol` first makes carol leave every room, so the
    /// unread totals start at zero.
    async fn start(empty_carol: bool) -> Self {
        let guard = SERIAL.lock().await;
        let env = TestEnv::load();
        assert_local_homeserver(env.homeserver_url());
        if empty_carol {
            leave_every_room(3).await;
        }
        reset_notification_baseline(1).await;
        reset_notification_baseline(3).await;
        let alice = raw_login(1).await;
        let carol_raw = raw_login(3).await;
        let alice_id = env.user_id(1);
        let carol_id = env.user_id(3);
        let dm = create_dm(&carol_raw, &alice_id).await;
        alice
            .join_room_by_id(&dm)
            .await
            .expect("alice joins the dm");

        let dir = tempfile::tempdir().unwrap();
        let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
        login_and_run(&mut core, &env, 3).await;
        let mut fixture = Self {
            _guard: guard,
            alice,
            carol_raw,
            alice_id,
            carol_id,
            core,
            seen: Seen::default(),
            dm: dm.clone(),
            created: vec![dm.clone()],
            _dir: dir,
        };
        // The settings arrive after login, and the DM in the room list.
        fixture
            .wait("the first settings and the dm", |s| {
                s.settings.is_some() && s.room(&dm).is_some()
            })
            .await;
        fixture
    }

    /// Applies core events until `done` holds; panics after the default
    /// timeout.
    async fn wait(&mut self, what: &str, done: impl Fn(&Seen) -> bool) {
        let Self { core, seen, .. } = self;
        let wait = async {
            while !done(seen) {
                match core.events.recv().await {
                    Some(event) => seen.apply(event),
                    None => panic!("event channel closed while waiting for {what}"),
                }
            }
        };
        tokio::time::timeout(DEFAULT_TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// Sends a command and waits for the next `NotificationSettings` event,
    /// which the core sends when the change has been saved.
    async fn change_settings(&mut self, command: Command) {
        let before = self.seen.settings_events;
        let failures = self.seen.failures.len();
        self.core.commands.send(command).await.unwrap();
        self.wait("the settings change", |s| {
            s.settings_events > before || s.failures.len() > failures
        })
        .await;
        assert_eq!(self.seen.failures.len(), failures, "the change failed");
    }

    /// Waits until the published group default is `mode`.
    ///
    /// The SDK's push-rule cache is eventually consistent: `NotificationSettings`
    /// applies a write locally, but a push-rules account-data event from the
    /// sync (the echo of an *earlier* write) replaces the whole cache when it
    /// arrives. An echo that lands after a later local write briefly shows the
    /// older value, and the core publishes it (as the forwarder does after every
    /// push-rules change). The echo of the latest write follows and corrects
    /// it, so a read straight after the change event is not enough.
    async fn wait_group_default(&mut self, mode: RoomNotificationModeView) {
        self.wait("the group default mode", |s| {
            s.settings.as_ref().map(|s| s.group_default) == Some(mode)
        })
        .await;
    }

    /// Waits until a notification with this preview has arrived.
    async fn wait_for(&mut self, text: &str) -> NotificationView {
        self.wait("a notification", |s| s.has_notification(text))
            .await;
        self.seen
            .notifications
            .iter()
            .find(|n| n.preview.as_ref().is_some_and(|p| p.expose() == text))
            .cloned()
            .unwrap()
    }

    /// Sends two sentinels into the DM and waits for both notifications. All
    /// notifications of the syncs that carried anything sent before this call
    /// are in `seen` afterwards.
    async fn fence(&mut self) {
        for n in 1..=2 {
            let text = format!("fence{n}-{}", hex());
            send_text(&self.alice, &self.dm, &text).await;
            self.wait_for(&text).await;
        }
    }

    async fn create_group(&mut self) -> OwnedRoomId {
        let room = create_room(
            &self.alice,
            &format!("tessera-test-{}", hex()),
            Some(&self.carol_id),
        )
        .await;
        self.carol_raw
            .join_room_by_id(&room)
            .await
            .expect("carol joins the group");
        self.created.push(room.clone());
        let id = room.clone();
        self.wait("the group in the room list", |s| s.room(&id).is_some())
            .await;
        room
    }

    async fn finish(mut self) {
        self.core.commands.send(Command::Logout).await.unwrap();
        common::expect_event(&mut self.core, |e| matches!(e, CoreEvent::LoggedOut)).await;
        common::leave_all_created(3, &self.created).await;
        common::leave_all_created(1, &self.created).await;
        reset_notification_baseline(3).await;
    }
}

fn mention_of(user: &str, body: &str) -> RoomMessageEventContent {
    let user: OwnedUserId = user.try_into().expect("valid user id");
    RoomMessageEventContent::text_plain(body).add_mentions(Mentions::with_user_ids([user]))
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_dm_message_notifies_with_the_preview() {
    let mut f = Fixture::new().await;
    let text = format!("dm-{}", hex());
    send_text(&f.alice, &f.dm, &text).await;
    let n = f.wait_for(&text).await;
    assert_eq!(n.kind, NotificationKind::Message);
    assert!(n.is_dm, "the room is a DM");
    assert_eq!(n.sender.as_str(), f.alice_id);
    assert_eq!(n.room_id.as_str(), f.dm.as_str());
    assert!(!n.room_name.is_empty());
    assert!(!format!("{n:?}").contains(&text), "Debug hides the preview");
    f.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_mention_in_a_muted_room_is_silent_until_the_mode_is_reset() {
    let mut f = Fixture::new().await;
    let group = f.create_group().await;
    let group_id = RoomId::from(group.as_str());

    f.change_settings(Command::SetRoomNotificationMode {
        room_id: group_id.clone(),
        mode: Some(RoomNotificationModeView::Mute),
    })
    .await;
    // The room list carries the mode once the sync has delivered the rule.
    f.wait("the muted mode in the room list", |s| {
        s.room(&group)
            .is_some_and(|r| r.notification_mode == Some(RoomNotificationModeView::Mute))
    })
    .await;

    let muted = format!("muted-{}", hex());
    send_content(&f.alice, &group, &mention_of(&f.carol_id, &muted)).await;
    f.fence().await;
    assert!(
        !f.seen.has_notification(&muted),
        "a muted room must not notify"
    );
    assert!(
        f.seen
            .notifications
            .iter()
            .all(|n| n.room_id.as_str() != group.as_str() || n.kind == NotificationKind::Invite),
        "nothing from the muted room but the invitation"
    );

    f.change_settings(Command::SetRoomNotificationMode {
        room_id: group_id,
        mode: None,
    })
    .await;
    // The room list refreshes a room when the room itself changes, not when
    // only its cached notification mode does, so a message makes it look
    // again.
    let probe = format!("probe-{}", hex());
    send_content(&f.alice, &group, &mention_of(&f.carol_id, &probe)).await;
    f.wait("the default mode in the room list", |s| {
        s.room(&group)
            .is_some_and(|r| r.notification_mode.is_none())
    })
    .await;
    let loud = format!("loud-{}", hex());
    send_content(&f.alice, &group, &mention_of(&f.carol_id, &loud)).await;
    let n = f.wait_for(&loud).await;
    assert_eq!(n.kind, NotificationKind::Mention);
    assert!(!n.is_dm);
    assert_eq!(n.sender.as_str(), f.alice_id);
    f.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_keyword_notifies_in_a_mentions_only_room() {
    let mut f = Fixture::new().await;
    let group = f.create_group().await;
    let keyword = format!("tessera-kw-{}", hex());

    f.change_settings(Command::AddKeyword {
        keyword: format!("  {keyword}  "),
    })
    .await;
    assert!(
        f.seen
            .settings
            .as_ref()
            .is_some_and(|s| s.keywords.contains(&keyword)),
        "the trimmed keyword is listed"
    );
    f.change_settings(Command::SetRoomNotificationMode {
        room_id: RoomId::from(group.as_str()),
        mode: Some(RoomNotificationModeView::MentionsAndKeywords),
    })
    .await;
    f.wait("the mode in the room list", |s| {
        s.room(&group).is_some_and(|r| {
            r.notification_mode == Some(RoomNotificationModeView::MentionsAndKeywords)
        })
    })
    .await;

    let plain = format!("plain-{}", hex());
    send_text(&f.alice, &group, &plain).await;
    f.fence().await;
    assert!(
        !f.seen.has_notification(&plain),
        "a plain message is silent"
    );

    let with_keyword = format!("hello {keyword} {}", hex());
    send_text(&f.alice, &group, &with_keyword).await;
    let n = f.wait_for(&with_keyword).await;
    assert_eq!(n.kind, NotificationKind::Mention);
    assert_eq!(n.room_id.as_str(), group.as_str());

    // Removing the keyword takes it off the list.
    f.change_settings(Command::RemoveKeyword {
        keyword: keyword.clone(),
    })
    .await;
    assert!(
        f.seen
            .settings
            .as_ref()
            .is_some_and(|s| !s.keywords.contains(&keyword))
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn bad_keywords_and_default_modes_are_handled() {
    let mut f = Fixture::new().await;
    // A bad keyword is refused with an action failure, not saved.
    for keyword in ["   ".to_owned(), "k".repeat(65)] {
        let failures = f.seen.failures.len();
        f.core
            .commands
            .send(Command::AddKeyword { keyword })
            .await
            .unwrap();
        f.wait("the refusal", |s| s.failures.len() > failures).await;
        assert_eq!(
            f.seen.failures.last().map(|(a, _)| *a),
            Some(ActionKind::Notifications)
        );
    }
    // A default mode is saved, read back, and put back.
    f.change_settings(Command::SetDefaultNotificationMode {
        one_to_one: false,
        mode: RoomNotificationModeView::MentionsAndKeywords,
    })
    .await;
    f.wait_group_default(RoomNotificationModeView::MentionsAndKeywords)
        .await;
    f.change_settings(Command::SetDefaultNotificationMode {
        one_to_one: false,
        mode: RoomNotificationModeView::AllMessages,
    })
    .await;
    f.wait_group_default(RoomNotificationModeView::AllMessages)
        .await;
    f.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn own_messages_never_notify() {
    let mut f = Fixture::new().await;
    let room = RoomId::from(f.dm.as_str());
    f.core
        .commands
        .send(Command::SelectRoom(room.clone()))
        .await
        .unwrap();
    let own = format!("own-{}", hex());
    f.core
        .commands
        .send(Command::Send(outgoing(room.as_str(), &own)))
        .await
        .unwrap();
    f.fence().await;
    assert!(!f.seen.has_notification(&own));
    assert!(
        f.seen
            .notifications
            .iter()
            .all(|n| n.sender.as_str() != f.carol_id),
        "no notification for carol's own events"
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn an_invite_notifies() {
    let mut f = Fixture::new().await;
    let room = create_room(
        &f.alice,
        &format!("tessera-test-{}", hex()),
        Some(&f.carol_id),
    )
    .await;
    f.created.push(room.clone());
    f.wait("the invite notification", |s| {
        s.notifications
            .iter()
            .any(|n| n.room_id.as_str() == room.as_str())
    })
    .await;
    let n = f
        .seen
        .notifications
        .iter()
        .find(|n| n.room_id.as_str() == room.as_str())
        .unwrap();
    assert_eq!(n.kind, NotificationKind::Invite);
    assert_eq!(n.sender.as_str(), f.alice_id);
    assert!(n.preview.is_none());
    f.finish().await;
}

/// Makes test user `n` leave and forget every room it has joined, so that the
/// unread totals start at zero.
async fn leave_every_room(n: u8) {
    use matrix_sdk::ruma::api::client::membership::{forget_room, leave_room};
    assert_local_homeserver(TestEnv::load().homeserver_url());
    let client = raw_login(n).await;
    let rooms = client
        .send(joined_rooms::v3::Request::new())
        .await
        .expect("list joined rooms")
        .joined_rooms;
    for room in &rooms {
        client
            .send(leave_room::v3::Request::new(room.clone()))
            .await
            .expect("leave room");
        client
            .send(forget_room::v3::Request::new(room.clone()))
            .await
            .expect("forget room");
    }
    println!("user {n} left {} rooms", rooms.len());
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn unread_totals_follow_a_message_and_mark_read() {
    // Carol starts without rooms, so every count below is hers alone.
    let mut f = Fixture::start(true).await;
    // The first value (zero: the DM is empty) went out while the login
    // helper was waiting for the sync, so only changes are seen here.
    assert!(f.seen.totals.iter().all(|t| *t == (0, 0)));

    let before = f.seen.totals.len();
    let text = format!("count-{}", hex());
    send_text(&f.alice, &f.dm, &text).await;
    f.wait_for(&text).await;
    f.wait("one unread notification", |s| {
        s.totals.len() > before && s.totals.last().is_some_and(|t| t.0 == 1)
    })
    .await;

    let before = f.seen.totals.len();
    f.core
        .commands
        .send(Command::SelectRoom(RoomId::from(f.dm.as_str())))
        .await
        .unwrap();
    f.core
        .commands
        .send(Command::MarkRead {
            room_id: RoomId::from(f.dm.as_str()),
            fully_read: true,
        })
        .await
        .unwrap();
    f.wait("totals back at zero", |s| {
        s.totals.len() > before && s.totals.last() == Some(&(0, 0))
    })
    .await;
    f.finish().await;
}
