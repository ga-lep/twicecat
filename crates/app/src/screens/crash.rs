//! The bar about a crash report an earlier run left (Phase 7 Task 10).

use iced::Element;

use crate::crash::{BAR_TEXT, CrashMessage};
use crate::state::{App, Message};
use crate::ui::kit::{Severity, banner_with, ghost_button, primary_button, secondary_button};

fn msg(message: CrashMessage) -> Message {
    Message::Crash(message)
}

/// The bar, for the main view's banner slot (none without a report).
pub fn bars(app: &App) -> Vec<Element<'_, Message>> {
    if app.crash.is_none() {
        return Vec::new();
    }
    vec![banner_with(
        Severity::Warning,
        BAR_TEXT,
        vec![
            ghost_button("Delete", Some(msg(CrashMessage::Delete))).into(),
            secondary_button("Show report", Some(msg(CrashMessage::Show))).into(),
            primary_button("Open an issue", Some(msg(CrashMessage::OpenIssue))).into(),
        ],
    )]
}
