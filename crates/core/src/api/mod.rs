//! The commands-in / events-out API of the core.
//!
//! Everything here is SDK-free: no type of the Matrix SDK or its helper crates
//! appears in it (enforced by `scripts/check-core-api.sh`).

use std::fmt;
use std::path::PathBuf;

mod audio;
mod call;
mod diff;
mod media;
mod rooms;
mod security;
mod types;

pub use audio::{
    AUDIO_FRAME_LEN, AudioBackend, AudioDevice, AudioDeviceId, AudioDevices, AudioProcessing,
    AudioSettings, AudioStats, CleaningAnnouncement, EnhancerBackend, GpuBackend, GpuEnhancement,
    InputLevel, InputMode, NoiseSuppression, NsLevel, PushToTalkBindings, RemoteCleaning,
    SkipCleaned, VirtualAudio, VoiceCleaning, limits,
};
pub use call::{
    CallEndReason, CallIntent, CallState, CleaningStatus, Participant, ParticipantId, SkipReason,
};
pub use diff::ListDiff;
pub use media::{
    AttachmentData, MediaBytes, MediaContent, MediaKind, MediaSize, OutgoingAttachment,
    UploadProgress, UploadThumbnail,
};
pub use rooms::{
    CreateRoomRequest, MembershipView, NotificationKind, NotificationSettingsView,
    NotificationView, OwnProfileView, PowerLevelView, Role, RoomDetailsView, RoomMemberView,
    RoomNotificationModeView, RoomPermissions, RoomVisibility, SpaceView, ThemeChoice,
    ThreadSummaryView, UiSettings,
};
pub use security::{
    CancelReason, CrossSigning, DeviceSummary, DeviceTrust, DeviceVerification, QrImage,
    ReauthAction, RecoveryKey, RecoveryProgress, RecoveryStatus, SasEmoji, SecurityStatus, Shield,
    ShieldCode, UserTrust, UtdReason, VerificationId, VerificationPeer, VerificationPhase,
};
pub use types::{
    ActionKind, AuthUrl, AvatarId, AvatarImage, Block, CoreError, DeviceId, Draft, EventId, Inline,
    ItemActions, Member, MentionRef, MentionTarget, MessageDetails, MessageKind, MessagePreview,
    MessageText, OutgoingMessage, PaginationState, Password, PlaceholderKind, ReactionGroup,
    ReplyPreview, ReplyState, RichText, RoomId, RoomListQuery, RoomListTab, RoomSummary,
    SendFailure, SendStatus, StateChange, SyncState, TextFormat, TextStyle, TimelineItem,
    TimelineItemKind, TypingUser, UserId,
};

