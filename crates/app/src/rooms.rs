//! Rooms in the app (plan Q13, Q14, Task 12): the create, direct message and
//! join dialogs, the room settings and member panels, the user card, the
//! join bar and the labels of pills.
//!
//! The first half is pure and table-tested: form validation, the leave
//! warning, the pill label rule and the avatar downscale. The second half is
//! the state and the `App` handlers; they send commands and return tasks,
//! never block. Names, topics, aliases, member names and the IDs a user types
//! belong to people: nothing here logs them and every `Debug` shows counts.

use std::collections::HashSet;
use std::fmt;

use iced::Task;
use iced::widget::operation;
use twicecat_core::api::{
    Block, Command, CoreError, CreateRoomRequest, Inline, MediaBytes, MembershipView,
    MentionTarget, MessageText, PowerLevelView, Role, RoomDetailsView, RoomId, RoomMemberView,
    RoomSummary, RoomVisibility, TimelineItemKind, UserId,
};

use crate::blocking;
use crate::links::{parse_room_target, parse_user_id};
use crate::media::SavePath;
use crate::screens::room_dialogs::{CREATE_NAME_ID, DM_INPUT_ID, JOIN_INPUT_ID};
use crate::state::{App, Message};
use crate::text_safety::{name_or_placeholder, one_line_capped};
use crate::verification::elide_middle;

/// Characters of an ID kept for a tooltip or the card.
pub const MAX_ID_CHARS: usize = 256;

/// Longest room name and display name, in characters (the core's limit).
pub const MAX_NAME_CHARS: usize = 255;
/// Longest topic, in characters (the core's limit).
pub const MAX_TOPIC_CHARS: usize = 1000;
/// Longest alias local part, in characters (the core's limit).
pub const MAX_ALIAS_CHARS: usize = 64;
/// Most users a new room invites (the core's limit).
pub const MAX_INVITEES: usize = 100;
/// Characters of a pill label before it is cut.
pub const MAX_PILL_CHARS: usize = 48;

/// Why a form is not sent. The words are fixed; they never repeat the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormError {
    /// The name is empty.
    NameEmpty,
    /// The name is over 255 characters.
    NameTooLong,
    /// The name has control or direction characters.
    NameUnsafe,
    /// The topic is over 1,000 characters.
    TopicTooLong,
    /// The topic has control or direction characters.
    TopicUnsafe,
    /// A public room needs an address.
    AliasEmpty,
    /// The address has characters other than a-z, 0-9 and `._=-`, or is too
    /// long.
    AliasInvalid,
    /// More than 100 people to invite.
    TooManyInvitees,
    /// One of the people to invite is not a user ID.
    BadInvitee,
}

impl FormError {
    /// The line under the form.
    pub fn text(self) -> &'static str {
        match self {
            Self::NameEmpty => "Give the room a name.",
            Self::NameTooLong => "The name can have at most 255 characters.",
            Self::NameUnsafe => "The name has characters that cannot be used.",
            Self::TopicTooLong => "The topic can have at most 1,000 characters.",
            Self::TopicUnsafe => "The topic has characters that cannot be used.",
            Self::AliasEmpty => "A public room needs an address.",
            Self::AliasInvalid => {
                "The address can use a-z, 0-9 and . _ = - only, up to 64 characters."
            }
            Self::TooManyInvitees => "You can invite at most 100 people when creating a room.",
            Self::BadInvitee => "Invite people by their full user ID, such as @name:server.",
        }
    }
}

/// The create-room form. `Debug` shows lengths only.
#[derive(Clone, PartialEq, Eq)]
pub struct CreateRoomForm {
    /// Name.
    pub name: String,
    /// Topic.
    pub topic: String,
    /// End-to-end encrypted; on by default and not reversible later.
    pub encrypted: bool,
    /// Public (with an address) rather than invite-only.
    pub public: bool,
    /// The address, without `#` and without the server.
    pub alias: String,
    /// People to invite: user IDs separated by spaces, commas or lines.
    pub invitees: String,
}

impl Default for CreateRoomForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            topic: String::new(),
            encrypted: true,
            public: false,
            alias: String::new(),
            invitees: String::new(),
        }
    }
}

impl fmt::Debug for CreateRoomForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreateRoomForm")
            .field("name", &self.name.len())
            .field("topic", &self.topic.len())
            .field("encrypted", &self.encrypted)
            .field("public", &self.public)
            .field("alias", &self.alias.len())
            .field("invitees", &self.invitees.len())
            .finish()
    }
}

impl CreateRoomForm {
    /// Checks the form as the core will and builds the request.
    ///
    /// The name is trimmed (1 to 255 characters), the topic may have lines
    /// (at most 1,000 characters), a public room needs an address of
    /// a-z, 0-9 and `._=-`, and the invitees are user IDs, each once.
    pub fn validate(&self) -> Result<CreateRoomRequest, FormError> {
        let name = check_name(&self.name)?;
        let topic = check_topic(&self.topic)?;
        let visibility = if self.public {
            let alias = self.alias.trim();
            if alias.is_empty() {
                return Err(FormError::AliasEmpty);
            }
            if !valid_alias_local_part(alias) {
                return Err(FormError::AliasInvalid);
            }
            RoomVisibility::Public {
                alias: alias.to_owned(),
            }
        } else {
            RoomVisibility::Private
        };
        Ok(CreateRoomRequest {
            name,
            topic,
            encrypted: self.encrypted,
            visibility,
            invitees: parse_invitees(&self.invitees)?,
        })
    }
}

/// A room name: trimmed, 1 to 255 characters, no control or direction
/// characters.
pub fn check_name(name: &str) -> Result<String, FormError> {
    let name = name.trim();
    let count = name.chars().count();
    if count == 0 {
        return Err(FormError::NameEmpty);
    }
    if count > MAX_NAME_CHARS {
        return Err(FormError::NameTooLong);
    }
    if has_unsafe_char(name, false) {
        return Err(FormError::NameUnsafe);
    }
    Ok(name.to_owned())
}

/// A topic: trimmed, at most 1,000 characters, lines allowed; blank is
/// `None` (which clears the topic).
pub fn check_topic(topic: &str) -> Result<Option<String>, FormError> {
    let topic = topic.trim();
    if topic.chars().count() > MAX_TOPIC_CHARS {
        return Err(FormError::TopicTooLong);
    }
    if has_unsafe_char(topic, true) {
        return Err(FormError::TopicUnsafe);
    }
    Ok((!topic.is_empty()).then(|| topic.to_owned()))
}

/// Splits `text` into user IDs (separated by whitespace, commas or
/// semicolons), each once, in order. `Err` for a bad ID or for more than
/// [`MAX_INVITEES`].
pub fn parse_invitees(text: &str) -> Result<Vec<UserId>, FormError> {
    let mut out: Vec<UserId> = Vec::new();
    for word in text.split(|c: char| c.is_whitespace() || c == ',' || c == ';') {
        if word.is_empty() {
            continue;
        }
        let user = parse_user_id(word).ok_or(FormError::BadInvitee)?;
        if !out.contains(&user) {
            if out.len() >= MAX_INVITEES {
                return Err(FormError::TooManyInvitees);
            }
            out.push(user);
        }
    }
    Ok(out)
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Control or direction characters, as the core refuses them.
pub(crate) fn has_unsafe_char(text: &str, allow_newline: bool) -> bool {
    text.chars()
        .any(|c| (c.is_control() && !(allow_newline && c == '\n')) || is_bidi_control(c))
}

fn valid_alias_local_part(alias: &str) -> bool {
    !alias.is_empty()
        && alias.len() <= MAX_ALIAS_CHARS
        && alias
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._=-".contains(&b))
}

/// Characters of a member's name and ID in the member list.
pub const MAX_MEMBER_CHARS: usize = 48;

/// The badge next to a member: the role, with the level when it is not one
/// of the usual ones; an invited person is marked too. `None` for a plain
/// member.
pub fn role_badge(member: &RoomMemberView) -> Option<String> {
    let role = match (member.role, member.power) {
        (Role::Creator, _) => Some("Creator".to_owned()),
        (Role::Admin, PowerLevelView::Level(n)) if n != 100 => Some(format!("Admin ({n})")),
        (Role::Admin, _) => Some("Admin".to_owned()),
        (Role::Moderator, PowerLevelView::Level(n)) if n != 50 => Some(format!("Moderator ({n})")),
        (Role::Moderator, _) => Some("Moderator".to_owned()),
        (Role::User, PowerLevelView::Level(n)) if n != 0 => Some(format!("Level {n}")),
        (Role::User, _) => None,
    };
    match (role, member.membership) {
        (role, MembershipView::Joined) => role,
        (Some(role), MembershipView::Invited) => Some(format!("{role}, invited")),
        (None, MembershipView::Invited) => Some("Invited".to_owned()),
    }
}

/// The two lines of a member row: the sanitised name (the ID when there is
/// none) and the ID, cut in the middle when long.
pub fn member_lines(member: &RoomMemberView) -> (String, String) {
    let id = elide_middle(member.user_id.as_str(), MAX_MEMBER_CHARS);
    let name = match member
        .display_name
        .as_deref()
        .filter(|n| !n.trim().is_empty())
    {
        Some(name) => name_or_placeholder(name, MAX_MEMBER_CHARS),
        None => id.clone(),
    };
    (name, id)
}

/// What leaving a room costs; each field is a line of the confirmation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LeaveWarning {
    /// The room is encrypted.
    pub encrypted: bool,
    /// The user is the room's only admin and others stay.
    pub last_admin: bool,
    /// The user is the only member.
    pub only_member: bool,
    /// The room or its members are not known yet, so the checks above are
    /// incomplete.
    pub unknown: bool,
}

