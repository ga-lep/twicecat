//! Plain data types of the core API. None of them names an SDK type.

use std::fmt;

use thiserror::Error;
use zeroize::Zeroizing;

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Borrows the inner string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_newtype!(
    /// A Matrix room ID such as `!abc:example.org`.
    RoomId
);
string_newtype!(
    /// A Matrix user ID such as `@alice:example.org`.
    UserId
);
string_newtype!(
    /// A Matrix device ID.
    DeviceId
);
string_newtype!(
    /// A Matrix event ID such as `$abc`.
    EventId
);
string_newtype!(
    /// Identifies an avatar image; holds its `mxc://` URI. The core checks the
    /// URI when the avatar is fetched, not when the ID is built.
    AvatarId
);

/// The bytes of an encoded avatar image. `Debug` shows only the length.
///
/// ```
/// use tessera_core::api::AvatarImage;
/// let img = AvatarImage::from(vec![1, 2, 3]);
/// assert_eq!(format!("{img:?}"), "AvatarImage(<3 bytes>)");
/// assert_eq!(img.as_bytes(), &[1, 2, 3]);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct AvatarImage(Vec<u8>);

impl AvatarImage {
    /// Borrows the encoded image bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Takes the encoded image bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
}

impl From<Vec<u8>> for AvatarImage {
    fn from(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl fmt::Debug for AvatarImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AvatarImage(<{} bytes>)", self.0.len())
    }
}

/// The last message of a room, for the room list. The body is untrusted user
/// content: `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct MessagePreview {
    /// Sender user ID.
    pub sender: UserId,
    /// Sender's display name, when known.
    pub sender_name: Option<String>,
    /// First line of the message text, shortened.
    pub body: String,
    /// Origin timestamp, milliseconds since the Unix epoch.
    pub timestamp_ms: u64,
    /// Sent by the logged-in user.
    pub is_own: bool,
}

// Manual impl: the body is user content and must not reach logs.
impl fmt::Debug for MessagePreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MessagePreview")
            .field("sender", &self.sender)
            .field("sender_name", &self.sender_name)
            .field(
                "body",
                &format_args!("<redacted, {} bytes>", self.body.len()),
            )
            .field("timestamp_ms", &self.timestamp_ms)
            .field("is_own", &self.is_own)
            .finish()
    }
}

/// Which rooms the room list shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RoomListTab {
    /// Every room.
    #[default]
    All,
    /// Direct messages.
    People,
    /// Group rooms that are not spaces.
    Rooms,
    /// Spaces.
    Spaces,
}

/// A room list query: a tab plus a name search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoomListQuery {
    /// Which rooms to show.
    pub tab: RoomListTab,
    /// Case- and diacritic-insensitive name filter; blank means no filter.
    pub search: String,
}

/// A password. `Debug` never shows the value, and the memory is zeroized on
/// drop (each clone separately).
///
/// ```
/// use tessera_core::api::Password;
/// let p = Password::from("hunter2");
/// assert!(!format!("{p:?}").contains("hunter2"));
/// assert_eq!(p.expose(), "hunter2");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct Password(Zeroizing<String>);

impl Password {
    /// Returns the secret. Call only where the password is actually needed.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Password {
    fn from(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
}

impl From<&str> for Password {
    fn from(value: &str) -> Self {
        Self(Zeroizing::new(value.to_owned()))
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

/// Text of an outgoing message. `Debug` shows only its byte length, and the
/// memory is zeroized on drop (each clone separately).
///
/// ```
/// use tessera_core::api::MessageText;
/// let t = MessageText::from("hello");
/// assert!(!format!("{t:?}").contains("hello"));
/// assert_eq!(t.expose(), "hello");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct MessageText(Zeroizing<String>);

impl MessageText {
    /// Returns the text. Call only where the message is actually sent.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for MessageText {
    fn from(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
}

impl From<&str> for MessageText {
    fn from(value: &str) -> Self {
        Self(Zeroizing::new(value.to_owned()))
    }
}

impl fmt::Debug for MessageText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MessageText(<{} bytes>)", self.0.len())
    }
}

/// An OAuth authorization URL. It carries `state` and the PKCE challenge in
/// its query, so `Debug` shows only scheme and host.
///
/// ```
/// use tessera_core::api::AuthUrl;
/// let u = AuthUrl::from("https://auth.example.org/a?state=s");
/// assert!(!format!("{u:?}").contains("state"));
/// assert_eq!(u.expose(), "https://auth.example.org/a?state=s");
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct AuthUrl(String);

impl AuthUrl {
    /// Returns the full URL, to be opened in a browser.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for AuthUrl {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for AuthUrl {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Debug for AuthUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Keep "scheme://host" only; drop userinfo, path, query, fragment.
        let (scheme, rest) = self.0.split_once("://").unwrap_or(("?", ""));
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let host = authority.rsplit('@').next().unwrap_or("");
        write!(f, "AuthUrl({scheme}://{host}/<redacted>)")
    }
}

/// Connection state of the sync loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncState {
    /// Not started yet.
    Idle,
    /// Syncing normally.
    Running,
    /// The server is unreachable; the SDK polls until it is back.
    Offline,
    /// A restart after a failure is pending or in progress.
    Reconnecting {
        /// Consecutive failed attempts so far.
        attempt: u32,
    },
    /// The sync loop failed with this (non-secret) description.
    Error(String),
    /// Stopped.
    Terminated,
}

