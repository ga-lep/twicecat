//! The two-pane main layout: the room list on the left (Task 10 fills it) and
//! a minimal room view on the right (plan Q15; Phase 3 replaces it).

use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Fill, Length};
use tessera_core::api::{MessageText, TimelineItem, TimelineItemKind, UserId};

use crate::screens::loading::sync_label;
use crate::state::{App, Message};
use crate::theme;

/// Width of the left pane.
pub const LIST_PANE_WIDTH: f32 = 300.0;

/// How many timeline messages the right pane shows.
pub const MAX_MESSAGES: usize = 50;

/// How many rooms the placeholder list shows until Task 10 replaces it with
/// the windowed list.
const PLACEHOLDER_ROWS: usize = 100;

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

/// The main screen: panes above a status bar.
pub fn view(app: &App) -> Element<'_, Message> {
    column![
        row![room_list_pane(app), room_pane(app)].height(Fill),
        status_bar(app)
    ]
    .into()
}

/// Placeholder for the room list: the first rows as plain buttons.
fn room_list_pane(app: &App) -> Element<'_, Message> {
    let rows = app.rooms.iter().take(PLACEHOLDER_ROWS).map(|room| {
        button(text(room.name.as_str()))
            .on_press(Message::SelectRoom(room.id.clone()))
            .style(button::text)
            .width(Fill)
            .into()
    });
    container(scrollable(column(rows)).height(Fill))
        .width(Length::Fixed(LIST_PANE_WIDTH))
        .height(Fill)
        .padding(8)
        .into()
}

/// The selected room: name, last messages, composer.
fn room_pane(app: &App) -> Element<'_, Message> {
    let selected = app
        .selected
        .as_ref()
        .and_then(|id| app.rooms.iter().find(|room| room.id == *id));
    let title = selected.map_or("Select a room", |room| room.name.as_str());

    let lines = last_messages(&app.timeline)
        .into_iter()
        .map(|(sender, body)| text(format!("{sender}: {body}")).into());
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

/// Sync state, account and **Log out**.
fn status_bar(app: &App) -> Element<'_, Message> {
    let account = app
        .user_id
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
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

    #[test]
    fn keeps_the_sender() {
        let items = vec![message(0)];
        assert_eq!(last_messages(&items)[0].0, &UserId::from("@a:x"));
    }
}
