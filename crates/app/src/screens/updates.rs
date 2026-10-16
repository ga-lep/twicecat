//! The update bars at the top of the main view (Phase 7 Task 9): the
//! question asked once, and the newer version with its release page.

use iced::Element;

use crate::state::{App, Message};
use crate::ui::kit::{Severity, banner_with, ghost_button, primary_button};
use crate::updates::{QUESTION, UpdateMessage};

fn msg(message: UpdateMessage) -> Message {
    Message::Update(message)
}

/// The update bars, for the main view's banner slot (none when there is
/// nothing to say).
pub fn bars(app: &App) -> Vec<Element<'_, Message>> {
    let mut bars: Vec<Element<'_, Message>> = Vec::new();
    if app.update_question_shown() {
        bars.push(banner_with(
            Severity::Info,
            QUESTION,
            vec![
                ghost_button("No thanks", Some(msg(UpdateMessage::Choose(false)))).into(),
                primary_button("Check daily", Some(msg(UpdateMessage::Choose(true)))).into(),
            ],
        ));
    }
    if let Some(available) = &app.updates.available {
        bars.push(banner_with(
            Severity::Info,
            available.text(),
            vec![
                ghost_button("Dismiss", Some(msg(UpdateMessage::Dismiss))).into(),
                primary_button(
                    "Open the release page",
                    Some(msg(UpdateMessage::OpenRelease)),
                )
                .into(),
            ],
        ));
    }
    bars
}
