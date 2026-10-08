//! The member list panel (plan Q13, Task 12): who is in the room, with a
//! role badge, plus **Invite**. Display only; changing power levels is not
//! part of Phase 5.

use iced::widget::{Column, button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Fill};
use tessera_core::api::MessageText;

use crate::rooms::{MembersPanel, RoomsMessage, member_lines, role_badge};
use crate::screens::room_dialogs::{error_line, modal, msg, muted, person_avatar};
use crate::state::{App, Message};

/// Width of the panel.
const PANEL_WIDTH: f32 = 520.0;
/// Tallest the list grows before it scrolls.
const LIST_HEIGHT: f32 = 360.0;
/// The note under a list that was cut.
pub const TRUNCATED_NOTE: &str = "Showing the first 5,000 members";

/// The title: how many members the list holds.
pub fn title_text(listed: Option<usize>, truncated: bool) -> String {
    match listed {
        None => "Members".to_owned(),
        Some(n) if truncated => format!("Members ({n}+)"),
        Some(n) => format!("Members ({n})"),
    }
}

/// The panel, when it is open.
pub fn view(app: &App) -> Option<Element<'_, Message>> {
    let panel: &MembersPanel = app.rooms_ui.members_panel.as_ref()?;
    let known = app.rooms_ui.known.as_ref().filter(|k| k.room == panel.room);
    let details = app
        .rooms_ui
        .details
        .as_ref()
        .filter(|d| d.room_id == panel.room);
    let can_invite = details.is_some_and(|d| d.permissions.can_invite);

    let header = row![
        text(title_text(
            known.map(|k| k.members.len()),
            known.is_some_and(|k| k.truncated)
        ))
        .size(22),
        space().width(Fill),
        button(text("\u{2715}"))
            .on_press(msg(RoomsMessage::CloseMembers))
            .style(button::secondary),
    ]
    .align_y(iced::Center);

    let invite = row![
        text_input("Invite by user ID, @name:server", &panel.invite)
            .on_input_maybe(
                can_invite
                    .then_some(|s: String| msg(RoomsMessage::InviteInput(MessageText::from(s))))
            )
            .on_submit_maybe(can_invite.then(|| msg(RoomsMessage::InviteSubmit)))
            .padding(6),
        button(text("Invite")).on_press_maybe(can_invite.then(|| msg(RoomsMessage::InviteSubmit))),
    ]
    .spacing(8);
    let invite_note: Element<'_, Message> = if details.is_none() {
        text("Checking what you may do\u{2026}")
            .size(12)
            .style(muted)
            .into()
    } else if !can_invite {
        text("You cannot invite people to this room.")
            .size(12)
            .style(muted)
            .into()
    } else {
        error_line(panel.status)
    };

    let mut list = Column::new().spacing(6);
    match known {
        None => list = list.push(text("Loading members\u{2026}").style(muted)),
        Some(known) => {
            for member in known.members.iter().take(panel.shown) {
                let (name, id) = member_lines(member);
                let mut who = Column::new().width(Fill);
                who = who.push(text(name.clone()).wrapping(text::Wrapping::None));
                if id != name {
                    who = who.push(
                        text(id)
                            .size(12)
                            .style(muted)
                            .wrapping(text::Wrapping::None),
                    );
                }
                let mut line = row![
                    person_avatar(&member.user_id, &name, member.avatar.as_ref(), &app.avatars),
                    who
                ]
                .spacing(10)
                .align_y(iced::Center);
                if let Some(badge) = role_badge(member) {
                    line = line.push(text(badge).size(12).style(muted));
                }
                list = list.push(line);
            }
            if known.members.len() > panel.shown {
                list = list.push(
                    button(text("Show more"))
                        .on_press(msg(RoomsMessage::ShowMoreMembers))
                        .style(button::secondary),
                );
            }
            if known.truncated {
                list = list.push(text(TRUNCATED_NOTE).size(12).style(muted));
            }
        }
    }

    let body = column![
        header,
        invite,
        invite_note,
        container(scrollable(list)).height(LIST_HEIGHT).width(Fill),
    ]
    .spacing(10);
    Some(modal(body, PANEL_WIDTH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_counts_the_listed_members_and_marks_a_cut_list() {
        assert_eq!(title_text(None, false), "Members");
        assert_eq!(title_text(Some(3), false), "Members (3)");
        assert_eq!(title_text(Some(5000), true), "Members (5000+)");
    }

    #[test]
    fn the_cut_list_note_says_5000() {
        assert_eq!(TRUNCATED_NOTE, "Showing the first 5,000 members");
    }
}
