//! Message actions: which buttons a message offers, the quote above the
//! composer, and the small state around hover, delete confirmation, the
//! reaction picker and the failure notice (plan Q18, Task 12).

use std::time::Duration;

use twicecat_core::api::{ActionKind, MessageDetails, TimelineItem, TimelineItemKind};

use crate::screens::timeline_pane::sender_label;
use crate::text_safety::one_line_capped;

/// How many characters of the quoted message the reply bar shows.
pub const MAX_QUOTE_CHARS: usize = 120;

/// How many characters of a sender name the reply bar shows.
pub const MAX_QUOTE_NAME_CHARS: usize = 48;

/// Widget ID of the picker's search field, focused when the picker opens.
pub const PICKER_INPUT_ID: &str = "reaction-picker-search";

/// How long a failure notice stays in the room pane.
pub const NOTICE_DURATION: Duration = Duration::from_secs(5);

/// How many characters of a reaction key a chip shows.
pub const MAX_KEY_CHARS: usize = 32;

/// Which buttons the hover bar of a message enables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActionButtons {
    /// **Reply**.
    pub reply: bool,
    /// **Edit** (own messages only).
    pub edit: bool,
    /// **React**.
    pub react: bool,
    /// **Copy**.
    pub copy: bool,
    /// **Delete**. The server still enforces power levels; the core's
    /// `can_redact` is the only source here.
    pub delete: bool,
}

impl ActionButtons {
    /// Whether any button is enabled.
    pub fn any(self) -> bool {
        self.reply || self.edit || self.react || self.copy || self.delete
    }
}

/// The buttons for a message: what the core allows, and **Edit** only on
/// the user's own messages.
pub fn action_buttons(details: &MessageDetails, is_own: bool) -> ActionButtons {
    let actions = details.actions;
    ActionButtons {
        reply: actions.can_reply,
        edit: actions.can_edit && is_own,
        react: actions.can_react,
        copy: actions.can_copy,
        delete: actions.can_redact,
    }
}

/// The first non-blank line of `body`, on one line and capped at
/// [`MAX_QUOTE_CHARS`], for the reply bar.
pub fn quote_line(body: &str) -> String {
    let first = body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    one_line_capped(first, MAX_QUOTE_CHARS)
}

/// The reply bar's text for message `id`: its sender and the first line of
/// its body, both sanitised and capped. `None` when `id` is not a message
/// of `items`.
pub fn quote_for(items: &[TimelineItem], id: &str) -> Option<String> {
    let item = items.iter().find(|item| item.id == id)?;
    let TimelineItemKind::Message {
        sender,
        body,
        details,
        ..
    } = &item.kind
    else {
        return None;
    };
    let name = sender_label(
        details.sender_name.as_deref(),
        details.sender_name_ambiguous,
        sender,
    );
    Some(format!(
        "{}: {}",
        one_line_capped(&name, MAX_QUOTE_NAME_CHARS),
        quote_line(body)
    ))
}

/// A reaction key ready to draw: one line, capped.
pub fn key_label(key: &str) -> String {
    one_line_capped(key, MAX_KEY_CHARS)
}

/// The open reaction picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    /// `TimelineItem::id` of the message the pick reacts to.
    pub target: String,
    /// What the user typed in the search field (not logged: `Debug` of
    /// [`ActionState`] hides it).
    pub query: String,
}

/// A failure line shown for [`NOTICE_DURATION`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Counts notices, so that an old timer cannot clear a newer one.
    pub seq: u64,
    /// One non-secret line.
    pub text: String,
}

/// Hover, delete confirmation, picker and notice of the open room.
#[derive(Default)]
pub struct ActionState {
    /// The message under the pointer.
    pub hovered: Option<String>,
    /// The message whose **Delete** awaits confirmation.
    pub confirm_delete: Option<String>,
    /// The open reaction picker.
    pub picker: Option<Picker>,
    /// The last failure line.
    pub notice: Option<Notice>,
    /// The sequence number of the last notice.
    pub notice_seq: u64,
}

// Manual impl: the picker query is something the user typed.
impl std::fmt::Debug for ActionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionState")
            .field("hovered", &self.hovered)
            .field("confirm_delete", &self.confirm_delete)
            .field("picker", &self.picker.as_ref().map(|p| &p.target))
            .field("notice", &self.notice.is_some())
            .finish_non_exhaustive()
    }
}

impl ActionState {
    /// Forgets everything about the room left. The sequence number is kept,
    /// so timers of the old room stay stale.
    pub fn reset(&mut self) {
        let seq = self.notice_seq;
        *self = Self::default();
        self.notice_seq = seq;
    }
}

