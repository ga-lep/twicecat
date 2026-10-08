//! System dialogs behind a trait, so `update` never opens a window and tests
//! use a fake (plan Q8, Task 9).
//!
//! The real implementation asks the desktop for a save dialog (the XDG portal
//! on Linux) parented to the app window. The path it returns is what the user
//! chose; nothing the sender wrote is ever joined onto it.

use std::path::{Path, PathBuf};

use iced::Task;
use iced::window;
use rfd::AsyncFileDialog;

/// What the app asks the desktop to show.
pub trait Dialogs: Send {
    /// A save dialog proposing `suggested_name`. The task yields the chosen
    /// path, or `None` when the user cancelled. The name is already
    /// sanitised by [`crate::media::suggested_file_name`].
    fn save_file(&self, suggested_name: &str) -> Task<Option<PathBuf>>;
}

/// The desktop's own dialogs through `rfd`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDialogs;

impl Dialogs for SystemDialogs {
    fn save_file(&self, suggested_name: &str) -> Task<Option<PathBuf>> {
        let name = suggested_name.to_owned();
        // The parent is read on the window thread (`window::run`); the
        // dialog itself then runs as an ordinary future.
        window::oldest()
            .then(move |id| {
                let name = name.clone();
                let build = move |parent: Option<&dyn window::Window>| {
                    let dialog = AsyncFileDialog::new()
                        .set_title("Save file")
                        .set_file_name(name);
                    match parent {
                        Some(window) => dialog.set_parent(window),
                        None => dialog,
                    }
                };
                match id {
                    Some(id) => window::run(id, move |window| build(Some(window))),
                    None => Task::done(build(None)),
                }
            })
            .then(|dialog| {
                Task::future(
                    async move { dialog.save_file().await.map(|h| h.path().to_path_buf()) },
                )
            })
    }
}

/// Asks the system to open a saved file with its default program. `true`
/// when the handler started. Callers ask the user first, with a warning for
/// a file that may run code (see [`crate::media::may_run_code`]).
pub fn open_file(path: &Path) -> bool {
    open::that_detached(path).is_ok()
}

#[cfg(test)]
pub use fake::RecordingDialogs;

#[cfg(test)]
mod fake {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use iced::Task;

    use super::Dialogs;

    /// A fake that records the proposed names and shows nothing. Tests send
    /// the user's answer as a message themselves.
    #[derive(Debug, Clone, Default)]
    pub struct RecordingDialogs {
        /// The names proposed so far.
        pub asked: Arc<Mutex<Vec<String>>>,
    }

    impl Dialogs for RecordingDialogs {
        fn save_file(&self, suggested_name: &str) -> Task<Option<PathBuf>> {
            self.asked
                .lock()
                .expect("test mutex")
                .push(suggested_name.to_owned());
            Task::none()
        }
    }
}
