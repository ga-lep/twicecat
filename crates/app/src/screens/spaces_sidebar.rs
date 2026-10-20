//! The spaces rail (plan Q15 Task 13; sidebar plan Task 5, "RailStates"):
//! **Home**, then the top-level spaces and folders in the user's order.
//!
//! Each item is a 72 x 48 slot holding a 44 px icon, a pill at the left
//! edge, an unread badge at the bottom right and a tooltip with the name.
//! One `mouse_area` per item reports hover and click; there is no button
//! inside it (a button would swallow the press, and iced 0.14 gives buttons
//! no keyboard focus anyway). Sub-spaces are not on the rail: they are
//! sections of the room list.
//!
//! The sidebar is always one slot of the main row: an empty space when the
//! user is in no space, so the room list and the timeline keep their place in
//! the widget tree. A space only filters the room list; the sidebar says
//! nothing about who may see or join a room.

use std::time::Duration;

use iced::widget::{
    Column, Stack, column, container, image, mouse_area, row, scrollable, space, text, tooltip,
};
use iced::{Background, Color, Element, Padding, border, mouse};
use twicecat_core::api::{SpaceView, UnreadCountsView};

use crate::avatars::{AvatarState, Avatars, NEUTRAL_INITIALS, initials, placeholder_color};
use crate::sidebar_menu::SidebarMenuMessage;
use crate::spaces::{RailFolder, RailItem, RailKey, SpacesMessage};
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder};
use crate::theme;
use crate::ui::icons::Icon;
use crate::ui::kit;
use crate::ui::tokens;

/// Width of the sidebar, in pixels.
pub const SIDEBAR_WIDTH: f32 = 72.0;

/// Height of an item's slot.
const SLOT_HEIGHT: f32 = 48.0;

/// Side of an item's icon.
const ICON: f32 = 44.0;

/// Width of the pill at the slot's left edge.
const PILL_WIDTH: f32 = 4.0;

/// Height and smallest width of the badge.
const BADGE: f32 = 18.0;

/// Width of the badge's outline, in the colour of the rail.
const BADGE_OUTLINE: f32 = 2.0;

/// Gap between the badge and the slot's right edge.
const BADGE_INSET: f32 = 10.0;

/// Width of the ring around the selected icon.
const RING: f32 = 2.0;

/// Indent of the spaces of an open folder, in pixels.
const NESTED_INDENT: f32 = 4.0;

/// Side of a space's icon in a folder's 2 x 2 preview.
const MINI: f32 = 18.0;

/// How long the pointer rests on an item before its tooltip shows.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

/// How an item looks. The strongest of selected, hover and unread wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    /// Read and quiet: a circle, no pill.
    Default,
    /// Something unread: an 8 px pill.
    Unread,
    /// Under the pointer: a squircle and a 20 px pill.
    Hover,
    /// The space shown in the room list: a squircle, a 32 px pill, a ring.
    Selected,
}

impl ItemState {
    /// The look for an item: selected wins over hover, and hover over unread.
    pub fn of(selected: bool, hover: bool, unread: bool) -> Self {
        if selected {
            Self::Selected
        } else if hover {
            Self::Hover
        } else if unread {
            Self::Unread
        } else {
            Self::Default
        }
    }
}

/// Height of the pill: 0, 8, 20 or 32 px.
pub fn pill_height(state: ItemState) -> f32 {
    match state {
        ItemState::Default => 0.0,
        ItemState::Unread => 8.0,
        ItemState::Hover => 20.0,
        ItemState::Selected => 32.0,
    }
}

/// Corner radius of the icon: a circle (22) idle, a squircle (14) when
/// hovered or selected.
pub fn icon_radius(state: ItemState) -> f32 {
    match state {
        ItemState::Default | ItemState::Unread => ICON / 2.0,
        ItemState::Hover | ItemState::Selected => 14.0,
    }
}

/// Whether the counts call for the pill: an unread room, a notification or a
/// mention.
pub fn is_unread(counts: &UnreadCountsView) -> bool {
    counts.unread || counts.notifications > 0 || counts.highlights > 0
}

