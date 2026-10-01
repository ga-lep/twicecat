//! The commands-in / events-out API of the core.
//!
//! Everything here is SDK-free: no type of the Matrix SDK or its helper crates
//! appears in it (enforced by `scripts/check-core-api.sh`).

mod diff;
mod types;

pub use diff::ListDiff;
pub use types::{
    CoreError, DeviceId, Password, RoomId, RoomSummary, SendStatus, SyncState, TimelineItem,
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
        text: String,
    },
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
    OAuthUrl(String),
    /// The session was ended by logout.
    LoggedOut,
    /// The server rejected the session token.
    SessionExpired,
    /// Sync state changed.
    SyncState(SyncState),
    /// Room list changed.
    RoomListChanged(Vec<ListDiff<RoomSummary>>),
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
