//! Rooms, members and the user's own profile: the pure rules (validation,
//! target parsing, roles and permissions) and the background work that runs
//! the SDK calls.
//!
//! Names, topics, aliases, member names and image bytes are user content:
//! nothing here logs them, and failures carry fixed messages only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::room::RoomMember;
use matrix_sdk::ruma::api::client::membership::{forget_room, leave_room};
use matrix_sdk::ruma::api::client::room::create_room;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::InitialStateEvent;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::events::room::encryption::RoomEncryptionEventContent;
use matrix_sdk::ruma::events::room::member::MembershipState;
use matrix_sdk::ruma::events::room::power_levels::{RoomPowerLevels, UserPowerLevel};
use matrix_sdk::ruma::matrix_uri::MatrixId;
use matrix_sdk::ruma::{
    MatrixToUri, OwnedRoomId, OwnedRoomOrAliasId, OwnedServerName, OwnedUserId, UserId as SdkUserId,
};
use matrix_sdk::{Client, Room, RoomMemberships, RoomState};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinSet};

use crate::api::{
    ActionKind, CoreError, CoreEvent, CreateRoomRequest, MembershipView, OwnProfileView,
    PowerLevelView, Role, RoomDetailsView, RoomId, RoomMemberView, RoomPermissions, RoomVisibility,
    UserId,
};
use crate::avatars::MAX_AVATAR_BYTES;
use crate::notifications::clean_line;
use crate::room_list::{is_bidi_control, pick_avatar};
use crate::spaces::{self, SpaceChildren};
use crate::upload::parse_mime;

/// Longest room name and display name, in characters.
pub(crate) const MAX_NAME_CHARS: usize = 255;
/// Longest topic, in characters.
pub(crate) const MAX_TOPIC_CHARS: usize = 1000;
/// Longest alias local part, in characters.
const MAX_ALIAS_CHARS: usize = 64;
/// Most users a new room can invite.
const MAX_INVITEES: usize = 100;
/// Longest room target text, in bytes.
const MAX_TARGET_BYTES: usize = 512;
/// Most `via` servers kept for a join.
const MAX_VIA: usize = 8;
/// Most members listed; the rest is reported as truncated.
pub(crate) const MAX_MEMBERS: usize = 5000;

fn has_unsafe_char(text: &str, allow_newline: bool) -> bool {
    text.chars()
        .any(|c| (c.is_control() && !(allow_newline && c == '\n')) || is_bidi_control(c))
}

/// A room or display name: trimmed, 1-255 characters, no control or
/// bidirectional characters.
pub(crate) fn clean_name(name: &str) -> Result<String, CoreError> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 || count > MAX_NAME_CHARS || has_unsafe_char(name, false) {
        return Err(CoreError::NotAllowed);
    }
    Ok(name.to_owned())
}

/// A topic: trimmed, at most 1,000 characters, line breaks allowed. A blank
/// topic is `None` (it clears the topic).
pub(crate) fn clean_topic(topic: &str) -> Result<Option<String>, CoreError> {
    let topic = topic.trim();
    if topic.chars().count() > MAX_TOPIC_CHARS || has_unsafe_char(topic, true) {
        return Err(CoreError::NotAllowed);
    }
    Ok((!topic.is_empty()).then(|| topic.to_owned()))
}

fn valid_alias_local_part(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= MAX_ALIAS_CHARS
        && alias
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._=-".contains(&b))
}

/// The server request for a new room. Encrypted rooms carry `m.room.encryption`
/// (Megolm) in their initial state; public rooms need an alias local part.
pub(crate) fn create_room_request(
    form: &CreateRoomRequest,
) -> Result<create_room::v3::Request, CoreError> {
    let mut request = create_room::v3::Request::new();
    request.name = Some(clean_name(&form.name)?);
    request.topic = match &form.topic {
        Some(topic) => clean_topic(topic)?,
        None => None,
    };
    match &form.visibility {
        RoomVisibility::Private => {
            request.preset = Some(create_room::v3::RoomPreset::PrivateChat);
        }
        RoomVisibility::Public { alias } => {
            if !valid_alias_local_part(alias) {
                return Err(CoreError::NotAllowed);
            }
            request.preset = Some(create_room::v3::RoomPreset::PublicChat);
            request.room_alias_name = Some(alias.clone());
        }
    }
    if form.invitees.len() > MAX_INVITEES {
        return Err(CoreError::NotAllowed);
    }
    for user in &form.invitees {
        let user = SdkUserId::parse(user.as_str()).map_err(|_| CoreError::NotAllowed)?;
        if !request.invite.contains(&user) {
            request.invite.push(user);
        }
    }
    if form.encrypted {
        request.initial_state.push(
            InitialStateEvent::with_empty_state_key(
                RoomEncryptionEventContent::with_recommended_defaults(),
            )
            .to_raw_any(),
        );
    }
    Ok(request)
}

