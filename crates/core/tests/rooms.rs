//! Rooms, members, the user's profile and spaces against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`), and smoke
//! tests for the Phase 5 raw-client test helpers in `common`.
//!
//! Alice (user 1) and carol (user 3) each run a core. The tests share the two
//! accounts, so they run one at a time. Created rooms are left at the end.
//! Nothing here prints names, topics or message text.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, TestEnv, create_room_raw, create_space_raw, fetch_raw_event_json, hex,
    leave_all_created, raw_login, reset_notification_baseline, send_attachment_raw, start_core,
    unique_room_name,
};
use matrix_sdk::ruma::api::client::state::get_state_events;
use matrix_sdk::ruma::{OwnedRoomId, RoomId as SdkRoomId};
use serde_json::Value;
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, Command, CoreError, CoreEvent, CreateRoomRequest, ListDiff, MediaBytes,
    MembershipView, OwnProfileView, Password, Role, RoomDetailsView, RoomId, RoomListQuery,
    RoomMemberView, RoomPermissions, RoomSummary, RoomVisibility, SpaceView, SyncState, UserId,
};
use tessera_core::secrets::InMemorySecretStore;

/// All state events of `room` as seen by `client`, as JSON.
async fn state_events(client: &matrix_sdk::Client, room: &SdkRoomId) -> Vec<Value> {
    client
        .send(get_state_events::v3::Request::new(room.to_owned()))
        .await
        .expect("fetch room state")
        .room_state
        .iter()
        .map(|raw| serde_json::from_str(raw.json().get()).expect("state event is JSON"))
        .collect()
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn create_room_raw_encrypted_is_readable_by_the_invitee() {
    let env = TestEnv::load();
    let carol_id = env.user_id(2);
    let room = create_room_raw(1, &unique_room_name(), true, &[carol_id.as_str()]).await;

    let carol = raw_login(2).await;
    carol.join_room_by_id(&room).await.expect("carol joins");
    let state = state_events(&carol, &room).await;
    let encryption = state
        .iter()
        .find(|e| e["type"] == "m.room.encryption")
        .expect("carol can read m.room.encryption");
    assert_eq!(encryption["content"]["algorithm"], "m.megolm.v1.aes-sha2");

    leave_all_created(2, std::slice::from_ref(&room)).await;
    leave_all_created(1, &[room]).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn create_room_raw_plain_has_no_encryption() {
    let room = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let alice = raw_login(1).await;
    let state = state_events(&alice, &room).await;
    assert!(state.iter().all(|e| e["type"] != "m.room.encryption"));
    leave_all_created(1, &[room]).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn create_space_raw_has_the_space_type_and_one_child() {
    let child = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let space = create_space_raw(1, &unique_room_name(), std::slice::from_ref(&child)).await;

    let alice = raw_login(1).await;
    let state = state_events(&alice, &space).await;
    let create = state
        .iter()
        .find(|e| e["type"] == "m.room.create")
        .expect("create event");
    assert_eq!(create["content"]["type"], "m.space");
    let children: Vec<&Value> = state
        .iter()
        .filter(|e| e["type"] == "m.space.child")
        .collect();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["state_key"], child.as_str());

    leave_all_created(1, &[space, child]).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn reset_notification_baseline_leaves_no_test_keyword() {
    let alice = raw_login(1).await;
    let settings = alice.notification_settings().await;
    let keyword = format!("tessera-kw-{}", common::hex());
    settings
        .add_keyword(keyword.clone())
        .await
        .expect("add keyword");
    assert!(settings.enabled_keywords().await.contains(&keyword));

    reset_notification_baseline(1).await;

    let fresh = raw_login(1).await.notification_settings().await;
    let left: Vec<String> = fresh
        .enabled_keywords()
        .await
        .into_iter()
        .filter(|k| k.starts_with("tessera-kw-"))
        .collect();
    assert!(left.is_empty(), "{} test keywords left", left.len());
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn send_attachment_raw_posts_an_image_event() {
    let room = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let bytes = include_bytes!("fixtures/small-64x48.png");
    let event = send_attachment_raw(1, &room, "small.png", "image/png", bytes.to_vec()).await;

    let alice = raw_login(1).await;
    let json = fetch_raw_event_json(&alice, &room, &event).await;
    assert_eq!(json["type"], "m.room.message");
    assert_eq!(json["content"]["msgtype"], "m.image");
    assert_eq!(json["content"]["info"]["mimetype"], "image/png");
    leave_all_created(1, &[room]).await;
}

// ---- The core's room, member, profile and space commands -------------------

/// All tests below share two accounts.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// What a core has reported so far.
#[derive(Default)]
struct Seen {
    logged_in: bool,
    running: bool,
    rooms: Vec<RoomSummary>,
    created: Vec<RoomId>,
    joined: Vec<RoomId>,
    details: Vec<RoomDetailsView>,
    members: Vec<(RoomId, Vec<RoomMemberView>, bool)>,
    profile: Vec<OwnProfileView>,
    spaces: Option<Vec<SpaceView>>,
    failures: Vec<(ActionKind, RoomId, CoreError)>,
    errors: usize,
}

impl Seen {
    fn apply(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::LoggedIn { .. } => self.logged_in = true,
            CoreEvent::SyncState(SyncState::Running) => self.running = true,
            CoreEvent::RoomListChanged(diffs) => {
                for diff in diffs {
                    ListDiff::apply(&diff, &mut self.rooms);
                }
            }
            CoreEvent::RoomCreated { room_id } => self.created.push(room_id),
            CoreEvent::RoomJoined { room_id } => self.joined.push(room_id),
            CoreEvent::RoomDetails(view) => self.details.push(view),
            CoreEvent::RoomMembers {
                room_id,
                members,
                truncated,
            } => self.members.push((room_id, members, truncated)),
            CoreEvent::OwnProfile(profile) => self.profile.push(profile),
            CoreEvent::Spaces(spaces) => self.spaces = Some(spaces),
            CoreEvent::ActionFailed {
                room_id,
                action,
                error,
            } => self.failures.push((action, room_id, error)),
            CoreEvent::Error(_) => self.errors += 1,
            _ => {}
        }
    }

    fn room(&self, id: &OwnedRoomId) -> Option<&RoomSummary> {
        self.rooms.iter().find(|r| r.id.as_str() == id.as_str())
    }
}

/// A core logged in as one test user.
struct Side {
    core: CoreHandle,
    seen: Seen,
    _dir: tempfile::TempDir,
}

impl Side {
    async fn login(n: u8) -> Self {
        let env = TestEnv::load();
        let dir = tempfile::tempdir().unwrap();
        let core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
        core.commands
            .send(Command::LoginPassword {
                username: env.user(n).to_owned(),
                password: Password::from(env.password(n)),
                server: None,
            })
            .await
            .unwrap();
        let mut side = Self {
            core,
            seen: Seen::default(),
            _dir: dir,
        };
        side.wait("the login and the first sync", |s| s.logged_in && s.running)
            .await;
        side
    }

    /// Applies events until `done` holds; panics after the default timeout.
    async fn wait(&mut self, what: &str, done: impl Fn(&Seen) -> bool) {
        self.wait_within(DEFAULT_TIMEOUT, what, done).await;
    }

    async fn wait_within(&mut self, timeout: Duration, what: &str, done: impl Fn(&Seen) -> bool) {
        let Self { core, seen, .. } = self;
        let wait = async {
            while !done(seen) {
                match core.events.recv().await {
                    Some(event) => seen.apply(event),
                    None => panic!("event channel closed while waiting for {what}"),
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// Applies whatever events arrive during `time`.
    async fn drain_for(&mut self, time: Duration) {
        let Self { core, seen, .. } = self;
        let _ = tokio::time::timeout(time, async {
            while let Some(event) = core.events.recv().await {
                seen.apply(event);
            }
        })
        .await;
    }

    async fn send(&self, command: Command) {
        self.core.commands.send(command).await.unwrap();
    }

    /// Creates a room and returns its ID once the core reports it.
    async fn create(&mut self, request: CreateRoomRequest) -> OwnedRoomId {
        let before = self.seen.created.len();
        self.send(Command::CreateRoom(request)).await;
        self.wait("RoomCreated", |s| s.created.len() > before).await;
        let id = self.seen.created.last().unwrap().clone();
        OwnedRoomId::try_from(id.as_str()).expect("the core reports a valid room ID")
    }

    /// Asks for the members until `ok` accepts them.
    async fn members_until(
        &mut self,
        room: &OwnedRoomId,
        what: &str,
        ok: impl Fn(&[RoomMemberView]) -> bool,
    ) -> Vec<RoomMemberView> {
        let deadline = tokio::time::Instant::now() + DEFAULT_TIMEOUT;
        loop {
            let before = self.seen.members.len();
            self.send(Command::ListRoomMembers {
                room_id: RoomId::from(room.as_str()),
            })
            .await;
            self.wait("the member list", |s| s.members.len() > before)
                .await;
            let (_, members, truncated) = self.seen.members.last().unwrap().clone();
            assert!(!truncated);
            if ok(&members) {
                return members;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn logout(mut self) {
        self.send(Command::Logout).await;
        common::expect_event(&mut self.core, |e| matches!(e, CoreEvent::LoggedOut)).await;
    }
}

fn private_room(
    name: &str,
    topic: Option<&str>,
    encrypted: bool,
    invitees: &[&str],
) -> CreateRoomRequest {
    CreateRoomRequest {
        name: name.to_owned(),
        topic: topic.map(ToOwned::to_owned),
        encrypted,
        visibility: RoomVisibility::Private,
        invitees: invitees.iter().map(|u| UserId::from(*u)).collect(),
    }
}

fn api_id(id: &OwnedRoomId) -> RoomId {
    RoomId::from(id.as_str())
}

/// Alice and carol, logged in, and what the tests need about both.
struct Pair {
    _guard: tokio::sync::MutexGuard<'static, ()>,
    alice: Side,
    carol: Side,
    carol_id: String,
    created: Vec<OwnedRoomId>,
}

impl Pair {
    async fn new() -> Self {
        let guard = SERIAL.lock().await;
        let env = TestEnv::load();
        let alice = Side::login(1).await;
        let carol = Side::login(3).await;
        Self {
            _guard: guard,
            alice,
            carol,
            carol_id: env.user_id(3),
            created: Vec::new(),
        }
    }

    /// A private room by alice with carol invited, accepted by carol.
    async fn shared_room(&mut self, name: &str, encrypted: bool) -> OwnedRoomId {
        let carol_id = self.carol_id.clone();
        let room = self
            .alice
            .create(private_room(name, None, encrypted, &[&carol_id]))
            .await;
        self.created.push(room.clone());
        self.carol
            .wait("the invitation", |s| {
                s.room(&room).is_some_and(|r| r.is_invite)
            })
            .await;
        self.carol
            .send(Command::AcceptInvite {
                room_id: api_id(&room),
            })
            .await;
        self.carol
            .wait("the room joined", |s| {
                s.room(&room).is_some_and(|r| !r.is_invite)
            })
            .await;
        room
    }

    async fn finish(self) {
        self.alice.logout().await;
        self.carol.logout().await;
        leave_all_created(3, &self.created).await;
        leave_all_created(1, &self.created).await;
    }
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn an_encrypted_room_is_created_invited_and_accepted() {
    let mut p = Pair::new().await;
    let name = unique_room_name();
    let topic = format!("topic-{}", hex());
    let carol_id = p.carol_id.clone();
    let room = p
        .alice
        .create(private_room(&name, Some(&topic), true, &[&carol_id]))
        .await;
    p.created.push(room.clone());

    p.alice
        .wait("alice's list", |s| {
            s.room(&room).is_some_and(|r| r.is_encrypted)
        })
        .await;
    assert_eq!(p.alice.seen.room(&room).unwrap().name, name);
    p.carol
        .wait("carol's invitation", |s| {
            s.room(&room).is_some_and(|r| r.is_invite)
        })
        .await;

    p.carol
        .send(Command::AcceptInvite {
            room_id: api_id(&room),
        })
        .await;
    p.carol
        .wait("carol joined", |s| {
            s.room(&room).is_some_and(|r| !r.is_invite)
        })
        .await;
    assert_eq!(p.carol.seen.room(&room).unwrap().name, name);
    assert!(
        p.carol
            .seen
            .joined
            .iter()
            .any(|r| r.as_str() == room.as_str())
    );

    p.carol
        .send(Command::OpenRoomDetails {
            room_id: api_id(&room),
        })
        .await;
    p.carol
        .wait("carol's room details", |s| {
            s.details
                .iter()
                .any(|d| d.topic.as_deref() == Some(topic.as_str()))
        })
        .await;
    let details = p.carol.seen.details.last().unwrap();
    assert!(details.is_encrypted);
    assert_eq!(details.name, name);
    assert!(p.alice.seen.failures.is_empty() && p.carol.seen.failures.is_empty());
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_public_room_is_joined_by_alias() {
    let mut p = Pair::new().await;
    let local = format!("tessera-{}", hex());
    let room = p
        .alice
        .create(CreateRoomRequest {
            name: unique_room_name(),
            topic: None,
            encrypted: false,
            visibility: RoomVisibility::Public {
                alias: local.clone(),
            },
            invitees: Vec::new(),
        })
        .await;
    p.created.push(room.clone());

    p.carol
        .send(Command::JoinRoom {
            target: format!("#{local}:{}", TestEnv::load().get("HOMESERVER_DOMAIN")),
            via: Vec::new(),
        })
        .await;
    p.carol.wait("RoomJoined", |s| !s.joined.is_empty()).await;
    assert_eq!(p.carol.seen.joined[0].as_str(), room.as_str());
    p.carol
        .wait("the room in carol's list", |s| s.room(&room).is_some())
        .await;

    // Junk is refused without a request.
    p.carol
        .send(Command::JoinRoom {
            target: "https://example.com/room".into(),
            via: Vec::new(),
        })
        .await;
    p.carol.wait("a refusal", |s| !s.failures.is_empty()).await;
    assert_eq!(
        p.carol.seen.failures[0],
        (ActionKind::Rooms, RoomId::from(""), CoreError::NotAllowed)
    );
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn leaving_a_selected_room_removes_it_and_the_member() {
    let mut p = Pair::new().await;
    let room = p.shared_room(&unique_room_name(), false).await;
    let carol_user = p.carol_id.clone();
    p.alice
        .members_until(&room, "carol joined", |m| {
            m.iter()
                .any(|x| x.user_id.as_str() == carol_user && x.membership == MembershipView::Joined)
        })
        .await;

    // The room is open in carol's core when she leaves it.
    p.carol.send(Command::SelectRoom(api_id(&room))).await;
    p.carol
        .wait("the open timeline", |s| {
            s.errors == 0 && s.room(&room).is_some()
        })
        .await;
    p.carol
        .send(Command::OpenRoomDetails {
            room_id: api_id(&room),
        })
        .await;
    p.carol
        .wait("the room details", |s| !s.details.is_empty())
        .await;
    p.carol
        .send(Command::LeaveRoom {
            room_id: api_id(&room),
        })
        .await;
    p.carol
        .wait("the room gone from carol's list", |s| {
            s.room(&room).is_none()
        })
        .await;
    assert!(p.carol.seen.failures.is_empty(), "leaving failed");
    // The watcher of a room that was left reports nothing more.
    let reported = p.carol.seen.details.len();
    p.carol.drain_for(Duration::from_secs(3)).await;
    assert_eq!(p.carol.seen.details.len(), reported);
    assert_eq!(p.carol.seen.errors, 0);

    p.alice
        .members_until(&room, "carol gone from the member list", |m| {
            m.iter().all(|x| x.user_id.as_str() != carol_user)
        })
        .await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_declined_invitation_leaves_the_list() {
    let mut p = Pair::new().await;
    let carol_id = p.carol_id.clone();
    let room = p
        .alice
        .create(private_room(&unique_room_name(), None, false, &[&carol_id]))
        .await;
    p.created.push(room.clone());
    p.carol
        .wait("the invitation", |s| {
            s.room(&room).is_some_and(|r| r.is_invite)
        })
        .await;
    p.carol
        .send(Command::DeclineInvite {
            room_id: api_id(&room),
        })
        .await;
    p.carol
        .wait("the invitation gone", |s| s.room(&room).is_none())
        .await;
    assert!(p.carol.seen.failures.is_empty());

    // Accepting what is no longer an invitation is refused.
    p.carol
        .send(Command::AcceptInvite {
            room_id: api_id(&room),
        })
        .await;
    p.carol.wait("a refusal", |s| !s.failures.is_empty()).await;
    p.finish().await;
}

/// A private room by alice with carol invited, whose `m.room.create` names
/// `predecessor` as the room it replaces (alice controls that claim).
async fn room_claiming_predecessor(predecessor: &OwnedRoomId, carol: &str) -> OwnedRoomId {
    use matrix_sdk::ruma::api::client::room::create_room;
    use matrix_sdk::ruma::serde::Raw;

    let content = serde_json::value::to_raw_value(&serde_json::json!({
        "predecessor": { "room_id": predecessor.as_str() }
    }))
    .expect("creation content is JSON");
    let mut request = create_room::v3::Request::new();
    request.name = Some(unique_room_name());
    request.invite = vec![carol.try_into().expect("valid user id")];
    request.creation_content = Some(Raw::from_json(content));
    let alice = raw_login(1).await;
    alice
        .send(request)
        .await
        .expect("create the room with a predecessor")
        .room_id
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn declining_or_leaving_does_not_leave_a_claimed_predecessor() {
    let mut p = Pair::new().await;
    let victim = p.shared_room(&unique_room_name(), false).await;
    let carol_id = p.carol_id.clone();

    // Declining an invitation whose create event names the victim room.
    let spam = room_claiming_predecessor(&victim, &carol_id).await;
    p.created.push(spam.clone());
    p.carol
        .wait("the invitation", |s| {
            s.room(&spam).is_some_and(|r| r.is_invite)
        })
        .await;
    p.carol
        .send(Command::DeclineInvite {
            room_id: api_id(&spam),
        })
        .await;
    p.carol
        .wait("the invitation gone", |s| s.room(&spam).is_none())
        .await;

    // Accepting and then leaving such a room.
    let spam = room_claiming_predecessor(&victim, &carol_id).await;
    p.created.push(spam.clone());
    p.carol
        .wait("the second invitation", |s| {
            s.room(&spam).is_some_and(|r| r.is_invite)
        })
        .await;
    p.carol
        .send(Command::AcceptInvite {
            room_id: api_id(&spam),
        })
        .await;
    p.carol
        .wait("the room joined", |s| {
            s.room(&spam).is_some_and(|r| !r.is_invite)
        })
        .await;
    p.carol
        .send(Command::LeaveRoom {
            room_id: api_id(&spam),
        })
        .await;
    p.carol
        .wait("the room gone", |s| s.room(&spam).is_none())
        .await;
    assert!(p.carol.seen.failures.is_empty());

    // Give a wrongly sent leave for the victim time to reach the server.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let carol_user = p.carol_id.clone();
    p.alice
        .members_until(&victim, "carol still joined", |m| {
            m.iter()
                .any(|x| x.user_id.as_str() == carol_user && x.membership == MembershipView::Joined)
        })
        .await;
    assert!(
        p.carol.seen.room(&victim).is_some_and(|r| !r.is_invite),
        "the unrelated room was left"
    );
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn room_settings_reach_the_other_member_and_need_power() {
    let mut p = Pair::new().await;
    let room = p.shared_room(&unique_room_name(), false).await;
    let id = api_id(&room);

    let new_name = format!("renamed-{}", hex());
    let new_topic = format!("topic-{}", hex());
    let avatar = include_bytes!("fixtures/small-64x48.png").to_vec();
    p.alice
        .send(Command::SetRoomName {
            room_id: id.clone(),
            name: new_name.clone(),
        })
        .await;
    p.alice
        .send(Command::SetRoomTopic {
            room_id: id.clone(),
            topic: Some(new_topic.clone()),
        })
        .await;
    p.alice
        .send(Command::SetRoomAvatar {
            room_id: id.clone(),
            data: MediaBytes::from(avatar.clone()),
            mimetype: "image/png".into(),
        })
        .await;
    p.carol
        .wait("the new name and avatar", |s| {
            s.room(&room)
                .is_some_and(|r| r.name == new_name && r.avatar.is_some())
        })
        .await;
    assert!(p.alice.seen.failures.is_empty(), "a change failed");

    // A wrong image type or a lie about the type is refused.
    p.alice
        .send(Command::SetRoomAvatar {
            room_id: id.clone(),
            data: MediaBytes::from(avatar),
            mimetype: "image/jpeg".into(),
        })
        .await;
    p.alice.wait("a refusal", |s| !s.failures.is_empty()).await;
    assert_eq!(p.alice.seen.failures[0].2, CoreError::NotAllowed);

    // Alice may change everything but encryption is still off, so she may
    // turn it on; both sides then see the room as encrypted.
    p.alice
        .send(Command::OpenRoomDetails {
            room_id: id.clone(),
        })
        .await;
    p.alice
        .wait("alice's details", |s| {
            s.details
                .iter()
                .any(|d| d.topic.as_deref() == Some(new_topic.as_str()))
        })
        .await;
    let d = p.alice.seen.details.last().unwrap();
    assert_eq!(
        d.permissions,
        RoomPermissions {
            can_set_name: true,
            can_set_topic: true,
            can_set_avatar: true,
            can_enable_encryption: true,
            can_invite: true,
        }
    );
    p.alice
        .send(Command::EnableRoomEncryption {
            room_id: id.clone(),
        })
        .await;
    p.alice
        .wait("alice sees encryption", |s| {
            s.room(&room).is_some_and(|r| r.is_encrypted)
        })
        .await;
    p.carol
        .wait("carol sees encryption", |s| {
            s.room(&room).is_some_and(|r| r.is_encrypted)
        })
        .await;
    p.alice
        .wait("alice's details after encryption", |s| {
            s.details
                .last()
                .is_some_and(|d| d.is_encrypted && !d.permissions.can_enable_encryption)
        })
        .await;

    // Carol has no power: all controls off, and a change is refused.
    p.carol
        .send(Command::OpenRoomDetails {
            room_id: id.clone(),
        })
        .await;
    p.carol
        .wait("carol's details", |s| !s.details.is_empty())
        .await;
    let denied = p.carol.seen.details.last().unwrap().permissions;
    // Synapse lets an ordinary member invite (level 0), so only that is on;
    // the plan expected all false (see PROGRESS.md, Phase 5 / Task 7 notes).
    assert_eq!(
        denied,
        RoomPermissions {
            can_invite: true,
            ..RoomPermissions::default()
        }
    );
    p.carol
        .send(Command::SetRoomName {
            room_id: id.clone(),
            name: "mine now".into(),
        })
        .await;
    p.carol.wait("a refusal", |s| !s.failures.is_empty()).await;
    assert_eq!(
        p.carol.seen.failures[0],
        (ActionKind::Rooms, id, CoreError::NotAllowed)
    );
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn members_have_roles_and_dms_are_reused() {
    let mut p = Pair::new().await;
    let room = p.shared_room(&unique_room_name(), false).await;
    let carol_user = p.carol_id.clone();
    let alice_user = TestEnv::load().user_id(1);
    let members = p
        .alice
        .members_until(&room, "both members joined", |m| {
            m.iter()
                .filter(|x| x.membership == MembershipView::Joined)
                .count()
                == 2
        })
        .await;
    let role = |user: &str| {
        members
            .iter()
            .find(|m| m.user_id.as_str() == user)
            .unwrap()
            .role
    };
    assert!(matches!(role(&alice_user), Role::Creator | Role::Admin));
    assert_eq!(role(&carol_user), Role::User);
    // Sorted by role: alice first.
    assert_eq!(members[0].user_id.as_str(), alice_user);

    // The same DM both times.
    let carol_api = UserId::from(carol_user.as_str());
    let before = p.alice.seen.created.len();
    p.alice
        .send(Command::CreateDm {
            user_id: carol_api.clone(),
        })
        .await;
    p.alice.send(Command::CreateDm { user_id: carol_api }).await;
    p.alice
        .wait("two answers", |s| {
            s.created.len() >= before + 2 || !s.failures.is_empty()
        })
        .await;
    assert!(p.alice.seen.failures.is_empty());
    let (first, second) = (
        &p.alice.seen.created[before],
        &p.alice.seen.created[before + 1],
    );
    assert_eq!(first, second, "the second CreateDm made a new room");
    let dm = OwnedRoomId::try_from(first.as_str()).unwrap();
    p.created.push(dm);
    // Yourself is not a DM.
    p.alice
        .send(Command::CreateDm {
            user_id: UserId::from(alice_user.as_str()),
        })
        .await;
    p.alice.wait("a refusal", |s| !s.failures.is_empty()).await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_new_display_name_reaches_the_other_member_and_own_profile() {
    let mut p = Pair::new().await;
    let room = p.shared_room(&unique_room_name(), false).await;
    let alice_user = TestEnv::load().user_id(1);

    // The own profile arrives after login.
    p.alice
        .wait("the first profile", |s| !s.profile.is_empty())
        .await;
    let original = p.alice.seen.profile[0].display_name.clone();

    let name = format!("Alice {}", hex());
    p.alice
        .send(Command::SetDisplayName { name: name.clone() })
        .await;
    p.alice
        .wait("the new profile", |s| {
            s.profile.last().and_then(|x| x.display_name.as_deref()) == Some(name.as_str())
        })
        .await;
    p.carol
        .members_until(&room, "the new name in carol's list", |m| {
            m.iter().any(|x| {
                x.user_id.as_str() == alice_user && x.display_name.as_deref() == Some(name.as_str())
            })
        })
        .await;

    // Put the name back (an account with no name goes through a raw client).
    match original {
        Some(original) => {
            p.alice
                .send(Command::SetDisplayName {
                    name: original.clone(),
                })
                .await;
            p.alice
                .wait("the restored profile", |s| {
                    s.profile.last().and_then(|x| x.display_name.as_deref())
                        == Some(original.as_str())
                })
                .await;
        }
        None => {
            raw_login(1)
                .await
                .account()
                .set_display_name(None)
                .await
                .expect("clear the display name");
        }
    }
    assert!(p.alice.seen.failures.is_empty());
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_space_filters_the_room_list_to_its_children() {
    let _guard = SERIAL.lock().await;
    let child = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let other = create_room_raw(1, &unique_room_name(), false, &[]).await;
    let space = create_space_raw(1, &unique_room_name(), std::slice::from_ref(&child)).await;

    let mut alice = Side::login(1).await;
    let space_api = api_id(&space);
    alice
        .wait("the space", |s| {
            s.spaces.as_ref().is_some_and(|list| {
                list.iter()
                    .any(|v| v.id == space_api && v.level == 0 && v.children == 1)
            })
        })
        .await;
    alice
        .wait("both rooms in the list", |s| {
            s.room(&child).is_some() && s.room(&other).is_some()
        })
        .await;

    alice
        .send(Command::SetRoomListQuery(RoomListQuery {
            space: Some(space_api.clone()),
            ..RoomListQuery::default()
        }))
        .await;
    alice
        .wait("only the child", |s| {
            s.rooms.len() == 1 && s.rooms[0].id.as_str() == child.as_str()
        })
        .await;

    // A space the core does not know shows nothing.
    alice
        .send(Command::SetRoomListQuery(RoomListQuery {
            space: Some(RoomId::from("!nowhere:localhost")),
            ..RoomListQuery::default()
        }))
        .await;
    alice.wait("an empty list", |s| s.rooms.is_empty()).await;

    alice.logout().await;
    leave_all_created(1, &[space, child, other]).await;
}
