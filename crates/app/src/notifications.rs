//! Desktop notifications (plan Q10, Q11).
//!
//! The core sends every notification the push rules produce, always with a
//! short preview. This module decides whether to show one, folds bursts into
//! one notification per room, builds the text that reaches the operating
//! system, and hands it to a [`Notifier`]. The decisions are pure functions so
//! tests need no desktop.
//!
//! Everything that reaches the OS is sanitised: names and previews go through
//! [`crate::text_safety`], user IDs are cleaned the same way, and the body is
//! escaped for the markup a notification server may interpret. With "Show
//! message content" off the preview is dropped before a [`Toast`] is built, so
//! it never leaves the app.

use std::collections::HashMap;
use std::convert::identity;
use std::fmt;
use std::time::{Duration, Instant};

use iced::Task;
use iced::futures::stream;
use tokio::sync::mpsc;
use twicecat_core::api::{
    NotificationKind, NotificationView, RoomId, RoomNotificationModeView, UiSettings,
};

use crate::state::Message;
use crate::text_safety::one_line_capped;

/// Notifications for one room within this long of the first one are folded
/// into that notification (plan Q10).
pub const FOLD_WINDOW: Duration = Duration::from_secs(3);

/// How many rooms the folding state remembers.
pub const MAX_FOLDED_ROOMS: usize = 64;

/// Longest title (room or sender name), in characters.
pub const MAX_TITLE_CHARS: usize = 80;

/// Longest sender name inside a body, in characters.
pub const MAX_SENDER_CHARS: usize = 48;

/// Longest preview, in characters. The core already caps it at 120.
pub const MAX_PREVIEW_CHARS: usize = 120;

/// Body without message content.
pub const BODY_NEW_MESSAGE: &str = "New message";
/// Body of a mention without message content.
pub const BODY_MENTIONED: &str = "Mentioned you";
/// Body of an invitation without message content.
pub const BODY_INVITED: &str = "Invited you";
/// Body of a ringing call without message content.
pub const BODY_CALLING: &str = "Incoming call";
/// Body of a group call's quiet notification without message content.
pub const BODY_CALL_STARTED: &str = "A call started";

/// Whether a notification may be shown now: desktop notifications are on and
/// the window is **not** focused. A focused window never shows one, which
/// covers the selected room too; an unfocused window shows the selected
/// room's as well, since nobody is looking at it.
pub fn should_show(window_focused: bool, settings: &UiSettings) -> bool {
    settings.desktop_notifications && !window_focused
}

/// How the next notification of a room is delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// A new notification.
    New,
    /// Update the room's notification: `count` messages so far. `replace` is
    /// the OS ID of the shown notification, when it is known yet.
    Fold {
        /// Messages in the window, this one included (at least 2).
        count: u32,
        /// The notification to replace.
        replace: Option<u32>,
    },
}

#[derive(Debug)]
struct Fold {
    started: Instant,
    count: u32,
    shown: Option<u32>,
}

/// Per-room folding of bursts (plan Q10): at most one new notification per
/// room every [`FOLD_WINDOW`].
#[derive(Debug, Default)]
pub struct Folding {
    rooms: HashMap<RoomId, Fold>,
}

impl Folding {
    /// Records a notification for `room` at `now` and says how to deliver it.
    pub fn admit(&mut self, room: &RoomId, now: Instant) -> Delivery {
        if let Some(fold) = self.rooms.get_mut(room)
            && now.saturating_duration_since(fold.started) < FOLD_WINDOW
        {
            fold.count = fold.count.saturating_add(1);
            return Delivery::Fold {
                count: fold.count,
                replace: fold.shown,
            };
        }
        if self.rooms.len() >= MAX_FOLDED_ROOMS && !self.rooms.contains_key(room) {
            self.rooms
                .retain(|_, fold| now.saturating_duration_since(fold.started) < FOLD_WINDOW);
            if self.rooms.len() >= MAX_FOLDED_ROOMS {
                self.rooms.clear();
            }
        }
        self.rooms.insert(
            room.clone(),
            Fold {
                started: now,
                count: 1,
                shown: None,
            },
        );
        Delivery::New
    }

    /// The OS gave the room's current notification this ID.
    pub fn shown(&mut self, room: &RoomId, id: u32) {
        if let Some(fold) = self.rooms.get_mut(room) {
            fold.shown = Some(id);
        }
    }