/// The count badge of an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RailBadge {
    /// The number, or "99+".
    pub text: String,
    /// Mentions: drawn in the highlight colour instead of the count colour.
    pub mention: bool,
}

/// The badge for `counts`: the mentions when there are any, else the
/// notifications; "99+" above 99. Unread alone gets none (the pill says it).
pub fn badge_text(counts: &UnreadCountsView) -> Option<RailBadge> {
    let (count, mention) = if counts.highlights > 0 {
        (counts.highlights, true)
    } else if counts.notifications > 0 {
        (counts.notifications, false)
    } else {
        return None;
    };
    Some(RailBadge {
        text: kit::badge_text(count),
        mention,
    })
}

/// What **Home** counts: the rooms that are in no space.
pub fn home_counts(app: &App) -> UnreadCountsView {
    app.spaces.outside()
}

/// The sidebar, or an empty slot when the user is in no space.
pub fn view(app: &App) -> Element<'_, Message> {
    if app.spaces.is_empty() {
        return space().width(0).into();
    }
    let selected = app.query.space.as_ref();
    let hover = app.rail_hover.as_ref();
    let mut items: Column<'_, Message> = Column::new().spacing(4).padding([6, 0]);
    let home = home_counts(app);
    items = items.push(slot(Slot {
        key: RailKey::Home,
        icon: round(Icon::Home.glyph().to_owned(), None, ICON / 2.0),
        state: ItemState::of(
            selected.is_none(),
            hover == Some(&RailKey::Home),
            is_unread(&home),
        ),
        counts: home,
        name: "Home".to_owned(),
        width: SIDEBAR_WIDTH,
    }));
    for item in app.spaces.rail_items(app.sidebar.layout()) {
        items = match item {
            RailItem::Space(space) => items.push(space_slot(app, space, SIDEBAR_WIDTH)),
            RailItem::Folder(folder) => items.push(folder_view(app, &folder)),
        };
    }
    row![
        container(scrollable(items))
            .width(SIDEBAR_WIDTH)
            .height(iced::Fill)
            .style(kit::sidebar_style),
        kit::vertical_divider(),
    ]
    .height(iced::Fill)
    .into()
}

/// The label of a space for its tooltip: one line, capped, a placeholder
/// when blank.
pub fn space_name(space: &SpaceView) -> String {
    name_or_placeholder(&space.name, MAX_NAME_CHARS)
}

/// The label of a folder: its name, or "Folder".
fn folder_name(folder: &RailFolder<'_>) -> String {
    match folder.name {
        Some(name) => name_or_placeholder(name, MAX_NAME_CHARS),
        None => "Folder".to_owned(),
    }
}

/// Everything one slot needs.
struct Slot<'a> {
    key: RailKey,
    icon: Element<'a, Message>,
    state: ItemState,
    counts: UnreadCountsView,
    name: String,
    width: f32,
}

fn space_slot<'a>(app: &'a App, space: &'a SpaceView, width: f32) -> Element<'a, Message> {
    let key = RailKey::Space(space.id.clone());
    let counts = app.spaces.unread_for(&space.id);
    let state = ItemState::of(
        app.query.space.as_ref() == Some(&space.id),
        app.rail_hover.as_ref() == Some(&key),
        is_unread(&counts),
    );
    slot(Slot {
        key,
        icon: space_icon(space, ICON, icon_radius(state), &app.avatars),
        state,
        counts,
        name: space_name(space),
        width,
    })
}

