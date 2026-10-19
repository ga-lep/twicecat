//! The update bars at the top of the main view (Phase 7 Task 9): the
//! question asked once, the newer version with its release page (and, in an
//! AppImage that can update itself, its install), the restart once it is
//! installed, and a failed install.

use iced::Element;

use crate::state::{App, Message};
use crate::ui::kit::{Severity, banner_with, ghost_button, primary_button};
use crate::updates::{FAILED, QUESTION, UpdateMessage};

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
    if let Some(version) = &app.updates.installed {
        bars.push(banner_with(
            Severity::Success,
            format!("Twicecat {version} is installed. It starts when Twicecat restarts."),
            vec![primary_button("Restart now", Some(msg(UpdateMessage::Restart))).into()],
        ));
    } else if let Some(available) = &app.updates.available {
        let release_page = Some(msg(UpdateMessage::OpenRelease));
        let mut actions: Vec<Element<'_, Message>> =
            vec![ghost_button("Dismiss", Some(msg(UpdateMessage::Dismiss))).into()];
        if available.installable {
            actions.push(ghost_button("Open the release page", release_page).into());
            let (label, install) = if app.updates.installing {
                ("Installing\u{2026}", None)
            } else {
                ("Install the update", Some(msg(UpdateMessage::Install)))
            };
            actions.push(primary_button(label, install).into());
        } else {
            actions.push(primary_button("Open the release page", release_page).into());
        }
        bars.push(banner_with(Severity::Info, available.text(), actions));
    }
    if app.updates.install_failed {
        bars.push(banner_with(
            Severity::Danger,
            FAILED,
            vec![ghost_button("Dismiss", Some(msg(UpdateMessage::Dismiss))).into()],
        ));
    }
    bars
}