/// A request from the front end to the engine.
///
/// `Debug` hides message text, reaction keys, redaction reasons and search
/// queries.
#[derive(Clone)]
pub enum Command {
    /// Restore the last session from the secret store.
    RestoreSession,
    /// Log in with a password.
    LoginPassword {
        /// Localpart or full MXID.
        username: String,
        /// The password.
        password: Password,
        /// Server name or homeserver URL; taken from the MXID when `None`.
        server: Option<String>,
    },
    /// Start an OAuth 2.0 login against `server`.
    LoginOAuth {
        /// Server name or homeserver URL.
        server: String,
    },
    /// Abort a pending OAuth login.
    CancelOAuth,
    /// Open a room's timeline.
    SelectRoom(RoomId),
    /// Send a text message to the selected room.
    SendMessage {
        /// Message text.
        text: MessageText,
    },
    /// Change the room list's tab and search filter.
    SetRoomListQuery(RoomListQuery),
    /// List one more page of rooms.
    LoadMoreRooms,
    /// Request avatar thumbnails; each ID is answered by one
    /// [`CoreEvent::Avatar`].
    FetchAvatars(Vec<AvatarId>),
    /// Send a message (markdown, mentions, reply) to the selected room.
    Send(OutgoingMessage),
    /// Replace the content of one of the user's own messages.
    Edit {
        /// `TimelineItem::id` of the message.
        item_id: String,
        /// The new content; its `room_id` names the room.
        message: OutgoingMessage,
    },
    /// Redact a message.
    Redact {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the message.
        item_id: String,
        /// Why, when given.
        reason: Option<MessageText>,
    },
    /// Add the user's reaction to a message, or remove it if present.
    ToggleReaction {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the message.
        item_id: String,
        /// The reaction, usually an emoji.
        key: String,
    },
    /// Send a failed local echo again.
    RetrySend {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the local echo.
        item_id: String,
    },
    /// Drop a failed local echo.
    DiscardSend {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the local echo.
        item_id: String,
    },
    /// Load older events into the selected room's timeline.
    PaginateBackwards {
        /// Room.
        room_id: RoomId,
    },
    /// Send a read receipt for the newest event.
    MarkRead {
        /// Room.
        room_id: RoomId,
        /// Also move the read marker.
        fully_read: bool,
    },
    /// Move only the private fully-read marker to the newest event, without
    /// sending a public read receipt.
    MoveReadMarker {
        /// Room.
        room_id: RoomId,
    },
    /// Tell the room whether the user is typing.
    SetTyping {
        /// Room.
        room_id: RoomId,
        /// Typing now.
        typing: bool,
    },
    /// Look up room members whose name or ID matches `query`.
    SearchMembers {
        /// Room.
        room_id: RoomId,
        /// Text to match.
        query: String,
    },
    /// Keep `draft` as the room's composer draft in the session store, or
    /// remove it (`None`).
    SaveDraft {
        /// Room.
        room_id: RoomId,
        /// The draft; `None` clears it.
        draft: Option<Draft>,
    },
    /// Set up recovery; the new key comes back once as
    /// [`CoreEvent::RecoveryKeyReady`].
    EnableRecovery,
    /// Restore secrets with a recovery key or passphrase.
    Recover {
        /// The key or passphrase.
        key: RecoveryKey,
    },
    /// Replace the recovery key; the new key comes back once as
    /// [`CoreEvent::RecoveryKeyReady`].
    ChangeRecoveryKey,
    /// Create the cross-signing identity.
    BootstrapCrossSigning {
        /// The account password, when the server asks for it.
        password: Option<Password>,
    },
    /// Replace the cross-signing identity.
    ResetIdentity {
        /// The account password, when the server asks for it.
        password: Option<Password>,
    },
    /// Abort a pending identity reset (OAuth approval).
    CancelIdentityReset,
    /// Verify this device against the account's other devices.
    RequestSelfVerification,
    /// Verify another device of this account.
    RequestDeviceVerification {
        /// The device.
        device_id: DeviceId,
    },
    /// Verify another user (in a direct message).
    RequestUserVerification {
        /// The user.
        user_id: UserId,
    },
    /// Accept an incoming verification request.
    AcceptVerification {
        /// The flow.
        id: VerificationId,
    },
    /// Compare emoji instead of scanning (while a QR code is shown).
    StartSas {
        /// The flow.
        id: VerificationId,
    },
    /// The user confirms that the other device shows success after scanning.
    ConfirmQrScanned {
        /// The flow.
        id: VerificationId,
    },
    /// The emoji or numbers match.
    ConfirmSas {
        /// The flow.
        id: VerificationId,
    },
    /// The emoji or numbers do not match.
    SasMismatch {
        /// The flow.
        id: VerificationId,
    },
    /// Cancel a verification, or decline an incoming request.
    CancelVerification {
        /// The flow.
        id: VerificationId,
    },
    /// List the account's devices; answered by [`CoreEvent::Devices`].
    ListDevices,
    /// Rename a device.
    RenameDevice {
        /// The device.
        device_id: DeviceId,
        /// The new display name.
        name: String,
    },
    /// Sign out other devices.
    SignOutDevices {
        /// The devices.
        device_ids: Vec<DeviceId>,
        /// The account password, when the server asks for it.
        password: Option<Password>,
    },
    /// Report [`CoreEvent::UserTrust`] for a user now and on every change.
    WatchUserTrust {
        /// The user.
        user_id: UserId,
    },
    /// Fetch a media message's thumbnail or file; answered by
    /// [`CoreEvent::Media`].
    FetchMedia {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the media message.
        item_id: String,
        /// Which rendition.
        size: MediaSize,
    },
    /// Save a media message's file to `path`; answered by
    /// [`CoreEvent::MediaSaved`] or an [`ActionKind::Media`] failure.
    SaveMedia {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the media message.
        item_id: String,
        /// Where to write the file.
        path: PathBuf,
    },
    /// Send a file as a media message into the selected room (a room that is
    /// joined but not selected fails with `NoRoomSelected`). With a
    /// `thread_root` the thread must be open (`OpenThread`), or the upload is
    /// refused with `NotAllowed`. Failures are `ActionFailed { Upload, .. }`.
    SendAttachment(OutgoingAttachment),
    /// Set a room's own notification mode; `None` goes back to the account
    /// default.
    SetRoomNotificationMode {
        /// Room.
        room_id: RoomId,
        /// The mode, or `None` for the default.
        mode: Option<RoomNotificationModeView>,
    },
    /// Set the account default for group rooms or direct messages.
    SetDefaultNotificationMode {
        /// Direct messages (true) or group rooms (false).
        one_to_one: bool,
        /// The mode.
        mode: RoomNotificationModeView,
    },
    /// Add a notification keyword.
    AddKeyword {
        /// The keyword, 1-64 characters.
        keyword: String,
    },
    /// Remove a notification keyword.
    RemoveKeyword {
        /// The keyword.
        keyword: String,
    },
    /// Create a room; answered by [`CoreEvent::RoomCreated`].
    CreateRoom(CreateRoomRequest),
    /// Open a direct message with a user, reusing an existing one; answered
    /// by [`CoreEvent::RoomCreated`].
    CreateDm {
        /// The other user.
        user_id: UserId,
    },
    /// Join a room by alias, ID or `matrix.to` link; answered by
    /// [`CoreEvent::RoomJoined`].
    JoinRoom {
        /// `#alias:server`, `!id:server` or a `matrix.to` link.
        target: String,
        /// Servers to try when joining by ID.
        via: Vec<String>,
    },
    /// Accept an invitation.
    AcceptInvite {
        /// Room.
        room_id: RoomId,
    },
    /// Decline an invitation.
    DeclineInvite {
        /// Room.
        room_id: RoomId,
    },
    /// Invite a user to a room.
    InviteUser {
        /// Room.
        room_id: RoomId,
        /// The user.
        user_id: UserId,
    },
    /// Leave a room.
    LeaveRoom {
        /// Room.
        room_id: RoomId,
    },
    /// Report [`CoreEvent::RoomDetails`] for a room now and on every change.
    OpenRoomDetails {
        /// Room.
        room_id: RoomId,
    },
    /// Change a room's name.
    SetRoomName {
        /// Room.
        room_id: RoomId,
        /// The new name.
        name: String,
    },
    /// Change a room's topic; `None` clears it.
    SetRoomTopic {
        /// Room.
        room_id: RoomId,
        /// The new topic.
        topic: Option<String>,
    },
    /// Change a room's avatar.
    SetRoomAvatar {
        /// Room.
        room_id: RoomId,
        /// The encoded image.
        data: MediaBytes,
        /// Its MIME type.
        mimetype: String,
    },
    /// Turn on encryption in a room. This cannot be undone.
    EnableRoomEncryption {
        /// Room.
        room_id: RoomId,
    },
    /// List a room's members; answered by [`CoreEvent::RoomMembers`].
    ListRoomMembers {
        /// Room.
        room_id: RoomId,
    },
    /// Change the user's display name.
    SetDisplayName {
        /// The new name.
        name: String,
    },
    /// Change the user's avatar.
    SetAvatar {
        /// The encoded image.
        data: MediaBytes,
        /// Its MIME type.
        mimetype: String,
    },
    /// Remove the user's avatar.
    RemoveAvatar,
    /// Open a thread in a second timeline, closing any other one.
    ///
    /// The thread's items have IDs of the form `thread:{root}:{n}`: an item
    /// command (react, redact, edit, fetch, save, retry, discard) with an ID of
    /// another thread, or of a thread that is not open, fails with
    /// `ItemNotFound`.
    OpenThread {
        /// Room.
        room_id: RoomId,
        /// The thread's root event.
        root: EventId,
    },
    /// Close the open thread.
    CloseThread,
    /// Load older events of the open thread.
    PaginateThread {
        /// Room.
        room_id: RoomId,
        /// The thread's root event.
        root: EventId,
    },
    /// Log out and delete the local session.
    Logout,
    /// Stop the engine.
    Shutdown,

