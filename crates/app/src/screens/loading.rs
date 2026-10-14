//! The loading screen shown during the first sync.

use iced::widget::{column, text};
use iced::{Element, Fill};
use twicecat_core::api::SyncState;

use crate::screens::login;
use crate::state::{App, Message};
use crate::ui::kit::{self, Severity};
use crate::ui::tokens;

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

/// How far the first load is, from 0 to 1. The only events that arrive
/// before the first room does are the sync states, so those are the steps:
/// starting, then running (waiting for the room list). A sync that is
/// offline, retrying or stopped does not advance, and its bar stays short.
pub fn progress(state: &SyncState) -> f32 {
    match state {
        SyncState::Idle => 0.1,
        SyncState::Running => 0.6,
        SyncState::Offline
        | SyncState::Reconnecting { .. }
        | SyncState::Error(_)
        | SyncState::Terminated => 0.3,
    }
}

/// The loading screen: the sync state, a bar and the rooms loaded so far.
pub fn view(app: &App) -> Element<'_, Message> {
    let fill = progress(&app.sync_state);
    login::centered_card(
        column![
            text("Loading your rooms\u{2026}").size(tokens::HEADLINE),
            kit::meter_bar(fill, Severity::Info),
            text(format!("Sync: {}", sync_label(&app.sync_state))),
            text(room_count_label(app.rooms.len()))
                .size(tokens::CAPTION)
                .style(kit::muted),
        ]
        .spacing(tokens::M)
        .width(Fill),
    )
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

    #[test]
    fn progress_steps_with_the_sync_and_stays_between_0_and_1() {
        assert!(progress(&SyncState::Idle) < progress(&SyncState::Running));
        for state in [
            SyncState::Idle,
            SyncState::Running,
            SyncState::Offline,
            SyncState::Reconnecting { attempt: 2 },
            SyncState::Error("x".into()),
            SyncState::Terminated,
        ] {
            let v = progress(&state);
            assert!(v > 0.0 && v < 1.0, "{state:?} {v}");
        }
        // A sync that is not running is behind one that is.
        assert!(progress(&SyncState::Offline) < progress(&SyncState::Running));
    }
}
