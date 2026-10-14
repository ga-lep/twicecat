//! The room settings panel (plan Q13, Task 12; restyled in UI/UX rework Task
//! 15): name, topic, picture, turning encryption on, and leaving. Each
//! control works only when the room's power levels allow it
//! (`RoomDetails.permissions`).
//!
//! The two questions that cannot be undone (turn encryption on, leave) keep
//! their confirmation: the button that asks sits at the left, in a different
//! place from the one that confirms, which is red, has words, and sits at the
//! right with at least [`tokens::M`] between it and **Cancel**. Esc cancels the
//! question first and only then closes the panel.

use iced::widget::{Column, column, container, responsive, row, text_input};
use iced::{Element, Fill};
use twicecat_core::api::MessageText;

use crate::room_list::row_name;
use crate::rooms::{RoomsMessage, SettingsPanel};
use crate::screens::room_dialogs::{error_line, msg};
use crate::state::{App, Message};
use crate::ui::kit::{
    self, danger_button, dialog, dialog_footer, dialog_header, divider, field, muted,
    secondary_button, words,
};
use crate::ui::tokens;
use crate::verification::elide_middle;

/// Longest address shown, in characters; a longer one is cut in the middle,
/// so its server name stays.
const ADDRESS_CHARS: usize = 128;
/// Width of the panel.
pub const PANEL_WIDTH: f32 = 520.0;
/// The note under the picture button of an encrypted room.
pub const AVATAR_NOTE: &str =
    "Room pictures are stored unencrypted on the server, even in an encrypted room.";
/// The warning before turning encryption on.
pub const ENCRYPT_WARNING: &str = "Turning on encryption cannot be undone. Messages sent \
    before stay readable by the server, and bots or bridges that cannot use encryption may \
    stop working in this room.";

/// The panel, when it is open.
pub fn view(app: &App) -> Option<Element<'_, Message>> {
    let panel: &SettingsPanel = app.rooms_ui.settings.as_ref()?;
    Some(responsive(move |size| card(app, panel, size.height)).into())
}

/// The name of the panel's room as the room list has it (cleaned), or its
/// ID, cut in the middle.
fn room_title(app: &App, panel: &SettingsPanel) -> String {
    app.rooms
        .iter()
        .find(|room| room.id == panel.room)
        .map_or_else(
            || crate::verification::elide_middle(panel.room.as_str(), 64),
            |room| row_name(&room.name),
        )
}

fn card<'a>(app: &'a App, panel: &'a SettingsPanel, window_height: f32) -> Element<'a, Message> {
    let details = app
        .rooms_ui
        .details
        .as_ref()
        .filter(|d| d.room_id == panel.room);
    let perms = details.map(|d| d.permissions).unwrap_or_default();

    // The title says which room: a settings panel for the wrong room is the
    // mistake to make visible.
    let header = column![
        dialog_header("Room settings", msg(RoomsMessage::CloseSettings)),
        words(room_title(app, panel))
            .size(tokens::BODY_LARGE)
            .style(muted),
    ]
    .spacing(tokens::XS)
    .into();

    let save_row = |label: &'static str,
                    value: &str,
                    allowed: bool,
                    input: fn(MessageText) -> RoomsMessage,
                    save: RoomsMessage| {
        field(
            label,
            row![
                text_input(label, value)
                    .on_input_maybe(
                        allowed.then_some(move |s: String| msg(input(MessageText::from(s)))),
                    )
                    .on_submit_maybe(allowed.then(|| msg(save.clone())))
                    .padding(tokens::S),
                secondary_button("Save", allowed.then(|| msg(save))),
            ]
            .spacing(tokens::M)
            .align_y(iced::Center),
        )
    };

    let mut body = Column::new().spacing(tokens::M).width(Fill);
    body = body.push(slot(
        details
            .is_none()
            .then(|| words("Loading\u{2026}").style(muted).into()),
    ));
    body = body
        .push(save_row(
            "Name",
            &panel.name,
            perms.can_set_name,
            RoomsMessage::NameInput,
            RoomsMessage::SaveName,
        ))
        .push(save_row(
            "Topic",
            &panel.topic,
            perms.can_set_topic,
            RoomsMessage::TopicInput,
            RoomsMessage::SaveTopic,
        ))
        .push(secondary_button(
            if panel.avatar_busy {
                "Working\u{2026}"
            } else {
                "Change picture\u{2026}"
            },
            (perms.can_set_avatar && !panel.avatar_busy).then(|| msg(RoomsMessage::ChooseAvatar)),
        ));
    body =
        body.push(slot(details.is_some_and(|d| d.is_encrypted).then(|| {
            words(AVATAR_NOTE).size(tokens::CAPTION).style(muted).into()
        })));
    // Cleaned and capped, and wrapped inside a word, so the end of the
    // address is never cut off.
    body = body.push(slot(details.and_then(|d| d.alias.as_deref()).map(
        |alias| {
            words(format!("Address: {}", elide_middle(alias, ADDRESS_CHARS)))
                .size(tokens::BODY)
                .style(muted)
                .into()
        },
    )));
    body = body.push(encryption_section(
        panel,
        details.map(|d| d.is_encrypted),
        perms.can_enable_encryption,
    ));
    body = body.push(error_line(panel.error));
    body = body.push(divider());
    body = body.push(leave_section(app, panel));
    let footer = dialog_footer(vec![
        secondary_button("Close", Some(msg(RoomsMessage::CloseSettings))).into(),
    ]);
    dialog(
        header,
        body.into(),
        Some(footer),
        PANEL_WIDTH,
        window_height,
    )
}