    // Calls (Phase 6). One call at a time; joining another room leaves the
    // current call first.
    /// Join (or start) the MatrixRTC session of a room.
    JoinCall {
        /// Room of the call.
        room_id: RoomId,
        /// Voice only for now.
        intent: CallIntent,
    },
    /// Leave the current call.
    LeaveCall,
    /// Decline the ringing call in `room_id` (on every device of this user).
    DeclineCall {
        /// Room that rings.
        room_id: RoomId,
    },
    /// Stop the ringtone of the rooms ringing now, and keep their rings
    /// (they can still be answered or declined). A later ring sounds again.
    SilenceRinging,
    /// Mute or unmute the microphone in the current call.
    SetMicMuted(bool),
    /// Deafen: play nothing and mute the microphone (Discord semantics).
    SetDeafened(bool),
    /// Push-to-talk key or button state, from any source (hotkey, gamepad,
    /// window key). Ignored unless the input mode is push-to-talk.
    PushToTalk {
        /// Held down.
        pressed: bool,
    },
    /// Playback volume for one user, percent (0..=200); remembered.
    SetUserVolume {
        /// Account.
        user_id: UserId,
        /// Percent; 100 is unchanged.
        percent: u16,
    },
    /// Whether to clean one user's voice ([`RemoteCleaning`]); remembered.
    SetUserCleaning {
        /// Account.
        user_id: UserId,
        /// Automatic follows the global switch.
        cleaning: VoiceCleaning,
    },
    /// Re-enumerate audio devices; answered by `AudioDevices`.
    ListAudioDevices,
    /// Ask for the current settings; answered by `AudioSettings`.
    GetAudioSettings,
    /// Find which GPU backends can run speech enhancement here (the first
    /// time takes about a second); answered by `GpuEnhancement`.
    ProbeGpuEnhancement,
    /// Replace all audio settings. They are validated, applied (to a running
    /// call too), saved, and echoed back as `AudioSettings`.
    UpdateAudioSettings(AudioSettings),
    /// Open the microphone without a call: level meter and, when `loopback`
    /// is set, hear yourself through the processing chain.
    StartMicTest {
        /// Play the processed microphone back.
        loopback: bool,
    },
    /// Close the mic test.
    StopMicTest,
}

