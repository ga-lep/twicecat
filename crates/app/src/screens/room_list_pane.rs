//! The room list pane: tabs, search and the windowed list of rows (plan Q12).
//!
//! Only the rows near the viewport are built, between two spacers that keep
//! the scrollable's content `len * ROW_HEIGHT` tall. Names and previews come
//! from other people, so they pass through [`crate::text_safety`] first (see
//! [`row_name`] and [`preview_line`]).

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, image, row, scrollable, space, text, text_input};
use iced::{Background, Color, Element, Fill, Length, border};
use tessera_core::api::{RoomListTab, RoomNotificationModeView, RoomSummary};

use crate::avatars::{AvatarState, Avatars, NEUTRAL_INITIALS, initials, placeholder_color};
use crate::notifications::muted_mark;
use crate::room_list::{
    Badge, OVERSCAN_ROWS, ROW_HEIGHT, badge_for, empty_label, preview_line, row_name,
    spacer_heights, visible_range,
};
use crate::rooms::RoomsMessage;
use crate::screens::centered_text;
use crate::state::{App, Message};
use crate::theme;

/// **New room**, **Direct message** and **Join**, above the tabs.
fn room_actions<'a>() -> Element<'a, Message> {
    let action = |label: &'static str, message: RoomsMessage| {
        button(text(label).size(13))
            .on_press(Message::Rooms(message))
            .padding([4, 8])
            .style(button::secondary)
    };
    row![
        action("New room", RoomsMessage::OpenCreate),
        action("Direct message", RoomsMessage::OpenDm),
        action("Join", RoomsMessage::OpenJoin),
    ]
    .spacing(4)
    .into()
}

/// The tabs, left to right.
pub const TABS: [RoomListTab; 4] = [
    RoomListTab::All,
    RoomListTab::People,
    RoomListTab::Rooms,
    RoomListTab::Spaces,
];

/// Side of the round avatar left of each row.
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

/// The mark of an encrypted room, in the list row and the room header.
pub const LOCK_MARK: &str = "\u{1F512}";

/// The lock of `room`, when it is known to be encrypted. The core reports
/// `is_encrypted` as false while the SDK does not know the room's encryption
/// yet, so a missing lock does not say "not encrypted": only the lock is
/// ever drawn, never a mark for the opposite.
pub fn encrypted_mark(room: &RoomSummary) -> Option<&'static str> {
    room.is_encrypted.then_some(LOCK_MARK)
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
    container(column![room_actions(), tabs, search, list].spacing(8))
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
    let rows = app.rooms[range].iter().map(|room| {
        room_row(
            room,
            &app.avatars,
            app.selected.as_ref() == Some(&room.id),
            app.effective_room_mode(&room.id),
        )
    });
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

