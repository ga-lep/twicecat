//! The room settings panel (plan Q13, Task 12): name, topic, picture,
//! turning encryption on, and leaving. Each control works only when the
//! room's power levels allow it (`RoomDetails.permissions`).

use iced::widget::{Column, button, column, container, row, scrollable, space, text, text_input};
use iced::{Element, Fill};
use twicecat_core::api::MessageText;

use crate::rooms::{RoomsMessage, SettingsPanel};
use crate::screens::room_dialogs::{error_line, modal, msg, muted};
use crate::state::{App, Message};
use crate::text_safety::one_line_capped;

/// Width of the panel.
const PANEL_WIDTH: f32 = 520.0;
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
    let details = app
        .rooms_ui
        .details
        .as_ref()
        .filter(|d| d.room_id == panel.room);
    let perms = details.map(|d| d.permissions).unwrap_or_default();

    let header = row![
        text("Room settings").size(22),
        space().width(Fill),
        button(text("\u{2715}"))
            .on_press(msg(RoomsMessage::CloseSettings))
            .style(button::secondary),
    ]
    .align_y(iced::Center);

    let field = |label: &'static str,
                 value: &str,
                 allowed: bool,
                 input: fn(MessageText) -> RoomsMessage,
                 save: RoomsMessage| {
        row![
            text_input(label, value)
                .on_input_maybe(
                    allowed.then_some(move |s: String| msg(input(MessageText::from(s)))),
                )
                .on_submit_maybe(allowed.then(|| msg(save.clone())))
                .padding(6),
            button(text("Save")).on_press_maybe(allowed.then(|| msg(save))),
        ]
        .spacing(8)
    };

    let mut body = Column::new().spacing(10).push(header);
    if details.is_none() {
        body = body.push(text("Loading\u{2026}").style(muted));
    }
    body = body
        .push(field(
            "Name",
            &panel.name,
            perms.can_set_name,
            RoomsMessage::NameInput,
            RoomsMessage::SaveName,
        ))
        .push(field(
            "Topic",
            &panel.topic,
            perms.can_set_topic,
            RoomsMessage::TopicInput,
            RoomsMessage::SaveTopic,
        ))
        .push(
            row![
                button(text(if panel.avatar_busy {
                    "Working\u{2026}"
                } else {
                    "Change picture\u{2026}"
                }))
                .on_press_maybe(
                    (perms.can_set_avatar && !panel.avatar_busy)
                        .then(|| msg(RoomsMessage::ChooseAvatar)),
                )
                .style(button::secondary),
            ]
            .spacing(8),
        );
    if details.is_some_and(|d| d.is_encrypted) {
        body = body.push(text(AVATAR_NOTE).size(12).style(muted));
    }
    if let Some(alias) = details.and_then(|d| d.alias.as_deref()) {
        body = body.push(
            text(format!("Address: {}", one_line_capped(alias, 128)))
                .size(13)
                .style(muted),
        );
    }
    body = body.push(encryption_section(
        panel,
        details.map(|d| d.is_encrypted),
        perms.can_enable_encryption,
    ));
    body = body.push(error_line(panel.error));
    body = body.push(leave_section(app, panel));
    Some(modal(
        container(scrollable(body)).max_height(560.0),
        PANEL_WIDTH,
    ))
}

fn encryption_section(
    panel: &SettingsPanel,
    encrypted: Option<bool>,
    allowed: bool,
) -> Element<'_, Message> {
    match encrypted {
        Some(true) => text("This room is encrypted. This cannot be turned off.")
            .size(13)
            .style(muted)
            .into(),
        None => space().into(),
        Some(false) if panel.encrypt_ask => container(
            column![
                text("Turn on encryption?").size(16),
                text(ENCRYPT_WARNING).size(13),
                row![
                    space().width(Fill),
                    button(text("Cancel"))
                        .on_press(msg(RoomsMessage::EncryptCancel))
                        .style(button::secondary),
                    button(text("Enable encryption"))
                        .on_press(msg(RoomsMessage::EncryptConfirm))
                        .style(button::danger),
                ]
                .spacing(8),
            ]
            .spacing(8),
        )
        .padding(10)
        .width(Fill)
        .style(container::bordered_box)
        .into(),
        Some(false) => button(text("Enable encryption\u{2026}"))
            .on_press_maybe(allowed.then(|| msg(RoomsMessage::EncryptAsk)))
            .style(button::secondary)
            .into(),
    }
}

fn leave_section<'a>(app: &'a App, panel: &SettingsPanel) -> Element<'a, Message> {
    if !panel.leave_ask {
        return button(text("Leave room\u{2026}"))
            .on_press(msg(RoomsMessage::LeaveAsk))
            .style(button::danger)
            .into();
    }
    let mut lines = Column::new()
        .spacing(6)
        .push(text("Leave this room?").size(16));
    for line in app.leave_warning_now().lines() {
        lines = lines.push(text(line).size(13));
    }
    lines = lines.push(
        row![
            space().width(Fill),
            button(text("Cancel"))
                .on_press(msg(RoomsMessage::LeaveCancel))
                .style(button::secondary),
            button(text("Leave"))
                .on_press(msg(RoomsMessage::LeaveConfirm))
                .style(button::danger),
        ]
        .spacing(8),
    );
    container(lines)
        .padding(10)
        .width(Fill)
        .style(container::bordered_box)
        .into()
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
