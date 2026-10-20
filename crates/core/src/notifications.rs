//! Notifications, notification settings and unread totals.
//!
//! The SDK evaluates the push rules on every synced event and calls one
//! handler per client. [`classify`] decides which of those become a
//! [`CoreEvent::Notification`]; the front end decides whether to show it.
//! Message text, names and keywords are user content: nothing here logs them,
//! and no error text quotes server data.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedTimelineEvent;
use matrix_sdk::notification_settings::{
    IsEncrypted, IsOneToOne, NotificationSettings, RoomNotificationMode,
};
use matrix_sdk::ruma::push::Action;
use matrix_sdk::ruma::{OwnedRoomId, UserId as SdkUserId};
use matrix_sdk::sync::Notification;
use matrix_sdk::{Client, NotificationSettingsError, Room};
use serde_json::Value;
use tokio::sync::{Notify, broadcast, mpsc, watch};
use tokio::task::JoinSet;

use crate::api::{
    ActionKind, Block, CoreError, CoreEvent, EventId, Inline, MessageText, NotificationKind,
    NotificationSettingsView, NotificationView, RoomId, RoomNotificationModeView, SpaceUnreadView,
    UnreadCountsView, UserId,
};
use crate::rich_text;
use crate::room_list::{highlights_of, is_bidi_control, name_or_id};
use crate::spaces::SpaceChildren;

/// Longest notification preview, in characters.
pub(crate) const PREVIEW_MAX_CHARS: usize = 120;
/// Events older than the session start by more than this are dropped
/// (initial sync and limited backfills).
pub(crate) const MAX_EVENT_AGE_MS: u64 = 60_000;
/// Event IDs remembered to drop duplicates.
pub(crate) const SEEN_CAPACITY: usize = 512;
/// The preview of an event that was still encrypted when the rules ran.
pub(crate) const ENCRYPTED_PREVIEW: &str = "Encrypted message";
/// What a spoiler is replaced by in a preview.
pub(crate) const SPOILER_PREVIEW: &str = "(spoiler)";

/// What an event is, as far as notifications care.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Content {
    /// A text message; the preview is already flattened.
    Text(String),
    /// A message without text (image, file, ...).
    Media,
    /// An event the SDK could not decrypt.
    Encrypted,
    /// An invitation for the user.
    Invite,
}

/// An event reduced to what [`classify`] needs. Its `Debug` hides everything
/// but the kind of content.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Incoming {
    pub(crate) event_id: String,
    pub(crate) sender: String,
    pub(crate) ts_ms: Option<u64>,
    pub(crate) content: Content,
    /// The push actions carry a highlight tweak.
    pub(crate) highlight: bool,
}

impl std::fmt::Debug for Incoming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Incoming")
            .field("highlight", &self.highlight)
            .finish_non_exhaustive()
    }
}

/// The outcome of [`classify`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Classified {
    pub(crate) kind: NotificationKind,
    pub(crate) preview: Option<String>,
}

/// Event IDs seen so far: a ring of [`SEEN_CAPACITY`].
#[derive(Default)]
pub(crate) struct Seen {
    order: VecDeque<String>,
    set: HashSet<String>,
}

impl Seen {
    /// Remembers `id`; false when it was already known.
    pub(crate) fn insert(&mut self, id: &str) -> bool {
        if self.set.contains(id) {
            return false;
        }
        if self.order.len() >= SEEN_CAPACITY
            && let Some(oldest) = self.order.pop_front()
        {
            self.set.remove(&oldest);
        }
        self.order.push_back(id.to_owned());
        self.set.insert(id.to_owned());
        true
    }
}

/// Decides whether `event` becomes a notification.
pub(crate) fn classify(
    event: &Incoming,
    own_user: &str,
    cutoff_ms: u64,
    seen: &mut Seen,
) -> Option<Classified> {
    if event.sender == own_user {
        return None;
    }
    if event.ts_ms.is_some_and(|ts| ts < cutoff_ms) {
        return None;
    }
    if !seen.insert(&event.event_id) {
        return None;
    }
    Some(match &event.content {
        Content::Invite => Classified {
            kind: NotificationKind::Invite,
            preview: None,
        },
        Content::Encrypted => Classified {
            kind: NotificationKind::Encrypted,
            preview: Some(ENCRYPTED_PREVIEW.to_owned()),
        },
        Content::Text(text) => Classified {
            kind: message_kind(event.highlight),
            preview: Some(clean_preview(text)),
        },
        Content::Media => Classified {
            kind: message_kind(event.highlight),
            preview: None,
        },
    })
}

fn message_kind(highlight: bool) -> NotificationKind {
    if highlight {
        NotificationKind::Mention
    } else {
        NotificationKind::Message
    }
}

/// One line of at most [`PREVIEW_MAX_CHARS`] characters: whitespace runs
/// become one space, other control and bidirectional characters are removed.
fn clean_preview(text: &str) -> String {
    clean_line(text, PREVIEW_MAX_CHARS)
}

/// One line of at most `max_chars` characters: whitespace runs become one
/// space, other control and bidirectional characters are removed.
pub(crate) fn clean_line(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut chars = 0;
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if c.is_control() || is_bidi_control(c) {
            continue;
        }
        if pending_space {
            out.push(' ');
            chars += 1;
            pending_space = false;
        }
        if chars >= max_chars {
            break;
        }
        out.push(c);
        chars += 1;
    }
    out
}

/// A name from the server (room, sender) on one clean line of at most
/// [`PREVIEW_MAX_CHARS`] characters: whitespace runs become one space, control
/// and bidirectional characters are removed.
pub(crate) fn clean_name(name: &str) -> String {
    clean_preview(name)
}

/// The text of rich text blocks, spoilers replaced and formatting dropped.
/// Stops once `limit` characters are collected.
fn flatten_blocks(blocks: &[Block], out: &mut String, limit: usize) {
    for block in blocks {
        if out.chars().count() >= limit {
            return;
        }
        match block {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                flatten_inlines(inlines, out);
                out.push(' ');
            }
            Block::Quote(inner) => flatten_blocks(inner, out, limit),
            Block::List { items, .. } => {
                for item in items {
                    flatten_blocks(item, out, limit);
                }
            }
            Block::Code { text } => {
                out.push_str(text);
                out.push(' ');
            }
            Block::Rule => out.push(' '),
        }
    }
}

fn flatten_inlines(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { text, .. } | Inline::Link { text, .. } => out.push_str(text),
            Inline::Mention { label, .. } => out.push_str(label),
            Inline::LineBreak => out.push(' '),
            Inline::Spoiler(_) => out.push_str(SPOILER_PREVIEW),
        }
    }
}

/// A plain body without the reply fallback some clients still put in front
/// (`> <@user> quoted` lines, then an empty line).
fn strip_reply_fallback(body: &str) -> &str {
    if !body.starts_with("> ") {
        return body;
    }
    let mut rest = body;
    while rest.starts_with('>') {
        match rest.split_once('\n') {
            Some((_, tail)) => rest = tail,
            None => return body,
        }
    }
    rest.strip_prefix('\n').unwrap_or(rest)
}