/// One row of the room list.
///
/// `Debug` hides the preview body (see [`MessagePreview`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomSummary {
    /// Room ID.
    pub id: RoomId,
    /// Display name, or the room ID if the room has none.
    pub name: String,
    /// Unread messages.
    pub unread_messages: u64,
    /// Unread mentions.
    pub unread_mentions: u64,
    /// Unread notifications.
    pub unread_notifications: u64,
    /// Avatar to show, if the room (or its DM partner) has one.
    pub avatar: Option<AvatarId>,
    /// A direct message.
    pub is_dm: bool,
    /// A space.
    pub is_space: bool,
    /// The user is invited and has not joined.
    pub is_invite: bool,
    /// Unread highlights (mentions and keyword matches).
    pub highlights: u64,
    /// Manually marked as unread.
    pub marked_unread: bool,
    /// The latest message, if any.
    pub last_message: Option<MessagePreview>,
}

impl RoomSummary {
    /// A summary with zero counts, `false` flags and no avatar or preview.
    ///
    /// ```
    /// use tessera_core::api::{RoomId, RoomSummary};
    /// let r = RoomSummary::new(RoomId::from("!r:x"), "Lounge".into());
    /// assert_eq!(r.unread_messages, 0);
    /// assert!(r.last_message.is_none());
    /// ```
    pub fn new(id: RoomId, name: String) -> Self {
        Self {
            id,
            name,
            unread_messages: 0,
            unread_mentions: 0,
            unread_notifications: 0,
            avatar: None,
            is_dm: false,
            is_space: false,
            is_invite: false,
            highlights: 0,
            marked_unread: false,
            last_message: None,
        }
    }
}

/// Delivery state of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendStatus {
    /// Queued or in flight.
    Sending,
    /// Accepted by the server (or received from it).
    Sent,
    /// Sending failed.
    Failed,
}

/// Character styling of a run of text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStyle {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
    /// Underlined.
    pub underline: bool,
    /// Inline code.
    pub code: bool,
}

/// What a mention points at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionTarget {
    /// A user.
    User(UserId),
    /// A room, by alias or ID.
    Room(String),
}

/// An inline element of formatted text. `Debug` hides all text.
#[derive(Clone, PartialEq, Eq)]
pub enum Inline {
    /// Styled text.
    Text {
        /// The text.
        text: String,
        /// Its style.
        style: TextStyle,
    },
    /// A link.
    Link {
        /// Link text.
        text: String,
        /// Target URL: `http`, `https` or `mailto`, normalized and checked by the core.
        url: String,
        /// Its style.
        style: TextStyle,
    },
    /// A user or room mention, shown as a pill.
    Mention {
        /// What it points at.
        target: MentionTarget,
        /// Text to show.
        label: String,
    },
    /// A forced line break.
    LineBreak,
    /// Hidden text, revealed on demand.
    Spoiler(Vec<Inline>),
}

impl Inline {
    fn count(&self) -> usize {
        match self {
            Self::Spoiler(inner) => 1 + inner.iter().map(Self::count).sum::<usize>(),
            _ => 1,
        }
    }
}