impl LeaveWarning {
    /// The lines of the confirmation, none for a plain room.
    pub fn lines(&self) -> Vec<&'static str> {
        let mut lines = Vec::new();
        if self.encrypted {
            lines.push(
                "This room is encrypted. If you leave, you may not be able to read its \
                 earlier messages again, even if you join back.",
            );
        }
        if self.last_admin {
            lines.push(
                "You are the only admin. After you leave, nobody can change the room's \
                 roles.",
            );
        }
        if self.only_member {
            lines.push("You are the only member. The room will be empty after you leave.");
        }
        if self.unknown {
            lines.push(
                "Twicecat could not check whether this room is encrypted or whether you \
                 are its last admin.",
            );
        }
        lines
    }
}

/// The warning for `own` leaving a room: `details` says whether it is
/// encrypted, `members` who is in it.
pub fn leave_warning(
    details: Option<&RoomDetailsView>,
    members: Option<&[RoomMemberView]>,
    own: Option<&UserId>,
) -> LeaveWarning {
    let (Some(details), Some(members), Some(own)) = (details, members, own) else {
        return LeaveWarning {
            encrypted: details.is_some_and(|d| d.is_encrypted),
            unknown: true,
            ..LeaveWarning::default()
        };
    };
    let is_admin = |m: &RoomMemberView| matches!(m.role, Role::Admin | Role::Creator);
    let others = members
        .iter()
        .filter(|m| m.membership == MembershipView::Joined && m.user_id != *own);
    let mut other_members = 0usize;
    let mut other_admins = 0usize;
    for member in others {
        other_members += 1;
        if is_admin(member) {
            other_admins += 1;
        }
    }
    let own_admin = members
        .iter()
        .any(|m| m.user_id == *own && m.membership == MembershipView::Joined && is_admin(m));
    LeaveWarning {
        encrypted: details.is_encrypted,
        last_admin: own_admin && other_members > 0 && other_admins == 0,
        only_member: other_members == 0,
        unknown: false,
    }
}

/// A pill ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PillLabel {
    /// The text on the pill.
    pub text: String,
    /// The full ID, for a tooltip or the user card.
    pub id: String,
}

/// The label of a pill, built from its target and never from the label the
/// sender wrote (a sender can name anyone).
///
/// - A user the app knows shows the current display name; any other user
///   shows the full ID, cut in the middle when long.
/// - A room in the list shows its name; any other room shows the alias or ID.
pub fn pill_label(
    target: &MentionTarget,
    members: &[RoomMemberView],
    rooms: &[RoomSummary],
) -> PillLabel {
    match target {
        MentionTarget::User(user) => {
            let name = members
                .iter()
                .find(|m| m.user_id == *user)
                .and_then(|m| m.display_name.as_deref())
                .filter(|name| !name.trim().is_empty());
            PillLabel {
                text: match name {
                    Some(name) => name_or_placeholder(name, MAX_PILL_CHARS),
                    None => elide_middle(user.as_str(), MAX_PILL_CHARS),
                },
                id: one_line_capped(user.as_str(), MAX_ID_CHARS),
            }
        }
        MentionTarget::Room(room) => {
            let name = rooms
                .iter()
                .find(|r| r.id.as_str() == room)
                .map(|r| r.name.as_str())
                .filter(|name| !name.trim().is_empty());
            PillLabel {
                text: match name {
                    Some(name) => name_or_placeholder(name, MAX_PILL_CHARS),
                    None => elide_middle(room, MAX_PILL_CHARS),
                },
                id: one_line_capped(room, MAX_ID_CHARS),
            }
        }
    }
}

/// Sorts by role (creator first), then name (case-insensitive, nameless
/// members last by ID), then ID.
pub fn sort_members(members: &mut [RoomMemberView]) {
    // Named members first, then the nameless ones by user ID.
    fn key(m: &RoomMemberView) -> (bool, String) {
        match m.display_name.as_deref().filter(|n| !n.trim().is_empty()) {
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

// ---- State ---------------------------------------------------------------

/// An open dialog of the room list's buttons.
#[derive(Debug, Clone)]
pub enum Dialog {
    /// **New room**.
    Create(CreateDialog),
    /// **Direct message**.
    Dm(InputDialog),
    /// **Join a room**.
    Join(InputDialog),
}

/// The create-room dialog.
#[derive(Debug, Clone)]
pub struct CreateDialog {
    /// The fields.
    pub form: CreateRoomForm,
    /// The line under the form.
    pub error: Option<&'static str>,
    /// The command was sent and the core has not answered.
    pub busy: bool,
}

/// A dialog with one text field.
#[derive(Clone)]
pub struct InputDialog {
    /// What was typed.
    pub input: String,
    /// The line under the field.
    pub error: Option<&'static str>,
    /// The command was sent and the core has not answered.
    pub busy: bool,
}

impl fmt::Debug for InputDialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputDialog")
            .field("input", &self.input.len())
            .field("error", &self.error)
            .field("busy", &self.busy)
            .finish()
    }
}

impl InputDialog {
    fn new() -> Self {
        Self {
            input: String::new(),
            error: None,
            busy: false,
        }
    }
}

/// The room settings panel.
#[derive(Clone)]
pub struct SettingsPanel {
    /// The room.
    pub room: RoomId,
    /// The name field.
    pub name: String,
    /// The topic field.
    pub topic: String,
    /// The user changed the name field: a new report from the core does not
    /// overwrite it.
    pub name_edited: bool,
    /// The same for the topic.
    pub topic_edited: bool,
    /// The warning card before **Enable encryption** is showing.
    pub encrypt_ask: bool,
    /// The leave confirmation is showing.
    pub leave_ask: bool,
    /// A picture is being read or sent.
    pub avatar_busy: bool,
    /// The line at the bottom: why the last change failed.
    pub error: Option<&'static str>,
}

impl fmt::Debug for SettingsPanel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SettingsPanel")
            .field("room", &self.room)
            .field("name", &self.name.len())
            .field("topic", &self.topic.len())
            .field("encrypt_ask", &self.encrypt_ask)
            .field("leave_ask", &self.leave_ask)
            .field("avatar_busy", &self.avatar_busy)
            .finish_non_exhaustive()
    }
}

/// Members shown at first, and added by each **Show more**.
pub const MEMBERS_PAGE: usize = 100;

/// The member list panel.
#[derive(Clone)]
pub struct MembersPanel {
    /// The room.
    pub room: RoomId,
    /// The invite field.
    pub invite: String,
    /// How many rows are drawn.
    pub shown: usize,
    /// The line under the invite field.
    pub status: Option<&'static str>,
}

impl fmt::Debug for MembersPanel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MembersPanel")
            .field("room", &self.room)
            .field("invite", &self.invite.len())
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

/// The members of the open room, as the core last listed them.
#[derive(Clone)]
pub struct KnownMembers {
    /// The room.
    pub room: RoomId,
    /// Sorted by role, then name.
    pub members: Vec<RoomMemberView>,
    /// The room has more members than the list holds.
    pub truncated: bool,
}

impl fmt::Debug for KnownMembers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KnownMembers")
            .field("room", &self.room)
            .field("members", &self.members.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// The card of a clicked user pill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCard {
    /// The user the pill points at (not what its label said).
    pub user: UserId,
    /// **Message** was pressed.
    pub busy: bool,
    /// Why the last **Message** failed.
    pub error: Option<&'static str>,
}

/// The bar that asks to join a room named by a pill.
#[derive(Clone, PartialEq, Eq)]
pub struct JoinBar {
    /// `!id:server` or `#alias:server`.
    pub id: String,
    /// Servers to try.
    pub via: Vec<String>,
    /// **Join** was pressed.
    pub busy: bool,
}

impl fmt::Debug for JoinBar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinBar")
            .field("id", &"<redacted>")
            .field("busy", &self.busy)
            .finish()
    }
}

/// Everything the room screens remember.
#[derive(Default)]
pub struct RoomsModel {
    /// The open dialog.
    pub dialog: Option<Dialog>,
    /// The settings panel of the selected room.
    pub settings: Option<SettingsPanel>,
    /// The member panel of the selected room.
    pub members_panel: Option<MembersPanel>,
    /// The latest settings the core reported for the panel's room.
    pub details: Option<RoomDetailsView>,
    /// The members of the selected room, once listed.
    pub known: Option<KnownMembers>,
    /// The room whose members were asked for (once per room selection).
    pub members_requested: Option<RoomId>,
    /// The user card.
    pub card: Option<UserCard>,
    /// The bar asking to join a room.
    pub join_bar: Option<JoinBar>,
    /// A room just created or joined, to select once the list has it.
    pub select_when_listed: Option<RoomId>,
    /// Invitations whose answer was sent and not yet seen.
    pub invite_busy: HashSet<RoomId>,
}