/// Reduces a raw event to an [`Incoming`] (without the push actions'
/// highlight). Anything that is not a message, an undecryptable event or an
/// invitation for `own_user` gives `None`.
///
/// A stripped invitation has no event ID; the caller supplies one.
///
/// The SDK's keyword push rules notify but carry no highlight tweak
/// (matrix-sdk `notification_settings/command.rs`, `SetKeywordPushRule`), so
/// a text message whose body contains one of `keywords` as a word is flagged
/// as a highlight here.
pub(crate) fn parse_event(
    json: &str,
    stripped: bool,
    own_user: &str,
    keywords: &[String],
) -> Option<Incoming> {
    let event: Value = serde_json::from_str(json).ok()?;
    let kind = event.get("type")?.as_str()?;
    let sender = event.get("sender")?.as_str()?.to_owned();
    let content = event.get("content")?;
    if stripped {
        let is_invite = kind == "m.room.member"
            && event.get("state_key").and_then(Value::as_str) == Some(own_user)
            && content.get("membership").and_then(Value::as_str) == Some("invite");
        return is_invite.then(|| Incoming {
            event_id: String::new(),
            sender,
            ts_ms: None,
            content: Content::Invite,
            highlight: false,
        });
    }
    // Messages are never state events.
    if event.get("state_key").is_some() {
        return None;
    }
    let (content, highlight) = match kind {
        "m.room.encrypted" => (Content::Encrypted, false),
        "m.room.message" => message_content(json, content, keywords)?,
        _ => return None,
    };
    Some(Incoming {
        event_id: event.get("event_id")?.as_str()?.to_owned(),
        sender,
        ts_ms: event.get("origin_server_ts").and_then(Value::as_u64),
        content,
        highlight,
    })
}

/// Whether `body` contains one of `keywords` as a whole word, ignoring case
/// (the push rule's content-pattern rule). A word ends at any character that
/// is not alphanumeric or `_`.
pub(crate) fn contains_keyword(body: &str, keywords: &[String]) -> bool {
    let body = body.to_lowercase();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    keywords.iter().any(|keyword| {
        let keyword = keyword.to_lowercase();
        !keyword.is_empty()
            && body.match_indices(&keyword).any(|(start, found)| {
                let before = body[..start].chars().next_back();
                let after = body[start + found.len()..].chars().next();
                !before.is_some_and(is_word) && !after.is_some_and(is_word)
            })
    })
}

/// The content of a message, and whether it matches a keyword.
fn message_content(json: &str, content: &Value, keywords: &[String]) -> Option<(Content, bool)> {
    let relation = content
        .get("m.relates_to")
        .and_then(|r| r.get("rel_type"))
        .and_then(Value::as_str);
    if relation == Some("m.replace") {
        return None;
    }
    let body = content.get("body")?.as_str()?;
    match content.get("msgtype")?.as_str()? {
        "m.text" | "m.notice" | "m.emote" | "m.server_notice" => {
            let rich = rich_text::message_rich_text(Some(json), None, strip_reply_fallback(body));
            let mut text = String::new();
            flatten_blocks(&rich.blocks, &mut text, PREVIEW_MAX_CHARS * 2);
            let keyword = contains_keyword(body, keywords);
            Some((Content::Text(text.trim().to_owned()), keyword))
        }
        "m.image" | "m.file" | "m.audio" | "m.video" | "m.location" => {
            Some((Content::Media, false))
        }
        _ => None,
    }
}

/// Longest keyword, in characters.
pub(crate) const KEYWORD_MAX_CHARS: usize = 64;
/// Most keywords an account may have.
pub(crate) const MAX_KEYWORDS: usize = 50;

/// Why a keyword is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeywordError {
    /// Empty after trimming, or longer than [`KEYWORD_MAX_CHARS`].
    Length,
    /// Contains a control character.
    Control,
    /// The account already has [`MAX_KEYWORDS`].
    TooMany,
}

impl KeywordError {
    /// Fixed text for the front end; it never quotes the keyword.
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Length => "a keyword must have 1 to 64 characters",
            Self::Control => "a keyword must not contain control characters",
            Self::TooMany => "there are too many keywords",
        }
    }
}

/// Trims `raw` and checks it. `existing` is the number of keywords the
/// account has and `already_there` whether this one is among them (adding it
/// again changes nothing, so it is allowed at the limit).
pub(crate) fn validate_keyword(
    raw: &str,
    existing: usize,
    already_there: bool,
) -> Result<String, KeywordError> {
    let keyword = raw.trim();
    let chars = keyword.chars().count();
    if chars == 0 || chars > KEYWORD_MAX_CHARS {
        return Err(KeywordError::Length);
    }
    if keyword.chars().any(char::is_control) {
        return Err(KeywordError::Control);
    }
    if existing >= MAX_KEYWORDS && !already_there {
        return Err(KeywordError::TooMany);
    }
    Ok(keyword.to_owned())
}

/// The unread counts of one joined room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoomCounts {
    pub(crate) id: OwnedRoomId,
    pub(crate) notifications: u64,
    pub(crate) highlights: u64,
    /// The room is marked unread, or has unread messages.
    pub(crate) unread: bool,
    /// The room's effective notification mode is "mute".
    pub(crate) muted: bool,
}

/// Unread totals over all rooms: `(notifications, highlights)`.
pub(crate) type Totals = (u64, u64);

/// Sums the counts of the rooms that are not muted (saturating).
pub(crate) fn totals<'a>(rooms: impl IntoIterator<Item = &'a RoomCounts>) -> Totals {
    rooms
        .into_iter()
        .filter(|r| !r.muted)
        .fold((0, 0), |(n, h), r| {
            (
                n.saturating_add(r.notifications),
                h.saturating_add(r.highlights),
            )
        })
}

/// Adds the rooms that are not muted into one [`UnreadCountsView`].
fn sum_counts<'a>(rooms: impl IntoIterator<Item = &'a RoomCounts>) -> UnreadCountsView {
    rooms
        .into_iter()
        .filter(|r| !r.muted)
        .fold(UnreadCountsView::default(), |sum, r| UnreadCountsView {
            notifications: sum.notifications.saturating_add(r.notifications),
            highlights: sum.highlights.saturating_add(r.highlights),
            unread: sum.unread || r.unread,
        })
}

/// Unread counts for each space (by ID order) and for the rooms in no space.
///
/// `children` holds each known space's SDK descendants. A space ID is never
/// a room: it is left out of every sum and of `outside`. A space that no
/// other space lists is top-level; its direct children that are spaces
/// themselves stand for their own descendants, and a room that is reached
/// twice counts once. A room in two top-level spaces counts in both.
pub(crate) fn space_totals(
    rooms: &[RoomCounts],
    children: &SpaceChildren,
) -> (Vec<SpaceUnreadView>, UnreadCountsView) {
    let listed: HashSet<&OwnedRoomId> = children.values().flatten().collect();
    let mut ids: Vec<&OwnedRoomId> = children.keys().collect();
    ids.sort_unstable();
    let spaces = ids
        .into_iter()
        .map(|space| {
            let top_level = !listed.contains(space);
            let mut members: HashSet<&OwnedRoomId> = HashSet::new();
            for child in &children[space] {
                match children.get(child) {
                    // A space is never a room; a top-level space also takes
                    // everything its sub-space holds.
                    Some(inner) if top_level => {
                        members.extend(inner.iter().filter(|id| !children.contains_key(*id)));
                    }
                    Some(_) => {}
                    None => {
                        members.insert(child);
                    }
                }
            }
            SpaceUnreadView {
                space: RoomId::from(space.as_str()),
                counts: sum_counts(rooms.iter().filter(|r| members.contains(&r.id))),
            }
        })
        .collect();
    let outside = sum_counts(
        rooms
            .iter()
            .filter(|r| !listed.contains(&r.id) && !children.contains_key(&r.id)),
    );
    (spaces, outside)
}