// Manual impl: the text is user content and must not reach logs.
impl fmt::Debug for Inline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text { text, .. } => write!(f, "Text(<redacted, {} bytes>)", text.len()),
            Self::Link { text, .. } => write!(f, "Link(<redacted, {} bytes>)", text.len()),
            Self::Mention { label, .. } => {
                write!(f, "Mention(<redacted, {} bytes>)", label.len())
            }
            Self::LineBreak => f.write_str("LineBreak"),
            Self::Spoiler(inner) => write!(f, "Spoiler(<{} inlines>)", inner.len()),
        }
    }
}

/// A block of formatted text. `Debug` hides all text.
#[derive(Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph.
    Paragraph(Vec<Inline>),
    /// A heading.
    Heading {
        /// Level, 1 to 6.
        level: u8,
        /// Content.
        inlines: Vec<Inline>,
    },
    /// A quotation.
    Quote(Vec<Block>),
    /// A list.
    List {
        /// Numbered rather than bulleted.
        ordered: bool,
        /// First number of an ordered list, when not 1.
        start: Option<u64>,
        /// One block sequence per item.
        items: Vec<Vec<Block>>,
    },
    /// A code block; whitespace is kept.
    Code {
        /// The code.
        text: String,
    },
    /// A horizontal rule.
    Rule,
}

impl Block {
    /// Counts of blocks and inlines, this block included.
    fn count(&self) -> (usize, usize) {
        match self {
            Self::Paragraph(i) | Self::Heading { inlines: i, .. } => {
                (1, i.iter().map(Inline::count).sum())
            }
            Self::Quote(b) => Self::sum(1, b.iter()),
            Self::List { items, .. } => Self::sum(1, items.iter().flatten()),
            Self::Code { .. } | Self::Rule => (1, 0),
        }
    }

    fn sum<'a>(own: usize, children: impl Iterator<Item = &'a Block>) -> (usize, usize) {
        children
            .map(Self::count)
            .fold((own, 0), |a, b| (a.0 + b.0, a.1 + b.1))
    }
}

// Manual impl: the text is user content and must not reach logs.
impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (blocks, inlines) = self.count();
        let name = match self {
            Self::Paragraph(_) => "Paragraph",
            Self::Heading { .. } => "Heading",
            Self::Quote(_) => "Quote",
            Self::List { .. } => "List",
            Self::Code { .. } => "Code",
            Self::Rule => "Rule",
        };
        write!(f, "{name}(<{blocks} blocks, {inlines} inlines>)")
    }
}

/// Formatted message text, free of HTML. `Debug` shows only counts.
///
/// ```
/// use tessera_core::api::RichText;
/// assert!(RichText::default().blocks.is_empty());
/// assert_eq!(format!("{:?}", RichText::default()), "RichText(<0 blocks, 0 inlines>)");
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RichText {
    /// The blocks, in order.
    pub blocks: Vec<Block>,
}

impl fmt::Debug for RichText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (blocks, inlines) = self
            .blocks
            .iter()
            .map(Block::count)
            .fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        write!(f, "RichText(<{blocks} blocks, {inlines} inlines>)")
    }
}

/// The kind of a text message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageKind {
    /// A normal message.
    #[default]
    Text,
    /// A notice, such as a bot answer.
    Notice,
    /// An emote (`/me`).
    Emote,
}

/// The unsent text of a room's composer, kept in the session store between
/// visits and restarts. `Debug` shows sizes only.
#[derive(Clone, PartialEq, Eq)]
pub struct Draft {
    /// The text as typed (markdown source).
    pub text: MessageText,
    /// Mentions picked from the suggestions whose labels are in `text`.
    pub mentions: Vec<MentionRef>,
    /// The message being replied to.
    pub reply_to: Option<EventId>,
}

// Manual impl: the text and labels are user content.
impl fmt::Debug for Draft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Draft")
            .field("text", &self.text)
            .field("mentions", &self.mentions.len())
            .field("reply_to", &self.reply_to.is_some())
            .finish()
    }
}

/// How the text of an [`OutgoingMessage`] is turned into an event (the
/// composer's `/plain` and `/html` commands, as in Element).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextFormat {
    /// Markdown, with mention pills.
    #[default]
    Markdown,
    /// The text as is, with no formatted body.
    Plain,
    /// The text is HTML: it is sent as the body and, sanitised, as the
    /// formatted body.
    Html,
    /// The text is hidden behind a spoiler: sent as the body, and inside a
    /// `data-mx-spoiler` span (sanitised) as the formatted body.
    Spoiler,
    /// Element's `/rainbow`: sent as the body, and with each character in a
    /// `data-mx-color` span of a colour sweep as the formatted body.
    Rainbow,
}

