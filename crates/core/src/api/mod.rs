//! The commands-in / events-out API of the core.
//!
//! Everything here is SDK-free: no type of the Matrix SDK or its helper crates
//! appears in it (enforced by `scripts/check-core-api.sh`).

mod diff;
mod types;

pub use diff::ListDiff;
pub use types::{
    AuthUrl, AvatarId, AvatarImage, CoreError, DeviceId, MessagePreview, MessageText, Password,
    RoomId, RoomListQuery, RoomListTab, RoomSummary, SendStatus, SyncState, TimelineItem,
    TimelineItemKind, UserId,
};

/// A request from the front end to the engine.
#[derive(Clone, Debug)]
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
    /// Log out and delete the local session.
    Logout,
    /// Stop the engine.
    Shutdown,
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
        let item = TimelineItem {
            id: "i".into(),
            kind: TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: "secret-body-x".into(),
                is_own: false,
                timestamp_ms: 1,
                sent: SendStatus::Sent,
            },
        };
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