/// One fixed-height row: avatar, name (and "Invite" or "Muted"), preview,
/// badge. `mode` is the room's notification mode as the app knows it.
fn room_row<'a>(
    room: &'a RoomSummary,
    avatars: &Avatars,
    selected: bool,
    mode: Option<RoomNotificationModeView>,
) -> Element<'a, Message> {
    let muted = |t: &iced::Theme| text::Style {
        color: Some(theme::semantic(t).muted),
    };
    let name = row_name(&room.name);
    let avatar = avatar_view(room, &name, avatars);
    let mut name_line = row![text(name).wrapping(Wrapping::None).width(Fill)]
        .spacing(6)
        .align_y(iced::Center);
    if let Some(lock) = encrypted_mark(room) {
        name_line = name_line.push(text(lock).size(12));
    }
    if room.is_invite {
        name_line = name_line.push(tag("Invite"));
    }
    if let Some(mark) = muted_mark(mode) {
        name_line = name_line.push(tag(mark));
    }
    let preview = preview_line(room).unwrap_or_default();
    let body = column![
        name_line,
        text(preview).size(13).wrapping(Wrapping::None).style(muted)
    ]
    .spacing(2)
    .width(Fill);
    let content = row![
        avatar,
        body,
        badge_view(badge_for(room), muted_mark(mode).is_some()),
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

/// The room's avatar: the decoded image once it arrived, otherwise (and when
/// it is unavailable) a round placeholder with the initials of the already
/// sanitised `name`.
fn avatar_view<'a>(room: &RoomSummary, name: &str, avatars: &Avatars) -> Element<'a, Message> {
    let ready = room
        .avatar
        .as_ref()
        .and_then(|id| avatars.state(id))
        .and_then(|state| match state {
            AvatarState::Ready(handle) => Some(handle),
            AvatarState::Unavailable => None,
        });
    if let Some(handle) = ready {
        return image(handle.clone())
            .width(AVATAR_SLOT)
            .height(AVATAR_SLOT)
            .border_radius(AVATAR_SLOT / 2.0)
            .into();
    }
    let label = initials(name);
    let fill = if label == NEUTRAL_INITIALS {
        None
    } else {
        Some(placeholder_color(&room.id))
    };
    container(text(label).size(16).color(Color::WHITE))
        .width(AVATAR_SLOT)
        .height(AVATAR_SLOT)
        .center_x(AVATAR_SLOT)
        .center_y(AVATAR_SLOT)
        .style(move |t| container::Style {
            background: Some(Background::Color(
                fill.unwrap_or_else(|| theme::semantic(t).muted),
            )),
            border: border::rounded(AVATAR_SLOT / 2.0),
            ..container::Style::default()
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

/// The badge at the right of a row. A muted room's count is drawn as an
/// outline in the muted text colour instead of a filled badge; mentions keep
/// their highlight.
fn badge_view<'a>(badge: Badge, muted: bool) -> Element<'a, Message> {
    match badge {
        Badge::None => space().width(0).into(),
        Badge::Highlight(count) => pill(count_label(count), Tone::Highlight),
        Badge::Count(count) if muted => pill(count_label(count), Tone::Muted),
        Badge::Count(count) => pill(count_label(count), Tone::Normal),
        Badge::Dot => container(space().width(10).height(10))
            .style(|t| container::Style {
                background: Some(Background::Color(theme::semantic(t).badge)),
                border: border::rounded(5.0),
                ..container::Style::default()
            })
            .into(),
    }
}

/// How a count badge is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    /// Mentions.
    Highlight,
    /// Ordinary unread notifications.
    Normal,
    /// Unread notifications of a muted room.
    Muted,
}

/// A rounded badge with a count; the highlight colour for mentions, an
/// outline for a muted room.
fn pill<'a>(label: String, tone: Tone) -> Element<'a, Message> {
    container(text(label).size(12))
        .padding([1, 7])
        .style(move |t| {
            let colours = theme::semantic(t);
            match tone {
                Tone::Highlight | Tone::Normal => {
                    let (background, foreground) = if tone == Tone::Highlight {
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
                }
                Tone::Muted => container::Style {
                    text_color: Some(colours.muted),
                    border: border::rounded(10.0).color(colours.muted).width(1),
                    ..container::Style::default()
                },
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_room_known_to_be_encrypted_has_a_lock() {
        let mut room = RoomSummary::new(tessera_core::api::RoomId::from("!r:x"), "r".to_owned());
        assert_eq!(encrypted_mark(&room), None);
        room.is_encrypted = true;
        assert_eq!(encrypted_mark(&room), Some(LOCK_MARK));
    }

    #[test]
    fn rows_build_in_every_notification_mode() {
        let mut room = RoomSummary::new(tessera_core::api::RoomId::from("!r:x"), "r".to_owned());
        room.unread_notifications = 3;
        let avatars = Avatars::default();
        for mode in [
            None,
            Some(RoomNotificationModeView::AllMessages),
            Some(RoomNotificationModeView::MentionsAndKeywords),
            Some(RoomNotificationModeView::Mute),
        ] {
            let _: Element<'_, Message> = room_row(&room, &avatars, false, mode);
        }
        room.highlights = 1;
        let _: Element<'_, Message> =
            room_row(&room, &avatars, true, Some(RoomNotificationModeView::Mute));
    }

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

    #[test]
    fn avatar_views_build_for_every_state() {
        use tessera_core::api::{AvatarId, AvatarImage, RoomId};

        let mut avatars = Avatars::default();
        let id = AvatarId::from("mxc://x/a");
        let _ = avatars.take_unrequested([id.clone()]);
        let mut room = RoomSummary::new(RoomId::from("!r:x"), String::new());
        room.avatar = Some(id.clone());
        // Requested, not answered: placeholder.
        let _: Element<'_, Message> = avatar_view(&room, "", &avatars);
        // Unavailable: placeholder.
        avatars.answer(&id, None);
        let _: Element<'_, Message> = avatar_view(&room, "Room", &avatars);
        // No avatar at all.
        room.avatar = None;
        let _: Element<'_, Message> = avatar_view(&room, "#x", &avatars);
        // Ready: the image.
        let img = ::image::RgbaImage::from_pixel(2, 2, ::image::Rgba([1, 2, 3, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, ::image::ImageFormat::Png)
            .expect("encode");
        let id2 = AvatarId::from("mxc://x/b");
        let _ = avatars.take_unrequested([id2.clone()]);
        avatars.answer(&id2, Some(AvatarImage::from(png.into_inner())));
        room.avatar = Some(id2);
        let _: Element<'_, Message> = avatar_view(&room, "Room", &avatars);
    }
}
