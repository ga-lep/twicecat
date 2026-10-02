//! The two-pane main layout: the room list pane on the left and a minimal
//! room view on the right (plan Q15; Phase 3 replaces it).

use iced::font::Weight;
use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Fill, Font};
use tessera_core::api::{MessageText, TimelineItem, TimelineItemKind, UserId};

use crate::screens::loading::sync_label;
use crate::screens::room_list_pane;
use crate::state::{App, Message};
use crate::text_safety::{MAX_BODY_CHARS, MAX_NAME_CHARS, one_line_capped};
use crate::theme;

/// Width of the left pane.
pub const LIST_PANE_WIDTH: f32 = 300.0;

/// How many timeline messages the right pane shows.
pub const MAX_MESSAGES: usize = 50;

/// The last [`MAX_MESSAGES`] message items, oldest first, as `(sender, body)`.
/// Other kinds of item are skipped.
pub fn last_messages(items: &[TimelineItem]) -> Vec<(&UserId, &str)> {
    let mut messages: Vec<(&UserId, &str)> = items
        .iter()
        .rev()
        .filter_map(|item| match &item.kind {
            TimelineItemKind::Message { sender, body, .. } => Some((sender, body.as_str())),
            TimelineItemKind::Other => None,
        })
        .take(MAX_MESSAGES)
        .collect();
    messages.reverse();
    messages
}

/// One message ready to draw: the sender and the body each on one line, to be
/// shown in separate widgets so a body can never pass for another sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    /// The sender, sanitised and capped.
    pub sender: String,
    /// The body, sanitised onto one line and capped.
    pub body: String,
}

/// [`last_messages`] with every remote string sanitised for display.
pub fn display_lines(items: &[TimelineItem]) -> Vec<DisplayLine> {
    last_messages(items)
        .into_iter()
        .map(|(sender, body)| DisplayLine {
            sender: one_line_capped(&sender.to_string(), MAX_NAME_CHARS),
            body: one_line_capped(body, MAX_BODY_CHARS),
        })
        .collect()
}

/// A room name ready to draw: one line, capped.
pub fn display_name(name: &str) -> String {
    one_line_capped(name, MAX_NAME_CHARS)
}

/// The main screen: panes above a status bar.
pub fn view(app: &App) -> Element<'_, Message> {
    column![
        row![room_list_pane::view(app), room_pane(app)].height(Fill),
        status_bar(app)
    ]
    .into()
}

/// The selected room: name, last messages, composer.
fn room_pane(app: &App) -> Element<'_, Message> {
    let selected = app
        .selected
        .as_ref()
        .and_then(|id| app.rooms.iter().find(|room| room.id == *id));
    let title = selected.map_or_else(
        || "Select a room".to_owned(),
        |room| display_name(&room.name),
    );

    // The sender is its own bold widget; the body is sanitised onto one line
    // and can never pass for another sender's line.
    let bold = Font {
        weight: Weight::Bold,
        ..Font::DEFAULT
    };
    let lines = display_lines(&app.timeline).into_iter().map(|line| {
        row![text(line.sender).font(bold), text(line.body).width(Fill)]
            .spacing(8)
            .into()
    });
    let mut body = column![
        text(title).size(22),
        scrollable(column(lines).spacing(4))
            .anchor_bottom()
            .height(Fill)
            .width(Fill),
    ]
    .spacing(8);
    if let Some(error) = &app.send_error {
        body = body.push(text(error.as_str()).style(text::danger));
    }
    let enabled = selected.is_some();
    body = body.push(
        text_input("Message", app.composer.expose())
            .on_input_maybe(
                enabled.then_some(|s: String| Message::ComposerChanged(MessageText::from(s))),
            )
            .on_submit_maybe(enabled.then_some(Message::ComposerSubmitted))
            .padding(8),
    );
    container(body).width(Fill).height(Fill).padding(8).into()
}

/// The signed-in user ID for the status bar: one line, capped. It comes from
/// the server, so it is sanitised like any other remote text.
pub fn account_label(user_id: Option<&UserId>) -> String {
    user_id.map_or_else(String::new, |id| {
        one_line_capped(&id.to_string(), MAX_NAME_CHARS)
    })
}