/// `part`, or an empty slot: a column keeps its widget types as parts come
/// and go.
fn slot<'a>(part: Option<Element<'a, Message>>) -> Element<'a, Message> {
    part.unwrap_or_else(|| iced::widget::space().into())
}

/// A question that cannot be undone: a titled card with its words, and
/// **Cancel** then the red action, `tokens::M` apart.
fn question<'a>(
    title: &'static str,
    lines: Vec<String>,
    cancel: RoomsMessage,
    confirm_label: &'static str,
    confirm: RoomsMessage,
) -> Element<'a, Message> {
    let mut body = Column::new()
        .spacing(tokens::S)
        .width(Fill)
        .push(words(title).size(tokens::TITLE));
    for line in lines {
        body = body.push(words(line).size(tokens::BODY));
    }
    body = body.push(dialog_footer(vec![
        secondary_button("Cancel", Some(msg(cancel))).into(),
        danger_button(confirm_label, Some(msg(confirm))).into(),
    ]));
    container(body)
        .padding(tokens::M)
        .width(Fill)
        .style(kit::card_style)
        .into()
}

fn encryption_section(
    panel: &SettingsPanel,
    encrypted: Option<bool>,
    allowed: bool,
) -> Element<'_, Message> {
    match encrypted {
        Some(true) => words("This room is encrypted. This cannot be turned off.")
            .size(tokens::BODY)
            .style(muted)
            .into(),
        None => iced::widget::space().into(),
        Some(false) if panel.encrypt_ask => question(
            "Turn on encryption?",
            vec![ENCRYPT_WARNING.to_owned()],
            RoomsMessage::EncryptCancel,
            "Enable encryption",
            RoomsMessage::EncryptConfirm,
        ),
        Some(false) => row![secondary_button(
            "Enable encryption\u{2026}",
            allowed.then(|| msg(RoomsMessage::EncryptAsk)),
        )]
        .into(),
    }
}

fn leave_section<'a>(app: &'a App, panel: &SettingsPanel) -> Element<'a, Message> {
    if !panel.leave_ask {
        // The question's button is at the left; the red **Leave** that
        // confirms comes up at the right, so one click never does both.
        return row![danger_button(
            "Leave room\u{2026}",
            Some(msg(RoomsMessage::LeaveAsk))
        )]
        .into();
    }
    question(
        "Leave this room?",
        app.leave_warning_now()
            .lines()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        RoomsMessage::LeaveCancel,
        "Leave",
        RoomsMessage::LeaveConfirm,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_warnings_say_what_cannot_be_undone() {
        assert!(ENCRYPT_WARNING.contains("cannot be undone"));
        assert!(AVATAR_NOTE.contains("unencrypted"));
    }
}
