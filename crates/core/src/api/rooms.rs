//! Types of the core API for notifications, rooms, members, the user's
//! profile, spaces, threads and local settings.
//!
//! Names, topics, keywords and previews are user content: every `Debug` impl
//! that could print them hides them.

use std::fmt;

use super::{AvatarId, EventId, MessageText, RoomId, UserId};

/// A name or text field printed as its length only.
struct Hidden(usize);

impl fmt::Debug for Hidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted, {} bytes>", self.0)
    }
}

fn hide(s: &str) -> Hidden {
    Hidden(s.len())
}

fn hide_opt(s: &Option<String>) -> Option<Hidden> {
    s.as_deref().map(hide)
}

// ---- Notifications -------------------------------------------------------

/// Why a notification is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    /// A message that the push rules notify about.
    Message,
    /// A message that mentions the user or matches a keyword.
    Mention,
    /// An invitation to a room.
    Invite,
    /// An event that was still encrypted when the rules ran.
    Encrypted,
}

/// A notification to show. `Debug` hides the preview and the names.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationView {
    /// Room.
    pub room_id: RoomId,
    /// The event that caused it.
    pub event_id: EventId,
    /// The room's display name.
    pub room_name: String,
    /// Sender.
    pub sender: UserId,
    /// Sender's display name, when known.
    pub sender_name: Option<String>,
    /// A direct message.
    pub is_dm: bool,
    /// Why it is shown.
    pub kind: NotificationKind,
    /// The start of the message text, without formatting.
    pub preview: Option<MessageText>,
}

impl fmt::Debug for NotificationView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationView")
            .field("room_id", &self.room_id)
            .field("event_id", &self.event_id)
            .field("room_name", &hide(&self.room_name))
            .field("sender", &self.sender)
            .field("sender_name", &hide_opt(&self.sender_name))
            .field("is_dm", &self.is_dm)
            .field("kind", &self.kind)
            .field("preview", &self.preview.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A notification mode of a room or a default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomNotificationModeView {
    /// Every message notifies.
    AllMessages,
    /// Only mentions and keywords notify.
    MentionsAndKeywords,
    /// Nothing notifies.
    Mute,
}

/// The account's notification defaults and keywords. `Debug` hides the
/// keywords.
#[derive(Clone, PartialEq, Eq)]
pub struct NotificationSettingsView {
    /// Default mode of group rooms.
    pub group_default: RoomNotificationModeView,
    /// Default mode of direct messages.
    pub dm_default: RoomNotificationModeView,
    /// Keywords that count as a mention.
    pub keywords: Vec<String>,
}

impl fmt::Debug for NotificationSettingsView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationSettingsView")
            .field("group_default", &self.group_default)
            .field("dm_default", &self.dm_default)
            .field("keywords", &self.keywords.len())
            .finish()
    }
}

// ---- Rooms ---------------------------------------------------------------

/// Who can find and join a new room.
#[derive(Clone, PartialEq, Eq)]
pub enum RoomVisibility {
    /// Invite only.
    Private,
    /// Public, with this alias local part (no `#`, no server).
    Public {
        /// The alias local part.
        alias: String,
    },
}

impl fmt::Debug for RoomVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Private => f.write_str("Private"),
            Self::Public { alias } => f
                .debug_struct("Public")
                .field("alias", &hide(alias))
                .finish(),
        }
    }
}

/// A room to create. `Debug` hides the name, topic and alias.
#[derive(Clone, PartialEq, Eq)]
pub struct CreateRoomRequest {
    /// Name, 1-255 characters.
    pub name: String,
    /// Topic.
    pub topic: Option<String>,
    /// End-to-end encrypted from the start.
    pub encrypted: bool,
    /// Private or public.
    pub visibility: RoomVisibility,
    /// Users to invite.
    pub invitees: Vec<UserId>,
}

impl fmt::Debug for CreateRoomRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateRoomRequest")
            .field("name", &hide(&self.name))
            .field("topic", &hide_opt(&self.topic))
            .field("encrypted", &self.encrypted)
            .field("visibility", &self.visibility)
            .field("invitees", &self.invitees)
            .finish()
    }
}

/// What the user may change in a room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoomPermissions {
    /// Change the name.
    pub can_set_name: bool,
    /// Change the topic.
    pub can_set_topic: bool,
    /// Change the avatar.
    pub can_set_avatar: bool,
    /// Turn encryption on.
    pub can_enable_encryption: bool,
    /// Invite users.
    pub can_invite: bool,
}