/// Everything one computation yields: the tray totals, and the per-space
/// counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rollup {
    pub(crate) totals: Totals,
    pub(crate) spaces: Vec<SpaceUnreadView>,
    pub(crate) outside: UnreadCountsView,
}

impl Rollup {
    /// The rollup of `rooms` given the spaces' descendants.
    pub(crate) fn of(rooms: &[RoomCounts], children: &SpaceChildren) -> Self {
        let (spaces, outside) = space_totals(rooms, children);
        Self {
            totals: totals(rooms),
            spaces,
            outside,
        }
    }
}

/// Where the totals come from, and when they may have changed.
pub(crate) trait TotalsSource: Send {
    /// The current totals and per-space counts.
    fn compute(&mut self) -> impl Future<Output = Rollup> + Send;
    /// Waits for a possible change; false when no change can come any more.
    fn changed(&mut self) -> impl Future<Output = bool> + Send;
}

/// Shortest time between two totals computations.
pub(crate) const TOTALS_INTERVAL: Duration = Duration::from_secs(1);

/// Sends [`CoreEvent::UnreadTotals`] and [`CoreEvent::SpaceUnread`], each when
/// its value changes: once at the start, then at most once per `interval`. A
/// change that arrives within the interval is picked up when it ends.
pub(crate) async fn run_totals<S: TotalsSource>(
    mut source: S,
    events: mpsc::Sender<CoreEvent>,
    interval: Duration,
) {
    let mut last_totals = None;
    let mut last_spaces: Option<(Vec<SpaceUnreadView>, UnreadCountsView)> = None;
    loop {
        let now = source.compute().await;
        if last_totals != Some(now.totals) {
            last_totals = Some(now.totals);
            let event = CoreEvent::UnreadTotals {
                notifications: now.totals.0,
                highlights: now.totals.1,
            };
            if events.send(event).await.is_err() {
                return;
            }
        }
        let spaces = (now.spaces, now.outside);
        if last_spaces.as_ref() != Some(&spaces) {
            last_spaces = Some(spaces.clone());
            let event = CoreEvent::SpaceUnread {
                spaces: spaces.0,
                outside: spaces.1,
            };
            if events.send(event).await.is_err() {
                return;
            }
        }
        tokio::time::sleep(interval).await;
        if !source.changed().await {
            return;
        }
    }
}

pub(crate) fn mode_view(mode: RoomNotificationMode) -> RoomNotificationModeView {
    match mode {
        RoomNotificationMode::AllMessages => RoomNotificationModeView::AllMessages,
        RoomNotificationMode::MentionsAndKeywordsOnly => {
            RoomNotificationModeView::MentionsAndKeywords
        }
        RoomNotificationMode::Mute => RoomNotificationModeView::Mute,
    }
}

fn sdk_mode(mode: RoomNotificationModeView) -> RoomNotificationMode {
    match mode {
        RoomNotificationModeView::AllMessages => RoomNotificationMode::AllMessages,
        RoomNotificationModeView::MentionsAndKeywords => {
            RoomNotificationMode::MentionsAndKeywordsOnly
        }
        RoomNotificationModeView::Mute => RoomNotificationMode::Mute,
    }
}

/// The account's enabled keywords, shared with the notification handler.
#[derive(Clone, Default)]
struct Keywords(Arc<Mutex<Vec<String>>>);

impl Keywords {
    fn set(&self, keywords: &[String]) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = keywords.to_vec();
    }

    fn get(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Reads the settings and publishes the keywords to the handler.
async fn publish_settings(
    settings: &NotificationSettings,
    keywords: &Keywords,
) -> NotificationSettingsView {
    let view = read_settings(settings).await;
    keywords.set(&view.keywords);
    view
}

/// The account's defaults and keywords. Defaults are read for unencrypted
/// rooms; they are always written for encrypted and unencrypted alike.
async fn read_settings(settings: &NotificationSettings) -> NotificationSettingsView {
    let group = settings
        .get_default_room_notification_mode(IsEncrypted::No, IsOneToOne::No)
        .await;
    let dm = settings
        .get_default_room_notification_mode(IsEncrypted::No, IsOneToOne::Yes)
        .await;
    NotificationSettingsView {
        group_default: mode_view(group),
        dm_default: mode_view(dm),
        keywords: settings.enabled_keywords().await.into_iter().collect(),
    }
}

/// Whether the room's effective mode (its own, else the account default for
/// its kind) is "mute".
async fn is_muted(settings: &NotificationSettings, room: &Room) -> bool {
    let mode = match settings
        .get_user_defined_room_notification_mode(room.room_id())
        .await
    {
        Some(mode) => mode,
        None => {
            settings
                .get_default_room_notification_mode(
                    IsEncrypted::from(room.encryption_state().is_encrypted()),
                    IsOneToOne::from(room.active_members_count() == 2),
                )
                .await
        }
    };
    mode == RoomNotificationMode::Mute
}

// ---- The SDK side --------------------------------------------------------

/// Unread totals of a client's joined rooms.
struct SdkTotals {
    client: Client,
    settings: NotificationSettings,
    /// Pinged by the forwarders when a room or the settings changed.
    changed: Arc<Notify>,
    /// The descendants of each space; a change recomputes the roll-up.
    spaces: watch::Receiver<SpaceChildren>,
}

impl TotalsSource for SdkTotals {
    async fn compute(&mut self) -> Rollup {
        let mut counts = Vec::new();
        for room in self.client.joined_rooms() {
            counts.push(RoomCounts {
                id: room.room_id().to_owned(),
                notifications: room.num_unread_notifications(),
                highlights: highlights_of(
                    room.num_unread_mentions(),
                    room.unread_notification_counts().highlight_count,
                ),
                unread: room.is_marked_unread() || room.num_unread_messages() > 0,
                muted: is_muted(&self.settings, &room).await,
            });
        }
        let children = self.spaces.borrow_and_update().clone();
        Rollup::of(&counts, &children)
    }

    async fn changed(&mut self) -> bool {
        tokio::select! {
            () = self.changed.notified() => {}
            // A closed watch only means no more space changes can come.
            Ok(()) = self.spaces.changed() => {}
        }
        true
    }
}

/// How long the handler waits for the front end to take an event before it
/// gives up on it; the SDK awaits handlers while it processes a sync.
const SEND_DEADLINE: Duration = Duration::from_secs(5);

/// State of the notification handler, shared with the SDK for the life of
/// the client. It holds no client, so it forms no cycle, and only a weak
/// sender: the client may outlive the session's tasks for a while, and the
/// event channel must close when the core is done.
struct Handler {
    events: mpsc::WeakSender<CoreEvent>,
    keywords: Keywords,
    seen: Mutex<Seen>,
    cutoff_ms: u64,
}

impl Handler {
    async fn handle(&self, notification: Notification, room: Room) {
        let (json, stripped) = match &notification.event {
            RawAnySyncOrStrippedTimelineEvent::Sync(raw) => (raw.json().get(), false),
            RawAnySyncOrStrippedTimelineEvent::Stripped(raw) => (raw.json().get(), true),
        };
        let own = room.own_user_id();
        let keywords = self.keywords.get();
        let Some(mut event) = parse_event(json, stripped, own.as_str(), &keywords) else {
            return;
        };
        if stripped {
            // Invitations are stripped state: they have no event ID.
            event.event_id = format!("invite:{}", room.room_id());
        }
        event.highlight |= notification.actions.iter().any(Action::is_highlight);
        let classified = {
            let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
            classify(&event, own.as_str(), self.cutoff_ms, &mut seen)
        };
        let Some(classified) = classified else {
            return;
        };
        let room_id = RoomId::from(room.room_id().as_str());
        let room_name = name_or_id(
            room.display_name()
                .await
                .ok()
                .map(|n| clean_name(&n.to_string())),
            &room_id,
        );
        let sender_name = match SdkUserId::parse(&event.sender) {
            Ok(sender) => room
                .get_member_no_sync(&sender)
                .await
                .ok()
                .flatten()
                .and_then(|m| m.display_name().map(clean_name))
                .filter(|name| !name.is_empty()),
            Err(_) => None,
        };
        let view = NotificationView {
            room_id,
            event_id: EventId::from(event.event_id),
            room_name,
            sender: UserId::from(event.sender),
            sender_name,
            is_dm: room.is_direct().await.unwrap_or(false),
            kind: classified.kind,
            preview: classified.preview.map(MessageText::from),
        };
        // No sender left means the core is shutting down.
        let Some(events) = self.events.upgrade() else {
            return;
        };
        // A dropped receiver just means nobody listens any more.
        let _ =
            tokio::time::timeout(SEND_DEADLINE, events.send(CoreEvent::Notification(view))).await;
    }
}

/// A change to the notification settings, run in order by the worker.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Job {
    /// Set a room's own mode, or (`None`) go back to the default.
    RoomMode {
        room: OwnedRoomId,
        mode: Option<RoomNotificationModeView>,
    },
    /// Set the default of group rooms or direct messages.
    DefaultMode {
        one_to_one: bool,
        mode: RoomNotificationModeView,
    },
    AddKeyword(String),
    RemoveKeyword(String),
}

