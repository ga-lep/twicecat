//! The commands-in / events-out API of the core.
//!
//! Everything here is SDK-free: no type of the Matrix SDK or its helper crates
//! appears in it (enforced by `scripts/check-core-api.sh`).

use std::fmt;

mod diff;
mod security;
mod types;

pub use diff::ListDiff;
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
    /// Log out and delete the local session.
    Logout,
    /// Stop the engine.
    Shutdown,
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
            Self::Logout => f.write_str("Logout"),
            Self::Shutdown => f.write_str("Shutdown"),
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
    /// Any other error.
    Error(CoreError),
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
