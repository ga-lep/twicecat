//! The spaces sidebar (plan Q15, Task 13): **Home** and one round button per
//! top-level space, with the selected space's sub-spaces under it.
//!
//! The sidebar is always one slot of the main row: an empty space when the
//! user is in no space, so the room list and the timeline keep their place in
//! the widget tree. A space only filters the room list; the sidebar says
//! nothing about who may see or join a room.

use iced::widget::{Column, button, container, image, scrollable, space, text, tooltip};
use iced::{Background, Color, Element, border};
use tessera_core::api::{RoomId, SpaceView};

use crate::avatars::{AvatarState, Avatars, NEUTRAL_INITIALS, initials, placeholder_color};
use crate::spaces::{Entry, SpacesMessage};
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder};
use crate::theme;

/// Width of the sidebar, in pixels.
pub const SIDEBAR_WIDTH: f32 = 64.0;

/// Side of a space's round icon.
const ICON: f32 = 40.0;

/// Indent of a sub-space's icon, in pixels.
const NESTED_INDENT: f32 = 8.0;

/// The sidebar, or an empty slot when the user is in no space.
pub fn view(app: &App) -> Element<'_, Message> {
    if app.spaces.is_empty() {
        return space().width(0).into();
    }
    let selected = app.query.space.as_ref();
    let mut buttons: Column<'_, Message> = Column::new().spacing(6).padding(6);
    buttons = buttons.push(home_button(selected.is_none()));
    for entry in app.spaces.entries(selected) {
        buttons = buttons.push(space_button(entry, selected, &app.avatars));
    }
    container(scrollable(buttons))
        .width(SIDEBAR_WIDTH)
        .height(iced::Fill)
        .into()
}

/// The label of a space for its tooltip: one line, capped, a placeholder
/// when blank.
pub fn space_name(space: &SpaceView) -> String {
    name_or_placeholder(&space.name, MAX_NAME_CHARS)
}

fn home_button<'a>(selected: bool) -> Element<'a, Message> {
    tip(
        icon_button(
            round("\u{2302}".to_owned(), None, selected),
            SpacesMessage::Select(None),
        ),
        "Home".to_owned(),
    )
}

fn space_button<'a>(
    entry: Entry<'a>,
    selected: Option<&RoomId>,
    avatars: &Avatars,
) -> Element<'a, Message> {
    let space = entry.space;
    let name = space_name(space);
    let is_selected = selected == Some(&space.id);
    let icon = match space
        .avatar
        .as_ref()
        .and_then(|id| avatars.state(id))
        .and_then(|state| match state {
            AvatarState::Ready(handle) => Some(handle),
            AvatarState::Unavailable => None,
        }) {
        Some(handle) => image(handle.clone())
            .width(ICON)
            .height(ICON)
            .border_radius(ICON / 2.0)
            .into(),
        None => {
            let label = initials(&name);
            let fill = (label != NEUTRAL_INITIALS).then(|| placeholder_color(&space.id));
            round(label, fill, is_selected)
        }
    };
    let button = icon_button(icon, SpacesMessage::Select(Some(space.id.clone())));
    let indent = if entry.nested { NESTED_INDENT } else { 0.0 };
    tip(
        container(button).padding(iced::Padding {
            left: indent,
            ..iced::Padding::ZERO
        }),
        name,
    )
}

fn icon_button<'a>(icon: Element<'a, Message>, message: SpacesMessage) -> Element<'a, Message> {
    button(icon)
        .padding(2)
        .style(button::text)
        .on_press(Message::Spaces(message))
        .into()
}

/// The round placeholder: `label` on `fill` (the muted colour when `None`),
/// ringed when selected.
fn round<'a>(label: String, fill: Option<Color>, selected: bool) -> Element<'a, Message> {
    container(text(label).size(15).color(Color::WHITE))
        .width(ICON)
        .height(ICON)
        .center_x(ICON)
        .center_y(ICON)
        .style(move |t| container::Style {
            background: Some(Background::Color(
                fill.unwrap_or_else(|| theme::semantic(t).muted),
            )),
            border: border::rounded(ICON / 2.0)
                .color(t.palette().primary)
                .width(if selected { 3 } else { 0 }),
            ..container::Style::default()
        })
        .into()
}

/// `content` with the space's name in a tooltip to its right.
fn tip<'a>(content: impl Into<Element<'a, Message>>, name: String) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(name).size(13))
            .padding(6)
            .style(container::bordered_box),
        tooltip::Position::Right,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_one_capped_line_with_a_placeholder() {
        let mut space = SpaceView {
            id: RoomId::from("!s:x"),
            name: "  ".into(),
            avatar: None,
            level: 0,
            children: 0,
        };
        assert_eq!(space_name(&space), crate::text_safety::NAME_PLACEHOLDER);
        space.name = format!("a\nb{}", "z".repeat(1000));
        let name = space_name(&space);
        assert!(!name.contains('\n') && name.chars().count() <= MAX_NAME_CHARS);
    }
}