/// Whether the quoted event of a reply could be loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyState {
    /// The quote is available.
    Ready,
    /// The quote is being fetched.
    Loading,
    /// The quoted event is missing or unreadable.
    Unavailable,
}

/// The message a reply answers. `Debug` hides the body.
#[derive(Clone, PartialEq, Eq)]
pub struct ReplyPreview {
    /// The quoted message's event.
    pub event_id: EventId,
    /// Sender of the quoted message.
    pub sender: UserId,
    /// Their display name, when known.
    pub sender_name: Option<String>,
    /// One line of the quoted text, at most 200 characters.
    pub body: String,
    /// Whether the quote could be loaded.
    pub state: ReplyState,
}

// Manual impl: the body is user content and must not reach logs.
impl fmt::Debug for ReplyPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReplyPreview")
            .field("event_id", &self.event_id)
            .field("sender", &self.sender)
            .field("sender_name", &self.sender_name)
            .field(
                "body",
                &format_args!("<redacted, {} bytes>", self.body.len()),
            )
            .field("state", &self.state)
            .finish()
    }
}

/// The reactions with one key on a message. `Debug` hides the key.
#[derive(Clone, PartialEq, Eq)]
pub struct ReactionGroup {
    /// The reaction, usually an emoji.
    pub key: String,
    /// How many users reacted with it.
    pub count: u32,
    /// The logged-in user is one of them.
    pub own: bool,
    /// Some of the reacting users (at most 5).
    pub senders: Vec<UserId>,
}

// Manual impl: the key is user content and must not reach logs.
impl fmt::Debug for ReactionGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReactionGroup")
            .field("key", &format_args!("<redacted, {} bytes>", self.key.len()))
            .field("count", &self.count)
            .field("own", &self.own)
            .field("senders", &self.senders)
            .finish()
    }
}

/// What the user may do with a message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemActions {
    /// Reply to it.
    pub can_reply: bool,
    /// Edit it.
    pub can_edit: bool,
    /// Redact it.
    pub can_redact: bool,
    /// React to it.
    pub can_react: bool,
    /// Copy its text.
    pub can_copy: bool,
}

/// Why a local echo failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendFailure {
    /// Retrying may help (for example after a network failure).
    pub recoverable: bool,
}

/// Content and state of a message beyond its plain text. `Debug` hides the
/// content, the quoted body and the reaction keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageDetails {
    /// Sender's display name, when known.
    pub sender_name: Option<String>,
    /// Another room member has the same display name.
    pub sender_name_ambiguous: bool,
    /// Text, notice or emote.
    pub kind: MessageKind,
    /// The formatted content; always filled (the plain body when there is no
    /// usable formatted body).
    pub content: RichText,
    /// The message was edited.
    pub edited: bool,
    /// The message mentions the logged-in user.
    pub highlighted: bool,
    /// The message this one replies to.
    pub reply_to: Option<ReplyPreview>,
    /// Reactions, grouped by key.
    pub reactions: Vec<ReactionGroup>,
    /// Allowed actions.
    pub actions: ItemActions,
    /// Set when sending failed.
    pub send_failure: Option<SendFailure>,
}