/// A room to join: an ID or alias, and servers that can route the join.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoomTarget {
    pub(crate) id: OwnedRoomOrAliasId,
    pub(crate) via: Vec<OwnedServerName>,
}

/// Parses `#alias:server`, `!id:server` or a `https://matrix.to/#/...` link to
/// a room or alias. Nothing is fetched. Anything else (users, events, other
/// hosts, schemes or arguments) is refused.
pub(crate) fn parse_room_target(text: &str) -> Option<RoomTarget> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_TARGET_BYTES || text.chars().any(char::is_whitespace) {
        return None;
    }
    if text.starts_with("https://") || text.starts_with("http://") || text.contains("//") {
        let link = MatrixToUri::parse(text).ok()?;
        let id = match link.id() {
            MatrixId::Room(id) => OwnedRoomOrAliasId::from(id.clone()),
            MatrixId::RoomAlias(alias) => OwnedRoomOrAliasId::from(alias.clone()),
            _ => return None,
        };
        let mut via = link.via().to_vec();
        via.truncate(MAX_VIA);
        return Some(RoomTarget { id, via });
    }
    // The parser accepts a bare sigil; a target needs something after it.
    if !(text.starts_with('#') || text.starts_with('!')) || text.len() < 2 {
        return None;
    }
    let id = OwnedRoomOrAliasId::try_from(text).ok()?;
    Some(RoomTarget {
        id,
        via: Vec::new(),
    })
}