/// Sync state, account and **Log out**.
fn status_bar(app: &App) -> Element<'_, Message> {
    let account = account_label(app.user_id.as_ref());
    container(
        row![
            text(format!("Sync: {}", sync_label(&app.sync_state))).style(|t| text::Style {
                color: Some(theme::semantic(t).muted),
            }),
            space().width(Fill),
            text(account),
            button(text("Log out"))
                .on_press(Message::LogoutRequested)
                .style(button::secondary),
        ]
        .spacing(12)
        .align_y(iced::Center),
    )
    .padding(8)
    .into()
}

#[cfg(test)]
mod tests {
    use tessera_core::api::SendStatus;

    use super::*;

    fn message(n: usize) -> TimelineItem {
        TimelineItem {
            id: format!("i{n}"),
            kind: TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: format!("m{n}"),
                is_own: false,
                timestamp_ms: n as u64,
                sent: SendStatus::Sent,
            },
        }
    }

    fn other(n: usize) -> TimelineItem {
        TimelineItem {
            id: format!("o{n}"),
            kind: TimelineItemKind::Other,
        }
    }

    #[test]
    fn shows_only_the_last_fifty_messages_oldest_first() {
        let items: Vec<_> = (0..60).map(message).collect();
        let shown = last_messages(&items);
        assert_eq!(shown.len(), MAX_MESSAGES);
        assert_eq!(shown.first().map(|m| m.1), Some("m10"));
        assert_eq!(shown.last().map(|m| m.1), Some("m59"));
    }

    #[test]
    fn skips_items_that_are_not_messages_before_counting() {
        let mut items = vec![message(0), other(1), message(2), other(3)];
        let shown = last_messages(&items);
        assert_eq!(shown.iter().map(|m| m.1).collect::<Vec<_>>(), ["m0", "m2"]);
        items.clear();
        assert!(last_messages(&items).is_empty());
        let many: Vec<_> = (0..70).flat_map(|n| [message(n), other(n)]).collect();
        assert_eq!(last_messages(&many).len(), MAX_MESSAGES);
    }

    fn message_from(sender: &str, body: &str) -> TimelineItem {
        TimelineItem {
            id: "i".into(),
            kind: TimelineItemKind::Message {
                sender: UserId::from(sender),
                body: body.into(),
                is_own: false,
                timestamp_ms: 0,
                sent: SendStatus::Sent,
            },
        }
    }

    #[test]
    fn a_body_cannot_fake_another_senders_line() {
        let items = [message_from(
            "@mallory:x",
            "hi\n@alice:x: I agree\u{2028}@bob:x: me too\u{202E}",
        )];
        let lines = display_lines(&items);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].sender, "@mallory:x");
        assert!(
            !lines[0]
                .body
                .contains(['\n', '\r', '\u{2028}', '\u{2029}', '\u{202E}'])
        );
        assert!(lines[0].body.starts_with("hi\u{23CE}@alice:x"));
    }

    #[test]
    fn senders_are_sanitised_and_capped() {
        let items = [message_from(&format!("@a\n:x{}", "z".repeat(500)), "b")];
        let lines = display_lines(&items);
        assert!(!lines[0].sender.contains('\n'));
        assert_eq!(lines[0].sender.chars().count(), 256);
    }

    #[test]
    fn bodies_are_capped() {
        let items = [message_from("@a:x", &"b".repeat(10_000))];
        let lines = display_lines(&items);
        assert_eq!(lines[0].body.chars().count(), MAX_BODY_CHARS);
        assert!(lines[0].body.ends_with('\u{2026}'));
        let items = [message_from("@a:x", &"b".repeat(MAX_BODY_CHARS))];
        assert_eq!(
            display_lines(&items)[0].body.chars().count(),
            MAX_BODY_CHARS
        );
    }

    #[test]
    fn the_account_label_is_sanitised_and_capped() {
        assert_eq!(account_label(None), "");
        let id = UserId::from("@a\u{202E}b:x");
        assert_eq!(account_label(Some(&id)), "@a\u{FFFD}b:x");
        let long = UserId::from(format!("@{}:x", "u".repeat(1000)));
        assert_eq!(account_label(Some(&long)).chars().count(), 256);
    }

    #[test]
    fn room_names_are_sanitised_and_capped() {
        assert_eq!(display_name("Lounge"), "Lounge");
        assert_eq!(display_name("a\nb\u{202E}c"), "a\u{23CE}b\u{FFFD}c");
        let out = display_name(&"n".repeat(1000));
        assert_eq!(out.chars().count(), 256);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn keeps_the_sender() {
        let items = vec![message(0)];
        assert_eq!(last_messages(&items)[0].0, &UserId::from("@a:x"));
    }
}