/// Settings of one room, for the settings panel. `Debug` hides the name,
/// topic and alias.
#[derive(Clone, PartialEq, Eq)]
pub struct RoomDetailsView {
    /// Room.
    pub room_id: RoomId,
    /// Name.
    pub name: String,
    /// Topic.
    pub topic: Option<String>,
    /// Avatar.
    pub avatar: Option<AvatarId>,
    /// End-to-end encrypted.
    pub is_encrypted: bool,
    /// Canonical alias.
    pub alias: Option<String>,
    /// What the user may change.
    pub permissions: RoomPermissions,
}

impl fmt::Debug for RoomDetailsView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomDetailsView")
            .field("room_id", &self.room_id)
            .field("name", &hide(&self.name))
            .field("topic", &hide_opt(&self.topic))
            .field("avatar", &self.avatar)
            .field("is_encrypted", &self.is_encrypted)
            .field("alias", &hide_opt(&self.alias))
            .field("permissions", &self.permissions)
            .finish()
    }
}

/// A member's power level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerLevelView {
    /// The room's creator, who has unlimited power.
    Creator,
    /// A numeric level.
    Level(i64),
}

/// A member's role, derived from the power level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// The room's creator.
    Creator,
    /// Level 100 or more.
    Admin,
    /// Level 50 or more.
    Moderator,
    /// Anyone else.
    User,
}

/// Whether a member has joined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MembershipView {
    /// In the room.
    Joined,
    /// Invited, not yet joined.
    Invited,
}

/// A member of a room. `Debug` hides the display name.
#[derive(Clone, PartialEq, Eq)]
pub struct RoomMemberView {
    /// User ID.
    pub user_id: UserId,
    /// Display name, when set.
    pub display_name: Option<String>,
    /// Avatar, when set.
    pub avatar: Option<AvatarId>,
    /// Power level.
    pub power: PowerLevelView,
    /// Role.
    pub role: Role,
    /// Joined or invited.
    pub membership: MembershipView,
}

impl fmt::Debug for RoomMemberView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomMemberView")
            .field("user_id", &self.user_id)
            .field("display_name", &hide_opt(&self.display_name))
            .field("avatar", &self.avatar)
            .field("power", &self.power)
            .field("role", &self.role)
            .field("membership", &self.membership)
            .finish()
    }
}

// ---- Own profile ---------------------------------------------------------

/// The user's own profile. `Debug` hides the display name.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnProfileView {
    /// Display name, when set.
    pub display_name: Option<String>,
    /// Avatar, when set.
    pub avatar: Option<AvatarId>,
}

impl fmt::Debug for OwnProfileView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnProfileView")
            .field("display_name", &hide_opt(&self.display_name))
            .field("avatar", &self.avatar)
            .finish()
    }
}

// ---- Spaces and threads --------------------------------------------------

/// A space the user has joined. `Debug` hides the name.
#[derive(Clone, PartialEq, Eq)]
pub struct SpaceView {
    /// Space room ID.
    pub id: RoomId,
    /// Display name.
    pub name: String,
    /// Avatar, when set.
    pub avatar: Option<AvatarId>,
    /// Nesting level; 0 is a top-level space.
    pub level: u32,
    /// Number of child rooms.
    pub children: u64,
}

impl fmt::Debug for SpaceView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpaceView")
            .field("id", &self.id)
            .field("name", &hide(&self.name))
            .field("avatar", &self.avatar)
            .field("level", &self.level)
            .field("children", &self.children)
            .finish()
    }
}

/// A thread's summary, on its root message. `Debug` hides the name and
/// preview.
#[derive(Clone, PartialEq, Eq)]
pub struct ThreadSummaryView {
    /// Number of replies.
    pub replies: u32,
    /// Sender of the latest reply.
    pub latest_sender: Option<UserId>,
    /// Their display name, when known.
    pub latest_sender_name: Option<String>,
    /// The start of the latest reply's text.
    pub latest_preview: Option<String>,
}

impl fmt::Debug for ThreadSummaryView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadSummaryView")
            .field("replies", &self.replies)
            .field("latest_sender", &self.latest_sender)
            .field("latest_sender_name", &hide_opt(&self.latest_sender_name))
            .field("latest_preview", &hide_opt(&self.latest_preview))
            .finish()
    }
}

// ---- Local settings ------------------------------------------------------

/// The theme the user picked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub enum ThemeChoice {
    /// Follow the system.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// Settings kept on this device only. No secrets, no content.
///
/// ```
/// use tessera_core::api::{ThemeChoice, UiSettings};
/// let s = UiSettings::default();
/// assert_eq!(s.theme, ThemeChoice::System);
/// assert!(s.desktop_notifications && s.show_message_content && s.tray_badge);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Theme override.
    pub theme: ThemeChoice,
    /// Show desktop notifications.
    pub desktop_notifications: bool,
    /// Show message text in notifications.
    pub show_message_content: bool,
    /// Show the unread badge on the tray icon.
    pub tray_badge: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            desktop_notifications: true,
            show_message_content: true,
            tray_badge: true,
        }
    }
}