    /// Forgets everything (a new session).
    pub fn clear(&mut self) {
        self.rooms.clear();
    }
}

/// The text of one notification. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct Toast {
    /// One plain line.
    pub title: String,
    /// One line, escaped for notification markup.
    pub body: String,
}

impl fmt::Debug for Toast {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Toast(<redacted>)")
    }
}

/// Escapes `&`, `<` and `>`: a notification server may read the body as
/// markup (links, images), and the text is someone else's.
pub fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// `text` as one capped line, or `None` when nothing visible is left.
fn visible_line(text: &str, max_chars: usize) -> Option<String> {
    let clean = one_line_capped(text, max_chars);
    let blank = clean
        .chars()
        .all(|c| c.is_whitespace() || c == char::REPLACEMENT_CHARACTER);
    (!blank).then_some(clean)
}

/// The sender as shown: the display name, else the user ID, sanitised.
fn sender_label(view: &NotificationView, max_chars: usize) -> String {
    view.sender_name
        .as_deref()
        .and_then(|name| visible_line(name, max_chars))
        .or_else(|| visible_line(view.sender.as_str(), max_chars))
        .unwrap_or_else(|| "Someone".to_owned())
}

/// Builds the notification text. `show_content` is the "Show message content"
/// setting: when off, neither the preview nor the sender is used in the body.
/// `folded` is the number of messages when the notification stands for a
/// burst (2 or more).
///
/// The title is the room's name; a direct message uses the sender's name.
pub fn build_toast(view: &NotificationView, show_content: bool, folded: Option<u32>) -> Toast {
    let title = if view.is_dm && show_content {
        sender_label(view, MAX_TITLE_CHARS)
    } else {
        visible_line(&view.room_name, MAX_TITLE_CHARS)
            .unwrap_or_else(|| crate::text_safety::NAME_PLACEHOLDER.to_owned())
    };
    let body = match folded {
        Some(count) if count >= 2 => format!("{count} new messages"),
        _ if !show_content => generic_body(view.kind).to_owned(),
        _ => content_body(view),
    };
    Toast {
        title,
        body: escape_markup(&body),
    }
}

/// The notification of an incoming call (`IncomingCall`): the room's name
/// as the title; the caller in the body only with "Show message content" on,
/// as for messages.
pub fn call_toast(
    room_name: Option<&str>,
    caller: &twicecat_core::api::UserId,
    ring: bool,
    show_content: bool,
) -> Toast {
    let title = room_name
        .and_then(|name| visible_line(name, MAX_TITLE_CHARS))
        .unwrap_or_else(|| crate::text_safety::NAME_PLACEHOLDER.to_owned());
    let body = match (show_content, ring) {
        (false, true) => BODY_CALLING.to_owned(),
        (false, false) => BODY_CALL_STARTED.to_owned(),
        (true, ring) => {
            let caller = visible_line(caller.as_str(), MAX_SENDER_CHARS)
                .unwrap_or_else(|| "Someone".to_owned());
            if ring {
                format!("{caller} is calling you")
            } else {
                format!("{caller} started a call")
            }
        }
    };
    Toast {
        title,
        body: escape_markup(&body),
    }
}

fn generic_body(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Mention => BODY_MENTIONED,
        NotificationKind::Invite => BODY_INVITED,
        NotificationKind::Message | NotificationKind::Encrypted => BODY_NEW_MESSAGE,
    }
}

fn content_body(view: &NotificationView) -> String {
    let sender = sender_label(view, MAX_SENDER_CHARS);
    match view.kind {
        NotificationKind::Invite => format!("{sender} invited you"),
        _ => {
            let preview = view
                .preview
                .as_ref()
                .and_then(|p| visible_line(p.expose(), MAX_PREVIEW_CHARS));
            match (preview, view.is_dm) {
                (Some(preview), true) => preview,
                (Some(preview), false) => format!("{sender}: {preview}"),
                (None, _) => generic_body(view.kind).to_owned(),
            }
        }
    }
}

/// What to hand to the operating system.
#[derive(Clone, Debug)]
pub struct Request {
    /// The room a click opens.
    pub room_id: RoomId,
    /// The text.
    pub toast: Toast,
    /// The shown notification to update instead of adding one.
    pub replace: Option<u32>,
}