// Manual impl: counts and flags only.
impl fmt::Debug for RoomsModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomsModel")
            .field("dialog", &self.dialog)
            .field("settings", &self.settings)
            .field("members_panel", &self.members_panel)
            .field("details", &self.details.is_some())
            .field("known", &self.known)
            .field("card", &self.card.is_some())
            .field("join_bar", &self.join_bar)
            .field("select_when_listed", &self.select_when_listed.is_some())
            .field("invite_busy", &self.invite_busy.len())
            .finish()
    }
}

impl RoomsModel {
    /// Forgets what belongs to the room that was open: the panels, the card,
    /// the bar and the member list. Dialogs stay.
    pub fn reset_room(&mut self) {
        self.settings = None;
        self.members_panel = None;
        self.details = None;
        self.known = None;
        self.members_requested = None;
        self.card = None;
        self.join_bar = None;
    }

    /// Whether anything of this model covers the main view.
    pub fn overlay_open(&self) -> bool {
        self.dialog.is_some()
            || self.settings.is_some()
            || self.members_panel.is_some()
            || self.card.is_some()
    }
}

/// A message of the room dialogs, panels, card and bar. Text typed by the
/// user is `MessageText`, which hides it from `Debug`.
#[derive(Debug, Clone)]
pub enum RoomsMessage {
    /// **New room**.
    OpenCreate,
    /// **Direct message**.
    OpenDm,
    /// **Join a room**.
    OpenJoin,
    /// The dialog was closed (**Cancel**, Esc).
    CloseDialog,
    /// The new room's name changed.
    CreateName(MessageText),
    /// The new room's topic changed.
    CreateTopic(MessageText),
    /// The **Encrypted** box was toggled.
    CreateEncrypted(bool),
    /// Public rather than private.
    CreatePublic(bool),
    /// The address changed.
    CreateAlias(MessageText),
    /// The people to invite changed.
    CreateInvitees(MessageText),
    /// **Create**.
    CreateSubmit,
    /// The direct message field changed.
    DmInput(MessageText),
    /// **Start**.
    DmSubmit,
    /// The join field changed.
    JoinInput(MessageText),
    /// **Join**.
    JoinSubmit,
    /// **Accept** on an invitation.
    Accept(RoomId),
    /// **Decline** on an invitation.
    Decline(RoomId),
    /// **Settings** in the room header.
    OpenSettings,
    /// The settings panel was closed.
    CloseSettings,
    /// The name field changed.
    NameInput(MessageText),
    /// The topic field changed.
    TopicInput(MessageText),
    /// **Save** the name.
    SaveName,
    /// **Save** the topic.
    SaveTopic,
    /// **Change picture...**.
    ChooseAvatar,
    /// The picture dialog answered; the path is redacted in `Debug`.
    AvatarPicked(Option<SavePath>),
    /// The picture was read and scaled (or refused).
    AvatarReady {
        /// The room it was meant for.
        room: RoomId,
        /// The picture, or why not.
        result: Result<Box<AvatarUpload>, AvatarRefusal>,
    },
    /// **Enable encryption** (opens the warning card).
    EncryptAsk,
    /// **Enable encryption** on the warning card.
    EncryptConfirm,
    /// **Cancel** on the warning card.
    EncryptCancel,
    /// **Leave room** (opens the confirmation).
    LeaveAsk,
    /// **Leave** on the confirmation.
    LeaveConfirm,
    /// **Cancel** on the confirmation.
    LeaveCancel,
    /// **Members** in the room header.
    OpenMembers,
    /// The member panel was closed.
    CloseMembers,
    /// The invite field changed.
    InviteInput(MessageText),
    /// **Invite**.
    InviteSubmit,
    /// **Show more** under the member list.
    ShowMoreMembers,
    /// **Message** on the user card.
    CardMessage,
    /// The user card was closed.
    CardClose,
    /// **Join** on the join bar.
    JoinBarJoin,
    /// **Cancel** on the join bar.
    JoinBarCancel,
    /// Esc: closes the topmost dialog, panel or card.
    Escape,
}

/// The words for a failed room action. The core's text is never shown: it
/// can name rooms and users.
pub fn failure_text(error: &CoreError) -> &'static str {
    match error {
        CoreError::NotAllowed => "Not allowed: check the details and your permissions.",
        CoreError::Busy => "Another room action is still running. Try again in a moment.",
        CoreError::Network(_) => "The server could not be reached.",
        CoreError::UnknownRoom(_) | CoreError::NoRoomSelected => "That room is not available.",
        _ => "That did not work. The server may have refused it.",
    }
}

impl App {
    /// A message of the room screens.
    pub(crate) fn on_rooms(&mut self, message: RoomsMessage) -> Task<Message> {
        use RoomsMessage as R;
        match message {
            R::OpenCreate => {
                self.rooms_ui.dialog = Some(Dialog::Create(CreateDialog {
                    form: CreateRoomForm::default(),
                    error: None,
                    busy: false,
                }));
                operation::focus(CREATE_NAME_ID)
            }
            R::OpenDm => {
                self.rooms_ui.dialog = Some(Dialog::Dm(InputDialog::new()));
                operation::focus(DM_INPUT_ID)
            }
            R::OpenJoin => {
                self.rooms_ui.dialog = Some(Dialog::Join(InputDialog::new()));
                operation::focus(JOIN_INPUT_ID)
            }
            R::CloseDialog => {
                self.rooms_ui.dialog = None;
                Task::none()
            }
            R::CreateName(text) => self.edit_create(|form| form.name = text.expose().to_owned()),
            R::CreateTopic(text) => self.edit_create(|form| form.topic = text.expose().to_owned()),
            R::CreateEncrypted(on) => self.edit_create(|form| form.encrypted = on),
            R::CreatePublic(on) => self.edit_create(|form| form.public = on),
            R::CreateAlias(text) => self.edit_create(|form| form.alias = text.expose().to_owned()),
            R::CreateInvitees(text) => {
                self.edit_create(|form| form.invitees = text.expose().to_owned())
            }
            R::CreateSubmit => self.create_submit(),
            R::DmInput(text) => self.edit_input(|dialog| matches!(dialog, Dialog::Dm(_)), text),
            R::DmSubmit => self.dm_submit(),
            R::JoinInput(text) => self.edit_input(|dialog| matches!(dialog, Dialog::Join(_)), text),
            R::JoinSubmit => self.join_submit(),
            R::Accept(room) => self.answer_invite(room, true),
            R::Decline(room) => self.answer_invite(room, false),
            R::OpenSettings => self.open_settings(),
            R::CloseSettings => {
                self.rooms_ui.settings = None;
                if self.rooms_ui.members_panel.is_none() {
                    self.rooms_ui.details = None;
                }
                Task::none()
            }
            R::NameInput(text) => {
                if let Some(panel) = &mut self.rooms_ui.settings {
                    panel.name = text.expose().to_owned();
                    panel.name_edited = true;
                    panel.error = None;
                }
                Task::none()
            }
            R::TopicInput(text) => {
                if let Some(panel) = &mut self.rooms_ui.settings {
                    panel.topic = text.expose().to_owned();
                    panel.topic_edited = true;
                    panel.error = None;
                }
                Task::none()
            }
            R::SaveName => self.save_name(),
            R::SaveTopic => self.save_topic(),
            R::ChooseAvatar => self.choose_avatar(),
            R::AvatarPicked(path) => self.avatar_picked(path),
            R::AvatarReady { room, result } => self.avatar_ready(&room, result),
            R::EncryptAsk => {
                let allowed = self
                    .settings_details()
                    .is_some_and(|d| d.permissions.can_enable_encryption && !d.is_encrypted);
                if let Some(panel) = &mut self.rooms_ui.settings
                    && allowed
                {
                    panel.encrypt_ask = true;
                }
                Task::none()
            }
            R::EncryptCancel => {
                if let Some(panel) = &mut self.rooms_ui.settings {
                    panel.encrypt_ask = false;
                }
                Task::none()
            }
            R::EncryptConfirm => self.encrypt_confirm(),
            R::LeaveAsk => {
                if let Some(panel) = &mut self.rooms_ui.settings {
                    panel.leave_ask = true;
                }
                Task::none()
            }
            R::LeaveCancel => {
                if let Some(panel) = &mut self.rooms_ui.settings {
                    panel.leave_ask = false;
                }
                Task::none()
            }
            R::LeaveConfirm => self.leave_confirm(),
            R::OpenMembers => self.open_members(),
            R::CloseMembers => {
                self.rooms_ui.members_panel = None;
                if self.rooms_ui.settings.is_none() {
                    self.rooms_ui.details = None;
                }
                Task::none()
            }
            R::InviteInput(text) => {
                if let Some(panel) = &mut self.rooms_ui.members_panel {
                    panel.invite = text.expose().to_owned();
                    panel.status = None;
                }
                Task::none()
            }
            R::InviteSubmit => self.invite_submit(),
            R::ShowMoreMembers => {
                let total = self.rooms_ui.known.as_ref().map_or(0, |k| k.members.len());
                if let Some(panel) = &mut self.rooms_ui.members_panel {
                    panel.shown = (panel.shown + 2 * MEMBERS_PAGE).min(total.max(MEMBERS_PAGE));
                }
                self.fetch_member_avatars()
            }
            R::CardMessage => self.card_message(),
            R::CardClose => {
                self.rooms_ui.card = None;
                Task::none()
            }
            R::JoinBarJoin => self.join_bar_join(),
            R::JoinBarCancel => {
                self.rooms_ui.join_bar = None;
                Task::none()
            }
            R::Escape => {
                self.escape();
                Task::none()
            }
        }
    }