// Manual impl: message text, reaction keys, reasons and queries are user
// content and must not reach logs.
impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RestoreSession => f.write_str("RestoreSession"),
            Self::LoginPassword {
                username,
                password,
                server,
            } => f
                .debug_struct("LoginPassword")
                .field("username", username)
                .field("password", password)
                .field("server", server)
                .finish(),
            Self::LoginOAuth { server } => f
                .debug_struct("LoginOAuth")
                .field("server", server)
                .finish(),
            Self::CancelOAuth => f.write_str("CancelOAuth"),
            Self::SelectRoom(room) => f.debug_tuple("SelectRoom").field(room).finish(),
            Self::SendMessage { text } => {
                f.debug_struct("SendMessage").field("text", text).finish()
            }
            Self::SetRoomListQuery(query) => {
                f.debug_tuple("SetRoomListQuery").field(query).finish()
            }
            Self::LoadMoreRooms => f.write_str("LoadMoreRooms"),
            Self::FetchAvatars(ids) => f.debug_tuple("FetchAvatars").field(ids).finish(),
            Self::Send(message) => f.debug_tuple("Send").field(message).finish(),
            Self::Edit { item_id, message } => f
                .debug_struct("Edit")
                .field("item_id", item_id)
                .field("message", message)
                .finish(),
            Self::Redact {
                room_id,
                item_id,
                reason,
            } => f
                .debug_struct("Redact")
                .field("room_id", room_id)
                .field("item_id", item_id)
                .field("reason", reason)
                .finish(),
            Self::ToggleReaction {
                room_id,
                item_id,
                key,
            } => f
                .debug_struct("ToggleReaction")
                .field("room_id", room_id)
                .field("item_id", item_id)
                .field("key", &format_args!("<redacted, {} bytes>", key.len()))
                .finish(),
            Self::RetrySend { room_id, item_id } => f
                .debug_struct("RetrySend")
                .field("room_id", room_id)
                .field("item_id", item_id)
                .finish(),
            Self::DiscardSend { room_id, item_id } => f
                .debug_struct("DiscardSend")
                .field("room_id", room_id)
                .field("item_id", item_id)
                .finish(),
            Self::PaginateBackwards { room_id } => f
                .debug_struct("PaginateBackwards")
                .field("room_id", room_id)
                .finish(),
            Self::MarkRead {
                room_id,
                fully_read,
            } => f
                .debug_struct("MarkRead")
                .field("room_id", room_id)
                .field("fully_read", fully_read)
                .finish(),
            Self::MoveReadMarker { room_id } => f
                .debug_struct("MoveReadMarker")
                .field("room_id", room_id)
                .finish(),
            Self::SetTyping { room_id, typing } => f
                .debug_struct("SetTyping")
                .field("room_id", room_id)
                .field("typing", typing)
                .finish(),
            Self::SearchMembers { room_id, query } => f
                .debug_struct("SearchMembers")
                .field("room_id", room_id)
                .field("query", &format_args!("<redacted, {} bytes>", query.len()))
                .finish(),
            Self::SaveDraft { room_id, draft } => f
                .debug_struct("SaveDraft")
                .field("room_id", room_id)
                .field("draft", draft)
                .finish(),
            Self::EnableRecovery => f.write_str("EnableRecovery"),
            Self::Recover { key } => f.debug_struct("Recover").field("key", key).finish(),
            Self::ChangeRecoveryKey => f.write_str("ChangeRecoveryKey"),
            Self::BootstrapCrossSigning { password } => f
                .debug_struct("BootstrapCrossSigning")
                .field("password", password)
                .finish(),
            Self::ResetIdentity { password } => f
                .debug_struct("ResetIdentity")
                .field("password", password)
                .finish(),
            Self::CancelIdentityReset => f.write_str("CancelIdentityReset"),
            Self::RequestSelfVerification => f.write_str("RequestSelfVerification"),
            Self::RequestDeviceVerification { device_id } => f
                .debug_struct("RequestDeviceVerification")
                .field("device_id", device_id)
                .finish(),
            Self::RequestUserVerification { user_id } => f
                .debug_struct("RequestUserVerification")
                .field("user_id", user_id)
                .finish(),
            Self::AcceptVerification { id } => f
                .debug_struct("AcceptVerification")
                .field("id", id)
                .finish(),
            Self::StartSas { id } => f.debug_struct("StartSas").field("id", id).finish(),
            Self::ConfirmQrScanned { id } => {
                f.debug_struct("ConfirmQrScanned").field("id", id).finish()
            }
            Self::ConfirmSas { id } => f.debug_struct("ConfirmSas").field("id", id).finish(),
            Self::SasMismatch { id } => f.debug_struct("SasMismatch").field("id", id).finish(),
            Self::CancelVerification { id } => f
                .debug_struct("CancelVerification")
                .field("id", id)
                .finish(),
            Self::ListDevices => f.write_str("ListDevices"),
            Self::RenameDevice { device_id, name } => f
                .debug_struct("RenameDevice")
                .field("device_id", device_id)
                .field("name", name)
                .finish(),
            Self::SignOutDevices {
                device_ids,
                password,
            } => f
                .debug_struct("SignOutDevices")
                .field("device_ids", device_ids)
                .field("password", password)
                .finish(),
            Self::WatchUserTrust { user_id } => f
                .debug_struct("WatchUserTrust")
                .field("user_id", user_id)
                .finish(),
            Self::FetchMedia {
                room_id,
                item_id,
                size,
            } => f
                .debug_struct("FetchMedia")
                .field("room_id", room_id)
                .field("item_id", item_id)
                .field("size", size)
                .finish(),
            Self::SaveMedia {
                room_id, item_id, ..
            } => f
                .debug_struct("SaveMedia")
                .field("room_id", room_id)
                .field("item_id", item_id)
                .field("path", &format_args!("<redacted>"))
                .finish(),
            Self::SendAttachment(attachment) => {
                f.debug_tuple("SendAttachment").field(attachment).finish()
            }
            Self::SetRoomNotificationMode { room_id, mode } => f
                .debug_struct("SetRoomNotificationMode")
                .field("room_id", room_id)
                .field("mode", mode)
                .finish(),
            Self::SetDefaultNotificationMode { one_to_one, mode } => f
                .debug_struct("SetDefaultNotificationMode")
                .field("one_to_one", one_to_one)
                .field("mode", mode)
                .finish(),
            Self::AddKeyword { keyword } => f
                .debug_struct("AddKeyword")
                .field(
                    "keyword",
                    &format_args!("<redacted, {} bytes>", keyword.len()),
                )
                .finish(),
            Self::RemoveKeyword { keyword } => f
                .debug_struct("RemoveKeyword")
                .field(
                    "keyword",
                    &format_args!("<redacted, {} bytes>", keyword.len()),
                )
                .finish(),
            Self::CreateRoom(request) => f.debug_tuple("CreateRoom").field(request).finish(),
            Self::CreateDm { user_id } => f
                .debug_struct("CreateDm")
                .field("user_id", user_id)
                .finish(),
            Self::JoinRoom { target, via } => f
                .debug_struct("JoinRoom")
                .field(
                    "target",
                    &format_args!("<redacted, {} bytes>", target.len()),
                )
                .field("via", via)
                .finish(),
            Self::AcceptInvite { room_id } => f
                .debug_struct("AcceptInvite")
                .field("room_id", room_id)
                .finish(),
            Self::DeclineInvite { room_id } => f
                .debug_struct("DeclineInvite")
                .field("room_id", room_id)
                .finish(),
            Self::InviteUser { room_id, user_id } => f
                .debug_struct("InviteUser")
                .field("room_id", room_id)
                .field("user_id", user_id)
                .finish(),
            Self::LeaveRoom { room_id } => f
                .debug_struct("LeaveRoom")
                .field("room_id", room_id)
                .finish(),
            Self::OpenRoomDetails { room_id } => f
                .debug_struct("OpenRoomDetails")
                .field("room_id", room_id)
                .finish(),
            Self::SetRoomName { room_id, name } => f
                .debug_struct("SetRoomName")
                .field("room_id", room_id)
                .field("name", &format_args!("<redacted, {} bytes>", name.len()))
                .finish(),
            Self::SetRoomTopic { room_id, topic } => f
                .debug_struct("SetRoomTopic")
                .field("room_id", room_id)
                .field("topic_bytes", &topic.as_ref().map(String::len))
                .finish(),
            Self::SetRoomAvatar {
                room_id,
                data,
                mimetype,
            } => f
                .debug_struct("SetRoomAvatar")
                .field("room_id", room_id)
                .field("data", data)
                .field("mimetype", mimetype)
                .finish(),
            Self::EnableRoomEncryption { room_id } => f
                .debug_struct("EnableRoomEncryption")
                .field("room_id", room_id)
                .finish(),
            Self::ListRoomMembers { room_id } => f
                .debug_struct("ListRoomMembers")
                .field("room_id", room_id)
                .finish(),
            Self::SetDisplayName { name } => f
                .debug_struct("SetDisplayName")
                .field("name", &format_args!("<redacted, {} bytes>", name.len()))
                .finish(),
            Self::SetAvatar { data, mimetype } => f
                .debug_struct("SetAvatar")
                .field("data", data)
                .field("mimetype", mimetype)
                .finish(),
            Self::RemoveAvatar => f.write_str("RemoveAvatar"),
            Self::OpenThread { room_id, root } => f
                .debug_struct("OpenThread")
                .field("room_id", room_id)
                .field("root", root)
                .finish(),
            Self::CloseThread => f.write_str("CloseThread"),
            Self::PaginateThread { room_id, root } => f
                .debug_struct("PaginateThread")
                .field("room_id", room_id)
                .field("root", root)
                .finish(),
            Self::Logout => f.write_str("Logout"),
            Self::Shutdown => f.write_str("Shutdown"),
            Self::JoinCall { room_id, intent } => f
                .debug_struct("JoinCall")
                .field("room_id", room_id)
                .field("intent", intent)
                .finish(),
            Self::LeaveCall => f.write_str("LeaveCall"),
            Self::SilenceRinging => f.write_str("SilenceRinging"),
            Self::DeclineCall { room_id } => f
                .debug_struct("DeclineCall")
                .field("room_id", room_id)
                .finish(),
            Self::SetMicMuted(muted) => f.debug_tuple("SetMicMuted").field(muted).finish(),
            Self::SetDeafened(deafened) => f.debug_tuple("SetDeafened").field(deafened).finish(),
            Self::PushToTalk { pressed } => f
                .debug_struct("PushToTalk")
                .field("pressed", pressed)
                .finish(),
            Self::SetUserVolume { user_id, percent } => f
                .debug_struct("SetUserVolume")
                .field("user_id", user_id)
                .field("percent", percent)
                .finish(),
            Self::SetUserCleaning { user_id, cleaning } => f
                .debug_struct("SetUserCleaning")
                .field("user_id", user_id)
                .field("cleaning", cleaning)
                .finish(),
            Self::ListAudioDevices => f.write_str("ListAudioDevices"),
            Self::GetAudioSettings => f.write_str("GetAudioSettings"),
            Self::ProbeGpuEnhancement => f.write_str("ProbeGpuEnhancement"),
            Self::UpdateAudioSettings(settings) => f
                .debug_tuple("UpdateAudioSettings")
                .field(settings)
                .finish(),
            Self::StartMicTest { loopback } => f
                .debug_struct("StartMicTest")
                .field("loopback", loopback)
                .finish(),
            Self::StopMicTest => f.write_str("StopMicTest"),
        }
    }
}