/// A folder: its icon, and when it is open its spaces in a tinted band.
fn folder_view<'a>(app: &'a App, folder: &RailFolder<'a>) -> Element<'a, Message> {
    let key = RailKey::Folder(folder.id);
    let counts = app.spaces.folder_counts(folder);
    // A closed folder holding the selected space stands for it.
    let holds_selected = app
        .query
        .space
        .as_ref()
        .is_some_and(|sel| folder.spaces.iter().any(|space| space.id == *sel));
    let state = ItemState::of(
        !folder.open && holds_selected,
        app.rail_hover.as_ref() == Some(&key),
        is_unread(&counts),
    );
    let head = slot(Slot {
        key,
        icon: folder_icon(folder, icon_radius(state), &app.avatars),
        state,
        counts,
        name: folder_name(folder),
        width: SIDEBAR_WIDTH,
    });
    if !folder.open {
        return head;
    }
    let mut inside: Column<'a, Message> = Column::new().spacing(4);
    for space in &folder.spaces {
        inside = inside.push(space_slot(app, space, SIDEBAR_WIDTH - NESTED_INDENT));
    }
    column![
        head,
        container(row![space().width(NESTED_INDENT), inside])
            .width(SIDEBAR_WIDTH)
            .padding([4, 0])
            .style(band_style),
    ]
    .spacing(4)
    .into()
}

/// The tinted band behind the spaces of an open folder.
fn band_style(t: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::semantic(t).surface)),
        border: border::rounded(tokens::LARGE),
        ..container::Style::default()
    }
}

/// One item: the icon, with the pill under the left edge and the badge over
/// the bottom right, in a `mouse_area` and a tooltip.
fn slot(slot: Slot<'_>) -> Element<'_, Message> {
    let Slot {
        key,
        icon,
        state,
        counts,
        name,
        width,
    } = slot;
    let icon = if state == ItemState::Selected {
        iced::widget::stack![icon, ring(icon_radius(state))].into()
    } else {
        icon
    };
    let mut layers: Stack<'_, Message> = Stack::new()
        .width(width)
        .height(SLOT_HEIGHT)
        .push(container(icon).center_x(width).center_y(SLOT_HEIGHT));
    let pill = pill_height(state);
    if pill > 0.0 {
        layers = layers.push(
            container(container(space().width(PILL_WIDTH).height(pill)).style(pill_style))
                .height(SLOT_HEIGHT)
                .center_y(SLOT_HEIGHT),
        );
    }
    if let Some(badge) = badge_text(&counts) {
        layers = layers.push(
            container(badge_view(badge))
                .align_right(width)
                .align_bottom(SLOT_HEIGHT)
                .padding(Padding {
                    right: BADGE_INSET,
                    ..Padding::ZERO
                }),
        );
    }
    let mut area = mouse_area(layers)
        .interaction(mouse::Interaction::Pointer)
        .on_enter(Message::Spaces(SpacesMessage::Enter(key.clone())))
        .on_exit(Message::Spaces(SpacesMessage::Exit(key.clone())))
        .on_press(Message::Spaces(SpacesMessage::Press(key.clone())))
        .on_release(Message::Spaces(SpacesMessage::Release(key.clone())));
    // A right-click opens the item's options card (Home has none).
    if key != RailKey::Home {
        area = area.on_right_press(Message::SidebarMenu(SidebarMenuMessage::Open(key)));
    }
    tooltip(
        area,
        container(text(name).size(tokens::CAPTION))
            .padding(tokens::S)
            .style(kit::card_style),
        tooltip::Position::Right,
    )
    .delay(TOOLTIP_DELAY)
    .into()
}

/// The pill: the text colour, round on its right side only.
fn pill_style(t: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(t.palette().text)),
        border: iced::Border {
            radius: border::right(PILL_WIDTH),
            ..iced::Border::default()
        },
        ..container::Style::default()
    }
}

/// The ring of the selected icon: the focus colour, inside the icon's box.
fn ring<'a>(radius: f32) -> Element<'a, Message> {
    container(space())
        .width(ICON)
        .height(ICON)
        .style(move |t| container::Style {
            border: border::rounded(radius)
                .color(theme::semantic(t).focus)
                .width(RING),
            ..container::Style::default()
        })
        .into()
}