    /// Edits the create form, if that dialog is open.
    fn edit_create(&mut self, edit: impl FnOnce(&mut CreateRoomForm)) -> Task<Message> {
        if let Some(Dialog::Create(dialog)) = &mut self.rooms_ui.dialog {
            edit(&mut dialog.form);
            dialog.error = None;
        }
        Task::none()
    }

    /// Sets the field of a one-field dialog that `is_open`.
    fn edit_input(&mut self, is_open: fn(&Dialog) -> bool, text: MessageText) -> Task<Message> {
        if let Some(dialog) = &mut self.rooms_ui.dialog
            && is_open(dialog)
            && let Dialog::Dm(d) | Dialog::Join(d) = dialog
        {
            d.input = text.expose().to_owned();
            d.error = None;
        }
        Task::none()
    }

    fn create_submit(&mut self) -> Task<Message> {
        let Some(Dialog::Create(dialog)) = &mut self.rooms_ui.dialog else {
            return Task::none();
        };
        if dialog.busy {
            return Task::none();
        }
        match dialog.form.validate() {
            Ok(request) => {
                dialog.busy = true;
                dialog.error = None;
                self.send(Command::CreateRoom(request))
            }
            Err(error) => {
                dialog.error = Some(error.text());
                Task::none()
            }
        }
    }

    fn dm_submit(&mut self) -> Task<Message> {
        let own = self.user_id.clone();
        let Some(Dialog::Dm(dialog)) = &mut self.rooms_ui.dialog else {
            return Task::none();
        };
        if dialog.busy {
            return Task::none();
        }
        match parse_user_id(&dialog.input) {
            Some(user) if own.as_ref() != Some(&user) => {
                dialog.busy = true;
                dialog.error = None;
                self.send(Command::CreateDm { user_id: user })
            }
            Some(_) => {
                dialog.error = Some("That is your own user ID.");
                Task::none()
            }
            None => {
                dialog.error = Some(USER_ID_HELP);
                Task::none()
            }
        }
    }

    fn join_submit(&mut self) -> Task<Message> {
        let Some(Dialog::Join(dialog)) = &mut self.rooms_ui.dialog else {
            return Task::none();
        };
        if dialog.busy {
            return Task::none();
        }
        match parse_room_target(&dialog.input) {
            Some(target) => {
                dialog.busy = true;
                dialog.error = None;
                self.send(Command::JoinRoom {
                    target: target.id,
                    via: target.via,
                })
            }
            None => {
                dialog.error = Some(
                    "Enter an address such as #room:server, a room ID such as !id:server, \
                     or a matrix.to link.",
                );
                Task::none()
            }
        }
    }

    /// **Accept** or **Decline** on an invitation, once until answered.
    fn answer_invite(&mut self, room: RoomId, accept: bool) -> Task<Message> {
        let invited = self.rooms.iter().any(|r| r.id == room && r.is_invite);
        if !invited || !self.rooms_ui.invite_busy.insert(room.clone()) {
            return Task::none();
        }
        self.send(if accept {
            Command::AcceptInvite { room_id: room }
        } else {
            Command::DeclineInvite { room_id: room }
        })
    }

    /// The details the core reported for the open room's panel.
    fn settings_details(&self) -> Option<&RoomDetailsView> {
        let panel = self.rooms_ui.settings.as_ref()?;
        self.rooms_ui
            .details
            .as_ref()
            .filter(|d| d.room_id == panel.room)
    }

    /// The open room, when it is a joined room (not an invitation).
    fn panel_room(&self) -> Option<RoomId> {
        let id = self.selected.as_ref()?;
        self.rooms
            .iter()
            .find(|r| r.id == *id && !r.is_invite)
            .map(|r| r.id.clone())
    }

    fn open_settings(&mut self) -> Task<Message> {
        let Some(room) = self.panel_room() else {
            return Task::none();
        };
        self.rooms_ui.members_panel = None;
        if self
            .rooms_ui
            .details
            .as_ref()
            .is_some_and(|d| d.room_id != room)
        {
            self.rooms_ui.details = None;
        }
        self.rooms_ui.settings = Some(SettingsPanel {
            room: room.clone(),
            name: String::new(),
            topic: String::new(),
            name_edited: false,
            topic_edited: false,
            encrypt_ask: false,
            leave_ask: false,
            avatar_busy: false,
            error: None,
        });
        self.rooms_ui.members_requested = Some(room.clone());
        Task::batch([
            self.send(Command::OpenRoomDetails {
                room_id: room.clone(),
            }),
            self.send(Command::ListRoomMembers { room_id: room }),
        ])
    }

    fn save_name(&mut self) -> Task<Message> {
        let allowed = self
            .settings_details()
            .is_some_and(|d| d.permissions.can_set_name);
        let Some(panel) = &mut self.rooms_ui.settings else {
            return Task::none();
        };
        if !allowed {
            return Task::none();
        }
        match check_name(&panel.name) {
            Ok(name) => {
                panel.name_edited = false;
                panel.error = None;
                let room_id = panel.room.clone();
                self.send(Command::SetRoomName { room_id, name })
            }
            Err(error) => {
                panel.error = Some(error.text());
                Task::none()
            }
        }
    }

    fn save_topic(&mut self) -> Task<Message> {
        let allowed = self
            .settings_details()
            .is_some_and(|d| d.permissions.can_set_topic);
        let Some(panel) = &mut self.rooms_ui.settings else {
            return Task::none();
        };
        if !allowed {
            return Task::none();
        }
        match check_topic(&panel.topic) {
            Ok(topic) => {
                panel.topic_edited = false;
                panel.error = None;
                let room_id = panel.room.clone();
                self.send(Command::SetRoomTopic { room_id, topic })
            }
            Err(error) => {
                panel.error = Some(error.text());
                Task::none()
            }
        }
    }

    fn choose_avatar(&mut self) -> Task<Message> {
        let allowed = self
            .settings_details()
            .is_some_and(|d| d.permissions.can_set_avatar);
        let Some(panel) = &mut self.rooms_ui.settings else {
            return Task::none();
        };
        if !allowed || panel.avatar_busy {
            return Task::none();
        }
        panel.avatar_busy = true;
        panel.error = None;
        self.dialogs
            .pick_image()
            .map(|path| Message::Rooms(RoomsMessage::AvatarPicked(path.map(SavePath::from))))
    }

    /// The dialog answered: read and scale the picture off the UI thread.
    fn avatar_picked(&mut self, path: Option<SavePath>) -> Task<Message> {
        let Some(panel) = &mut self.rooms_ui.settings else {
            return Task::none();
        };
        let Some(path) = path else {
            panel.avatar_busy = false;
            return Task::none();
        };
        panel.avatar_busy = true;
        let room = panel.room.clone();
        let path = path.as_path().to_path_buf();
        Task::future(async move {
            let result = match blocking::run(move || read_avatar_file(&path)).await {
                Some(Ok(upload)) => Ok(Box::new(upload)),
                Some(Err(refusal)) => Err(refusal),
                None => Err(AvatarRefusal::Unreadable),
            };
            Message::Rooms(RoomsMessage::AvatarReady { room, result })
        })
    }

    fn avatar_ready(
        &mut self,
        room: &RoomId,
        result: Result<Box<AvatarUpload>, AvatarRefusal>,
    ) -> Task<Message> {
        let Some(panel) = &mut self.rooms_ui.settings else {
            return Task::none();
        };
        if panel.room != *room || !panel.avatar_busy {
            return Task::none();
        }
        panel.avatar_busy = false;
        match result {
            Ok(upload) => {
                let upload = *upload;
                self.send(Command::SetRoomAvatar {
                    room_id: room.clone(),
                    data: MediaBytes::from(upload.data),
                    mimetype: upload.mimetype.to_owned(),
                })
            }
            Err(refusal) => {
                panel.error = Some(refusal.text());
                Task::none()
            }
        }
    }

    fn encrypt_confirm(&mut self) -> Task<Message> {
        let allowed = self
            .settings_details()
            .is_some_and(|d| d.permissions.can_enable_encryption && !d.is_encrypted);
        let Some(panel) = &mut self.rooms_ui.settings else {
            return Task::none();
        };
        if !panel.encrypt_ask || !allowed {
            return Task::none();
        }
        panel.encrypt_ask = false;
        let room_id = panel.room.clone();
        self.send(Command::EnableRoomEncryption { room_id })
    }

    /// **Leave**: one command, then the room's screens close. The room stays
    /// in the list until the next sync says it is left.
    fn leave_confirm(&mut self) -> Task<Message> {
        let Some(panel) = &self.rooms_ui.settings else {
            return Task::none();
        };
        if !panel.leave_ask {
            return Task::none();
        }
        let room_id = panel.room.clone();
        let task = self.send(Command::LeaveRoom {
            room_id: room_id.clone(),
        });
        if self.selected.as_ref() == Some(&room_id) {
            self.deselect();
        }
        self.rooms_ui.reset_room();
        task
    }