/// Shows desktop notifications. The real one talks to the desktop's
/// notification service from a worker thread; tests use a fake.
pub trait Notifier: Send {
    /// A task that shows the notification. It yields
    /// [`Message::NotificationShown`] with the OS ID, and (on Linux) a
    /// [`Message::OpenRoomFromNotification`] when the user clicks it.
    fn show(&self, request: Request) -> Task<Message>;
}

/// A notifier that shows nothing: the default until the real one is
/// installed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoNotifier;

impl Notifier for NoNotifier {
    fn show(&self, _request: Request) -> Task<Message> {
        Task::none()
    }
}

/// The desktop's notification service through `notify-rust`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemNotifier;

/// Most notifications whose click is awaited at once; each waits on its own
/// thread until the notification is closed.
#[cfg(all(unix, not(target_os = "macos")))]
const MAX_WAITING: usize = 16;

#[cfg(all(unix, not(target_os = "macos")))]
static WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

impl Notifier for SystemNotifier {
    fn show(&self, request: Request) -> Task<Message> {
        let (tx, rx) = mpsc::unbounded_channel();
        // If the thread cannot start, `tx` is dropped with the closure and
        // the stream below ends at once.
        let _ = std::thread::Builder::new()
            .name("notification".into())
            .spawn(move || deliver(request, &tx));
        Task::run(
            stream::unfold(rx, |mut rx| async move {
                let message = rx.recv().await?;
                Some((message, rx))
            }),
            identity,
        )
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn deliver(request: Request, tx: &mpsc::UnboundedSender<Message>) {
    use std::sync::atomic::Ordering;

    let Request {
        room_id,
        toast,
        replace,
    } = request;
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("Twicecat")
        .summary(&toast.title)
        .body(&toast.body)
        .action("default", "Open");
    if let Some(id) = replace {
        notification.id(id);
    }
    // No service, or a refusal: nothing to tell the user about.
    let Ok(handle) = notification.show() else {
        return;
    };
    let _ = tx.send(Message::NotificationShown {
        room_id: room_id.clone(),
        id: handle.id(),
    });
    if WAITING.fetch_add(1, Ordering::AcqRel) >= MAX_WAITING {
        WAITING.fetch_sub(1, Ordering::AcqRel);
        return;
    }
    handle.wait_for_action(|action| {
        if action == "default" {
            let _ = tx.send(Message::OpenRoomFromNotification(room_id));
        }
    });
    WAITING.fetch_sub(1, Ordering::AcqRel);
}

/// Windows and macOS: show only; a click raises the OS default behaviour
/// (plan Q10, not tested).
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn deliver(request: Request, _tx: &mpsc::UnboundedSender<Message>) {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname("Twicecat")
        .summary(&request.toast.title)
        .body(&request.toast.body);
    let _ = notification.show();
}

/// The choices of the room header's notification menu (plan Q11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeChoice {
    /// The account's default for this kind of room.
    Default,
    /// Every message notifies.
    AllMessages,
    /// Only mentions and keywords notify.
    MentionsAndKeywords,
    /// Nothing notifies.
    Mute,
}

impl ModeChoice {
    /// All choices, in menu order.
    pub const ALL: [Self; 4] = [
        Self::AllMessages,
        Self::MentionsAndKeywords,
        Self::Mute,
        Self::Default,
    ];

    /// The choice that stands for a room's mode (`None` is the default).
    pub fn from_mode(mode: Option<RoomNotificationModeView>) -> Self {
        match mode {
            None => Self::Default,
            Some(RoomNotificationModeView::AllMessages) => Self::AllMessages,
            Some(RoomNotificationModeView::MentionsAndKeywords) => Self::MentionsAndKeywords,
            Some(RoomNotificationModeView::Mute) => Self::Mute,
        }
    }

    /// The mode to send; `None` goes back to the default.
    pub fn mode(self) -> Option<RoomNotificationModeView> {
        match self {
            Self::Default => None,
            Self::AllMessages => Some(RoomNotificationModeView::AllMessages),
            Self::MentionsAndKeywords => Some(RoomNotificationModeView::MentionsAndKeywords),
            Self::Mute => Some(RoomNotificationModeView::Mute),
        }
    }
}

impl fmt::Display for ModeChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "Use default",
            Self::AllMessages => "All messages",
            Self::MentionsAndKeywords => "Mentions and keywords",
            Self::Mute => "Mute",
        })
    }
}

/// The mark a muted room shows in the list.
pub fn muted_mark(mode: Option<RoomNotificationModeView>) -> Option<&'static str> {
    (mode == Some(RoomNotificationModeView::Mute)).then_some("Muted")
}

