//! The two-pane main layout: the room list pane on the left and the room view
//! (timeline pane and composer) on the right.

use iced::widget::{button, column, container, row, space, text, text_editor};
use iced::{Element, Fill};
use tessera_core::api::UserId;

use crate::composer::{self, EditorAction, Mode, Suggestions};
use crate::links::LinkBar;
use crate::screens::loading::sync_label;
use crate::screens::timeline_pane::RichEnv;
use crate::screens::{room_list_pane, timeline_pane};
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder, one_line_capped};
use crate::theme;

/// Width of the left pane.
pub const LIST_PANE_WIDTH: f32 = 300.0;

/// A room name ready to draw: one line, capped, a placeholder when blank.
pub fn display_name(name: &str) -> String {
    name_or_placeholder(name, MAX_NAME_CHARS)
}

/// The main screen: panes above a status bar.
pub fn view(app: &App) -> Element<'_, Message> {
    column![
        row![room_list_pane::view(app), room_pane(app)].height(Fill),
        status_bar(app)
    ]
    .into()
}

/// The selected room: name, timeline, composer.
fn room_pane(app: &App) -> Element<'_, Message> {
    let selected = app
        .selected
        .as_ref()
        .and_then(|id| app.rooms.iter().find(|room| room.id == *id));
    let title = selected.map_or_else(
        || "Select a room".to_owned(),
        |room| display_name(&room.name),
    );

    let theme = theme::theme_for(app.theme_mode);
    let env = RichEnv {
        theme: &theme,
        own_user: app.user_id.as_ref(),
        revealed: &app.revealed,
    };
    let mut body = column![
        text(title).size(22),
        container(timeline_pane::view(&app.timeline, env)).height(Fill),
    ]
    .spacing(8);
    if let Some(bar) = &app.link_bar {
        body = body.push(link_bar(bar));
    }
    if let Some(error) = &app.send_error {
        body = body.push(text(error.as_str()).style(text::danger));
    }
    let enabled = selected.is_some();
    if let Some(list) = suggestion_list(&app.composer.suggestions) {
        body = body.push(list);
    }
    if let Some(bar) = mode_bar(&app.composer.mode) {
        body = body.push(bar);
    }
    body = body.push(editor(app, enabled));
    container(body).width(Fill).height(Fill).padding(8).into()
}

/// Tallest the editor grows before it scrolls, in pixels.
const EDITOR_MAX_HEIGHT: f32 = 160.0;

/// The composer's text editor. Without a selected room it has no `on_action`,
/// which disables it.
fn editor(app: &App, enabled: bool) -> Element<'_, Message> {
    let open = app.composer.suggestions.is_open();
    let mut editor = text_editor(app.composer.content())
        .placeholder("Message")
        .padding(8)
        .max_height(EDITOR_MAX_HEIGHT)
        .key_binding(move |press| composer::binding(press, open));
    if enabled {
        editor = editor.on_action(|action| Message::Composer(EditorAction(action)));
    }
    editor.into()
}

/// The reply or edit bar above the editor.
fn mode_bar(mode: &Mode) -> Option<Element<'_, Message>> {
    let label = match mode {
        Mode::Normal => return None,
        Mode::Reply(_) => "Replying to a message",
        Mode::Edit(_) => "Editing a message",
    };
    Some(
        container(
            row![
                text(label),
                space().width(Fill),
                button(text("Cancel"))
                    .on_press(Message::ComposerKey(composer::ComposerKey::Escape))
                    .style(button::secondary),
            ]
            .spacing(8)
            .align_y(iced::Center),
        )
        .padding(4)
        .into(),
    )
}

/// The autocomplete rows (members or emojis) above the editor, or `None`
/// while there is nothing to pick.
fn suggestion_list(suggestions: &Suggestions) -> Option<Element<'_, Message>> {
    let rows: Vec<(String, bool)> = match suggestions {
        Suggestions::Closed => return None,
        Suggestions::Mentions {
            members, selected, ..
        } => members
            .iter()
            .enumerate()
            .map(|(i, member)| (member_row_label(member), i == *selected))
            .collect(),
        Suggestions::Emoji {
            items, selected, ..
        } => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                (
                    format!("{} :{}:", item.glyph, item.shortcode),
                    i == *selected,
                )
            })
            .collect(),
    };
    if rows.is_empty() {
        return None;
    }
    let mut list = column![].spacing(2);
    for (index, (label, selected)) in rows.into_iter().enumerate() {
        list = list.push(
            button(text(label))
                .on_press(Message::SuggestionPicked(index))
                .width(Fill)
                .style(if selected {
                    button::primary
                } else {
                    button::text
                }),
        );
    }
    Some(container(list).padding(4).into())
}

/// One member row: the display name and the user ID, sanitised and capped.
pub fn member_row_label(member: &tessera_core::api::Member) -> String {
    let id = one_line_capped(member.user_id.as_str(), MAX_NAME_CHARS);
    match member.display_name.as_deref() {
        Some(name) if !name.trim().is_empty() => {
            format!("{} ({id})", one_line_capped(name, MAX_NAME_CHARS))
        }
        _ => id,
    }
}

/// What the link bar says. Only the host of a link is ever shown.
pub fn link_bar_text(bar: &LinkBar) -> String {
    match bar {
        LinkBar::Confirm(url) => format!("Open {}?", one_line_capped(url.host(), MAX_NAME_CHARS)),
        LinkBar::Refused => "This link cannot be opened.".to_owned(),
        LinkBar::Failed => "The link could not be opened.".to_owned(),
    }
}

/// The bar above the composer about a clicked link.
fn link_bar(bar: &LinkBar) -> Element<'_, Message> {
    let mut bar_row = row![text(link_bar_text(bar)), space().width(Fill)]
        .spacing(8)
        .align_y(iced::Center);
    match bar {
        LinkBar::Confirm(_) => {
            bar_row = bar_row
                .push(button(text("Open")).on_press(Message::LinkConfirmed))
                .push(
                    button(text("Cancel"))
                        .on_press(Message::LinkDismissed)
                        .style(button::secondary),
                );
        }
        LinkBar::Refused | LinkBar::Failed => {
            bar_row = bar_row.push(
                button(text("Dismiss"))
                    .on_press(Message::LinkDismissed)
                    .style(button::secondary),
            );
        }
    }
    container(bar_row).padding(4).into()
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
    use super::*;

    #[test]
    fn the_account_label_is_sanitised_and_capped() {
        assert_eq!(account_label(None), "");
        let id = UserId::from("@a\u{202E}b:x");
        assert_eq!(account_label(Some(&id)), "@a\u{FFFD}b:x");
        let long = UserId::from(format!("@{}:x", "u".repeat(1000)));
        assert_eq!(account_label(Some(&long)).chars().count(), 256);
    }

    #[test]
    fn the_link_bar_shows_only_the_host() {
        let url = crate::links::check_link_url("https://Evil.example:8443/secret/path?token=abc")
            .expect("valid link");
        let text = link_bar_text(&LinkBar::Confirm(url));
        assert_eq!(text, "Open evil.example?");
        assert!(!link_bar_text(&LinkBar::Refused).is_empty());
        assert!(!link_bar_text(&LinkBar::Failed).is_empty());
    }

    #[test]
    fn room_names_are_sanitised_and_capped() {
        assert_eq!(display_name("Lounge"), "Lounge");
        assert_eq!(display_name("a\nb\u{202E}c"), "a\u{23CE}b\u{FFFD}c");
        let out = display_name(&"n".repeat(1000));
        assert_eq!(out.chars().count(), 256);
        assert!(out.ends_with('\u{2026}'));
    }
}