    fn open_members(&mut self) -> Task<Message> {
        let Some(room) = self.panel_room() else {
            return Task::none();
        };
        self.rooms_ui.settings = None;
        if self
            .rooms_ui
            .details
            .as_ref()
            .is_some_and(|d| d.room_id != room)
        {
            self.rooms_ui.details = None;
        }
        self.rooms_ui.members_panel = Some(MembersPanel {
            room: room.clone(),
            invite: String::new(),
            shown: MEMBERS_PAGE,
            status: None,
        });
        self.rooms_ui.members_requested = Some(room.clone());
        Task::batch([
            self.send(Command::OpenRoomDetails {
                room_id: room.clone(),
            }),
            self.send(Command::ListRoomMembers { room_id: room }),
            self.fetch_member_avatars(),
        ])
    }

    fn invite_submit(&mut self) -> Task<Message> {
        let can_invite = self
            .rooms_ui
            .members_panel
            .as_ref()
            .and_then(|panel| {
                self.rooms_ui
                    .details
                    .as_ref()
                    .filter(|d| d.room_id == panel.room)
            })
            .is_some_and(|d| d.permissions.can_invite);
        let Some(panel) = &mut self.rooms_ui.members_panel else {
            return Task::none();
        };
        if !can_invite {
            return Task::none();
        }
        let Some(user) = parse_user_id(&panel.invite) else {
            panel.status = Some(USER_ID_HELP);
            return Task::none();
        };
        panel.invite.clear();
        panel.status = Some("Invitation requested.");
        let room_id = panel.room.clone();
        self.send(Command::InviteUser {
            room_id,
            user_id: user,
        })
    }

    fn card_message(&mut self) -> Task<Message> {
        let Some(card) = &mut self.rooms_ui.card else {
            return Task::none();
        };
        if card.busy {
            return Task::none();
        }
        card.busy = true;
        card.error = None;
        let user_id = card.user.clone();
        self.send(Command::CreateDm { user_id })
    }

    fn join_bar_join(&mut self) -> Task<Message> {
        let Some(bar) = &mut self.rooms_ui.join_bar else {
            return Task::none();
        };
        if bar.busy {
            return Task::none();
        }
        bar.busy = true;
        let (target, via) = (bar.id.clone(), bar.via.clone());
        self.send(Command::JoinRoom { target, via })
    }

    /// Esc closes the topmost screen: the dialog, the card, the warning
    /// cards of the settings, then the panels.
    fn escape(&mut self) {
        let ui = &mut self.rooms_ui;
        if ui.dialog.is_some() {
            ui.dialog = None;
        } else if ui.card.is_some() {
            ui.card = None;
        } else if let Some(panel) = &mut ui.settings {
            if panel.encrypt_ask || panel.leave_ask {
                panel.encrypt_ask = false;
                panel.leave_ask = false;
            } else {
                ui.settings = None;
            }
        } else if ui.members_panel.is_some() {
            ui.members_panel = None;
        } else if ui.join_bar.is_some() {
            ui.join_bar = None;
        }
    }

    /// `RoomCreated`: the dialog is over; select the room once listed.
    pub(crate) fn on_room_created(&mut self, room: RoomId) -> Task<Message> {
        if matches!(
            self.rooms_ui.dialog,
            Some(Dialog::Create(_) | Dialog::Dm(_))
        ) {
            self.rooms_ui.dialog = None;
        }
        self.rooms_ui.card = None;
        self.select_when_listed(room)
    }

    /// `RoomJoined`: as `RoomCreated`, and the join bar and dialog are over.
    pub(crate) fn on_room_joined(&mut self, room: RoomId) -> Task<Message> {
        if matches!(self.rooms_ui.dialog, Some(Dialog::Join(_))) {
            self.rooms_ui.dialog = None;
        }
        self.rooms_ui.join_bar = None;
        self.rooms_ui.invite_busy.remove(&room);
        self.select_when_listed(room)
    }

    fn select_when_listed(&mut self, room: RoomId) -> Task<Message> {
        if crate::room_list::contains(&self.rooms, &room) {
            self.rooms_ui.select_when_listed = None;
            self.update(Message::SelectRoom(room))
        } else {
            self.rooms_ui.select_when_listed = Some(room);
            Task::none()
        }
    }

    /// `RoomDetails` for the settings or member panel.
    pub(crate) fn on_room_details(&mut self, details: RoomDetailsView) -> Task<Message> {
        let wanted = self
            .rooms_ui
            .settings
            .as_ref()
            .map(|p| &p.room)
            .or(self.rooms_ui.members_panel.as_ref().map(|p| &p.room))
            .is_some_and(|room| *room == details.room_id);
        if !wanted {
            return Task::none();
        }
        if let Some(panel) = &mut self.rooms_ui.settings {
            if !panel.name_edited {
                panel.name.clone_from(&details.name);
            }
            if !panel.topic_edited {
                panel.topic = details.topic.clone().unwrap_or_default();
            }
        }
        let avatar = details.avatar.clone();
        self.rooms_ui.details = Some(details);
        let ids = self.avatars.take_unrequested(avatar);
        if ids.is_empty() {
            Task::none()
        } else {
            self.send(Command::FetchAvatars(ids))
        }
    }

    /// `RoomMembers`: kept for pills, the leave warning and the panel.
    pub(crate) fn on_room_members(
        &mut self,
        room: RoomId,
        mut members: Vec<RoomMemberView>,
        truncated: bool,
    ) -> Task<Message> {
        if self.selected.as_ref() != Some(&room) {
            return Task::none();
        }
        sort_members(&mut members);
        self.rooms_ui.known = Some(KnownMembers {
            room,
            members,
            truncated,
        });
        self.fetch_member_avatars()
    }

    /// Asks for the avatars of the rows on screen and of the card's user.
    fn fetch_member_avatars(&mut self) -> Task<Message> {
        let Some(known) = &self.rooms_ui.known else {
            return Task::none();
        };
        let shown = self
            .rooms_ui
            .members_panel
            .as_ref()
            .map_or(0, |panel| panel.shown);
        let card = self.rooms_ui.card.as_ref().map(|card| &card.user);
        let wanted: Vec<_> = known
            .members
            .iter()
            .enumerate()
            .filter(|(i, m)| *i < shown || card == Some(&m.user_id))
            .filter_map(|(_, m)| m.avatar.clone())
            .collect();
        let ids = self.avatars.take_unrequested(wanted);
        if ids.is_empty() {
            Task::none()
        } else {
            self.send(Command::FetchAvatars(ids))
        }
    }

    /// `ActionFailed { Rooms }`: the words go to the screen that asked.
    pub(crate) fn on_rooms_failed(&mut self, room: &RoomId, error: &CoreError) -> Task<Message> {
        let text = failure_text(error);
        self.rooms_ui.invite_busy.remove(room);
        let ui = &mut self.rooms_ui;
        match &mut ui.dialog {
            Some(Dialog::Create(d)) if d.busy => {
                d.busy = false;
                d.error = Some(text);
                return Task::none();
            }
            Some(Dialog::Dm(d) | Dialog::Join(d)) if d.busy => {
                d.busy = false;
                d.error = Some(text);
                return Task::none();
            }
            _ => {}
        }
        if let Some(card) = &mut ui.card
            && card.busy
        {
            card.busy = false;
            card.error = Some(text);
            return Task::none();
        }
        if let Some(bar) = &mut ui.join_bar
            && bar.busy
        {
            bar.busy = false;
            return self.show_notice_text(text.to_owned());
        }
        if let Some(panel) = &mut ui.settings {
            panel.avatar_busy = false;
            panel.error = Some(text);
            return Task::none();
        }
        if let Some(panel) = &mut ui.members_panel {
            panel.status = Some(text);
            return Task::none();
        }
        self.show_notice_text(text.to_owned())
    }

    /// After a room list update: select a room just created or joined, and
    /// forget answered invitations.
    pub(crate) fn rooms_after_list_change(&mut self) -> Task<Message> {
        let rooms = &self.rooms;
        self.rooms_ui
            .invite_busy
            .retain(|id| rooms.iter().any(|r| r.id == *id && r.is_invite));
        match self.rooms_ui.select_when_listed.clone() {
            Some(room) if crate::room_list::contains(&self.rooms, &room) => {
                self.rooms_ui.select_when_listed = None;
                self.update(Message::SelectRoom(room))
            }
            _ => Task::none(),
        }
    }

    /// A click on a user pill: the card for the pill's target, but not for
    /// the user themselves.
    pub(crate) fn user_pill_clicked(&mut self, user: UserId) -> Task<Message> {
        if self.user_id.as_ref() == Some(&user) {
            return Task::none();
        }
        self.rooms_ui.card = Some(UserCard {
            user,
            busy: false,
            error: None,
        });
        self.fetch_member_avatars()
    }

    /// A click on a room pill that is not in the list: the join bar.
    pub(crate) fn room_pill_clicked(&mut self, target: &str) -> Task<Message> {
        if let Some(target) = parse_room_target(target) {
            self.rooms_ui.join_bar = Some(JoinBar {
                id: target.id,
                via: target.via,
                busy: false,
            });
        }
        Task::none()
    }

