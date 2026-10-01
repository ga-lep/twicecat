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
    },
    /// Anything else (state events, separators, unsupported content).
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
            } => f
                .debug_struct("Message")
                .field("sender", sender)
                .field("body", &format_args!("<redacted, {} bytes>", body.len()))
                .field("is_own", is_own)
                .field("timestamp_ms", timestamp_ms)
                .field("sent", sent)
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
    /// Content.
    pub kind: TimelineItemKind,
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
    /// Unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
}
