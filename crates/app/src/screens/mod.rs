//! The screens of the window: login, loading and the two-pane main layout.
//!
//! Views only read the [`App`](crate::state::App) state and build widgets;
//! every state change goes through `App::update`.

pub mod loading;
pub mod login;
pub mod main_view;
pub mod media_viewer;
pub mod members;
pub mod room_dialogs;
pub mod room_list_pane;
pub mod room_settings;
pub mod security;
pub mod sessions;
pub mod settings;
pub mod spaces_sidebar;
pub mod thread_panel;
pub mod timeline_pane;
pub mod upload_dialog;
pub mod verification;

use iced::widget::{container, text};
use iced::{Element, Fill};

use crate::state::Message;

/// A single line of text in the middle of the window.
pub fn centered_text(label: &str) -> Element<'_, Message> {
    container(text(label).size(20))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