/// Jobs that may wait for the worker; more are answered `Busy`.
const MAX_QUEUED_JOBS: usize = 32;

/// A failed settings change as a fixed message: nothing from the server or
/// the keyword is quoted.
fn settings_error(e: &NotificationSettingsError) -> CoreError {
    match e {
        NotificationSettingsError::UnableToSavePushRules => {
            CoreError::Network("the notification settings could not be saved".into())
        }
        _ => CoreError::Internal("the notification setting could not be changed".into()),
    }
}

async fn run_job(settings: &NotificationSettings, job: Job) -> Result<(), CoreError> {
    let fail = |e: NotificationSettingsError| settings_error(&e);
    match job {
        Job::RoomMode {
            room,
            mode: Some(mode),
        } => settings
            .set_room_notification_mode(&room, sdk_mode(mode))
            .await
            .map_err(fail),
        Job::RoomMode { room, mode: None } => settings
            .delete_user_defined_room_rules(&room)
            .await
            .map_err(fail),
        Job::DefaultMode { one_to_one, mode } => {
            // Encrypted and unencrypted rooms alike, as Element X does.
            for encrypted in [IsEncrypted::No, IsEncrypted::Yes] {
                settings
                    .set_default_room_notification_mode(
                        encrypted,
                        IsOneToOne::from(one_to_one),
                        sdk_mode(mode),
                    )
                    .await
                    .map_err(fail)?;
            }
            Ok(())
        }
        Job::AddKeyword(raw) => {
            let current = settings.enabled_keywords().await;
            let keyword = validate_keyword(&raw, current.len(), current.contains(raw.trim()))
                .map_err(|e| CoreError::Internal(e.message().into()))?;
            settings.add_keyword(keyword).await.map_err(fail)
        }
        Job::RemoveKeyword(raw) => {
            let keyword = validate_keyword(&raw, 0, true)
                .map_err(|e| CoreError::Internal(e.message().into()))?;
            settings.remove_keyword(&keyword).await.map_err(fail)
        }
    }
}

/// The room a failed job belongs to; empty for account-wide settings.
fn job_room(job: &Job) -> RoomId {
    match job {
        Job::RoomMode { room, .. } => RoomId::from(room.as_str()),
        _ => RoomId::from(""),
    }
}

