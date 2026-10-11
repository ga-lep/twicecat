//! The loading screen shown during the first sync.

use iced::widget::{column, text};
use iced::{Element, Fill};
use twicecat_core::api::SyncState;

use crate::state::{App, Message};

/// A short, non-secret line for the sync state.
pub fn sync_label(state: &SyncState) -> String {
    match state {
        SyncState::Idle => "Starting sync".to_owned(),
        SyncState::Running => "Running".to_owned(),
        SyncState::Offline => "Offline".to_owned(),
        SyncState::Reconnecting { attempt } => format!("Reconnecting (attempt {attempt})"),
        SyncState::Error(_) => "Sync error".to_owned(),
        SyncState::Terminated => "Stopped".to_owned(),
    }
}

/// How many rooms are loaded so far.
pub fn room_count_label(count: usize) -> String {
    match count {
        1 => "1 room loaded".to_owned(),
        n => format!("{n} rooms loaded"),
    }
}

/// The loading screen: the sync state and the rooms loaded so far.
pub fn view(app: &App) -> Element<'_, Message> {
    let content = column![
        text("Loading your rooms\u{2026}").size(24),
        text(format!("Sync: {}", sync_label(&app.sync_state))),
        text(room_count_label(app.rooms.len())),
    ]
    .spacing(12)
    .align_x(iced::Center);
    iced::widget::container(content)
        .center_x(Fill)
        .center_y(Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_labels_name_every_state() {
        assert_eq!(sync_label(&SyncState::Running), "Running");
        assert_eq!(sync_label(&SyncState::Offline), "Offline");
        assert_eq!(
            sync_label(&SyncState::Reconnecting { attempt: 3 }),
            "Reconnecting (attempt 3)"
        );
        assert_eq!(sync_label(&SyncState::Idle), "Starting sync");
        assert_eq!(sync_label(&SyncState::Terminated), "Stopped");
        assert_eq!(sync_label(&SyncState::Error("x".into())), "Sync error");
    }

    #[test]
    fn room_count_label_pluralises() {
        assert_eq!(room_count_label(0), "0 rooms loaded");
        assert_eq!(room_count_label(1), "1 room loaded");
        assert_eq!(room_count_label(2000), "2000 rooms loaded");
    }
}