/// A notification from the engine to the front end.
#[derive(Clone, Debug)]
pub enum CoreEvent {
    /// No stored session exists.
    NoSession,
    /// A session is active.
    LoggedIn {
        /// Account.
        user_id: UserId,
        /// This device.
        device_id: DeviceId,
        /// True when restored rather than freshly logged in.
        restored: bool,
    },
    /// Login failed.
    LoginFailed(CoreError),
    /// Open this URL in a browser to continue OAuth login.
    OAuthUrl(AuthUrl),
    /// The session was ended by logout.
    LoggedOut,
    /// The server rejected the session token.
    SessionExpired,
    /// Sync state changed.
    SyncState(SyncState),
    /// Room list changed.
    RoomListChanged(Vec<ListDiff<RoomSummary>>),
    /// The room list finished its initial load.
    RoomListLoaded {
        /// Number of rooms the server reported, when known.
        total_rooms: Option<u32>,
    },
    /// Answer to [`Command::FetchAvatars`] for one avatar.
    Avatar {
        /// The requested avatar.
        id: AvatarId,
        /// The image bytes, or `None` when it is unavailable.
        image: Option<AvatarImage>,
    },
    /// Selected room's timeline changed.
    TimelineDiff {
        /// Room.
        room_id: RoomId,
        /// Changes, in order.
        diffs: Vec<ListDiff<TimelineItem>>,
    },
    /// A message could not be sent.
    SendFailed {
        /// Room.
        room_id: RoomId,
        /// Why.
        error: CoreError,
    },
    /// Back-pagination of a room changed state.
    Pagination {
        /// Room.
        room_id: RoomId,
        /// The new state.
        state: PaginationState,
    },
    /// Who is typing in a room (other users only).
    Typing {
        /// Room.
        room_id: RoomId,
        /// The users, replacing the previous list.
        users: Vec<TypingUser>,
    },
    /// Answer to [`Command::SearchMembers`].
    Members {
        /// Room.
        room_id: RoomId,
        /// The query this answers. A [`MessageText`], so that `Debug` does
        /// not print what the user typed.
        query: MessageText,
        /// Matching members.
        members: Vec<Member>,
    },
    /// The stored composer draft of a room just opened; sent after the
    /// room's first timeline items, only when there is one.
    DraftLoaded {
        /// Room.
        room_id: RoomId,
        /// The draft.
        draft: Draft,
    },
    /// An action on a room or item failed. Actions that belong to no room
    /// (recovery, cross-signing, verification, devices) carry an empty
    /// `room_id`.
    ActionFailed {
        /// Room.
        room_id: RoomId,
        /// Which action.
        action: ActionKind,
        /// Why.
        error: CoreError,
    },
    /// The account's security state, sent after login or restore and on every
    /// change.
    Security(SecurityStatus),
    /// Progress of setting recovery up.
    RecoveryProgress(RecoveryProgress),
    /// A new recovery key. Show it once; the core never stores it.
    RecoveryKeyReady(RecoveryKey),
    /// [`Command::Recover`] succeeded.
    Recovered,
    /// The action needs the account password; resend the command with it.
    ReauthRequired {
        /// The action that was refused.
        action: ReauthAction,
    },
    /// Open this account-management URL in a browser to approve the action
    /// (OAuth sessions).
    OpenAccountManagement(AuthUrl),
    /// A verification request arrived or was started.
    VerificationRequested {
        /// The flow.
        id: VerificationId,
        /// The other side.
        peer: VerificationPeer,
    },
    /// A verification flow moved to a new phase.
    Verification {
        /// The flow.
        id: VerificationId,
        /// The new phase.
        phase: VerificationPhase,
    },
    /// Answer to [`Command::ListDevices`].
    Devices(Vec<DeviceSummary>),
    /// A watched user's trust, now or after a change.
    UserTrust {
        /// The user.
        user_id: UserId,
        /// Their trust.
        trust: UserTrust,
    },
    /// Answer to [`Command::FetchMedia`].
    Media {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the media message.
        item_id: String,
        /// Which rendition was asked for.
        size: MediaSize,
        /// The bytes, or why there are none.
        result: Result<MediaBytes, CoreError>,
    },
    /// [`Command::SaveMedia`] wrote the file.
    MediaSaved {
        /// Room.
        room_id: RoomId,
        /// `TimelineItem::id` of the media message.
        item_id: String,
    },
    /// The server's upload limit, sent after login or restore.
    MediaLimits {
        /// Largest upload in bytes, when the server says.
        max_upload_bytes: Option<u64>,
    },
    /// A notification to show; the front end decides whether to.
    Notification(NotificationView),
    /// The account's notification defaults and keywords, sent after login or
    /// restore and on every change.
    NotificationSettings(NotificationSettingsView),
    /// Unread totals over all joined, non-muted rooms, for the tray badge.
    UnreadTotals {
        /// Unread notifications.
        notifications: u64,
        /// Unread highlights.
        highlights: u64,
    },
    /// A room was created.
    RoomCreated {
        /// The new room.
        room_id: RoomId,
    },
    /// A room was joined.
    RoomJoined {
        /// The room.
        room_id: RoomId,
    },
    /// A room's settings, after [`Command::OpenRoomDetails`] and on every
    /// change.
    RoomDetails(RoomDetailsView),
    /// Answer to [`Command::ListRoomMembers`].
    RoomMembers {
        /// Room.
        room_id: RoomId,
        /// Members, sorted by role then name; at most 5,000.
        members: Vec<RoomMemberView>,
        /// There were more members than the list holds.
        truncated: bool,
    },
    /// The user's own profile, sent after login or restore and on change.
    OwnProfile(OwnProfileView),
    /// The spaces the user has joined.
    Spaces(Vec<SpaceView>),
    /// The open thread's timeline changed.
    ThreadDiff {
        /// Room.
        room_id: RoomId,
        /// The thread's root event.
        root: EventId,
        /// Changes, in order.
        diffs: Vec<ListDiff<TimelineItem>>,
    },
    /// Back-pagination of the open thread changed state.
    ThreadPagination {
        /// Room.
        room_id: RoomId,
        /// The thread's root event.
        root: EventId,
        /// The new state.
        state: PaginationState,
    },
    /// Any other error.
    Error(CoreError),

    // Calls (Phase 6).
    /// The local call state changed.
    CallState(CallState),
    /// Participants of the current call changed (including this device).
    CallParticipants {
        /// Room of the call.
        room_id: RoomId,
        /// Changes, in order.
        diffs: Vec<ListDiff<Participant>>,
    },
    /// How many devices are in a room's call; sent when it changes, for the
    /// room list (voice rooms). Zero means the call ended.
    ActiveCall {
        /// Room.
        room_id: RoomId,
        /// Devices in the call.
        participants: u32,
    },
    /// Someone rings this user, or (with `ring: false`) lets them know a
    /// call started in a group room.
    ///
    /// A ring also plays a ringtone on the output device
    /// ([`AudioSettings::ringtone`]) until the ringing stops
    /// ([`CoreEvent::RingingStopped`]), the user joins a call, or
    /// [`Command::SilenceRinging`]; never while this device is in a call.
    IncomingCall {
        /// Room that rings.
        room_id: RoomId,
        /// Who started the call.
        caller: UserId,
        /// Voice or (later) video.
        intent: CallIntent,
        /// A ring (a DM call); `false` for a quiet notification.
        ring: bool,
    },
    /// Someone declined the ring of the call we started.
    CallDeclined {
        /// Room of the call.
        room_id: RoomId,
        /// Who declined.
        user_id: UserId,
    },
    /// The ringing in a room stopped: declined (maybe on another device),
    /// answered, or timed out.
    RingingStopped {
        /// Room.
        room_id: RoomId,
    },
    /// Audio devices, answering `ListAudioDevices` or after a hot-plug.
    AudioDevices(AudioDevices),
    /// The effective audio settings.
    AudioSettings(AudioSettings),
    /// Microphone level (about 20 Hz while the microphone is open).
    InputLevel(InputLevel),
    /// Processing statistics (about 1 Hz while the microphone is open).
    AudioStats(AudioStats),
    /// GPU speech enhancement: answering `ProbeGpuEnhancement`, and when
    /// where DeepFilterNet3 runs changes.
    GpuEnhancement(GpuEnhancement),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_summary_new_has_zero_defaults() {
        let r = RoomSummary::new(RoomId::from("!r:x"), "Lounge".into());
        assert_eq!(r.id, RoomId::from("!r:x"));
        assert_eq!(r.name, "Lounge");
        assert_eq!(
            (r.unread_messages, r.unread_mentions, r.unread_notifications),
            (0, 0, 0)
        );
        assert_eq!(r.highlights, 0);
        assert!(!r.is_dm && !r.is_space && !r.is_invite && !r.marked_unread);
        assert!(r.avatar.is_none() && r.last_message.is_none());
    }

