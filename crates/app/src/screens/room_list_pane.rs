//! The room list pane: tabs, search and the windowed list of rows (plan Q12).
//!
//! Only the rows near the viewport are built, between two spacers that keep
//! the scrollable's content `len * ROW_HEIGHT` tall. Names and previews come
//! from other people, so they pass through [`crate::text_safety`] first (see
//! [`row_name`] and [`preview_line`]).

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, row, scrollable, space, text, text_input};
use iced::{Background, Element, Fill, Length, border};
use tessera_core::api::{RoomListTab, RoomSummary};

use crate::room_list::{
    Badge, OVERSCAN_ROWS, ROW_HEIGHT, badge_for, empty_label, preview_line, row_name,
    spacer_heights, visible_range,
};
use crate::screens::centered_text;
use crate::state::{App, Message};
use crate::theme;

/// The tabs, left to right.
pub const TABS: [RoomListTab; 4] = [
    RoomListTab::All,
    RoomListTab::People,
    RoomListTab::Rooms,
    RoomListTab::Spaces,
];

/// Side of the square left of each row, kept for the avatar (Task 11).
pub const AVATAR_SLOT: f32 = 40.0;

/// The label of a tab.
pub fn tab_label(tab: RoomListTab) -> &'static str {
    match tab {
        RoomListTab::All => "All",
        RoomListTab::People => "DMs",
        RoomListTab::Rooms => "Rooms",
        RoomListTab::Spaces => "Spaces",
    }
}

/// A badge count as text: `99+` above 99.
///
/// ```
/// use tessera_app::screens::room_list_pane::count_label;
/// assert_eq!(count_label(7), "7");
/// assert_eq!(count_label(100), "99+");
/// ```
pub fn count_label(count: u64) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// The pane: tabs, search, then the list or an empty-state line.
pub fn view(app: &App) -> Element<'_, Message> {
    let tabs = row(TABS.into_iter().map(|tab| {
        let active = tab == app.query.tab;
        button(text(tab_label(tab)).size(14))
            .on_press(Message::TabSelected(tab))
            .padding([4, 10])
            .style(if active {
                button::primary
            } else {
                button::secondary
            })
            .into()
    }))
    .spacing(4);
    let search = text_input("Search rooms", &app.search_input)
        .on_input(Message::SearchChanged)
        .padding(8);
    let list = if app.rooms.is_empty() {
        centered_text(empty_label(app.query.tab, &app.query.search))
    } else {
        windowed_list(app)
    };
    container(column![tabs, search, list].spacing(8))
        .width(Length::Fixed(crate::screens::main_view::LIST_PANE_WIDTH))
        .height(Fill)
        .padding(8)
        .into()
}

/// The scrollable with only the rows near the viewport built.
fn windowed_list(app: &App) -> Element<'_, Message> {
    let len = app.rooms.len();
    let range = visible_range(
        app.list_offset_y,
        app.list_viewport_h,
        ROW_HEIGHT,
        len,
        OVERSCAN_ROWS,
    );
    let (top, bottom) = spacer_heights(&range, len, ROW_HEIGHT);
    let rows = app.rooms[range]
        .iter()
        .map(|room| room_row(room, app.selected.as_ref() == Some(&room.id)));
    let content = column![space().height(top)]
        .extend(rows)
        .push(space().height(bottom));
    scrollable(content)
        .on_scroll(|viewport| Message::RoomListScrolled {
            offset_y: viewport.absolute_offset().y,
            viewport_h: viewport.bounds().height,
        })
        .height(Fill)
        .into()
}

/// One fixed-height row: avatar slot, name (and "Invite"), preview, badge.
fn room_row(room: &RoomSummary, selected: bool) -> Element<'_, Message> {
    let muted = |t: &iced::Theme| text::Style {
        color: Some(theme::semantic(t).muted),
    };
    let mut name_line = row![
        text(row_name(&room.name))
            .wrapping(Wrapping::None)
            .width(Fill)
    ]
    .spacing(6)
    .align_y(iced::Center);
    if room.is_invite {
        name_line = name_line.push(tag("Invite"));
    }
    let preview = preview_line(room).unwrap_or_default();
    let body = column![
        name_line,
        text(preview).size(13).wrapping(Wrapping::None).style(muted)
    ]
    .spacing(2)
    .width(Fill);
    let content = row![
        space().width(AVATAR_SLOT).height(AVATAR_SLOT),
        body,
        badge_view(badge_for(room)),
    ]
    .spacing(8)
    .align_y(iced::Center);
    button(content)
        .on_press(Message::SelectRoom(room.id.clone()))
        .width(Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .padding([4, 8])
        .clip(true)
        .style(move |t, status| {
            let base = button::text(t, status);
            if selected {
                button::Style {
                    background: Some(Background::Color(theme::semantic(t).selection)),
                    ..base
                }
            } else {
                base
            }
        })
        .into()
}

/// A small pill with a label.
fn tag(label: &str) -> Element<'_, Message> {
    container(text(label).size(11))
        .padding([1, 6])
        .style(|t| container::Style {
            border: border::rounded(8.0)
                .color(theme::semantic(t).muted)
                .width(1),
            ..container::Style::default()
        })
        .into()
}

/// The badge at the right of a row.
fn badge_view<'a>(badge: Badge) -> Element<'a, Message> {
    match badge {
        Badge::None => space().width(0).into(),
        Badge::Highlight(count) => pill(count_label(count), true),
        Badge::Count(count) => pill(count_label(count), false),
        Badge::Dot => container(space().width(10).height(10))
            .style(|t| container::Style {
                background: Some(Background::Color(theme::semantic(t).badge)),
                border: border::rounded(5.0),
                ..container::Style::default()
            })
            .into(),
    }
}

/// A rounded badge with a count; the highlight colour for mentions.
fn pill<'a>(label: String, highlight: bool) -> Element<'a, Message> {
    container(text(label).size(12))
        .padding([1, 7])
        .style(move |t| {
            let colours = theme::semantic(t);
            let (background, foreground) = if highlight {
                (colours.highlight, colours.highlight_text)
            } else {
                (colours.badge, colours.badge_text)
            };
            container::Style {
                text_color: Some(foreground),
                background: Some(Background::Color(background)),
                border: border::rounded(10.0),
                ..container::Style::default()
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_labels() {
        let labels: Vec<_> = TABS.iter().map(|t| tab_label(*t)).collect();
        assert_eq!(labels, ["All", "DMs", "Rooms", "Spaces"]);
    }

    #[test]
    fn count_labels_cap_at_99() {
        assert_eq!(count_label(1), "1");
        assert_eq!(count_label(99), "99");
        assert_eq!(count_label(100), "99+");
        assert_eq!(count_label(u64::MAX), "99+");
    }
}