/// The notice line for a failed action, or `None` for actions the room pane
/// does not report. `detail` is the already sanitised error text.
pub fn notice_text(action: ActionKind, detail: &str) -> Option<String> {
    let what = match action {
        ActionKind::Redact => "Could not delete the message",
        ActionKind::React => "Could not update the reaction",
        _ => return None,
    };
    Some(format!("{what}: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use twicecat_core::api::ItemActions;

    fn details(actions: ItemActions) -> MessageDetails {
        MessageDetails {
            actions,
            ..MessageDetails::default()
        }
    }

    const ALL: ItemActions = ItemActions {
        can_reply: true,
        can_edit: true,
        can_redact: true,
        can_react: true,
        can_copy: true,
    };

    #[test]
    fn buttons_follow_the_core_and_edit_needs_an_own_message() {
        let own = action_buttons(&details(ALL), true);
        assert!(own.reply && own.edit && own.react && own.copy && own.delete);
        let other = action_buttons(&details(ALL), false);
        assert!(other.reply && !other.edit && other.react && other.copy && other.delete);

        let none = action_buttons(&details(ItemActions::default()), true);
        assert_eq!(none, ActionButtons::default());
        assert!(!none.any());

        let only_copy = action_buttons(
            &details(ItemActions {
                can_copy: true,
                ..ItemActions::default()
            }),
            true,
        );
        assert!(only_copy.copy && !only_copy.reply && !only_copy.delete && only_copy.any());
        // Delete is offered only when the core allows it.
        let no_redact = action_buttons(
            &details(ItemActions {
                can_redact: false,
                ..ALL
            }),
            true,
        );
        assert!(!no_redact.delete);
    }

    #[test]
    fn the_quote_is_the_first_non_blank_line_sanitised_and_capped() {
        assert_eq!(quote_line("hello"), "hello");
        assert_eq!(quote_line("\n  \nfirst\nsecond"), "first");
        assert_eq!(quote_line("a\u{202E}b"), "a\u{FFFD}b");
        assert_eq!(quote_line(""), "");
        let long = quote_line(&"q".repeat(1000));
        assert_eq!(long.chars().count(), MAX_QUOTE_CHARS);
        assert!(long.ends_with('\u{2026}'));
    }

    #[test]
    fn only_redact_and_react_failures_get_a_notice() {
        assert!(
            notice_text(ActionKind::Redact, "no")
                .expect("redact")
                .contains("delete")
        );
        assert!(notice_text(ActionKind::React, "no").is_some());
        assert!(notice_text(ActionKind::Typing, "no").is_none());
        assert!(notice_text(ActionKind::Receipt, "no").is_none());
    }

    #[test]
    fn debug_hides_the_picker_query() {
        let state = ActionState {
            picker: Some(Picker {
                target: "item".into(),
                query: "secret-query-x".into(),
            }),
            ..ActionState::default()
        };
        assert!(!format!("{state:?}").contains("secret-query-x"));
    }

    #[test]
    fn reset_keeps_the_notice_sequence() {
        let mut state = ActionState {
            hovered: Some("a".into()),
            notice_seq: 7,
            ..ActionState::default()
        };
        state.reset();
        assert_eq!(state.hovered, None);
        assert_eq!(state.notice_seq, 7);
    }

    #[test]
    fn the_reply_quote_names_the_sender_and_quotes_one_line() {
        let item = |id: &str, name: Option<&str>, body: &str| {
            TimelineItem::new(
                id.to_owned(),
                TimelineItemKind::Message {
                    sender: twicecat_core::api::UserId::from("@a:x"),
                    body: body.to_owned(),
                    is_own: false,
                    timestamp_ms: 1,
                    sent: twicecat_core::api::SendStatus::Sent,
                    details: Box::new(MessageDetails {
                        sender_name: name.map(str::to_owned),
                        ..MessageDetails::default()
                    }),
                },
            )
        };
        let items = vec![
            item("m", Some("Al\u{202E}ice"), "\nfirst\nsecond"),
            item("n", None, "x"),
            TimelineItem::new("d".to_owned(), TimelineItemKind::ReadMarker),
        ];
        assert_eq!(
            quote_for(&items, "m").as_deref(),
            Some("Al\u{FFFD}ice: first")
        );
        assert_eq!(quote_for(&items, "n").as_deref(), Some("@a:x: x"));
        assert_eq!(quote_for(&items, "d"), None, "not a message");
        assert_eq!(quote_for(&items, "gone"), None);
        let long = item("l", Some(&"N".repeat(500)), &"q".repeat(500));
        let line = quote_for(&[long], "l").expect("a quote");
        assert!(line.chars().count() <= MAX_QUOTE_NAME_CHARS + 2 + MAX_QUOTE_CHARS);
    }
}