/// A state change shown as a muted line. Targets carry an optional display
/// name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateChange {
    /// The sender joined.
    Joined,
    /// The sender left.
    Left,
    /// The sender invited `target`.
    Invited {
        /// Invited user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender removed `target`.
    Kicked {
        /// Removed user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender banned `target`.
    Banned {
        /// Banned user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender lifted the ban of `target`.
    Unbanned {
        /// Unbanned user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender accepted an invite.
    InviteAccepted,
    /// The sender rejected an invite.
    InviteRejected,
    /// The sender withdrew the invite of `target`.
    InviteRevoked {
        /// Uninvited user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender asked to join.
    Knocked,
    /// The sender accepted the request of `target`.
    KnockAccepted {
        /// Accepted user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender withdrew the request to join.
    KnockRetracted,
    /// The sender denied the request of `target`.
    KnockDenied {
        /// Denied user.
        target: UserId,
        /// Their display name, when known.
        target_name: Option<String>,
    },
    /// The sender changed their display name.
    DisplayName {
        /// The old name.
        old: Option<String>,
        /// The new name.
        new: Option<String>,
    },
    /// The sender changed their avatar.
    Avatar,
    /// The room was created.
    RoomCreated,
    /// The room name changed.
    RoomName {
        /// The new name, `None` when removed.
        name: Option<String>,
    },
    /// The topic changed.
    Topic,
    /// The room avatar changed.
    RoomAvatar,
    /// Encryption was turned on.
    Encryption,
    /// Any other state event.
    Other {
        /// Its event type.
        event_type: String,
    },
}

/// Why an event shows as a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderKind {
    /// The message was deleted.
    Redacted,
    /// The message could not be decrypted.
    UnableToDecrypt,
    /// An image.
    Image,
    /// A video.
    Video,
    /// An audio clip.
    Audio,
    /// A file.
    File,
    /// A sticker.
    Sticker,
    /// A poll.
    Poll,
    /// A location.
    Location,
    /// Anything else the core cannot show.
    Unsupported,
}

/// Progress of back-pagination in a room.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaginationState {
    /// Nothing is loading.
    Idle,
    /// Older events are being loaded.
    Loading,
    /// The start of the room has been reached.
    ReachedStart,
}

/// A user who is typing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypingUser {
    /// User ID.
    pub user_id: UserId,
    /// Display name, when known.
    pub name: Option<String>,
}

/// A room member, for mention autocomplete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// User ID.
    pub user_id: UserId,
    /// Display name, when set.
    pub display_name: Option<String>,
}

/// A mention in an outgoing message: the label as typed and its user. `Debug`
/// hides the label.
#[derive(Clone, PartialEq, Eq)]
pub struct MentionRef {
    /// The text that stands for the user in the message source.
    pub label: String,
    /// The mentioned user.
    pub user_id: UserId,
}

// Manual impl: the label is user content and must not reach logs.
impl fmt::Debug for MentionRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MentionRef")
            .field(
                "label",
                &format_args!("<redacted, {} bytes>", self.label.len()),
            )
            .field("user_id", &self.user_id)
            .finish()
    }
}

/// A message to send: markdown source plus structured mentions and reply.
/// `Debug` shows lengths only.
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingMessage {
    /// Target room.
    pub room_id: RoomId,
    /// Markdown source.
    pub text: MessageText,
    /// Mentions whose labels appear in `text`.
    pub mentions: Vec<MentionRef>,
    /// `TimelineItem::id` of the message to reply to.
    pub reply_to: Option<String>,
    /// Text, notice or emote.
    pub kind: MessageKind,
    /// How `text` becomes the event's body.
    pub format: TextFormat,
}

// Manual impl: the text and labels are user content.
impl fmt::Debug for OutgoingMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingMessage")
            .field("room_id", &self.room_id)
            .field("text", &self.text)
            .field("mentions", &self.mentions.len())
            .field("reply_to", &self.reply_to.is_some())
            .field("kind", &self.kind)
            .field("format", &self.format)
            .finish()
    }
}

/// Which action an [`ActionFailed`](super::CoreEvent::ActionFailed) is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    /// Send a message.
    Send,
    /// Edit a message.
    Edit,
    /// Redact a message.
    Redact,
    /// Toggle a reaction.
    React,
    /// Retry a failed send.
    Retry,
    /// Discard a failed send.
    Discard,
    /// Load older events.
    Paginate,
    /// Send a read receipt.
    Receipt,
    /// Send a typing notice.
    Typing,
    /// Look up room members.
    Members,
}

