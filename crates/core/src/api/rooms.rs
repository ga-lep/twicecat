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
    /// The rooms this space holds, from the SDK space filter's descendants,
    /// unchanged and at most [`MAX_SPACE_ROOMS`]. For a level 0 space these
    /// are its direct children, rooms and sub-spaces alike; for a level 1
    /// space they are all its descendants, recursively, and the SDK also
    /// lists the space itself, last. A consumer that wants rooms only leaves
    /// out `id`.
    pub rooms: Vec<RoomId>,
}

/// Unread counts of a group of rooms. Muted rooms are left out of the sums,
/// and the sums saturate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnreadCountsView {
    /// Unread notifications.
    pub notifications: u64,
    /// Unread highlights (mentions).
    pub highlights: u64,
    /// Some room is marked unread or has unread messages.
    pub unread: bool,
}

/// The unread counts of one space, over every room it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceUnreadView {
    /// The space's room ID.
    pub space: RoomId,
    /// Counts over the rooms of the space. For a top-level space these are
    /// its direct rooms and the descendants of its sub-spaces, each room
    /// once; a space ID never counts as a room.
    pub counts: UnreadCountsView,
}

/// The most room IDs a [`SpaceView`] carries in `rooms`; the rest are left out.
pub const MAX_SPACE_ROOMS: usize = 2000;

impl fmt::Debug for SpaceView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpaceView")
            .field("id", &self.id)
            .field("name", &hide(&self.name))
            .field("avatar", &self.avatar)
            .field("level", &self.level)
            .field("children", &self.children)
            .field("rooms", &self.rooms.len())
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

/// The name of a custom colour theme, as kept in the local settings.
///
/// Valid names are 1 to 32 bytes of `[A-Za-z0-9 _-]` with no leading or
/// trailing space, and not "twicecat" or "default" in any case (those name the
/// built-in theme), nor a Windows device name in any case (`CON`, `PRN`, `AUX`,
/// `NUL`, `COM1` to `COM9`, `LPT1` to `LPT9`), which Windows will not take as a
/// file name. Such a name is safe to show and to use in a file name.
/// `None` in [`UiSettings::theme_name`] means the built-in theme.
///
/// ```
/// use twicecat_core::api::ThemeName;
/// let name = ThemeName::try_from("Nord Dark").unwrap();
/// assert_eq!(name.as_str(), "Nord Dark");
/// assert!(ThemeName::try_from("a_b-c 1").is_ok());
/// assert!(ThemeName::try_from("../x").is_err());
/// assert!(ThemeName::try_from("").is_err());
/// assert!(ThemeName::try_from(" lead").is_err());
/// assert!(ThemeName::try_from("TwiceCat").is_err());
/// assert!(ThemeName::try_from("con").is_err());
/// assert!(ThemeName::try_from("COM1").is_err());
/// assert!(ThemeName::try_from("COM10").is_ok());
/// assert!(ThemeName::try_from("a".repeat(33).as_str()).is_err());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThemeName {
    bytes: [u8; Self::MAX_LEN],
    len: u8,
}

/// A theme name that fails the rules of [`ThemeName`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a valid theme name")]
pub struct ThemeNameError;

impl ThemeName {
    /// The longest name, in bytes.
    pub const MAX_LEN: usize = 32;

    /// The name as text; always valid ASCII.
    pub fn as_str(&self) -> &str {
        // The bytes were checked to be ASCII when the name was made.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

impl TryFrom<&str> for ThemeName {
    type Error = ThemeNameError;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'-');
        let ok = (1..=Self::MAX_LEN).contains(&name.len())
            && name.bytes().all(plain)
            && !name.starts_with(' ')
            && !name.ends_with(' ')
            && !["twicecat", "default"]
                .iter()
                .any(|reserved| name.eq_ignore_ascii_case(reserved))
            && !is_windows_device_name(name);
        if !ok {
            return Err(ThemeNameError);
        }
        let mut bytes = [0; Self::MAX_LEN];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        let len = u8::try_from(name.len()).map_err(|_| ThemeNameError)?;
        Ok(Self { bytes, len })
    }
}

/// Whether Windows reserves the name for a device (`CON`, `COM1`, ...).
fn is_windows_device_name(name: &str) -> bool {
    const PLAIN: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    if PLAIN.iter().any(|d| name.eq_ignore_ascii_case(d)) {
        return true;
    }
    let bytes = name.as_bytes();
    bytes.len() == 4
        && (bytes[..3].eq_ignore_ascii_case(b"COM") || bytes[..3].eq_ignore_ascii_case(b"LPT"))
        && (b'1'..=b'9').contains(&bytes[3])
}

impl std::fmt::Debug for ThemeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ThemeName({:?})", self.as_str())
    }
}

/// How much room the lists and messages take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DensityChoice {
    /// Roomy spacing.
    #[default]
    Comfortable,
    /// Tighter spacing.
    Compact,
}

/// How the room list is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoomListChoice {
    /// Names and details.
    #[default]
    Expanded,
    /// Avatars only.
    Rail,
}

/// Whether Twicecat may check GitHub for a newer version once a day (Phase 7
/// Task 9). Asked once, at the first start where the check applies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpdateCheckChoice {
    /// Not asked yet: the app asks.
    #[default]
    Unasked,
    /// No checks.
    Off,
    /// A check at most once a day.
    On,
}

/// Settings kept on this device only. No secrets, no content.
///
/// The appearance fields default to "no visible change": no theme name (the
/// built-in theme), comfortable density, an expanded room list and 100 % text.
///
/// ```
/// use twicecat_core::api::{DensityChoice, RoomListChoice, ThemeChoice, UiSettings};
/// let s = UiSettings::default();
/// assert_eq!(s.theme, ThemeChoice::System);
/// assert!(s.desktop_notifications && s.show_message_content && s.tray_badge);
/// assert_eq!(s.theme_name, None);
/// assert_eq!(s.density, DensityChoice::Comfortable);
/// assert_eq!(s.room_list, RoomListChoice::Expanded);
/// assert_eq!(s.text_scale_percent, 100);
/// assert_eq!(s.update_check, twicecat_core::api::UpdateCheckChoice::Unasked);
/// assert_eq!(s.update_checked_at, 0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UiSettings {
    /// Theme override.
    pub theme: ThemeChoice,
    /// Show desktop notifications.
    pub desktop_notifications: bool,
    /// Show message text in notifications.
    pub show_message_content: bool,
    /// Show the unread badge on the tray icon.
    pub tray_badge: bool,
    /// The custom theme chosen by name; `None` is the built-in theme. An older
    /// `palette` key in the file is ignored.
    pub theme_name: Option<ThemeName>,
    /// Spacing density.
    pub density: DensityChoice,
    /// Expanded or collapsed room list.
    pub room_list: RoomListChoice,
    /// Text scale in percent (80 to 200, in steps of 10).
    pub text_scale_percent: u16,
    /// Show every setting, audio and video in depth, instead of the few a
    /// person with little technical knowledge needs. Off by default.
    pub advanced_settings: bool,
    /// The daily update check, where this copy checks itself.
    pub update_check: UpdateCheckChoice,
    /// When the last update check was sent, in seconds since the Unix epoch
    /// (0: never).
    pub update_checked_at: u64,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            desktop_notifications: true,
            show_message_content: true,
            tray_badge: true,
            theme_name: None,
            density: DensityChoice::Comfortable,
            room_list: RoomListChoice::Expanded,
            text_scale_percent: 100,
            advanced_settings: false,
            update_check: UpdateCheckChoice::Unasked,
            update_checked_at: 0,
        }
    }
}