/// The count badge: highlight colours for mentions, the count colours
/// otherwise, outlined in the rail's colour so it parts from the icon.
fn badge_view<'a>(badge: RailBadge) -> Element<'a, Message> {
    let mention = badge.mention;
    let single = badge.text.chars().count() <= 1;
    let label = container(text(badge.text).size(tokens::CAPTION))
        .height(BADGE)
        .center_y(BADGE)
        .style(move |t| {
            let sem = theme::semantic(t);
            let (fill, ink) = if mention {
                (sem.highlight, sem.highlight_text)
            } else {
                (sem.badge, sem.badge_text)
            };
            container::Style {
                background: Some(Background::Color(fill)),
                text_color: Some(ink),
                border: border::rounded(BADGE / 2.0)
                    .color(sem.sidebar)
                    .width(BADGE_OUTLINE),
                ..container::Style::default()
            }
        });
    if single {
        label
            .width(BADGE)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    } else {
        label.padding([0.0, 5.0]).into()
    }
}

/// The avatar of `space` at `size`, or its initials on a colour.
fn space_icon<'a>(
    space: &SpaceView,
    size: f32,
    radius: f32,
    avatars: &Avatars,
) -> Element<'a, Message> {
    let ready = space
        .avatar
        .as_ref()
        .and_then(|id| avatars.state(id))
        .and_then(|state| match state {
            AvatarState::Ready(handle) => Some(handle),
            AvatarState::Unavailable => None,
        });
    match ready {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .border_radius(radius)
            .into(),
        None => {
            let name = space_name(space);
            let label = initials(&name);
            let fill = (label != NEUTRAL_INITIALS).then(|| placeholder_color(&space.id));
            if size < ICON {
                // A small preview carries one letter only.
                let first: String = label.chars().take(1).collect();
                mini_round(first, fill, size, radius)
            } else {
                round(label, fill, radius)
            }
        }
    }
}

/// The round (or squircle) placeholder: `label` on `fill` (the muted colour
/// when `None`).
fn round<'a>(label: String, fill: Option<Color>, radius: f32) -> Element<'a, Message> {
    sized_round(label, fill, ICON, radius, 15)
}

fn mini_round<'a>(
    label: String,
    fill: Option<Color>,
    size: f32,
    radius: f32,
) -> Element<'a, Message> {
    sized_round(label, fill, size, radius, 10)
}

fn sized_round<'a>(
    label: String,
    fill: Option<Color>,
    size: f32,
    radius: f32,
    font: u32,
) -> Element<'a, Message> {
    container(text(label).size(font).color(Color::WHITE))
        .width(size)
        .height(size)
        .center_x(size)
        .center_y(size)
        .style(move |t| container::Style {
            background: Some(Background::Color(
                fill.unwrap_or_else(|| theme::semantic(t).muted),
            )),
            border: border::rounded(radius),
            ..container::Style::default()
        })
        .into()
}

