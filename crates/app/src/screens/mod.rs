//! The screens of the window: login, loading and the two-pane main layout.
//!
//! Views only read the [`App`](crate::state::App) state and build widgets;
//! every state change goes through `App::update`.

pub mod audio_settings;
pub mod call;
pub mod crash;
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
pub mod stage;
pub mod switcher;
pub mod thread_panel;
pub mod timeline_pane;
pub mod updates;
pub mod upload_dialog;
pub mod verification;
pub mod video_settings;

use iced::widget::container;
use iced::{Element, Fill};

use crate::state::Message;

/// A short title and a hint in the middle of the window (starting, closing
/// and a stopped core).
pub fn centered_state<'a>(title: &'a str, hint: &'a str) -> Element<'a, Message> {
    container(crate::ui::kit::empty_state(title, hint))
        .center_x(Fill)
        .center_y(Fill)
        .into()
}