/// The servers of a link followed by the ones the caller gave, without
/// repeats and without malformed names, at most eight.
pub(crate) fn merge_via(from_link: Vec<OwnedServerName>, given: &[String]) -> Vec<OwnedServerName> {
    let mut out = Vec::new();
    let given = given
        .iter()
        .filter_map(|name| OwnedServerName::try_from(name.as_str()).ok());
    for name in from_link.into_iter().chain(given) {
        if out.len() < MAX_VIA && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The role of a power level: `Infinite` is the creator, 100 and more admin,
/// 50 and more moderator.
pub(crate) fn role_of(level: UserPowerLevel) -> Role {
    match level {
        UserPowerLevel::Infinite => Role::Creator,
        UserPowerLevel::Int(n) => match i64::from(n) {
            100.. => Role::Admin,
            50.. => Role::Moderator,
            _ => Role::User,
        },
        _ => Role::User,
    }
}

/// The API form of a power level.
pub(crate) fn power_view(level: UserPowerLevel) -> PowerLevelView {
    match level {
        UserPowerLevel::Int(n) => PowerLevelView::Level(i64::from(n)),
        _ => PowerLevelView::Creator,
    }
}

/// What `own` may change in a room with these power levels.
pub(crate) fn permissions(levels: &RoomPowerLevels, own: &SdkUserId) -> RoomPermissions {
    let state = |kind: StateEventType| levels.user_can_send_state(own, kind);
    RoomPermissions {
        can_set_name: state(StateEventType::RoomName),
        can_set_topic: state(StateEventType::RoomTopic),
        can_set_avatar: state(StateEventType::RoomAvatar),
        can_enable_encryption: state(StateEventType::RoomEncryption),
        can_invite: levels.user_can_invite(own),
    }
}

/// Sorts by role (creator first), then by name (case-insensitive; members
/// without a name come last, by user ID), then by user ID.
pub(crate) fn sort_members(members: &mut [RoomMemberView]) {
    // Named members first, then the nameless ones by user ID.
    fn key(m: &RoomMemberView) -> (bool, String) {
        match m.display_name.as_deref().filter(|n| !n.is_empty()) {
            Some(name) => (false, name.to_lowercase()),
            None => (true, m.user_id.as_str().to_lowercase()),
        }
    }
    members.sort_by(|a, b| {
        a.role
            .cmp(&b.role)
            .then_with(|| key(a).cmp(&key(b)))
            .then_with(|| a.user_id.as_str().cmp(b.user_id.as_str()))
    });
}

/// The image type that `bytes` start with, by signature.
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Accepts an avatar image: not empty, at most 1 MiB, a PNG, JPEG, GIF or
/// WebP by content, and declared as exactly that type. Returns the type.
pub(crate) fn check_image(data: &[u8], declared: &str) -> Result<mime::Mime, CoreError> {
    if data.is_empty() || data.len() > MAX_AVATAR_BYTES {
        return Err(CoreError::NotAllowed);
    }
    let declared = parse_mime(declared)?;
    match sniff_image(data) {
        Some(found) if found == declared.essence_str() => Ok(declared),
        _ => Err(CoreError::NotAllowed),
    }
}

/// Most room, member and profile requests running at once; more are `Busy`.
const MAX_PENDING_JOBS: usize = 32;
/// How long one request may take.
const JOB_DEADLINE: Duration = Duration::from_secs(60);

/// One request to run in the background. The engine has already checked its
/// input and found the rooms.
pub(crate) enum Job {
    Create(Box<create_room::v3::Request>),
    Dm(OwnedUserId),
    Join(RoomTarget),
    Accept(Room),
    Decline(Room),
    Invite(Room, OwnedUserId),
    Leave(Room),
    Name(Room, String),
    Topic(Room, String),
    RoomAvatar(Room, mime::Mime, Vec<u8>),
    Encrypt(Room),
    Members(Room),
    DisplayName(String),
    OwnAvatar(mime::Mime, Vec<u8>),
    RemoveOwnAvatar,
}

/// DMs started in this session, by the other user. The SDK learns of a new
/// DM from the next sync, so a second request right after the first would
/// otherwise start another room.
type StartedDms = Arc<tokio::sync::Mutex<HashMap<OwnedUserId, OwnedRoomId>>>;

/// The room, member and profile work of one session.
pub(crate) struct RoomTasks {
    tasks: JoinSet<()>,
    /// The open room details watcher, replaced by the next `OpenRoomDetails`.
    details: Option<(OwnedRoomId, AbortHandle)>,
    children: Arc<watch::Sender<SpaceChildren>>,
    dms: StartedDms,
}

impl RoomTasks {
    pub(crate) fn new() -> Self {
        Self {
            tasks: JoinSet::new(),
            details: None,
            children: Arc::new(watch::channel(SpaceChildren::default()).0),
            dms: StartedDms::default(),
        }
    }

    /// Starts the spaces forwarder and sends the user's own profile.
    pub(crate) fn start(&mut self, client: &Client, events: &mpsc::Sender<CoreEvent>) {
        self.tasks.spawn(spaces::run(
            client.clone(),
            events.clone(),
            self.children.clone(),
        ));
        let (client, events) = (client.clone(), events.clone());
        self.tasks.spawn(async move {
            // A failure leaves the profile unknown until the next change.
            if let Ok(Ok(profile)) = tokio::time::timeout(JOB_DEADLINE, own_profile(&client)).await
            {
                let _ = events.send(CoreEvent::OwnProfile(profile)).await;
            }
        });
    }

    /// The descendants of every space, for the room list filter.
    pub(crate) fn subscribe_children(&self) -> watch::Receiver<SpaceChildren> {
        self.children.subscribe()
    }

    /// Runs `job` in the background; its result is an event, or an
    /// `ActionFailed` for `room_id` and `action`.
    pub(crate) fn submit(
        &mut self,
        client: &Client,
        events: &mpsc::Sender<CoreEvent>,
        room_id: RoomId,
        action: ActionKind,
        job: Job,
    ) -> Result<(), CoreError> {
        while self.tasks.try_join_next().is_some() {}
        if self.tasks.len() >= MAX_PENDING_JOBS {
            return Err(CoreError::Busy);
        }
        // The details of a room being left are over.
        if let Job::Leave(room) = &job
            && self
                .details
                .as_ref()
                .is_some_and(|(id, _)| id == room.room_id())
            && let Some((_, watcher)) = self.details.take()
        {
            watcher.abort();
        }
        let (client, events, dms) = (client.clone(), events.clone(), self.dms.clone());
        self.tasks.spawn(async move {
            let result = match tokio::time::timeout(JOB_DEADLINE, execute(&client, &dms, job)).await
            {
                Ok(result) => result,
                Err(_) => Err(CoreError::Internal("the request timed out".into())),
            };
            let event = match result {
                Ok(event) => event,
                Err(error) => Some(CoreEvent::ActionFailed {
                    room_id,
                    action,
                    error,
                }),
            };
            // A dropped receiver just means nobody listens any more.
            if let Some(event) = event {
                let _ = events.send(event).await;
            }
        });
        Ok(())
    }

    /// Reports the details of `room` now and on every change, in place of the
    /// ones of an earlier room.
    pub(crate) fn open_details(
        &mut self,
        room: Room,
        own: OwnedUserId,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        if let Some((_, previous)) = self.details.take() {
            previous.abort();
        }
        let id = room.room_id().to_owned();
        let handle = self.tasks.spawn(watch_details(room, own, events.clone()));
        self.details = Some((id, handle));
    }

    /// Ends every task; they hold clones of the client.
    pub(crate) async fn stop(&mut self) {
        self.details = None;
        self.tasks.shutdown().await;
    }
}

/// A server refusal for lack of power is `NotAllowed`; anything else gets the
/// fixed `what`. The server's own words are never passed on.
fn failure(kind: Option<&ErrorKind>, what: &'static str) -> CoreError {
    if matches!(kind, Some(ErrorKind::Forbidden)) {
        CoreError::NotAllowed
    } else {
        CoreError::Internal(what.into())
    }
}

fn sdk_failure(error: &matrix_sdk::Error, what: &'static str) -> CoreError {
    failure(error.client_api_error_kind(), what)
}

/// Leaves `room` and nothing else. `Room::leave` also leaves every room the
/// `m.room.create` event names as a predecessor, and whoever created or
/// invited to the room writes that event, so it is never used. The SDK
/// marks the room left at the next sync (`Room::leave` also set that state
/// at once through a crate-private call). A declined invitation is also
/// forgotten on the server, as `Room::leave` does; a failure to forget is
/// not an error because the leave itself worked.
async fn leave_only(client: &Client, room: &Room) -> Result<(), matrix_sdk::Error> {
    // The engine checked the state when it queued the job.
    let state = room.state();
    let id = room.room_id().to_owned();
    client
        .send(leave_room::v3::Request::new(id.clone()))
        .await?;
    if state == RoomState::Invited {
        let _ = client.send(forget_room::v3::Request::new(id)).await;
    }
    Ok(())
}

fn joined_id(room: &Room) -> RoomId {
    RoomId::from(room.room_id().as_str())
}

/// Refuses an action the user's power level does not allow. Unknown power
/// levels leave the decision to the server.
async fn require(
    client: &Client,
    room: &Room,
    allowed: impl Fn(&RoomPermissions) -> bool,
) -> Result<(), CoreError> {
    let (Some(own), Ok(levels)) = (client.user_id(), room.power_levels().await) else {
        return Ok(());
    };
    if allowed(&permissions(&levels, own)) {
        Ok(())
    } else {
        Err(CoreError::NotAllowed)
    }
}

/// What a finished job reports: an event, or nothing (the room list and the
/// room details show the change when it syncs).
type Done = Result<Option<CoreEvent>, CoreError>;

fn event(event: CoreEvent) -> Done {
    Ok(Some(event))
}

async fn execute(client: &Client, dms: &StartedDms, job: Job) -> Done {
    match job {
        Job::Create(request) => {
            let room = client
                .create_room(*request)
                .await
                .map_err(|e| sdk_failure(&e, "could not create the room"))?;
            event(CoreEvent::RoomCreated {
                room_id: joined_id(&room),
            })
        }
        Job::Dm(user) => {
            // One at a time, so that a repeat finds the room the first made.
            let mut started = dms.lock().await;
            let existing = started
                .get(&user)
                .and_then(|id| client.get_room(id))
                .filter(|room| room.state() == RoomState::Joined)
                .or_else(|| {
                    // The SDK lists DMs in no fixed order; take the same one
                    // every time.
                    client
                        .get_dm_rooms(&user)
                        .min_by(|a, b| a.room_id().cmp(b.room_id()))
                });
            let room = match existing {
                Some(room) => room,
                None => client
                    .create_dm(&user)
                    .await
                    .map_err(|e| sdk_failure(&e, "could not start the chat"))?,
            };
            started.insert(user, room.room_id().to_owned());
            event(CoreEvent::RoomCreated {
                room_id: joined_id(&room),
            })
        }
        Job::Join(target) => {
            let room = client
                .join_room_by_id_or_alias(&target.id, &target.via)
                .await
                .map_err(|e| sdk_failure(&e, "could not join the room"))?;
            event(CoreEvent::RoomJoined {
                room_id: joined_id(&room),
            })
        }
        Job::Accept(room) => {
            room.join()
                .await
                .map_err(|e| sdk_failure(&e, "could not accept the invitation"))?;
            event(CoreEvent::RoomJoined {
                room_id: joined_id(&room),
            })
        }
        Job::Decline(room) => {
            leave_only(client, &room)
                .await
                .map_err(|e| sdk_failure(&e, "could not decline the invitation"))?;
            Ok(None)
        }
        Job::Invite(room, user) => {
            require(client, &room, |p| p.can_invite).await?;
            room.invite_user_by_id(&user)
                .await
                .map_err(|e| sdk_failure(&e, "could not invite the user"))?;
            Ok(None)
        }
        Job::Leave(room) => {
            leave_only(client, &room)
                .await
                .map_err(|e| sdk_failure(&e, "could not leave the room"))?;
            Ok(None)
        }
        Job::Name(room, name) => {
            require(client, &room, |p| p.can_set_name).await?;
            room.set_name(name)
                .await
                .map_err(|e| sdk_failure(&e, "could not change the room name"))?;
            Ok(None)
        }
        Job::Topic(room, topic) => {
            require(client, &room, |p| p.can_set_topic).await?;
            room.set_room_topic(&topic)
                .await
                .map_err(|e| sdk_failure(&e, "could not change the topic"))?;
            Ok(None)
        }
        Job::RoomAvatar(room, mime, data) => {
            require(client, &room, |p| p.can_set_avatar).await?;
            room.upload_avatar(&mime, data, None)
                .await
                .map_err(|e| sdk_failure(&e, "could not change the room avatar"))?;
            Ok(None)
        }
        Job::Encrypt(room) => {
            require(client, &room, |p| p.can_enable_encryption).await?;
            room.enable_encryption()
                .await
                .map_err(|e| sdk_failure(&e, "could not turn on encryption"))?;
            Ok(None)
        }
        Job::Members(room) => {
            let members = room
                .members(RoomMemberships::ACTIVE)
                .await
                .map_err(|e| sdk_failure(&e, "could not list the members"))?;
            let (members, truncated) = member_views(&members);
            event(CoreEvent::RoomMembers {
                room_id: joined_id(&room),
                members,
                truncated,
            })
        }
        Job::DisplayName(name) => {
            client
                .account()
                .set_display_name(Some(&name))
                .await
                .map_err(|e| sdk_failure(&e, "could not change the display name"))?;
            event(CoreEvent::OwnProfile(own_profile(client).await?))
        }
        Job::OwnAvatar(mime, data) => {
            client
                .account()
                .upload_avatar(&mime, data)
                .await
                .map_err(|e| sdk_failure(&e, "could not change the avatar"))?;
            event(CoreEvent::OwnProfile(own_profile(client).await?))
        }
        Job::RemoveOwnAvatar => {
            client
                .account()
                .set_avatar_url(None)
                .await
                .map_err(|e| sdk_failure(&e, "could not remove the avatar"))?;
            event(CoreEvent::OwnProfile(own_profile(client).await?))
        }
    }
}

/// The user's profile as the server has it.
async fn own_profile(client: &Client) -> Result<OwnProfileView, CoreError> {
    let account = client.account();
    let read = |e: matrix_sdk::Error| sdk_failure(&e, "could not read the profile");
    let name = account.get_display_name().await.map_err(read)?;
    let avatar = account.get_avatar_url().await.map_err(read)?;
    Ok(OwnProfileView {
        display_name: name
            .map(|n| clean_line(&n, MAX_NAME_CHARS))
            .filter(|n| !n.is_empty()),
        avatar: pick_avatar(avatar.as_deref().map(|u| u.as_str()), false, &[]),
    })
}

/// The members as API views, sorted and cut at [`MAX_MEMBERS`].
fn member_views(members: &[RoomMember]) -> (Vec<RoomMemberView>, bool) {
    let mut views: Vec<RoomMemberView> = members
        .iter()
        .filter_map(|m| {
            let membership = match m.membership() {
                MembershipState::Join => MembershipView::Joined,
                MembershipState::Invite => MembershipView::Invited,
                _ => return None,
            };
            let level = m.power_level();
            Some(RoomMemberView {
                user_id: UserId::from(m.user_id().as_str()),
                display_name: m
                    .display_name()
                    .map(|n| clean_line(n, MAX_NAME_CHARS))
                    .filter(|n| !n.is_empty()),
                avatar: pick_avatar(m.avatar_url().map(|u| u.as_str()), false, &[]),
                power: power_view(level),
                role: role_of(level),
                membership,
            })
        })
        .collect();
    sort_members(&mut views);
    let truncated = views.len() > MAX_MEMBERS;
    views.truncate(MAX_MEMBERS);
    (views, truncated)
}

/// The settings of `room` for the panel.
async fn details_of(room: &Room, own: &SdkUserId) -> RoomDetailsView {
    let mut permissions = match room.power_levels().await {
        Ok(levels) => permissions(&levels, own),
        Err(_) => RoomPermissions::default(),
    };
    let is_encrypted = room.encryption_state().is_encrypted();
    permissions.can_enable_encryption &= !is_encrypted;
    let name = room
        .name()
        .or_else(|| room.cached_display_name().map(|n| n.to_string()))
        .map(|n| clean_line(&n, MAX_NAME_CHARS))
        .unwrap_or_default();
    RoomDetailsView {
        room_id: joined_id(room),
        name,
        topic: room
            .topic()
            .map(|t| scrub_topic(&t))
            .filter(|t| !t.is_empty()),
        avatar: pick_avatar(room.avatar_url().as_deref().map(|u| u.as_str()), false, &[]),
        is_encrypted,
        alias: room
            .canonical_alias()
            .map(|a| clean_line(a.as_str(), MAX_NAME_CHARS)),
        permissions,
    }
}

/// A topic from the server without control and bidirectional characters
/// (line breaks stay), cut at [`MAX_TOPIC_CHARS`].
fn scrub_topic(topic: &str) -> String {
    topic
        .chars()
        .filter(|c| *c == '\n' || !(c.is_control() || is_bidi_control(*c)))
        .take(MAX_TOPIC_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Sends the details now, then again whenever they change.
async fn watch_details(room: Room, own: OwnedUserId, events: mpsc::Sender<CoreEvent>) {
    let mut info = room.subscribe_info();
    let mut last: Option<RoomDetailsView> = None;
    loop {
        let view = details_of(&room, &own).await;
        if last.as_ref() != Some(&view) {
            last = Some(view.clone());
            if events.send(CoreEvent::RoomDetails(view)).await.is_err() {
                return;
            }
        }
        if info.next().await.is_none() {
            return;
        }
        // Left, kicked or banned: nothing more to report.
        if room.state() != RoomState::Joined {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::ruma::events::room::power_levels::{
        RoomPowerLevelsEventContent, RoomPowerLevelsSource,
    };
    use matrix_sdk::ruma::room_version_rules::AuthorizationRules;
    use matrix_sdk::ruma::{Int, int};
    use serde_json::Value;

    use super::*;
    use crate::api::{MembershipView, UserId};

    fn form(name: &str) -> CreateRoomRequest {
        CreateRoomRequest {
            name: name.into(),
            topic: None,
            encrypted: false,
            visibility: RoomVisibility::Private,
            invitees: Vec::new(),
        }
    }

    fn state_types(request: &create_room::v3::Request) -> Vec<Value> {
        request
            .initial_state
            .iter()
            .map(|e| serde_json::from_str(e.json().get()).unwrap())
            .collect()
    }

    #[test]
    fn names_are_one_to_255_characters_without_control_characters() {
        assert_eq!(clean_name("  Lounge ").unwrap(), "Lounge");
        assert_eq!(clean_name(&"a".repeat(255)).unwrap().chars().count(), 255);
        // Characters, not bytes.
        assert!(clean_name(&"é".repeat(255)).is_ok());
        for bad in [
            "",
            "   ",
            "a\nb",
            "a\u{0}b",
            "x\u{202E}y",
            &"a".repeat(256),
            &"é".repeat(256),
        ] {
            assert_eq!(clean_name(bad), Err(CoreError::NotAllowed), "{bad:?}");
        }
    }

    #[test]
    fn topics_may_have_lines_and_blank_means_none() {
        assert_eq!(clean_topic("").unwrap(), None);
        assert_eq!(clean_topic("  \n ").unwrap(), None);
        assert_eq!(clean_topic("a\nb").unwrap().as_deref(), Some("a\nb"));
        for bad in ["a\u{7}b", "x\u{202E}y", &"a".repeat(1001)] {
            assert_eq!(clean_topic(bad), Err(CoreError::NotAllowed), "{bad:?}");
        }
    }

    #[test]
    fn a_private_room_uses_the_private_preset_and_no_alias() {
        let r = create_room_request(&form("Lounge")).unwrap();
        assert_eq!(r.name.as_deref(), Some("Lounge"));
        assert_eq!(r.preset, Some(create_room::v3::RoomPreset::PrivateChat));
        assert_eq!(r.room_alias_name, None);
        assert!(r.initial_state.is_empty());
        assert!(r.invite.is_empty());
        assert_eq!(r.topic, None);
    }

    #[test]
    fn an_encrypted_room_carries_megolm_in_its_initial_state() {
        let mut f = form("Lounge");
        f.encrypted = true;
        let r = create_room_request(&f).unwrap();
        let states = state_types(&r);
        assert_eq!(states.len(), 1);
        assert_eq!(states[0]["type"], "m.room.encryption");
        assert_eq!(states[0]["state_key"], "");
        assert_eq!(states[0]["content"]["algorithm"], "m.megolm.v1.aes-sha2");
    }

    #[test]
    fn a_public_room_needs_a_valid_alias_local_part() {
        let public = |alias: &str| {
            let mut f = form("Lounge");
            f.visibility = RoomVisibility::Public {
                alias: alias.into(),
            };
            create_room_request(&f)
        };
        let r = public("twicecat-1a.b_c=d").unwrap();
        assert_eq!(r.preset, Some(create_room::v3::RoomPreset::PublicChat));
        assert_eq!(r.room_alias_name.as_deref(), Some("twicecat-1a.b_c=d"));
        for bad in [
            "",
            "Upper",
            "has space",
            "a:b",
            "#a",
            "a/b",
            "é",
            "a\n",
            &"a".repeat(65),
        ] {
            assert_eq!(public(bad).unwrap_err(), CoreError::NotAllowed, "{bad:?}");
        }
    }

    #[test]
    fn the_topic_and_invitees_are_checked_and_carried() {
        let mut f = form("Lounge");
        f.topic = Some(" Chat ".into());
        f.invitees = vec![
            UserId::from("@bob:localhost"),
            UserId::from("@bob:localhost"),
            UserId::from("@carol:localhost"),
        ];
        let r = create_room_request(&f).unwrap();
        assert_eq!(r.topic.as_deref(), Some("Chat"));
        let invited: Vec<&str> = r.invite.iter().map(|u| u.as_str()).collect();
        assert_eq!(invited, ["@bob:localhost", "@carol:localhost"]);

        f.invitees = vec![UserId::from("not a user")];
        assert_eq!(create_room_request(&f).unwrap_err(), CoreError::NotAllowed);
        f.invitees = (0..101)
            .map(|i| UserId::from(format!("@u{i}:localhost")))
            .collect();
        assert_eq!(create_room_request(&f).unwrap_err(), CoreError::NotAllowed);
        f.invitees.clear();
        f.name = String::new();
        assert_eq!(create_room_request(&f).unwrap_err(), CoreError::NotAllowed);
    }

    #[test]
    fn room_targets_are_aliases_ids_and_matrix_to_links_only() {
        let ok = |text: &str| parse_room_target(text).unwrap_or_else(|| panic!("{text:?}"));
        assert_eq!(ok("#lounge:localhost").id.as_str(), "#lounge:localhost");
        assert_eq!(ok("  !abc:localhost ").id.as_str(), "!abc:localhost");
        assert!(ok("#lounge:localhost").via.is_empty());
        // Room versions 12 and later have IDs without a server part.
        assert_eq!(ok("!abc").id.as_str(), "!abc");

        let link = ok("https://matrix.to/#/!abc:localhost?via=a.example&via=b.example");
        assert_eq!(link.id.as_str(), "!abc:localhost");
        let via: Vec<&str> = link.via.iter().map(|s| s.as_str()).collect();
        assert_eq!(via, ["a.example", "b.example"]);
        assert_eq!(
            ok("https://matrix.to/#/%23lounge:localhost").id.as_str(),
            "#lounge:localhost"
        );
        assert_eq!(
            ok("https://matrix.to/#/#lounge:localhost").id.as_str(),
            "#lounge:localhost"
        );
    }

    #[test]
    fn junk_is_not_a_room_target() {
        for bad in [
            "",
            "   ",
            "lounge",
            "lounge:localhost",
            "#lounge",
            "!",
            "@bob:localhost",
            "$event:localhost",
            "#a b:localhost",
            "#a:localhost extra",
            "http://matrix.to/#/!abc:localhost",
            "https://evil.example/#/!abc:localhost",
            "https://matrix.to.evil.example/#/!abc:localhost",
            "https://matrix.to/#/@bob:localhost",
            "https://matrix.to/#/!abc:localhost/$event:localhost",
            "https://matrix.to/#/!abc:localhost?via=not a server",
            "https://matrix.to/#/!abc:localhost?other=1",
            "matrix:roomid/abc:localhost",
            "https://example.com/room",
            "#a:localhost\n",
            &format!("#{}:localhost", "a".repeat(600)),
        ] {
            // A trailing newline is trimmed; everything else is refused.
            if bad == "#a:localhost\n" {
                assert!(parse_room_target(bad).is_some());
                continue;
            }
            assert_eq!(parse_room_target(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn via_lists_merge_without_repeats_and_drop_bad_names() {
        let link: Vec<OwnedServerName> = vec!["a.example".try_into().unwrap()];
        let given = vec![
            "a.example".to_owned(),
            "b.example".to_owned(),
            "not a server".to_owned(),
        ];
        let merged = merge_via(link, &given);
        let names: Vec<&str> = merged.iter().map(|s| s.as_str()).collect();
        assert_eq!(names, ["a.example", "b.example"]);

        let many: Vec<String> = (0..50).map(|i| format!("s{i}.example")).collect();
        assert_eq!(merge_via(Vec::new(), &many).len(), 8);
    }

    #[test]
    fn roles_follow_the_power_level() {
        let level = |n: i64| UserPowerLevel::Int(Int::new(n).unwrap());
        assert_eq!(role_of(UserPowerLevel::Infinite), Role::Creator);
        assert_eq!(role_of(level(150)), Role::Admin);
        assert_eq!(role_of(level(100)), Role::Admin);
        assert_eq!(role_of(level(99)), Role::Moderator);
        assert_eq!(role_of(level(50)), Role::Moderator);
        assert_eq!(role_of(level(49)), Role::User);
        assert_eq!(role_of(level(0)), Role::User);
        assert_eq!(role_of(level(-5)), Role::User);
        assert_eq!(
            power_view(UserPowerLevel::Infinite),
            PowerLevelView::Creator
        );
        assert_eq!(power_view(level(50)), PowerLevelView::Level(50));
    }

    fn levels(users: &[(&str, i64)], state_default: i64, invite: i64) -> RoomPowerLevels {
        let rules = AuthorizationRules::V10;
        let mut content = RoomPowerLevelsEventContent::new(&rules);
        content.state_default = Int::new(state_default).unwrap();
        content.invite = Int::new(invite).unwrap();
        for (user, level) in users {
            content
                .users
                .insert((*user).try_into().unwrap(), Int::new(*level).unwrap());
        }
        RoomPowerLevels::new(RoomPowerLevelsSource::Original(content), &rules, Vec::new())
    }

    #[test]
    fn permissions_come_from_the_power_levels() {
        let me: OwnedUserId = "@me:x".try_into().unwrap();
        let admin = levels(&[("@me:x", 100)], 50, 0);
        assert_eq!(
            permissions(&admin, &me),
            RoomPermissions {
                can_set_name: true,
                can_set_topic: true,
                can_set_avatar: true,
                can_enable_encryption: true,
                can_invite: true,
            }
        );
        // Ordinary user: may invite (level 0 suffices) but change no state.
        let user = levels(&[], 50, 0);
        assert_eq!(
            permissions(&user, &me),
            RoomPermissions {
                can_invite: true,
                ..RoomPermissions::default()
            }
        );
        // Nothing allowed: invites need 50 too.
        let locked = levels(&[("@me:x", 10)], 50, 50);
        assert_eq!(permissions(&locked, &me), RoomPermissions::default());
        // Per-event overrides count.
        let rules = AuthorizationRules::V10;
        let mut content = RoomPowerLevelsEventContent::new(&rules);
        content.events.insert(
            matrix_sdk::ruma::events::StateEventType::RoomTopic.into(),
            int!(0),
        );
        let power =
            RoomPowerLevels::new(RoomPowerLevelsSource::Original(content), &rules, Vec::new());
        let p = permissions(&power, &me);
        assert!(p.can_set_topic && !p.can_set_name);
    }

    fn member(id: &str, name: Option<&str>, role: Role) -> RoomMemberView {
        RoomMemberView {
            user_id: UserId::from(id),
            display_name: name.map(Into::into),
            avatar: None,
            power: PowerLevelView::Level(0),
            role,
            membership: MembershipView::Joined,
        }
    }

    #[test]
    fn members_sort_by_role_then_name_ignoring_case() {
        let mut list = vec![
            member("@z:x", Some("zed"), Role::User),
            member("@b:x", Some("Bea"), Role::User),
            member("@m:x", Some("mod"), Role::Moderator),
            member("@c:x", None, Role::User),
            member("@a:x", Some("amy"), Role::Admin),
            member("@o:x", Some("Owner"), Role::Creator),
            member("@a2:x", Some("amy"), Role::User),
        ];
        sort_members(&mut list);
        let order: Vec<&str> = list.iter().map(|m| m.user_id.as_str()).collect();
        // Nameless members come last; ties go by user ID.
        assert_eq!(
            order,
            ["@o:x", "@a:x", "@m:x", "@a2:x", "@b:x", "@z:x", "@c:x"]
        );
    }

    #[test]
    fn an_image_must_be_what_its_mime_type_says() {
        let png = include_bytes!("../tests/fixtures/small-64x48.png");
        let jpg = include_bytes!("../tests/fixtures/exif-orientation-6.jpg");
        assert_eq!(
            check_image(png, "image/png").unwrap().essence_str(),
            "image/png"
        );
        assert_eq!(
            check_image(jpg, "image/jpeg").unwrap().essence_str(),
            "image/jpeg"
        );
        assert!(check_image(b"GIF89a\x01\0\x01\0", "image/gif").is_ok());
        assert!(check_image(b"RIFF\x04\0\0\0WEBPVP8 ", "image/webp").is_ok());
        for (bytes, mime) in [
            (&png[..], "image/jpeg"),
            (&jpg[..], "image/png"),
            (&png[..], "text/html"),
            (&png[..], "image/"),
            (&png[..], "image/*"),
            (&png[..], "garbage"),
            (b"<html><script>", "image/png"),
            (b"", "image/png"),
            (b"\x89PNG", "image/png"),
        ] {
            assert_eq!(
                check_image(bytes, mime),
                Err(CoreError::NotAllowed),
                "{mime}"
            );
        }
        // Parameters on the declared type are ignored; the case is not.
        assert!(check_image(png, "image/png; charset=binary").is_ok());
        let big = vec![0x89u8; 1024 * 1024 + 1];
        assert_eq!(check_image(&big, "image/png"), Err(CoreError::NotAllowed));
    }
}