/// A folder's icon: a squircle with a 2 x 2 preview of its first four
/// spaces.
fn folder_icon<'a>(
    folder: &RailFolder<'a>,
    radius: f32,
    avatars: &Avatars,
) -> Element<'a, Message> {
    let cell = |at: usize| -> Element<'a, Message> {
        match folder.spaces.get(at) {
            Some(space) => space_icon(space, MINI, 5.0, avatars),
            None => space().width(MINI).height(MINI).into(),
        }
    };
    container(
        column![
            row![cell(0), cell(1)].spacing(2),
            row![cell(2), cell(3)].spacing(2)
        ]
        .spacing(2),
    )
    .width(ICON)
    .height(ICON)
    .center_x(ICON)
    .center_y(ICON)
    .style(move |t| container::Style {
        background: Some(Background::Color(theme::semantic(t).surface_raised)),
        border: border::rounded(radius),
        ..container::Style::default()
    })
    .into()
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::RoomId;

    use super::*;

    fn counts(notifications: u64, highlights: u64, unread: bool) -> UnreadCountsView {
        UnreadCountsView {
            notifications,
            highlights,
            unread,
        }
    }

    #[test]
    fn the_name_is_one_capped_line_with_a_placeholder() {
        let mut space = SpaceView {
            id: RoomId::from("!s:x"),
            name: "  ".into(),
            avatar: None,
            level: 0,
            children: 0,
            rooms: Vec::new(),
        };
        assert_eq!(space_name(&space), crate::text_safety::NAME_PLACEHOLDER);
        space.name = format!("a\nb{}", "z".repeat(1000));
        let name = space_name(&space);
        assert!(!name.contains('\n') && name.chars().count() <= MAX_NAME_CHARS);
    }

    #[test]
    fn the_pill_is_0_8_20_or_32_and_the_strongest_state_wins() {
        let h = |selected, hover, unread| pill_height(ItemState::of(selected, hover, unread));
        assert_eq!(h(false, false, false), 0.0);
        assert_eq!(h(false, false, true), 8.0);
        assert_eq!(h(false, true, false), 20.0);
        assert_eq!(h(true, false, false), 32.0);
        // Hover wins over unread; selected wins over hover.
        assert_eq!(h(false, true, true), 20.0);
        assert_eq!(h(true, true, false), 32.0);
        assert_eq!(h(true, true, true), 32.0);
        assert_eq!(h(true, false, true), 32.0);
    }

    #[test]
    fn the_icon_is_a_circle_idle_and_a_squircle_when_hovered_or_selected() {
        assert_eq!(icon_radius(ItemState::Default), 22.0);
        assert_eq!(icon_radius(ItemState::Unread), 22.0);
        assert_eq!(icon_radius(ItemState::Hover), 14.0);
        assert_eq!(icon_radius(ItemState::Selected), 14.0);
        // The radius of the idle circle is half the icon.
        assert_eq!(icon_radius(ItemState::Default) * 2.0, ICON);
    }

    #[test]
    fn the_badge_shows_mentions_first_then_notifications_and_never_unread_alone() {
        let b = |c| badge_text(&c);
        assert_eq!(
            b(counts(9, 2, true)),
            Some(RailBadge {
                text: "2".into(),
                mention: true
            })
        );
        assert_eq!(
            b(counts(9, 0, true)),
            Some(RailBadge {
                text: "9".into(),
                mention: false
            })
        );
        assert_eq!(b(counts(0, 0, true)), None, "the pill says plain unread");
        assert_eq!(b(counts(0, 0, false)), None);
    }

    #[test]
    fn the_badge_caps_at_99_plus() {
        let text = |n, h| badge_text(&counts(n, h, true)).expect("a badge").text;
        assert_eq!(text(99, 0), "99");
        assert_eq!(text(100, 0), "99+");
        assert_eq!(text(u64::MAX, 0), "99+");
        assert_eq!(text(0, 100), "99+");
        assert_eq!(
            text(500, 3),
            "3",
            "mentions first, whatever the notifications"
        );
    }

    #[test]
    fn the_pill_follows_any_sign_of_unread() {
        assert!(!is_unread(&counts(0, 0, false)));
        assert!(is_unread(&counts(0, 0, true)));
        assert!(is_unread(&counts(1, 0, false)));
        assert!(is_unread(&counts(0, 1, false)));
    }

    #[test]
    fn the_rail_marks_read_on_the_rail_background_in_every_test_theme() {
        for t in theme::test_themes() {
            let p = t.palette();
            let s = theme::semantic(&t);
            let d = |a, b| theme::delta_e76(a, b);
            // The badges, the pill and the ring part from the rail.
            for (what, c) in [
                ("badge", s.badge),
                ("mention badge", s.highlight),
                ("pill", p.text),
                ("ring", s.focus),
            ] {
                assert!(
                    d(c, s.sidebar) >= 20.0,
                    "{what} on the rail: {}",
                    d(c, s.sidebar)
                );
            }
            // The label is readable on both badges.
            for fill in [s.badge, s.highlight] {
                assert!(theme::contrast(s.badge_text, fill) >= 4.5);
                assert!(theme::contrast(s.highlight_text, fill) >= 4.5);
            }
            // The folder tile and the band can be told from the rail.
            assert!(d(s.surface_raised, s.sidebar) >= 2.0, "folder tile");
            assert!(d(s.surface, s.sidebar) >= 2.0, "band");
        }
    }
}