#[cfg(test)]
pub use fake::RecordingNotifier;

#[cfg(test)]
mod fake {
    use std::sync::{Arc, Mutex};

    use iced::Task;

    use super::{Notifier, Request};
    use crate::state::Message;

    /// A fake that records what would reach the desktop.
    #[derive(Debug, Clone, Default)]
    pub struct RecordingNotifier {
        /// Every request, in order.
        pub shown: Arc<Mutex<Vec<Request>>>,
    }

    impl Notifier for RecordingNotifier {
        fn show(&self, request: Request) -> Task<Message> {
            self.shown.lock().expect("test mutex").push(request);
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn call_toasts_name_the_caller_only_with_content_on() {
        use twicecat_core::api::UserId;
        let caller = UserId::from("@bob<b>:x");
        let shown = super::call_toast(Some("Bob"), &caller, true, true);
        assert_eq!(shown.title, "Bob");
        assert_eq!(shown.body, "@bob&lt;b&gt;:x is calling you");
        let quiet = super::call_toast(Some("Team\nroom"), &caller, false, true);
        assert_eq!(quiet.title, "Team\u{23ce}room");
        assert_eq!(quiet.body, "@bob&lt;b&gt;:x started a call");
        let hidden = super::call_toast(None, &caller, true, false);
        assert_eq!(hidden.title, crate::text_safety::NAME_PLACEHOLDER);
        assert_eq!(hidden.body, super::BODY_CALLING);
        assert_eq!(
            super::call_toast(Some("Team"), &caller, false, false).body,
            super::BODY_CALL_STARTED
        );
    }

    use twicecat_core::api::{EventId, MessageText, UserId};

    use super::*;

    fn view(kind: NotificationKind, is_dm: bool) -> NotificationView {
        NotificationView {
            room_id: RoomId::from("!r:x"),
            event_id: EventId::from("$e"),
            room_name: "Room".into(),
            sender: UserId::from("@alice:x"),
            sender_name: Some("Alice".into()),
            is_dm,
            kind,
            preview: Some(MessageText::from("hello there")),
        }
    }

    fn settings(desktop: bool, content: bool) -> UiSettings {
        UiSettings {
            desktop_notifications: desktop,
            show_message_content: content,
            ..UiSettings::default()
        }
    }

    #[test]
    fn should_show_only_when_unfocused_and_on() {
        // (focused, desktop on, expected)
        for (focused, on, expected) in [
            (false, true, true),
            (true, true, false),
            (false, false, false),
            (true, false, false),
        ] {
            assert_eq!(
                should_show(focused, &settings(on, true)),
                expected,
                "focused={focused} on={on}"
            );
        }
    }

    #[test]
    fn content_on_shows_sender_and_preview() {
        let group = build_toast(&view(NotificationKind::Message, false), true, None);
        assert_eq!(group.title, "Room");
        assert_eq!(group.body, "Alice: hello there");
        let dm = build_toast(&view(NotificationKind::Message, true), true, None);
        assert_eq!(dm.title, "Alice");
        assert_eq!(dm.body, "hello there");
        let invite = build_toast(&view(NotificationKind::Invite, false), true, None);
        assert_eq!(invite.body, "Alice invited you");
    }

    #[test]
    fn content_off_uses_the_fixed_bodies_and_never_the_preview() {
        for (kind, body) in [
            (NotificationKind::Message, BODY_NEW_MESSAGE),
            (NotificationKind::Encrypted, BODY_NEW_MESSAGE),
            (NotificationKind::Mention, BODY_MENTIONED),
            (NotificationKind::Invite, BODY_INVITED),
        ] {
            for is_dm in [false, true] {
                let toast = build_toast(&view(kind, is_dm), false, None);
                assert_eq!(toast.body, body);
                assert_eq!(toast.title, "Room", "the room name, not the sender");
                assert!(!toast.body.contains("hello"));
                assert!(!toast.title.contains("Alice"));
            }
        }
    }

    #[test]
    fn a_missing_preview_falls_back_to_the_fixed_body() {
        let mut v = view(NotificationKind::Mention, false);
        v.preview = None;
        assert_eq!(build_toast(&v, true, None).body, BODY_MENTIONED);
    }

    #[test]
    fn names_and_previews_are_sanitised() {
        let mut v = view(NotificationKind::Message, false);
        v.room_name = "Ro\nom\u{202E}x".into();
        v.sender_name = Some("Al\r\nice".into());
        v.preview = Some(MessageText::from("a\nb <b>bold</b> & <img src=x>"));
        let toast = build_toast(&v, true, None);
        assert!(!toast.title.contains(['\n', '\r', '\u{202E}']));
        assert!(!toast.body.contains(['\n', '\r']));
        assert!(!toast.body.contains('<'));
        assert!(
            toast
                .body
                .contains("&lt;b&gt;bold&lt;/b&gt; &amp; &lt;img src=x&gt;")
        );
    }

    #[test]
    fn a_blank_or_missing_sender_name_falls_back_to_the_cleaned_user_id() {
        let mut v = view(NotificationKind::Message, false);
        v.sender_name = Some("  \u{200B} ".into());
        v.sender = UserId::from("@bo\nb:x");
        let toast = build_toast(&v, true, None);
        assert!(toast.body.starts_with("@bo"));
        assert!(!toast.body.contains('\n'));
        v.sender_name = None;
        assert!(build_toast(&v, true, None).body.starts_with("@bo"));
    }

    #[test]
    fn long_text_is_capped() {
        let mut v = view(NotificationKind::Message, false);
        v.room_name = "r".repeat(500);
        v.sender_name = Some("s".repeat(500));
        v.preview = Some(MessageText::from("p".repeat(500)));
        let toast = build_toast(&v, true, None);
        assert!(toast.title.chars().count() <= MAX_TITLE_CHARS);
        assert!(toast.body.chars().count() <= MAX_SENDER_CHARS + MAX_PREVIEW_CHARS + ": ".len());
    }

    #[test]
    fn a_folded_burst_says_how_many_messages_in_either_mode() {
        for content in [true, false] {
            let toast = build_toast(&view(NotificationKind::Message, false), content, Some(3));
            assert_eq!(toast.body, "3 new messages");
            assert!(!toast.body.contains("hello"));
        }
    }

    #[test]
    fn toast_debug_is_redacted() {
        let toast = build_toast(&view(NotificationKind::Message, false), true, None);
        assert!(!format!("{toast:?}").contains("hello"));
    }

    #[test]
    fn folding_shows_one_per_room_per_window() {
        let room = RoomId::from("!a:x");
        let other = RoomId::from("!b:x");
        let t0 = Instant::now();
        let mut folding = Folding::default();
        assert_eq!(folding.admit(&room, t0), Delivery::New);
        // Before the OS answered, there is nothing to replace yet.
        assert_eq!(
            folding.admit(&room, t0 + Duration::from_secs(1)),
            Delivery::Fold {
                count: 2,
                replace: None
            }
        );
        folding.shown(&room, 41);
        assert_eq!(
            folding.admit(&room, t0 + Duration::from_secs(2)),
            Delivery::Fold {
                count: 3,
                replace: Some(41)
            }
        );
        // Another room has its own window.
        assert_eq!(
            folding.admit(&other, t0 + Duration::from_secs(2)),
            Delivery::New
        );
        // The window runs from the first message, not the last.
        assert_eq!(folding.admit(&room, t0 + FOLD_WINDOW), Delivery::New);
        assert_eq!(
            folding.admit(&room, t0 + FOLD_WINDOW + Duration::from_secs(1)),
            Delivery::Fold {
                count: 2,
                replace: None
            }
        );
    }

    #[test]
    fn folding_state_is_bounded() {
        let t0 = Instant::now();
        let mut folding = Folding::default();
        for i in 0..(MAX_FOLDED_ROOMS * 3) {
            folding.admit(&RoomId::from(format!("!r{i}:x")), t0);
        }
        assert!(folding.rooms.len() <= MAX_FOLDED_ROOMS);
    }

    #[test]
    fn mode_choices_map_to_the_right_modes() {
        assert_eq!(ModeChoice::Default.mode(), None);
        assert_eq!(
            ModeChoice::Mute.mode(),
            Some(RoomNotificationModeView::Mute)
        );
        for choice in ModeChoice::ALL {
            assert_eq!(ModeChoice::from_mode(choice.mode()), choice);
        }
        assert_eq!(
            muted_mark(Some(RoomNotificationModeView::Mute)),
            Some("Muted")
        );
        assert_eq!(muted_mark(None), None);
        assert_eq!(
            muted_mark(Some(RoomNotificationModeView::AllMessages)),
            None
        );
    }
}
