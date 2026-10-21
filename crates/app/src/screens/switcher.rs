//! The room switcher (Ctrl+K, plan Q18, Task 16; restyled in UI/UX rework
//! Task 15): a command palette near the top of the window with a large field
//! and the ranked rooms.
//!
//! Like the other overlays it is a layer that is always a stack of two (the
//! base first), so opening it keeps the timeline's scroll offset and the
//! composer's state. Room names come from other people and pass through
//! `elide_middle` (cleaned, and cut in the middle at [`NAME_CHARS`], so the
//! end stays) before they are shown. Rows are all one height, which
//! `App::scroll_to_selected` relies on. Views only read the app state; every
//! change goes through `App::update`.

use iced::widget::{Column, button, column, container, row, scrollable, space, stack, text_input};
use iced::{Element, Fill, Length};
use twicecat_core::api::{RoomSummary, UserId};

use crate::room_list::badge_for;
use crate::screens::room_list_pane::{badge_view, call_pill};
use crate::state::{App, Message};
use crate::switcher::{INPUT_ID, LIST_ID, SwitcherMessage};
use crate::ui::kit::{self, empty_state, muted, palette, words};
use crate::ui::tokens;
use crate::verification::elide_middle;

/// Width of the card.
pub const CARD_WIDTH: f32 = 560.0;
/// Height of every row.
pub const ROW_HEIGHT: f32 = 48.0;
/// Tallest the list grows before it scrolls.
const LIST_MAX_HEIGHT: f32 = 400.0;
/// Longest room name a row shows, in characters; a longer one is cut in the
/// middle, so its end stays.
pub const NAME_CHARS: usize = 60;
/// The hint under the list.
pub const HINT: &str = "Up and Down to move, Enter to open, Esc to close";

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

/// A room's name as a row shows it: cleaned, and cut in the middle when long.
pub fn row_title(room: &RoomSummary) -> String {
    elide_middle(&room.name, NAME_CHARS)
}

fn room_row<'a>(app: &'a App, room: &'a RoomSummary, selected: bool) -> Element<'a, Message> {
    let name = row_title(room);
    let avatar = kit::avatar_sized(
        &UserId::from(room.id.as_str()),
        &name,
        room.avatar.as_ref(),
        &app.avatars,
        32.0,
    );
    let content = row![
        avatar,
        // Two lines of the body size fit a row; a name has no more room than
        // that (it is capped), and it breaks inside a word, so it never runs
        // under the call pill or the badge.
        words(name).size(tokens::BODY).width(Fill),
        call_pill(app.calls.pill(&room.id)),
        badge_view(badge_for(room), false),
    ]
    .spacing(tokens::M)
    .align_y(iced::Center);
    button(content)
        .width(Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .padding([tokens::XS, tokens::M])
        .on_press(msg(SwitcherMessage::Pick(room.id.clone())))
        .style(if selected {
            kit::active_icon_style
        } else {
            kit::ghost_style
        })
        .into()
}

fn view(app: &App) -> Element<'_, Message> {
    let field = text_input("Go to a room\u{2026}", &app.switcher.query)
        .style(kit::text_input_style)
        .id(INPUT_ID)
        .on_input(|text| msg(SwitcherMessage::Input(text)))
        .on_submit(msg(SwitcherMessage::Submit))
        .size(tokens::TITLE)
        .padding(tokens::M);
    let results = app.switcher.results(&app.rooms);
    let mut rows = Column::new().spacing(tokens::XS);
    for (at, id) in results.iter().enumerate() {
        if let Some(room) = app.switcher.name(&app.rooms, id) {
            rows = rows.push(room_row(app, room, at == app.switcher.selected));
        }
    }
    let list: Element<'_, Message> = if results.is_empty() {
        empty_state("No room matches", "Try part of a name.")
    } else {
        container(
            scrollable(rows)
                .id(LIST_ID)
                .spacing(tokens::S)
                .height(Length::Shrink),
        )
        .max_height(LIST_MAX_HEIGHT)
        .into()
    };
    palette(
        column![field, list, words(HINT).size(tokens::CAPTION).style(muted)].spacing(tokens::M),
        CARD_WIDTH,
    )
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{RoomId, RoomSummary};

    use super::*;

    #[test]
    fn a_long_room_name_is_cut_in_the_middle_so_its_end_stays() {
        let room = RoomSummary::new(
            RoomId::from("!a:x"),
            format!("{} the end", "word ".repeat(100)),
        );
        let shown = row_title(&room);
        assert_eq!(shown.chars().count(), NAME_CHARS);
        assert!(shown.ends_with(" the end"), "{shown}");
        let plain = RoomSummary::new(RoomId::from("!b:x"), "Lounge".to_owned());
        assert_eq!(row_title(&plain), "Lounge");
        let blank = RoomSummary::new(RoomId::from("!c:x"), "  ".to_owned());
        assert_eq!(row_title(&blank), crate::text_safety::NAME_PLACEHOLDER);
    }

    #[test]
    fn the_hint_names_the_keys_that_work() {
        for key in ["Up and Down", "Enter", "Esc"] {
            assert!(HINT.contains(key), "{HINT}");
        }
    }
}