    /// Asks for the open room's members once, when its timeline holds a user
    /// pill (their names label the pills).
    pub(crate) fn members_for_pills(&mut self) -> Task<Message> {
        let Some(room) = self.selected.clone() else {
            return Task::none();
        };
        if self.rooms_ui.members_requested.as_ref() == Some(&room) {
            return Task::none();
        }
        let has_pill = self.timeline.items.iter().any(|item| match &item.kind {
            TimelineItemKind::Message { details, .. } => has_user_mention(&details.content.blocks),
            _ => false,
        });
        if !has_pill {
            return Task::none();
        }
        self.rooms_ui.members_requested = Some(room.clone());
        self.send(Command::ListRoomMembers { room_id: room })
    }

    /// The members of the open room, when they are listed.
    pub fn known_members(&self) -> &[RoomMemberView] {
        match (&self.rooms_ui.known, &self.selected) {
            (Some(known), Some(room)) if known.room == *room => &known.members,
            _ => &[],
        }
    }

    /// What leaving the open room would cost, from what is known.
    pub fn leave_warning_now(&self) -> LeaveWarning {
        let room = self.selected.as_ref();
        let details = self
            .rooms_ui
            .details
            .as_ref()
            .filter(|d| Some(&d.room_id) == room);
        let members = self
            .rooms_ui
            .known
            .as_ref()
            .filter(|k| Some(&k.room) == room)
            .map(|k| k.members.as_slice());
        leave_warning(details, members, self.user_id.as_ref())
    }
}

/// Whether any of `blocks` holds a pill for a user.
fn has_user_mention(blocks: &[Block]) -> bool {
    fn inlines(list: &[Inline]) -> bool {
        list.iter().any(|inline| match inline {
            Inline::Mention {
                target: MentionTarget::User(_),
                ..
            } => true,
            Inline::Spoiler(inner) => inlines(inner),
            _ => false,
        })
    }
    blocks.iter().any(|block| match block {
        Block::Paragraph(list) | Block::Heading { inlines: list, .. } => inlines(list),
        Block::Quote(inner) => has_user_mention(inner),
        Block::List { items, .. } => items.iter().any(|item| has_user_mention(item)),
        Block::Code { .. } | Block::Rule => false,
    })
}

/// The line under a field that wants a user ID.
const USER_ID_HELP: &str = "Enter a full user ID, such as @name:server.";

/// Longest side of a room picture after the downscale, in pixels.
pub const AVATAR_SIDE: u32 = 512;
/// Most bytes of an encoded room picture (the core's limit too).
pub const AVATAR_MAX_BYTES: usize = 1024 * 1024;
/// JPEG quality steps tried when a PNG is too big.
const AVATAR_JPEG_QUALITIES: [u8; 3] = [90, 75, 55];

/// A room picture ready to send. `Debug` shows the size only.
#[derive(Clone, PartialEq, Eq)]
pub struct AvatarUpload {
    /// The encoded picture, at most [`AVATAR_MAX_BYTES`].
    pub data: Vec<u8>,
    /// `image/png` or `image/jpeg`, matching the bytes.
    pub mimetype: &'static str,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl fmt::Debug for AvatarUpload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AvatarUpload({}x{}, {} bytes)",
            self.width,
            self.height,
            self.data.len()
        )
    }
}

/// Why a picture was not used as a room picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarRefusal {
    /// The file could not be opened or read.
    Unreadable,
    /// It is a folder or another thing that is not a file.
    NotAFile,
    /// The file is bigger than the app reads.
    TooLarge,
    /// Not a PNG, JPEG, WebP or GIF the decoder accepts within its limits.
    NotAPicture,
    /// Even scaled down, the picture did not fit in 1 MiB.
    StillTooBig,
}

impl AvatarRefusal {
    /// The line under the picture button.
    pub fn text(self) -> &'static str {
        match self {
            Self::Unreadable => "The picture could not be read.",
            Self::NotAFile => "Choose a picture file, not a folder.",
            Self::TooLarge => "That file is too large to use as a picture.",
            Self::NotAPicture => "Use a PNG, JPEG, WebP or GIF picture.",
            Self::StillTooBig => "The picture is too detailed to fit in 1 MiB.",
        }
    }
}

/// Decodes untrusted picture bytes (format allow-list and size limits of
/// [`crate::media::decode_rgba`], first frame of a GIF), scales the picture
/// down to fit [`AVATAR_SIDE`] x [`AVATAR_SIDE`] (never up) and encodes it:
/// a PNG, or a JPEG when the PNG is over 1 MiB or the picture would not fit.
/// Blocking: run it with [`crate::blocking::run`].
pub fn downscale_avatar(bytes: &[u8]) -> Result<AvatarUpload, AvatarRefusal> {
    let rgba = crate::media::decode_rgba(bytes, crate::media::decode_limits())
        .ok_or(AvatarRefusal::NotAPicture)?;
    let mut picture = image::DynamicImage::ImageRgba8(rgba);
    if picture.width() == 0 || picture.height() == 0 {
        return Err(AvatarRefusal::NotAPicture);
    }
    let mut side = AVATAR_SIDE;
    // The second round halves the side: a picture of fine noise can stay over
    // 1 MiB as a 512 pixel JPEG at the lowest quality.
    for _ in 0..2 {
        if picture.width() > side || picture.height() > side {
            picture = picture.resize(side, side, image::imageops::FilterType::Lanczos3);
        }
        if let Some(upload) = encode_avatar(&picture) {
            return Ok(upload);
        }
        side /= 2;
    }
    Err(AvatarRefusal::StillTooBig)
}

/// A PNG if it fits, else the best JPEG that does.
fn encode_avatar(picture: &image::DynamicImage) -> Option<AvatarUpload> {
    use image::codecs::jpeg::JpegEncoder;
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder as _};

    let (width, height) = (picture.width(), picture.height());
    let rgba = picture.to_rgba8();
    let mut png = Vec::new();
    let encoded = PngEncoder::new(&mut png).write_image(
        rgba.as_raw(),
        width,
        height,
        ExtendedColorType::Rgba8,
    );
    if encoded.is_ok() && png.len() <= AVATAR_MAX_BYTES {
        return Some(AvatarUpload {
            data: png,
            mimetype: "image/png",
            width,
            height,
        });
    }
    // JPEG has no alpha: lay the picture over white.
    let flat: Vec<u8> = rgba
        .pixels()
        .flat_map(|p| {
            let a = u32::from(p.0[3]);
            [p.0[0], p.0[1], p.0[2]].map(|c| ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8)
        })
        .collect();
    for quality in AVATAR_JPEG_QUALITIES {
        let mut jpeg = Vec::new();
        let encoded = JpegEncoder::new_with_quality(&mut jpeg, quality).write_image(
            &flat,
            width,
            height,
            ExtendedColorType::Rgb8,
        );
        if encoded.is_ok() && jpeg.len() <= AVATAR_MAX_BYTES {
            return Some(AvatarUpload {
                data: jpeg,
                mimetype: "image/jpeg",
                width,
                height,
            });
        }
    }
    None
}