/// What a timeline item is.
#[derive(Clone, PartialEq, Eq)]
pub enum TimelineItemKind {
    /// A text message.
    Message {
        /// Sender user ID.
        sender: UserId,
        /// Message text.
        body: String,
        /// Sent by the logged-in user.
        is_own: bool,
        /// Origin timestamp, milliseconds since the Unix epoch.
        timestamp_ms: u64,
        /// Delivery state.
        sent: SendStatus,
        /// Everything beyond the plain text: rich content, replies,
        /// reactions and allowed actions.
        details: Box<MessageDetails>,
    },
    /// A day separator.
    DateDivider {
        /// Start of the day (or any instant of it), milliseconds since the
        /// Unix epoch.
        timestamp_ms: u64,
    },
    /// The point up to which the user has read.
    ReadMarker,
    /// The start of the room's history.
    TimelineStart,
    /// A state event such as a join or a name change.
    State {
        /// Who caused the change.
        sender: UserId,
        /// Their display name, when known.
        sender_name: Option<String>,
        /// Origin timestamp, milliseconds since the Unix epoch.
        timestamp_ms: u64,
        /// What changed.
        change: StateChange,
    },
    /// An event that has no content to show: redacted, undecryptable, media
    /// or otherwise unsupported.
    Placeholder {
        /// Sender user ID.
        sender: UserId,
        /// Their display name, when known.
        sender_name: Option<String>,
        /// Sent by the logged-in user.
        is_own: bool,
        /// Origin timestamp, milliseconds since the Unix epoch.
        timestamp_ms: u64,
        /// Why there is no content.
        what: PlaceholderKind,
    },
    /// Anything not covered by the other variants.
    Other,
}

// Manual impl: the body is user content and must not reach logs.
impl fmt::Debug for TimelineItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message {
                sender,
                is_own,
                timestamp_ms,
                sent,
                body,
                details,
            } => f
                .debug_struct("Message")
                .field("sender", sender)
                .field("body", &format_args!("<redacted, {} bytes>", body.len()))
                .field("is_own", is_own)
                .field("timestamp_ms", timestamp_ms)
                .field("sent", sent)
                .field("details", details)
                .finish(),
            Self::DateDivider { timestamp_ms } => f
                .debug_struct("DateDivider")
                .field("timestamp_ms", timestamp_ms)
                .finish(),
            Self::ReadMarker => f.write_str("ReadMarker"),
            Self::TimelineStart => f.write_str("TimelineStart"),
            Self::State {
                sender,
                sender_name,
                timestamp_ms,
                change,
            } => f
                .debug_struct("State")
                .field("sender", sender)
                .field("sender_name", sender_name)
                .field("timestamp_ms", timestamp_ms)
                .field("change", change)
                .finish(),
            Self::Placeholder {
                sender,
                sender_name,
                is_own,
                timestamp_ms,
                what,
            } => f
                .debug_struct("Placeholder")
                .field("sender", sender)
                .field("sender_name", sender_name)
                .field("is_own", is_own)
                .field("timestamp_ms", timestamp_ms)
                .field("what", what)
                .finish(),
            Self::Other => f.write_str("Other"),
        }
    }
}

/// One timeline entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineItem {
    /// Stable identifier of the item within its timeline.
    pub id: String,
    /// The Matrix event, once the server has one (not for dividers, markers
    /// or local echoes not sent yet).
    pub event_id: Option<EventId>,
    /// Content.
    pub kind: TimelineItemKind,
    /// Other users whose read receipt points at this item (at most 20).
    pub read_by: Vec<UserId>,
}

impl TimelineItem {
    /// An item that no one has read yet.
    ///
    /// ```
    /// use tessera_core::api::{TimelineItem, TimelineItemKind};
    /// let item = TimelineItem::new("i".into(), TimelineItemKind::ReadMarker);
    /// assert!(item.read_by.is_empty());
    /// ```
    pub fn new(id: String, kind: TimelineItemKind) -> Self {
        Self {
            id,
            event_id: None,
            kind,
            read_by: Vec::new(),
        }
    }
}

/// Errors reported through the API. Messages never contain secrets.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CoreError {
    /// Login was rejected or could not complete.
    #[error("login failed: {0}")]
    Login(String),
    /// The homeserver could not be reached or discovered.
    #[error("network error: {0}")]
    Network(String),
    /// The local store or secret store failed.
    #[error("storage error: {0}")]
    Storage(String),
    /// The server does not support OAuth 2.0 login.
    #[error("the server does not support OAuth login")]
    OAuthNotSupported,
    /// The operation was cancelled by the user.
    #[error("cancelled")]
    Cancelled,
    /// The command needs a logged-in session.
    #[error("not logged in")]
    NotLoggedIn,
    /// The room is not known to this account.
    #[error("unknown room {0}")]
    UnknownRoom(RoomId),
    /// `SendMessage` was issued before `SelectRoom`.
    #[error("no room selected")]
    NoRoomSelected,
    /// The item named by an action is not in the timeline (any more).
    #[error("timeline item not found")]
    ItemNotFound,
    /// The action is not allowed on that item.
    #[error("action not allowed")]
    NotAllowed,
    /// Unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}