async fn work(
    settings: NotificationSettings,
    keywords: Keywords,
    mut jobs: mpsc::Receiver<Job>,
    events: mpsc::Sender<CoreEvent>,
) {
    while let Some(job) = jobs.recv().await {
        let room_id = job_room(&job);
        let event = match run_job(&settings, job).await {
            Ok(()) => CoreEvent::NotificationSettings(publish_settings(&settings, &keywords).await),
            Err(error) => CoreEvent::ActionFailed {
                room_id,
                action: ActionKind::Notifications,
                error,
            },
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

/// Sends the settings now and after every change.
async fn forward_settings(
    settings: NotificationSettings,
    keywords: Keywords,
    events: mpsc::Sender<CoreEvent>,
    changed: Arc<Notify>,
) {
    let mut changes = settings.subscribe_to_changes();
    loop {
        let view = publish_settings(&settings, &keywords).await;
        if events
            .send(CoreEvent::NotificationSettings(view))
            .await
            .is_err()
        {
            return;
        }
        match changes.recv().await {
            Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => changed.notify_one(),
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Pings `changed` whenever any room's info changes (unread counts, the
/// cached notification mode, membership, ...).
async fn forward_room_changes(client: Client, changed: Arc<Notify>) {
    let mut updates = client.room_info_notable_update_receiver();
    loop {
        match updates.recv().await {
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => changed.notify_one(),
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Keeps each joined room's cached notification mode in step with the push
/// rules, and tells the room list about every room whose mode changed. The
/// SDK refreshes that cache after a sync but raises no room info update for
/// it, so the room list showed a new mode (set here or on another device)
/// only once something else happened in the room.
async fn refresh_room_modes(client: Client, settings: NotificationSettings) {
    let mut changes = settings.subscribe_to_changes();
    let mut shown: HashMap<OwnedRoomId, Option<RoomNotificationMode>> = client
        .joined_rooms()
        .iter()
        .map(|room| {
            let mode = room.cached_user_defined_notification_mode();
            (room.room_id().to_owned(), mode)
        })
        .collect();
    loop {
        match changes.recv().await {
            Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return,
        }
        for room in client.joined_rooms() {
            let mode = settings
                .get_user_defined_room_notification_mode(room.room_id())
                .await;
            if shown.insert(room.room_id().to_owned(), mode) == Some(mode) {
                continue;
            }
            match mode {
                Some(mode) => room.update_cached_user_defined_notification_mode(mode),
                None => room.clear_user_defined_notification_mode(),
            }
            // No reason given: the SDK sends its catch-all one, which the room
            // list answers with the room's entry.
            room.update_room_info(|info| (info, Default::default()))
                .await;
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The notification work of one session: the push-rule handler's state, the
/// settings worker and forwarder, and the unread totals.
pub(crate) struct NotificationTasks {
    jobs: mpsc::Sender<Job>,
    tasks: JoinSet<()>,
}

impl NotificationTasks {
    /// Registers the notification handler (once: this runs once per client)
    /// and starts the tasks. Call it before the sync starts, so that no
    /// event is missed.
    pub(crate) async fn start(
        client: &Client,
        events: &mpsc::Sender<CoreEvent>,
        spaces: watch::Receiver<SpaceChildren>,
    ) -> Self {
        let keywords = Keywords::default();
        let handler = Arc::new(Handler {
            events: events.downgrade(),
            keywords: keywords.clone(),
            seen: Mutex::new(Seen::default()),
            cutoff_ms: now_ms().saturating_sub(MAX_EVENT_AGE_MS),
        });
        client
            .register_notification_handler(move |notification, room, _client| {
                let handler = handler.clone();
                async move { handler.handle(notification, room).await }
            })
            .await;
        let settings = client.notification_settings().await;
        let changed = Arc::new(Notify::new());
        let (jobs, queue) = mpsc::channel(MAX_QUEUED_JOBS);
        let mut tasks = JoinSet::new();
        tasks.spawn(work(
            settings.clone(),
            keywords.clone(),
            queue,
            events.clone(),
        ));
        tasks.spawn(forward_settings(
            settings.clone(),
            keywords,
            events.clone(),
            changed.clone(),
        ));
        tasks.spawn(forward_room_changes(client.clone(), changed.clone()));
        tasks.spawn(refresh_room_modes(client.clone(), settings.clone()));
        tasks.spawn(run_totals(
            SdkTotals {
                client: client.clone(),
                settings,
                changed,
                spaces,
            },
            events.clone(),
            TOTALS_INTERVAL,
        ));
        Self { jobs, tasks }
    }

    /// Queues a settings change.
    pub(crate) fn submit(&self, job: Job) -> Result<(), CoreError> {
        self.jobs.try_send(job).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => CoreError::Busy,
            mpsc::error::TrySendError::Closed(_) => {
                CoreError::Internal("notification settings have stopped".into())
            }
        })
    }

    /// Ends every task; they hold clones of the client.
    pub(crate) async fn stop(&mut self) {
        self.tasks.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const ME: &str = "@carol:x";
    const NOW: u64 = 1_000_000_000;

    fn incoming(id: &str, sender: &str, content: Content) -> Incoming {
        Incoming {
            event_id: id.into(),
            sender: sender.into(),
            ts_ms: Some(NOW),
            content,
            highlight: false,
        }
    }

    fn text(id: &str, sender: &str, body: &str) -> Incoming {
        incoming(id, sender, Content::Text(body.into()))
    }

    fn run(event: &Incoming) -> Option<Classified> {
        classify(event, ME, NOW - MAX_EVENT_AGE_MS, &mut Seen::default())
    }

    #[test]
    fn a_message_from_someone_else_notifies() {
        let got = run(&text("$1", "@alice:x", "hello")).expect("notifies");
        assert_eq!(got.kind, NotificationKind::Message);
        assert_eq!(got.preview.as_deref(), Some("hello"));
    }

    #[test]
    fn own_messages_are_dropped() {
        assert_eq!(run(&text("$1", ME, "hello")), None);
    }

    #[test]
    fn old_events_are_dropped_and_the_boundary_is_kept() {
        let cutoff = NOW - MAX_EVENT_AGE_MS;
        let mut e = text("$1", "@alice:x", "old");
        e.ts_ms = Some(cutoff - 1);
        assert_eq!(classify(&e, ME, cutoff, &mut Seen::default()), None);
        e.ts_ms = Some(cutoff);
        assert!(classify(&e, ME, cutoff, &mut Seen::default()).is_some());
        // An event without a timestamp (a stripped invite) is never "old".
        e.ts_ms = None;
        assert!(classify(&e, ME, cutoff, &mut Seen::default()).is_some());
    }

    #[test]
    fn duplicates_are_dropped_but_a_dropped_event_is_not_remembered() {
        let mut seen = Seen::default();
        let cutoff = NOW - MAX_EVENT_AGE_MS;
        let e = text("$1", "@alice:x", "hi");
        assert!(classify(&e, ME, cutoff, &mut seen).is_some());
        assert_eq!(classify(&e, ME, cutoff, &mut seen), None);
        // An own event dropped earlier must not hide a later real one.
        let own = text("$2", ME, "mine");
        assert_eq!(classify(&own, ME, cutoff, &mut seen), None);
        assert!(seen.insert("$2"));
    }

    #[test]
    fn the_seen_ring_forgets_the_oldest_id() {
        let mut seen = Seen::default();
        for i in 0..SEEN_CAPACITY {
            assert!(seen.insert(&format!("$e{i}")));
        }
        assert!(!seen.insert("$e1"));
        assert!(seen.insert("$new"));
        // `$e0` was the oldest and fell out; `$e1` is still known.
        assert!(seen.insert("$e0"));
        assert!(!seen.insert("$new"));
        assert!(seen.order.len() <= SEEN_CAPACITY);
        assert_eq!(seen.order.len(), seen.set.len());
    }

    #[test]
    fn a_highlight_gives_a_mention() {
        let mut e = text("$1", "@alice:x", "hey carol");
        e.highlight = true;
        let got = run(&e).expect("notifies");
        assert_eq!(got.kind, NotificationKind::Mention);
        assert_eq!(got.preview.as_deref(), Some("hey carol"));
    }

    #[test]
    fn an_encrypted_event_gives_the_fixed_preview() {
        let mut e = incoming("$1", "@alice:x", Content::Encrypted);
        // Mention rules cannot match an encrypted event, but even a highlight
        // flag must not reveal anything.
        e.highlight = true;
        let got = run(&e).expect("notifies");
        assert_eq!(got.kind, NotificationKind::Encrypted);
        assert_eq!(got.preview.as_deref(), Some(ENCRYPTED_PREVIEW));
    }

    #[test]
    fn an_invite_gives_no_preview() {
        let got = run(&incoming("invite:!r:x", "@alice:x", Content::Invite)).expect("notifies");
        assert_eq!(got.kind, NotificationKind::Invite);
        assert_eq!(got.preview, None);
    }

    #[test]
    fn a_media_message_has_no_preview() {
        let got = run(&incoming("$1", "@alice:x", Content::Media)).expect("notifies");
        assert_eq!(got.kind, NotificationKind::Message);
        assert_eq!(got.preview, None);
    }

    #[test]
    fn the_preview_is_capped_at_120_characters() {
        let long = "é".repeat(500);
        let got = run(&text("$1", "@alice:x", &long)).expect("notifies");
        let preview = got.preview.expect("a preview");
        assert_eq!(preview.chars().count(), PREVIEW_MAX_CHARS);
        assert!(preview.chars().all(|c| c == 'é'));
    }

    #[test]
    fn the_preview_is_one_clean_line() {
        let body = "a\n\nb\tc\u{0}d\u{202E}e  ";
        let got = run(&text("$1", "@alice:x", body)).expect("notifies");
        assert_eq!(got.preview.as_deref(), Some("a b cde"));
    }

    fn message(body: &str, extra: Value) -> String {
        let mut content = json!({"msgtype": "m.text", "body": body});
        content
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!({
            "type": "m.room.message",
            "event_id": "$m",
            "sender": "@alice:x",
            "origin_server_ts": NOW,
            "content": content,
        })
        .to_string()
    }

    fn parsed_text(json: &str) -> Option<String> {
        match parse_event(json, false, ME, &[])?.content {
            Content::Text(t) => Some(t),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_plain_message_parses() {
        let e = parse_event(&message("hi there", json!({})), false, ME, &[]).expect("parsed");
        assert_eq!(e.event_id, "$m");
        assert_eq!(e.sender, "@alice:x");
        assert_eq!(e.ts_ms, Some(NOW));
        assert_eq!(e.content, Content::Text("hi there".into()));
        assert!(!e.highlight);
    }

    #[test]
    fn spoilers_in_the_formatted_body_are_replaced() {
        let html = r#"the answer is <span data-mx-spoiler="why">forty-two</span>!"#;
        let json = message(
            "the answer is forty-two!",
            json!({"format": "org.matrix.custom.html", "formatted_body": html}),
        );
        let text = parsed_text(&json).expect("text");
        assert!(text.contains(SPOILER_PREVIEW), "{text:?}");
        assert!(!text.contains("forty-two"), "{text:?}");
        assert!(text.starts_with("the answer is "));
    }

    #[test]
    fn formatting_is_dropped_from_the_preview() {
        let html = "<b>bold</b> and <i>italic</i><br>next";
        let json = message(
            "bold and italic\nnext",
            json!({"format": "org.matrix.custom.html", "formatted_body": html}),
        );
        assert_eq!(parsed_text(&json).as_deref(), Some("bold and italic next"));
    }

    #[test]
    fn a_plain_reply_fallback_is_not_part_of_the_preview() {
        let json = message("> <@bob:x> quoted\n\nthe reply", json!({}));
        assert_eq!(parsed_text(&json).as_deref(), Some("the reply"));
    }

    #[test]
    fn notices_and_emotes_have_text_and_files_do_not() {
        let mut v: Value = serde_json::from_str(&message("x", json!({}))).unwrap();
        v["content"]["msgtype"] = json!("m.emote");
        assert!(matches!(
            parse_event(&v.to_string(), false, ME, &[]).unwrap().content,
            Content::Text(_)
        ));
        v["content"]["msgtype"] = json!("m.image");
        assert_eq!(
            parse_event(&v.to_string(), false, ME, &[]).unwrap().content,
            Content::Media
        );
        v["content"]["msgtype"] = json!("m.key.verification.request");
        assert_eq!(parse_event(&v.to_string(), false, ME, &[]), None);
    }

    #[test]
    fn edits_are_dropped() {
        let json = message(
            "* fixed",
            json!({"m.relates_to": {"rel_type": "m.replace", "event_id": "$o"}}),
        );
        assert_eq!(parse_event(&json, false, ME, &[]), None);
    }

    #[test]
    fn an_encrypted_event_parses_without_its_content() {
        let json = json!({
            "type": "m.room.encrypted",
            "event_id": "$e",
            "sender": "@alice:x",
            "origin_server_ts": NOW,
            "content": {"algorithm": "m.megolm.v1.aes-sha2", "ciphertext": "x"},
        })
        .to_string();
        assert_eq!(
            parse_event(&json, false, ME, &[]).unwrap().content,
            Content::Encrypted
        );
    }

    #[test]
    fn state_events_and_other_types_are_dropped() {
        for json in [
            json!({"type": "m.room.name", "state_key": "", "event_id": "$s",
                "sender": "@alice:x", "origin_server_ts": NOW, "content": {"name": "n"}}),
            json!({"type": "m.room.member", "state_key": "@bob:x", "event_id": "$s",
                "sender": "@alice:x", "origin_server_ts": NOW,
                "content": {"membership": "join"}}),
            json!({"type": "m.reaction", "event_id": "$s", "sender": "@alice:x",
                "origin_server_ts": NOW, "content": {}}),
            // A redacted message has no body and no msgtype.
            json!({"type": "m.room.message", "event_id": "$s", "sender": "@alice:x",
                "origin_server_ts": NOW, "content": {}}),
        ] {
            assert_eq!(
                parse_event(&json.to_string(), false, ME, &[]),
                None,
                "{json}"
            );
        }
        assert_eq!(parse_event("not json", false, ME, &[]), None);
    }

    fn member(state_key: &str, membership: &str) -> String {
        json!({
            "type": "m.room.member",
            "state_key": state_key,
            "sender": "@alice:x",
            "content": {"membership": membership},
        })
        .to_string()
    }

    #[test]
    fn an_invite_for_the_user_parses() {
        let e = parse_event(&member(ME, "invite"), true, ME, &[]).expect("parsed");
        assert_eq!(e.content, Content::Invite);
        assert_eq!(e.sender, "@alice:x");
        assert_eq!(e.ts_ms, None);
    }

    #[test]
    fn other_stripped_events_are_dropped() {
        assert_eq!(
            parse_event(&member("@bob:x", "invite"), true, ME, &[]),
            None
        );
        assert_eq!(parse_event(&member(ME, "join"), true, ME, &[]), None);
        let name = json!({"type": "m.room.name", "state_key": "", "sender": "@a:x",
            "content": {"name": "n"}});
        assert_eq!(parse_event(&name.to_string(), true, ME, &[]), None);
    }

    #[test]
    fn debug_hides_the_content() {
        let e = text("$1", "@alice:x", "secret-body-x");
        assert!(!format!("{e:?}").contains("secret-body-x"));
    }

    #[test]
    fn keywords_are_trimmed_and_bounded() {
        assert_eq!(
            validate_keyword("  twicecat  ", 0, false).as_deref(),
            Ok("twicecat")
        );
        assert_eq!(validate_keyword("   ", 0, false), Err(KeywordError::Length));
        assert_eq!(validate_keyword("", 0, false), Err(KeywordError::Length));
        let max = "k".repeat(KEYWORD_MAX_CHARS);
        assert_eq!(
            validate_keyword(&max, 0, false).as_deref(),
            Ok(max.as_str())
        );
        let over = "k".repeat(KEYWORD_MAX_CHARS + 1);
        assert_eq!(validate_keyword(&over, 0, false), Err(KeywordError::Length));
        // Characters, not bytes.
        let wide = "é".repeat(KEYWORD_MAX_CHARS);
        assert!(validate_keyword(&wide, 0, false).is_ok());
    }

    #[test]
    fn keywords_refuse_control_characters() {
        assert_eq!(
            validate_keyword("a\nb", 0, false),
            Err(KeywordError::Control)
        );
        assert_eq!(
            validate_keyword("a\u{0}", 0, false),
            Err(KeywordError::Control)
        );
        // Surrounding whitespace is trimmed first.
        assert_eq!(validate_keyword("\tok\n", 0, false).as_deref(), Ok("ok"));
    }

    #[test]
    fn at_most_fifty_keywords() {
        assert!(validate_keyword("k", MAX_KEYWORDS - 1, false).is_ok());
        assert_eq!(
            validate_keyword("k", MAX_KEYWORDS, false),
            Err(KeywordError::TooMany)
        );
        assert!(validate_keyword("k", MAX_KEYWORDS, true).is_ok());
    }

    #[test]
    fn keyword_messages_never_quote_the_keyword() {
        for e in [
            KeywordError::Length,
            KeywordError::Control,
            KeywordError::TooMany,
        ] {
            assert!(!e.message().is_empty());
        }
    }

    fn room_id(id: &str) -> OwnedRoomId {
        id.try_into().unwrap()
    }

    fn counts(n: u64, h: u64, muted: bool) -> RoomCounts {
        RoomCounts {
            id: room_id("!r:x"),
            notifications: n,
            highlights: h,
            unread: false,
            muted,
        }
    }

    #[test]
    fn totals_sum_the_rooms_that_are_not_muted() {
        let table: [(&[RoomCounts], Totals); 5] = [
            (&[], (0, 0)),
            (&[counts(3, 1, false), counts(2, 0, false)], (5, 1)),
            (&[counts(3, 1, false), counts(9, 4, true)], (3, 1)),
            (&[counts(9, 4, true)], (0, 0)),
            (
                &[counts(u64::MAX, 1, false), counts(5, 0, false)],
                (u64::MAX, 1),
            ),
        ];
        for (rooms, want) in table {
            assert_eq!(totals(rooms), want, "{rooms:?}");
        }
    }

    fn room(id: &str, n: u64, h: u64) -> RoomCounts {
        RoomCounts {
            id: room_id(id),
            notifications: n,
            highlights: h,
            unread: false,
            muted: false,
        }
    }

    /// Descendants as the SDK builds them: a level-0 space lists its direct
    /// children; a level-1 space lists all its descendants and itself, last.
    fn tree(spaces: &[(&str, &[&str])]) -> SpaceChildren {
        Arc::new(
            spaces
                .iter()
                .map(|(space, kids)| (room_id(space), kids.iter().map(|k| room_id(k)).collect()))
                .collect(),
        )
    }

    fn of(spaces: &[SpaceUnreadView], id: &str) -> UnreadCountsView {
        spaces
            .iter()
            .find(|s| s.space.as_str() == id)
            .unwrap_or_else(|| panic!("no entry for {id}"))
            .counts
    }

    fn view(n: u64, h: u64) -> UnreadCountsView {
        UnreadCountsView {
            notifications: n,
            highlights: h,
            unread: false,
        }
    }

    #[test]
    fn a_muted_room_counts_nowhere() {
        let mut muted = room("!a:x", 9, 4);
        muted.muted = true;
        muted.unread = true;
        let rooms = [muted, room("!b:x", 2, 1), room("!out:x", 1, 0)];
        let (spaces, outside) = space_totals(&rooms, &tree(&[("!s:x", &["!a:x", "!b:x"])]));
        assert_eq!(of(&spaces, "!s:x"), view(2, 1));
        assert_eq!(outside, view(1, 0));
        // A room only muted gives no unread flag either.
        let (spaces, _) = space_totals(&rooms[..1], &tree(&[("!s:x", &["!a:x"])]));
        assert_eq!(of(&spaces, "!s:x"), UnreadCountsView::default());
    }

    #[test]
    fn a_room_of_a_sub_space_counts_for_it_and_for_the_top_space_once() {
        // `!top` holds `!direct` and the sub-space `!sub`, which holds `!deep`
        // and, as the SDK lists it, itself last. `!both` is in both lists.
        let children = tree(&[
            ("!top:x", &["!direct:x", "!sub:x", "!both:x"]),
            ("!sub:x", &["!deep:x", "!both:x", "!sub:x"]),
        ]);
        let rooms = [
            room("!direct:x", 1, 0),
            room("!deep:x", 10, 2),
            room("!both:x", 100, 0),
            // A space's own counts are not a room's.
            room("!sub:x", 1000, 0),
            room("!top:x", 10_000, 0),
        ];
        let (spaces, outside) = space_totals(&rooms, &children);
        assert_eq!(of(&spaces, "!sub:x"), view(110, 2));
        assert_eq!(of(&spaces, "!top:x"), view(111, 2));
        assert_eq!(outside, UnreadCountsView::default());
        assert_eq!(spaces.len(), 2);
    }

    #[test]
    fn a_room_in_two_top_level_spaces_counts_in_both() {
        let children = tree(&[("!s:x", &["!a:x"]), ("!t:x", &["!a:x", "!b:x"])]);
        let rooms = [room("!a:x", 3, 1), room("!b:x", 4, 0)];
        let (spaces, outside) = space_totals(&rooms, &children);
        assert_eq!(of(&spaces, "!s:x"), view(3, 1));
        assert_eq!(of(&spaces, "!t:x"), view(7, 1));
        assert_eq!(outside, UnreadCountsView::default());
    }

    #[test]
    fn rooms_in_no_space_count_outside() {
        let children = tree(&[("!s:x", &["!a:x"])]);
        let rooms = [room("!a:x", 3, 0), room("!dm:x", 5, 2), room("!r:x", 1, 0)];
        let (spaces, outside) = space_totals(&rooms, &children);
        assert_eq!(of(&spaces, "!s:x"), view(3, 0));
        assert_eq!(outside, view(6, 2));
        let (spaces, outside) = space_totals(&rooms, &tree(&[]));
        assert!(spaces.is_empty());
        assert_eq!(outside, view(9, 2));
    }

    #[test]
    fn a_space_id_is_not_a_room_even_when_the_list_is_cut_or_odd() {
        // A space whose own ID is the only entry, and a space nobody lists
        // that is still a space: neither adds to `outside` or to a sum.
        let children = tree(&[("!s:x", &["!s:x"]), ("!lone:x", &[])]);
        let rooms = [room("!s:x", 7, 0), room("!lone:x", 8, 0)];
        let (spaces, outside) = space_totals(&rooms, &children);
        assert_eq!(of(&spaces, "!s:x"), UnreadCountsView::default());
        assert_eq!(of(&spaces, "!lone:x"), UnreadCountsView::default());
        assert_eq!(outside, UnreadCountsView::default());
    }

    #[test]
    fn the_sums_saturate() {
        let children = tree(&[("!s:x", &["!a:x", "!b:x"])]);
        let rooms = [
            room("!a:x", u64::MAX, u64::MAX),
            room("!b:x", 5, 5),
            room("!c:x", u64::MAX, 1),
            room("!d:x", 1, u64::MAX),
        ];
        let (spaces, outside) = space_totals(&rooms, &children);
        assert_eq!(of(&spaces, "!s:x"), view(u64::MAX, u64::MAX));
        assert_eq!(outside, view(u64::MAX, u64::MAX));
    }

    #[test]
    fn unread_is_the_flag_alone() {
        let mut flagged = room("!a:x", 0, 0);
        flagged.unread = true;
        let children = tree(&[("!s:x", &["!a:x", "!b:x"])]);
        let (spaces, outside) = space_totals(&[flagged.clone(), room("!b:x", 0, 0)], &children);
        assert_eq!(
            of(&spaces, "!s:x"),
            UnreadCountsView {
                notifications: 0,
                highlights: 0,
                unread: true
            }
        );
        assert!(!outside.unread);
        let (spaces, _) = space_totals(&[room("!a:x", 0, 0)], &children);
        assert!(!of(&spaces, "!s:x").unread);
        // And outside a space.
        flagged.id = room_id("!free:x");
        assert!(space_totals(&[flagged], &children).1.unread);
    }

    #[test]
    fn spaces_come_in_id_order() {
        let children = tree(&[("!b:x", &[]), ("!a:x", &[]), ("!c:x", &[])]);
        let (spaces, _) = space_totals(&[], &children);
        let ids: Vec<&str> = spaces.iter().map(|s| s.space.as_str()).collect();
        assert_eq!(ids, ["!a:x", "!b:x", "!c:x"]);
    }

    /// A source whose rollup and change signals the test drives.
    struct Fake {
        value: std::sync::Arc<std::sync::Mutex<Rollup>>,
        changes: mpsc::UnboundedReceiver<()>,
        computes: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl TotalsSource for Fake {
        async fn compute(&mut self) -> Rollup {
            self.computes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.value.lock().unwrap().clone()
        }

        async fn changed(&mut self) -> bool {
            self.changes.recv().await.is_some()
        }
    }

    fn roll(n: u64, h: u64) -> Rollup {
        Rollup {
            totals: (n, h),
            spaces: Vec::new(),
            outside: UnreadCountsView::default(),
        }
    }

    /// What was sent: the totals, and how many `SpaceUnread` events.
    fn sent(rx: &mut mpsc::Receiver<CoreEvent>) -> Vec<Totals> {
        sent_both(rx).0
    }

    fn sent_both(rx: &mut mpsc::Receiver<CoreEvent>) -> (Vec<Totals>, Vec<UnreadCountsView>) {
        let (mut totals, mut outside) = (Vec::new(), Vec::new());
        while let Ok(event) = rx.try_recv() {
            match event {
                CoreEvent::UnreadTotals {
                    notifications,
                    highlights,
                } => totals.push((notifications, highlights)),
                CoreEvent::SpaceUnread { outside: o, .. } => outside.push(o),
                other => panic!("{other:?}"),
            }
        }
        (totals, outside)
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn totals_are_sent_at_most_once_a_second_and_only_on_change() {
        let value = std::sync::Arc::new(std::sync::Mutex::new(roll(0, 0)));
        let computes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (change_tx, changes) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::channel(16);
        let source = Fake {
            value: value.clone(),
            changes,
            computes: computes.clone(),
        };
        let task = tokio::spawn(run_totals(source, tx, TOTALS_INTERVAL));
        settle().await;
        // The first value is sent at once.
        assert_eq!(sent(&mut rx), vec![(0, 0)]);

        // A burst of changes inside the interval: nothing yet.
        for n in 1..=5 {
            *value.lock().unwrap() = roll(n, 0);
            change_tx.send(()).unwrap();
        }
        tokio::time::advance(Duration::from_millis(900)).await;
        settle().await;
        assert!(sent(&mut rx).is_empty());

        // When the interval ends, one event carries the latest value.
        tokio::time::advance(Duration::from_millis(101)).await;
        settle().await;
        assert_eq!(sent(&mut rx), vec![(5, 0)]);

        // A signal without a change sends nothing.
        change_tx.send(()).unwrap();
        tokio::time::advance(TOTALS_INTERVAL).await;
        settle().await;
        assert!(sent(&mut rx).is_empty());

        // After a quiet spell a change goes out right away, and is followed
        // by a pause again.
        tokio::time::advance(Duration::from_secs(30)).await;
        *value.lock().unwrap() = roll(0, 0);
        change_tx.send(()).unwrap();
        settle().await;
        assert_eq!(sent(&mut rx), vec![(0, 0)]);
        *value.lock().unwrap() = roll(1, 1);
        change_tx.send(()).unwrap();
        tokio::time::advance(Duration::from_millis(500)).await;
        settle().await;
        assert!(sent(&mut rx).is_empty());
        tokio::time::advance(Duration::from_millis(501)).await;
        settle().await;
        assert_eq!(sent(&mut rx), vec![(1, 1)]);

        // The task ends when no change can come any more.
        drop(change_tx);
        tokio::time::advance(TOTALS_INTERVAL).await;
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn space_counts_are_sent_only_on_a_change_and_at_most_once_a_second() {
        let value = std::sync::Arc::new(std::sync::Mutex::new(roll(0, 0)));
        let (change_tx, changes) = mpsc::unbounded_channel();
        let (tx, mut rx) = mpsc::channel(16);
        let source = Fake {
            value: value.clone(),
            changes,
            computes: std::sync::Arc::default(),
        };
        let task = tokio::spawn(run_totals(source, tx, TOTALS_INTERVAL));
        settle().await;
        // Both events go out at the start.
        let (totals, outside) = sent_both(&mut rx);
        assert_eq!((totals.len(), outside.len()), (1, 1));

        // Only the space counts change, in a burst: nothing inside the
        // interval, then one event, and no `UnreadTotals`.
        for n in 1..=5 {
            value.lock().unwrap().outside = view(n, 0);
            change_tx.send(()).unwrap();
        }
        tokio::time::advance(Duration::from_millis(900)).await;
        settle().await;
        assert_eq!(sent_both(&mut rx), (vec![], vec![]));
        tokio::time::advance(Duration::from_millis(101)).await;
        settle().await;
        assert_eq!(sent_both(&mut rx), (vec![], vec![view(5, 0)]));

        // A signal without a change sends nothing.
        change_tx.send(()).unwrap();
        tokio::time::advance(TOTALS_INTERVAL).await;
        settle().await;
        assert_eq!(sent_both(&mut rx), (vec![], vec![]));

        // A change in one space's counts is a change.
        value.lock().unwrap().spaces = vec![SpaceUnreadView {
            space: RoomId::from("!s:x"),
            counts: view(1, 0),
        }];
        change_tx.send(()).unwrap();
        tokio::time::advance(TOTALS_INTERVAL).await;
        settle().await;
        assert_eq!(sent_both(&mut rx), (vec![], vec![view(5, 0)]));

        drop(change_tx);
        tokio::time::advance(TOTALS_INTERVAL).await;
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn totals_stop_when_nobody_listens() {
        let (change_tx, changes) = mpsc::unbounded_channel();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let source = Fake {
            value: std::sync::Arc::new(std::sync::Mutex::new(roll(1, 0))),
            changes,
            computes: std::sync::Arc::default(),
        };
        run_totals(source, tx, TOTALS_INTERVAL).await;
        drop(change_tx);
    }

    #[test]
    fn modes_map_both_ways() {
        for mode in [
            RoomNotificationModeView::AllMessages,
            RoomNotificationModeView::MentionsAndKeywords,
            RoomNotificationModeView::Mute,
        ] {
            assert_eq!(mode_view(sdk_mode(mode)), mode);
        }
    }

    #[test]
    fn names_are_one_clean_line() {
        assert_eq!(clean_name("  Ro\u{202E}om\n\tname\u{0} "), "Room name");
        assert_eq!(
            clean_name(&"n".repeat(500)).chars().count(),
            PREVIEW_MAX_CHARS
        );
        assert_eq!(clean_name("\u{202E}"), "");
    }

    #[test]
    fn settings_errors_are_fixed_text() {
        let e = settings_error(&NotificationSettingsError::RuleNotFound("secret-x".into()));
        assert!(!format!("{e}").contains("secret-x"));
        assert!(matches!(
            settings_error(&NotificationSettingsError::UnableToSavePushRules),
            CoreError::Network(_)
        ));
    }

    #[test]
    fn keywords_match_whole_words_without_regard_to_case() {
        let kw = |k: &[&str]| k.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
        let table: [(&str, &[&str], bool); 9] = [
            ("hello twicecat-kw-ab", &["twicecat-kw-ab"], true),
            ("Hello TWICECAT-KW-AB!", &["twicecat-kw-ab"], true),
            ("twicecat-kw-ab", &["twicecat-kw-ab"], true),
            ("xtwicecat-kw-ab", &["twicecat-kw-ab"], false),
            ("twicecat-kw-abc", &["twicecat-kw-ab"], false),
            ("a twicecat-kw-ab_ b", &["twicecat-kw-ab"], false),
            ("nothing here", &["twicecat-kw-ab"], false),
            ("anything", &[], false),
            ("blank", &[""], false),
        ];
        for (body, keywords, want) in table {
            assert_eq!(contains_keyword(body, &kw(keywords)), want, "{body}");
        }
    }

    #[test]
    fn a_keyword_in_a_text_message_is_a_highlight() {
        let json = message("see you at Twicecat-KW-1 soon", json!({}));
        let kw = ["twicecat-kw-1".to_owned()];
        assert!(parse_event(&json, false, ME, &kw).unwrap().highlight);
        assert!(!parse_event(&json, false, ME, &[]).unwrap().highlight);
        // The match is on the whole body, also past the preview.
        let long = format!("{} twicecat-kw-1", "x ".repeat(400));
        let json = message(&long, json!({}));
        assert!(parse_event(&json, false, ME, &kw).unwrap().highlight);
    }
}