/// Reads the picture at `path` (a regular file, at most
/// [`crate::upload::MAX_INSPECT_BYTES`]) and runs [`downscale_avatar`] on it.
/// Blocking.
pub fn read_avatar_file(path: &std::path::Path) -> Result<AvatarUpload, AvatarRefusal> {
    use std::io::Read as _;

    use crate::upload::MAX_INSPECT_BYTES;

    // The type is checked before the file is opened: opening a FIFO for
    // reading would block this worker until someone writes to it.
    let meta = std::fs::metadata(path).map_err(|_| AvatarRefusal::Unreadable)?;
    if !meta.is_file() {
        return Err(AvatarRefusal::NotAFile);
    }
    if meta.len() > MAX_INSPECT_BYTES {
        return Err(AvatarRefusal::TooLarge);
    }
    let file = std::fs::File::open(path).map_err(|_| AvatarRefusal::Unreadable)?;
    let mut bytes = Vec::new();
    file.take(MAX_INSPECT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| AvatarRefusal::Unreadable)?;
    if bytes.len() as u64 > MAX_INSPECT_BYTES {
        return Err(AvatarRefusal::TooLarge);
    }
    downscale_avatar(&bytes)
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{PowerLevelView, RoomId};

    use super::*;

    fn form(name: &str) -> CreateRoomForm {
        CreateRoomForm {
            name: name.into(),
            ..CreateRoomForm::default()
        }
    }

    #[test]
    fn encryption_is_on_by_default() {
        assert!(CreateRoomForm::default().encrypted);
        let request = form("Lounge").validate().expect("valid");
        assert!(request.encrypted);
        assert_eq!(request.visibility, RoomVisibility::Private);
    }

    #[test]
    fn names_are_trimmed_one_to_255_characters_without_controls() {
        assert_eq!(form("  Lounge ").validate().expect("ok").name, "Lounge");
        assert_eq!(form("").validate().unwrap_err(), FormError::NameEmpty);
        assert_eq!(form("   ").validate().unwrap_err(), FormError::NameEmpty);
        assert!(form(&"n".repeat(255)).validate().is_ok());
        assert_eq!(
            form(&"n".repeat(256)).validate().unwrap_err(),
            FormError::NameTooLong
        );
        for bad in ["a\nb", "a\u{0}b", "a\u{202E}b", "a\u{2066}b"] {
            assert_eq!(form(bad).validate().unwrap_err(), FormError::NameUnsafe);
        }
    }

    #[test]
    fn topics_may_have_lines_and_blank_means_none() {
        let mut f = form("Lounge");
        f.topic = "  line one\nline two ".into();
        assert_eq!(
            f.validate().expect("ok").topic.as_deref(),
            Some("line one\nline two")
        );
        f.topic = "   ".into();
        assert_eq!(f.validate().expect("ok").topic, None);
        f.topic = "t".repeat(1001);
        assert_eq!(f.validate().unwrap_err(), FormError::TopicTooLong);
        f.topic = "a\u{202E}b".into();
        assert_eq!(f.validate().unwrap_err(), FormError::TopicUnsafe);
        f.topic = "a\u{7}b".into();
        assert_eq!(f.validate().unwrap_err(), FormError::TopicUnsafe);
    }

    #[test]
    fn a_public_room_needs_a_valid_alias_local_part() {
        let mut f = form("Lounge");
        f.public = true;
        assert_eq!(f.validate().unwrap_err(), FormError::AliasEmpty);
        for bad in ["Lounge", "a b", "a:b", "#a", "a/b", &"a".repeat(65)] {
            f.alias = bad.into();
            assert_eq!(f.validate().unwrap_err(), FormError::AliasInvalid, "{bad}");
        }
        f.alias = "lounge._=-9".into();
        assert_eq!(
            f.validate().expect("ok").visibility,
            RoomVisibility::Public {
                alias: "lounge._=-9".into()
            }
        );
        // The alias of a private room is ignored.
        f.public = false;
        f.alias = "Not Valid".into();
        assert_eq!(
            f.validate().expect("ok").visibility,
            RoomVisibility::Private
        );
    }

    #[test]
    fn invitees_are_user_ids_each_once() {
        let mut f = form("Lounge");
        f.invitees = "@bob:localhost, @carol:localhost\n@bob:localhost;  @dan:x ".into();
        let ids: Vec<String> = f
            .validate()
            .expect("ok")
            .invitees
            .iter()
            .map(|u| u.as_str().to_owned())
            .collect();
        assert_eq!(ids, ["@bob:localhost", "@carol:localhost", "@dan:x"]);
        for bad in ["bob", "@bob", "@bob:localhost, nope", "!room:x"] {
            f.invitees = bad.into();
            assert_eq!(f.validate().unwrap_err(), FormError::BadInvitee, "{bad}");
        }
        f.invitees = (0..101)
            .map(|i| format!("@u{i}:x"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(f.validate().unwrap_err(), FormError::TooManyInvitees);
        f.invitees = (0..100)
            .map(|i| format!("@u{i}:x"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(f.validate().expect("ok").invitees.len(), 100);
        f.invitees = String::new();
        assert!(f.validate().expect("ok").invitees.is_empty());
    }

    #[test]
    fn the_form_debug_shows_lengths_only() {
        let mut f = form("sentinel-name");
        f.topic = "sentinel-topic".into();
        f.alias = "sentinel-alias".into();
        f.invitees = "@sentinel:x".into();
        let shown = format!("{f:?}");
        assert!(!shown.contains("sentinel"), "{shown}");
    }

    fn member(id: &str, name: Option<&str>, role: Role, state: MembershipView) -> RoomMemberView {
        RoomMemberView {
            user_id: UserId::from(id),
            display_name: name.map(str::to_owned),
            avatar: None,
            power: match role {
                Role::Creator => PowerLevelView::Creator,
                Role::Admin => PowerLevelView::Level(100),
                Role::Moderator => PowerLevelView::Level(50),
                Role::User => PowerLevelView::Level(0),
            },
            role,
            membership: state,
        }
    }

    fn joined(id: &str, role: Role) -> RoomMemberView {
        member(id, None, role, MembershipView::Joined)
    }

    fn details(encrypted: bool) -> RoomDetailsView {
        RoomDetailsView {
            room_id: RoomId::from("!r:x"),
            name: "Lounge".into(),
            topic: None,
            avatar: None,
            is_encrypted: encrypted,
            alias: None,
            permissions: twicecat_core::api::RoomPermissions::default(),
        }
    }

    #[test]
    fn leaving_a_plain_room_with_other_admins_warns_of_nothing() {
        let own = UserId::from("@me:x");
        let members = [
            joined("@me:x", Role::Admin),
            joined("@other:x", Role::Admin),
            joined("@u:x", Role::User),
        ];
        let w = leave_warning(Some(&details(false)), Some(&members), Some(&own));
        assert_eq!(w, LeaveWarning::default());
        assert!(w.lines().is_empty());
    }

    #[test]
    fn leaving_an_encrypted_room_warns() {
        let own = UserId::from("@me:x");
        let members = [joined("@me:x", Role::User), joined("@a:x", Role::Admin)];
        let w = leave_warning(Some(&details(true)), Some(&members), Some(&own));
        assert!(w.encrypted && !w.last_admin && !w.only_member && !w.unknown);
        assert_eq!(w.lines().len(), 1);
        assert!(w.lines()[0].contains("encrypted"));
    }

    #[test]
    fn the_last_admin_is_warned_and_a_moderator_does_not_count() {
        let own = UserId::from("@me:x");
        let members = [
            joined("@me:x", Role::Admin),
            joined("@mod:x", Role::Moderator),
            joined("@u:x", Role::User),
        ];
        let w = leave_warning(Some(&details(false)), Some(&members), Some(&own));
        assert!(w.last_admin && !w.only_member);
        assert!(w.lines()[0].contains("admin"));
        // The creator counts as an admin seat, too.
        let members = [joined("@me:x", Role::Creator), joined("@u:x", Role::User)];
        assert!(leave_warning(Some(&details(false)), Some(&members), Some(&own)).last_admin);
        // An invited admin has not taken the seat.
        let members = [
            joined("@me:x", Role::Admin),
            member("@inv:x", None, Role::Admin, MembershipView::Invited),
            joined("@u:x", Role::User),
        ];
        assert!(leave_warning(Some(&details(false)), Some(&members), Some(&own)).last_admin);
        // An ordinary member is never the last admin.
        let members = [joined("@me:x", Role::User), joined("@a:x", Role::Admin)];
        assert!(!leave_warning(Some(&details(false)), Some(&members), Some(&own)).last_admin);
    }

    #[test]
    fn the_only_member_is_told_the_room_will_be_empty() {
        let own = UserId::from("@me:x");
        let members = [joined("@me:x", Role::Admin)];
        let w = leave_warning(Some(&details(false)), Some(&members), Some(&own));
        assert!(w.only_member && !w.last_admin);
        assert!(w.lines()[0].contains("only member"));
    }

    #[test]
    fn warnings_combine_and_unknowns_are_said() {
        let own = UserId::from("@me:x");
        let members = [joined("@me:x", Role::Admin), joined("@u:x", Role::User)];
        let w = leave_warning(Some(&details(true)), Some(&members), Some(&own));
        assert_eq!(w.lines().len(), 2);
        let w = leave_warning(None, Some(&members), Some(&own));
        assert!(w.unknown);
        let w = leave_warning(Some(&details(false)), None, Some(&own));
        assert!(w.unknown && !w.last_admin);
        let w = leave_warning(Some(&details(false)), Some(&members), None);
        assert!(w.unknown);
        assert!(!w.lines().is_empty());
    }

    fn room(id: &str, name: &str) -> RoomSummary {
        RoomSummary::new(RoomId::from(id), name.into())
    }

    #[test]
    fn a_user_pill_shows_the_known_display_name() {
        let members = [member(
            "@bob:x",
            Some("Bob"),
            Role::User,
            MembershipView::Joined,
        )];
        let pill = pill_label(&MentionTarget::User(UserId::from("@bob:x")), &members, &[]);
        assert_eq!(pill.text, "Bob");
        assert_eq!(pill.id, "@bob:x");
    }

    #[test]
    fn an_unknown_user_or_a_nameless_member_shows_the_full_id() {
        let members = [joined("@nameless:x", Role::User)];
        let unknown = pill_label(&MentionTarget::User(UserId::from("@eve:x")), &members, &[]);
        assert_eq!(unknown.text, "@eve:x");
        let nameless = pill_label(
            &MentionTarget::User(UserId::from("@nameless:x")),
            &members,
            &[],
        );
        assert_eq!(nameless.text, "@nameless:x");
        let blank = [member(
            "@b:x",
            Some("  "),
            Role::User,
            MembershipView::Joined,
        )];
        let pill = pill_label(&MentionTarget::User(UserId::from("@b:x")), &blank, &[]);
        assert_eq!(pill.text, "@b:x");
    }

    #[test]
    fn a_long_id_is_cut_in_the_middle_and_keeps_the_server() {
        let id = format!("@{}:evil.example", "a".repeat(200));
        let pill = pill_label(&MentionTarget::User(UserId::from(id.as_str())), &[], &[]);
        assert!(pill.text.chars().count() <= MAX_PILL_CHARS, "{}", pill.text);
        assert!(pill.text.ends_with(":evil.example"), "{}", pill.text);
        assert!(pill.text.starts_with("@aaa"), "{}", pill.text);
        assert!(pill.text.contains('\u{2026}'));
        assert_eq!(pill.id, id.as_str(), "the tooltip keeps the whole ID");
    }

    #[test]
    fn the_senders_label_has_no_way_in() {
        // The function takes no label at all; a member named like someone
        // else still shows the target's own name.
        let members = [
            member(
                "@alice:x",
                Some("Alice"),
                Role::User,
                MembershipView::Joined,
            ),
            member("@bob:x", Some("Bob"), Role::User, MembershipView::Joined),
        ];
        let pill = pill_label(&MentionTarget::User(UserId::from("@bob:x")), &members, &[]);
        assert_eq!(pill.text, "Bob");
    }

    #[test]
    fn names_on_pills_are_one_clean_line() {
        let members = [member(
            "@m:x",
            Some("Ma\u{202E}llory\nRoot"),
            Role::User,
            MembershipView::Joined,
        )];
        let pill = pill_label(&MentionTarget::User(UserId::from("@m:x")), &members, &[]);
        assert!(!pill.text.contains('\n'));
        assert!(!pill.text.contains('\u{202E}'));
        let long = [member(
            "@l:x",
            Some(&"w".repeat(500)),
            Role::User,
            MembershipView::Joined,
        )];
        let pill = pill_label(&MentionTarget::User(UserId::from("@l:x")), &long, &[]);
        assert!(pill.text.chars().count() <= MAX_PILL_CHARS);
    }

    #[test]
    fn a_room_pill_shows_the_listed_name_else_the_alias_or_id() {
        let rooms = [room("!lounge:x", "Lounge")];
        let listed = pill_label(&MentionTarget::Room("!lounge:x".into()), &[], &rooms);
        assert_eq!(listed.text, "Lounge");
        assert_eq!(listed.id, "!lounge:x");
        let alias = pill_label(&MentionTarget::Room("#other:x".into()), &[], &rooms);
        assert_eq!(alias.text, "#other:x");
        let id = pill_label(&MentionTarget::Room("!unknown:x".into()), &[], &rooms);
        assert_eq!(id.text, "!unknown:x");
        let long = format!("#{}:server.example", "r".repeat(200));
        let cut = pill_label(&MentionTarget::Room(long.clone()), &[], &rooms);
        assert!(cut.text.chars().count() <= MAX_PILL_CHARS);
        assert!(cut.text.ends_with(":server.example"));
        assert_eq!(cut.id, long);
        // A listed room whose name is blank falls back to the ID.
        let rooms = [room("!blank:x", "   ")];
        let blank = pill_label(&MentionTarget::Room("!blank:x".into()), &[], &rooms);
        assert_eq!(blank.text, "!blank:x");
    }

    #[test]
    fn members_sort_by_role_then_name_ignoring_case() {
        let mut list = vec![
            member("@z:x", Some("zed"), Role::User, MembershipView::Joined),
            member("@a:x", Some("Alice"), Role::User, MembershipView::Joined),
            member("@n:x", None, Role::User, MembershipView::Joined),
            member("@m:x", Some("mod"), Role::Moderator, MembershipView::Joined),
            member("@c:x", Some("Boss"), Role::Creator, MembershipView::Joined),
            member("@d:x", Some("Admin"), Role::Admin, MembershipView::Invited),
        ];
        sort_members(&mut list);
        let ids: Vec<&str> = list.iter().map(|m| m.user_id.as_str()).collect();
        assert_eq!(ids, ["@c:x", "@d:x", "@m:x", "@a:x", "@z:x", "@n:x"]);
    }

    fn png_of(width: u32, height: u32, noisy: bool) -> Vec<u8> {
        use image::{ExtendedColorType, ImageEncoder as _, codecs::png::PngEncoder};
        let mut seed = 0x1234_5678_u32;
        let pixels: Vec<u8> = (0..width * height * 4)
            .map(|i| {
                if noisy {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (seed >> 24) as u8
                } else {
                    [10, 120, 200, 255][(i % 4) as usize]
                }
            })
            .collect();
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&pixels, width, height, ExtendedColorType::Rgba8)
            .expect("encode");
        out
    }

    fn size_of_encoded(upload: &AvatarUpload) -> (u32, u32) {
        let image = crate::media::decode_rgba(&upload.data, crate::media::decode_limits())
            .expect("the result decodes");
        image.dimensions()
    }

    #[test]
    fn a_big_picture_is_scaled_to_fit_512_keeping_its_shape() {
        let up = downscale_avatar(&png_of(1024, 768, false)).expect("ok");
        assert_eq!((up.width, up.height), (512, 384));
        assert_eq!(size_of_encoded(&up), (512, 384));
        let up = downscale_avatar(&png_of(600, 1200, false)).expect("ok");
        assert_eq!((up.width, up.height), (256, 512));
        assert_eq!(size_of_encoded(&up), (256, 512));
    }

    #[test]
    fn a_small_picture_is_not_scaled_up() {
        let up = downscale_avatar(&png_of(100, 50, false)).expect("ok");
        assert_eq!((up.width, up.height), (100, 50));
        assert_eq!(up.mimetype, "image/png");
        assert!(up.data.starts_with(b"\x89PNG"));
        assert!(up.data.len() <= AVATAR_MAX_BYTES);
    }

    #[test]
    fn a_picture_whose_png_is_over_1_mib_becomes_a_jpeg_under_1_mib() {
        let up = downscale_avatar(&png_of(900, 900, true)).expect("ok");
        assert_eq!((up.width, up.height), (512, 512));
        assert_eq!(up.mimetype, "image/jpeg");
        assert!(up.data.starts_with(&[0xFF, 0xD8, 0xFF]));
        assert!(up.data.len() <= AVATAR_MAX_BYTES, "{}", up.data.len());
        assert_eq!(size_of_encoded(&up), (512, 512));
    }

    #[test]
    fn anything_that_is_not_an_accepted_picture_is_refused() {
        assert_eq!(downscale_avatar(b""), Err(AvatarRefusal::NotAPicture));
        assert_eq!(
            downscale_avatar(b"not a picture at all"),
            Err(AvatarRefusal::NotAPicture)
        );
        // A header that promises more than the decoder limits allow.
        let mut huge = png_of(8, 8, false);
        huge[16..20].copy_from_slice(&40_000_u32.to_be_bytes());
        huge[20..24].copy_from_slice(&40_000_u32.to_be_bytes());
        assert_eq!(downscale_avatar(&huge), Err(AvatarRefusal::NotAPicture));
        // Truncated data.
        let png = png_of(64, 64, true);
        assert_eq!(
            downscale_avatar(&png[..png.len() / 2]),
            Err(AvatarRefusal::NotAPicture)
        );
    }

    #[test]
    fn the_avatar_file_must_be_a_regular_file() {
        let dir = std::env::temp_dir().join(format!("twicecat-avatar-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("a.png");
        std::fs::write(&file, png_of(32, 32, false)).expect("write");
        let up = read_avatar_file(&file).expect("ok");
        assert_eq!((up.width, up.height), (32, 32));
        assert_eq!(read_avatar_file(&dir), Err(AvatarRefusal::NotAFile));
        assert_eq!(
            read_avatar_file(&dir.join("missing.png")),
            Err(AvatarRefusal::Unreadable)
        );
        let junk = dir.join("junk.png");
        std::fs::write(&junk, b"junk").expect("write");
        assert_eq!(read_avatar_file(&junk), Err(AvatarRefusal::NotAPicture));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_role_badge_names_unusual_levels_and_invitations() {
        let mut m = member("@a:x", Some("A"), Role::User, MembershipView::Joined);
        assert_eq!(role_badge(&m), None);
        m.power = PowerLevelView::Level(25);
        assert_eq!(role_badge(&m).as_deref(), Some("Level 25"));
        m.role = Role::Moderator;
        m.power = PowerLevelView::Level(50);
        assert_eq!(role_badge(&m).as_deref(), Some("Moderator"));
        m.role = Role::Admin;
        m.power = PowerLevelView::Level(100);
        assert_eq!(role_badge(&m).as_deref(), Some("Admin"));
        m.power = PowerLevelView::Level(200);
        assert_eq!(role_badge(&m).as_deref(), Some("Admin (200)"));
        m.role = Role::Creator;
        m.power = PowerLevelView::Creator;
        assert_eq!(role_badge(&m).as_deref(), Some("Creator"));
        let invited = member("@i:x", None, Role::User, MembershipView::Invited);
        assert_eq!(role_badge(&invited).as_deref(), Some("Invited"));
    }

    #[test]
    fn a_member_row_shows_a_clean_name_and_an_elided_id() {
        let m = member(
            &format!("@{}:evil.example", "a".repeat(200)),
            Some("Ma\u{202E}l\nlory"),
            Role::User,
            MembershipView::Joined,
        );
        let (name, id) = member_lines(&m);
        assert!(!name.contains(['\u{202E}', '\n']), "{name:?}");
        assert!(id.chars().count() <= MAX_MEMBER_CHARS);
        assert!(id.ends_with(":evil.example"));
        let nameless = member("@n:x", None, Role::User, MembershipView::Joined);
        assert_eq!(
            member_lines(&nameless),
            ("@n:x".to_owned(), "@n:x".to_owned())
        );
    }
}
