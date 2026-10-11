//! The room switcher (Ctrl+K, plan Q18, Task 16): a card with a field and
//! the ranked rooms.
//!
//! Like the other overlays it is a layer that is always a stack of two (the
//! base first), so opening it keeps the timeline's scroll offset and the
//! composer's state. Room names come from other people and pass through
//! `room_list::row_name` before they are shown. Views only read the app
//! state; every change goes through `App::update`.

use iced::widget::{Column, button, column, container, scrollable, space, stack, text, text_input};
use iced::{Element, Fill};

use crate::room_list::row_name;
use crate::screens::room_dialogs::{modal, muted};
use crate::state::{App, Message};
use crate::switcher::{INPUT_ID, LIST_ID, SwitcherMessage};

/// Width of the card.
const CARD_WIDTH: f32 = 480.0;

fn msg(message: SwitcherMessage) -> Message {
    Message::Switcher(message)
}

/// The base with the switcher on top when it is open: always a stack of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let top = if app.switcher.open {
        view(app)
    } else {
        space().into()
    };
    stack([base, top]).into()
}

fn view(app: &App) -> Element<'_, Message> {
    let field = text_input("Go to a room\u{2026}", &app.switcher.query)
        .id(INPUT_ID)
        .on_input(|text| msg(SwitcherMessage::Input(text)))
        .on_submit(msg(SwitcherMessage::Submit))
        .padding(8);
    let results = app.switcher.results(&app.rooms);
    let mut rows = Column::new().spacing(4);
    for (at, id) in results.iter().enumerate() {
        let name = app
            .switcher
            .name(&app.rooms, id)
            .map_or_else(String::new, |room| row_name(&room.name));
        rows = rows.push(
            button(text(name))
                .width(Fill)
                .on_press(msg(SwitcherMessage::Pick(id.clone())))
                .style(if at == app.switcher.selected {
                    button::primary
                } else {
                    button::secondary
                }),
        );
    }
    let list: Element<'_, Message> = if results.is_empty() {
        text("No room matches.").size(13).style(muted).into()
    } else {
        scrollable(rows)
            .id(LIST_ID)
            .height(iced::Length::Shrink)
            .into()
    };
    modal(
        container(column![field, list].spacing(10)).max_height(480.0),
        CARD_WIDTH,
    )
}