    #[test]
    fn room_list_query_defaults() {
        let q = RoomListQuery::default();
        assert_eq!(q.tab, RoomListTab::All);
        assert_eq!(RoomListTab::default(), RoomListTab::All);
        assert!(q.search.is_empty());
    }

    fn preview() -> MessagePreview {
        MessagePreview {
            sender: UserId::from("@a:x"),
            sender_name: Some("Ann".into()),
            body: "secret-body-x".into(),
            timestamp_ms: 4242,
            is_own: false,
        }
    }

    #[test]
    fn message_preview_debug_hides_body() {
        let dbg = format!("{:?}", preview());
        assert!(!dbg.contains("secret-body-x"), "{dbg}");
        assert!(dbg.contains("@a:x"), "{dbg}");
        assert!(dbg.contains("4242"), "{dbg}");
    }

    #[test]
    fn room_summary_debug_hides_preview_body() {
        let mut room = RoomSummary::new(RoomId::from("!r:x"), "Lounge".into());
        room.last_message = Some(preview());
        let dbg = format!("{room:?}");
        assert!(!dbg.contains("secret-body-x"), "{dbg}");
        assert!(dbg.contains("Lounge"), "{dbg}");
        let ev = CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: vec![room] }]);
        assert!(!format!("{ev:?}").contains("secret-body-x"));
    }

    #[test]
    fn avatar_image_debug_shows_length_only() {
        let img = AvatarImage::from(vec![0xAB; 3]);
        let dbg = format!("{img:?}");
        assert!(dbg.contains('3'), "{dbg}");
        assert!(
            !dbg.to_lowercase().contains("ab") && !dbg.contains("171"),
            "{dbg}"
        );
        assert_eq!(img.as_bytes(), &[0xAB; 3]);
        let ev = CoreEvent::Avatar {
            id: AvatarId::from("mxc://x/y"),
            image: Some(img),
        };
        assert!(format!("{ev:?}").contains("mxc://x/y"));
    }

    #[test]
    fn password_debug_is_redacted() {
        let dbg = format!("{:?}", Password::from("hunter2-x"));
        assert!(!dbg.contains("hunter2-x"), "{dbg}");
    }

    #[test]
    fn message_item_debug_hides_body() {
        let item = TimelineItem::new(
            "i".into(),
            TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: "secret-body-x".into(),
                is_own: false,
                timestamp_ms: 1,
                sent: SendStatus::Sent,
                details: Box::default(),
            },
        );
        assert!(!format!("{item:?}").contains("secret-body-x"));
        let ev = CoreEvent::TimelineDiff {
            room_id: RoomId::from("!r:x"),
            diffs: vec![ListDiff::Reset { values: vec![item] }],
        };
        assert!(!format!("{ev:?}").contains("secret-body-x"));
    }

    #[test]
    fn send_message_debug_hides_text() {
        let cmd = Command::SendMessage {
            text: MessageText::from("secret-body-x"),
        };
        let dbg = format!("{cmd:?}");
        assert!(!dbg.contains("secret-body-x"), "{dbg}");
    }

    #[test]
    fn oauth_url_debug_hides_query() {
        let ev = CoreEvent::OAuthUrl(AuthUrl::from(
            "https://auth.example.org/authorize?state=secret-state-x&code_challenge=c",
        ));
        let dbg = format!("{ev:?}");
        assert!(!dbg.contains("secret-state-x"), "{dbg}");
        assert!(!dbg.contains("code_challenge"), "{dbg}");
        assert!(dbg.contains("auth.example.org"), "{dbg}");
    }

    #[test]
    fn new_item_has_no_readers_and_default_details_are_empty() {
        let item = TimelineItem::new("i".into(), TimelineItemKind::ReadMarker);
        assert_eq!(item.id, "i");
        assert!(item.read_by.is_empty());
        let d = MessageDetails::default();
        assert!(d.content.blocks.is_empty());
        assert_eq!(d.kind, MessageKind::Text);
        assert!(!d.sender_name_ambiguous && !d.edited && !d.highlighted);
        assert!(d.sender_name.is_none() && d.reply_to.is_none() && d.send_failure.is_none());
        assert!(d.reactions.is_empty());
        assert_eq!(d.actions, ItemActions::default());
        let a = d.actions;
        assert!(!a.can_reply && !a.can_edit && !a.can_redact && !a.can_react && !a.can_copy);
    }

    const SENTINELS: [&str; 4] = [
        "secret-body-x",
        "secret-key-x",
        "secret-url-x",
        "secret-reason-x",
    ];

    fn assert_clean(dbg: &str) {
        for s in SENTINELS {
            assert!(!dbg.contains(s), "{s} leaked in {dbg}");
        }
    }

    fn rich() -> RichText {
        RichText {
            blocks: vec![
                Block::Paragraph(vec![
                    Inline::Text {
                        text: "secret-body-x".into(),
                        style: TextStyle::default(),
                    },
                    Inline::Link {
                        text: "secret-body-x".into(),
                        url: "https://secret-url-x".into(),
                        style: TextStyle::default(),
                    },
                    Inline::Mention {
                        target: MentionTarget::User(UserId::from("@a:x")),
                        label: "secret-body-x".into(),
                    },
                    Inline::Spoiler(vec![Inline::Text {
                        text: "secret-body-x".into(),
                        style: TextStyle::default(),
                    }]),
                ]),
                Block::Code {
                    text: "secret-body-x".into(),
                },
                Block::Quote(vec![Block::Heading {
                    level: 1,
                    inlines: vec![Inline::LineBreak],
                }]),
            ],
        }
    }

    fn preview_reply() -> ReplyPreview {
        ReplyPreview {
            event_id: EventId::from("$e"),
            sender: UserId::from("@a:x"),
            sender_name: Some("Ann".into()),
            body: "secret-body-x".into(),
            state: ReplyState::Ready,
        }
    }

    fn reaction() -> ReactionGroup {
        ReactionGroup {
            key: "secret-key-x".into(),
            count: 2,
            own: true,
            senders: vec![UserId::from("@a:x")],
        }
    }

    fn outgoing() -> OutgoingMessage {
        OutgoingMessage {
            room_id: RoomId::from("!r:x"),
            text: MessageText::from("secret-body-x"),
            mentions: vec![MentionRef {
                label: "secret-body-x".into(),
                user_id: UserId::from("@a:x"),
            }],
            reply_to: Some("item-1".into()),
            kind: MessageKind::Text,
            format: TextFormat::Markdown,
            thread_root: None,
        }
    }

    fn rich_item() -> TimelineItem {
        let details = MessageDetails {
            content: rich(),
            reply_to: Some(preview_reply()),
            reactions: vec![reaction()],
            ..MessageDetails::default()
        };
        TimelineItem::new(
            "i".into(),
            TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: "secret-body-x".into(),
                is_own: false,
                timestamp_ms: 7,
                sent: SendStatus::Sent,
                details: Box::new(details),
            },
        )
    }

    #[test]
    fn rich_text_debug_shows_counts_only() {
        let dbg = format!("{:?}", rich());
        assert_clean(&dbg);
        assert_eq!(dbg, "RichText(<4 blocks, 6 inlines>)");
        for block in &rich().blocks {
            assert_clean(&format!("{block:?}"));
        }
    }

    #[test]
    fn rich_message_item_debug_is_clean() {
        let item = rich_item();
        assert_clean(&format!("{item:?}"));
        let ev = CoreEvent::TimelineDiff {
            room_id: RoomId::from("!r:x"),
            diffs: vec![
                ListDiff::Reset {
                    values: vec![item.clone()],
                },
                ListDiff::PushBack { value: item },
            ],
        };
        assert_clean(&format!("{ev:?}"));
    }

    #[test]
    fn reply_reaction_and_outgoing_debug_are_clean() {
        let dbg = format!("{:?}", preview_reply());
        assert_clean(&dbg);
        assert!(dbg.contains("@a:x"), "{dbg}");
        let dbg = format!("{:?}", reaction());
        assert_clean(&dbg);
        assert!(dbg.contains('2'), "{dbg}");
        assert_clean(&format!("{:?}", outgoing()));
        assert_clean(&format!(
            "{:?}",
            MentionRef {
                label: "secret-body-x".into(),
                user_id: UserId::from("@a:x")
            }
        ));
    }

    #[test]
    fn members_event_debug_hides_the_query() {
        let ev = CoreEvent::Members {
            room_id: RoomId::from("!r:x"),
            query: MessageText::from("secret-body-x"),
            members: vec![Member {
                user_id: UserId::from("@a:x"),
                display_name: None,
            }],
        };
        let dbg = format!("{ev:?}");
        assert_clean(&dbg);
        assert!(dbg.contains("@a:x") && dbg.contains("!r:x"), "{dbg}");
    }

    #[test]
    fn new_commands_debug_is_clean() {
        let cmds = [
            Command::Send(outgoing()),
            Command::Edit {
                item_id: "i".into(),
                message: outgoing(),
            },
            Command::Redact {
                room_id: RoomId::from("!r:x"),
                item_id: "i".into(),
                reason: Some(MessageText::from("secret-reason-x")),
            },
            Command::ToggleReaction {
                room_id: RoomId::from("!r:x"),
                item_id: "i".into(),
                key: "secret-key-x".into(),
            },
            Command::SearchMembers {
                room_id: RoomId::from("!r:x"),
                query: "secret-body-x".into(),
            },
        ];
        for cmd in cmds {
            assert_clean(&format!("{cmd:?}"));
        }
        let dbg = format!(
            "{:?}",
            Command::Redact {
                room_id: RoomId::from("!r:x"),
                item_id: "item-9".into(),
                reason: None,
            }
        );
        assert!(dbg.contains("item-9") && dbg.contains("!r:x"), "{dbg}");
    }

    #[test]
    fn plain_new_commands_are_constructible() {
        let r = RoomId::from("!r:x");
        let cmds = [
            Command::RetrySend {
                room_id: r.clone(),
                item_id: "i".into(),
            },
            Command::DiscardSend {
                room_id: r.clone(),
                item_id: "i".into(),
            },
            Command::PaginateBackwards { room_id: r.clone() },
            Command::MarkRead {
                room_id: r.clone(),
                fully_read: true,
            },
            Command::MoveReadMarker { room_id: r.clone() },
            Command::SetTyping {
                room_id: r,
                typing: false,
            },
        ];
        assert_eq!(cmds.len(), 6);
    }

    #[test]
    fn new_events_are_constructible_and_comparable() {
        let r = RoomId::from("!r:x");
        let _ = CoreEvent::Pagination {
            room_id: r.clone(),
            state: PaginationState::Loading,
        };
        let _ = CoreEvent::Typing {
            room_id: r.clone(),
            users: vec![TypingUser {
                user_id: UserId::from("@a:x"),
                name: None,
            }],
        };
        let _ = CoreEvent::Members {
            room_id: r.clone(),
            query: MessageText::from(""),
            members: vec![Member {
                user_id: UserId::from("@a:x"),
                display_name: None,
            }],
        };
        let _ = CoreEvent::ActionFailed {
            room_id: r,
            action: ActionKind::Send,
            error: CoreError::ItemNotFound,
        };
        assert_ne!(CoreError::ItemNotFound, CoreError::NotAllowed);
        assert_ne!(PaginationState::Idle, PaginationState::ReachedStart);
        assert_eq!(
            TimelineItemKind::DateDivider { timestamp_ms: 1 },
            TimelineItemKind::DateDivider { timestamp_ms: 1 }
        );
    }

    const FILE_SENTINEL: &str = "secret-name-x.png";

    fn assert_no_phase5_secret(dbg: &str) {
        for s in [
            FILE_SENTINEL,
            "secret-body-x",
            "secret-name-x",
            "secret-key-x",
        ] {
            assert!(!dbg.contains(s), "{s} leaked in {dbg}");
        }
    }

    fn media_content() -> MediaContent {
        MediaContent::new(MediaKind::Image, FILE_SENTINEL.into())
    }

    fn attachment() -> OutgoingAttachment {
        OutgoingAttachment {
            room_id: RoomId::from("!r:x"),
            filename: FILE_SENTINEL.into(),
            mimetype: "image/png".into(),
            data: AttachmentData::Path(PathBuf::from("/home/secret-name-x/pic.png")),
            kind: MediaKind::Image,
            width: Some(2),
            height: Some(3),
            thumbnail: Some(UploadThumbnail {
                data: MediaBytes::from(b"secret-body-x".to_vec()),
                mimetype: "image/jpeg".into(),
                width: 1,
                height: 1,
            }),
            reply_to: None,
            thread_root: None,
        }
    }

    #[test]
    fn media_bytes_compare_by_content_and_debug_shows_the_length() {
        let a = MediaBytes::from(b"secret-body-x".to_vec());
        assert_eq!(a, MediaBytes::from(b"secret-body-x".to_vec()));
        assert_ne!(a, MediaBytes::from(vec![1]));
        assert_eq!(a.clone().as_bytes(), a.as_bytes());
        let dbg = format!("{a:?}");
        assert_eq!(dbg, "MediaBytes(<13 bytes>)");
        let ev = CoreEvent::Media {
            room_id: RoomId::from("!r:x"),
            item_id: "i".into(),
            size: MediaSize::Full,
            result: Ok(a),
        };
        assert_no_phase5_secret(&format!("{ev:?}"));
    }

    #[test]
    fn media_attachment_and_command_debug_hide_names_paths_and_bytes() {
        let dbg = format!("{:?}", media_content());
        assert_no_phase5_secret(&dbg);
        assert!(dbg.contains("Image"), "{dbg}");
        let details = MessageDetails {
            media: Some(Box::new(media_content())),
            ..MessageDetails::default()
        };
        let item = TimelineItem::new(
            "i".into(),
            TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: FILE_SENTINEL.into(),
                is_own: false,
                timestamp_ms: 1,
                sent: SendStatus::Sent,
                details: Box::new(details),
            },
        );
        assert_no_phase5_secret(&format!("{item:?}"));
        assert_no_phase5_secret(&format!("{:?}", attachment()));
        assert_no_phase5_secret(&format!("{:?}", Command::SendAttachment(attachment())));
        let save = Command::SaveMedia {
            room_id: RoomId::from("!r:x"),
            item_id: "i".into(),
            path: PathBuf::from("/home/secret-name-x/secret-name-x.png"),
        };
        let dbg = format!("{save:?}");
        assert_no_phase5_secret(&dbg);
        assert!(dbg.contains("!r:x"), "{dbg}");
    }

    #[test]
    fn notification_keyword_and_profile_debug_are_clean() {
        let view = NotificationView {
            room_id: RoomId::from("!r:x"),
            event_id: EventId::from("$e"),
            room_name: "secret-name-x".into(),
            sender: UserId::from("@a:x"),
            sender_name: Some("secret-name-x".into()),
            is_dm: false,
            kind: NotificationKind::Mention,
            preview: Some(MessageText::from("secret-body-x")),
        };
        let dbg = format!("{view:?}");
        assert_no_phase5_secret(&dbg);
        assert!(dbg.contains("@a:x") && dbg.contains("Mention"), "{dbg}");
        assert_no_phase5_secret(&format!("{:?}", CoreEvent::Notification(view)));
        let settings = NotificationSettingsView {
            group_default: RoomNotificationModeView::AllMessages,
            dm_default: RoomNotificationModeView::Mute,
            keywords: vec!["secret-name-x".into()],
        };
        assert_no_phase5_secret(&format!("{:?}", CoreEvent::NotificationSettings(settings)));
        for cmd in [
            Command::AddKeyword {
                keyword: "secret-name-x".into(),
            },
            Command::RemoveKeyword {
                keyword: "secret-name-x".into(),
            },
            Command::SetDisplayName {
                name: "secret-name-x".into(),
            },
            Command::SetAvatar {
                data: MediaBytes::from(b"secret-body-x".to_vec()),
                mimetype: "image/png".into(),
            },
            Command::SetRoomName {
                room_id: RoomId::from("!r:x"),
                name: "secret-name-x".into(),
            },
            Command::SetRoomTopic {
                room_id: RoomId::from("!r:x"),
                topic: Some("secret-name-x".into()),
            },
            Command::JoinRoom {
                target: "#secret-name-x:localhost".into(),
                via: Vec::new(),
            },
            Command::CreateRoom(CreateRoomRequest {
                name: "secret-name-x".into(),
                topic: Some("secret-name-x".into()),
                encrypted: true,
                visibility: RoomVisibility::Public {
                    alias: "secret-name-x".into(),
                },
                invitees: Vec::new(),
            }),
        ] {
            assert_no_phase5_secret(&format!("{cmd:?}"));
        }
        let events = [
            CoreEvent::OwnProfile(OwnProfileView {
                display_name: Some("secret-name-x".into()),
                avatar: None,
            }),
            CoreEvent::RoomDetails(RoomDetailsView {
                room_id: RoomId::from("!r:x"),
                name: "secret-name-x".into(),
                topic: Some("secret-name-x".into()),
                avatar: None,
                is_encrypted: true,
                alias: Some("secret-name-x".into()),
                permissions: RoomPermissions::default(),
            }),
            CoreEvent::RoomMembers {
                room_id: RoomId::from("!r:x"),
                members: vec![RoomMemberView {
                    user_id: UserId::from("@a:x"),
                    display_name: Some("secret-name-x".into()),
                    avatar: None,
                    power: PowerLevelView::Level(50),
                    role: Role::Moderator,
                    membership: MembershipView::Joined,
                }],
                truncated: false,
            },
            CoreEvent::Spaces(vec![SpaceView {
                id: RoomId::from("!s:x"),
                name: "secret-name-x".into(),
                avatar: None,
                level: 0,
                children: 3,
            }]),
        ];
        for ev in events {
            assert_no_phase5_secret(&format!("{ev:?}"));
        }
        let thread = ThreadSummaryView {
            replies: 2,
            latest_sender: Some(UserId::from("@a:x")),
            latest_sender_name: Some("secret-name-x".into()),
            latest_preview: Some("secret-body-x".into()),
        };
        let details = MessageDetails {
            thread: Some(thread),
            ..MessageDetails::default()
        };
        assert_no_phase5_secret(&format!("{details:?}"));
        assert_no_phase5_secret(&format!("{:?}", UiSettings::default()));
    }

    #[test]
    fn existing_constructors_give_phase5_fields_their_defaults() {
        let r = RoomSummary::new(RoomId::from("!r:x"), "Lounge".into());
        assert!(r.notification_mode.is_none());
        assert!(RoomListQuery::default().space.is_none());
        let d = MessageDetails::default();
        assert!(d.media.is_none() && d.thread.is_none());
        assert!(outgoing().thread_root.is_none());
        let s = UiSettings::default();
        assert_eq!(s.theme, ThemeChoice::System);
        assert!(s.desktop_notifications && s.show_message_content && s.tray_badge);
        assert_eq!(
            MediaContent::new(MediaKind::File, "f".into()),
            MediaContent::new(MediaKind::File, "f".into())
        );
    }

    #[test]
    fn login_command_debug_hides_password() {
        let cmd = Command::LoginPassword {
            username: "alice".into(),
            password: Password::from("hunter2-x"),
            server: None,
        };
        let dbg = format!("{cmd:?}");
        assert!(!dbg.contains("hunter2-x"), "{dbg}");
        assert!(dbg.contains("alice"));
    }
}
