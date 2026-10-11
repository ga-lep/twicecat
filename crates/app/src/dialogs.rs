//! System dialogs and the clipboard behind traits, so `update` never opens a
//! window or touches the desktop and tests use fakes (plan Q8, Q9, Tasks 9
//! and 10).
//!
//! The real implementation asks the desktop for a save or open dialog (the
//! XDG portal on Linux) parented to the app window. The path a save dialog
//! returns is what the user chose; nothing the sender wrote is ever joined
//! onto it. The clipboard is read by `arboard` on a worker thread.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::Task;
use iced::window;
use rfd::AsyncFileDialog;
use twicecat_core::api::MessageText;

use crate::blocking;

/// What the app asks the desktop to show.
pub trait Dialogs: Send {
    /// A save dialog proposing `suggested_name`. The task yields the chosen
    /// path, or `None` when the user cancelled. The name is already
    /// sanitised by [`crate::media::suggested_file_name`].
    fn save_file(&self, suggested_name: &str) -> Task<Option<PathBuf>>;

    /// A dialog to choose files to send. The task yields the chosen paths,
    /// empty when the user cancelled.
    fn pick_files(&self) -> Task<Vec<PathBuf>>;

    /// A dialog to choose one picture (a room picture). The task yields the
    /// chosen path, or `None` when the user cancelled.
    fn pick_image(&self) -> Task<Option<PathBuf>>;
}

/// The desktop's own dialogs through `rfd`.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemDialogs;

/// Builds a dialog parented to the oldest window, when there is one, and
/// runs `show` on it.
fn parented<T: Send + 'static>(
    title: &'static str,
    file_name: Option<String>,
    show: impl Fn(AsyncFileDialog) -> iced::futures::future::BoxFuture<'static, T>
    + Clone
    + Send
    + 'static,
) -> Task<T> {
    window::oldest()
        .then(move |id| {
            let file_name = file_name.clone();
            let build = move |parent: Option<&dyn window::Window>| {
                let mut dialog = AsyncFileDialog::new().set_title(title);
                if let Some(name) = file_name {
                    dialog = dialog.set_file_name(name);
                }
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
        .then(move |dialog| Task::future(show(dialog)))
}

impl Dialogs for SystemDialogs {
    fn save_file(&self, suggested_name: &str) -> Task<Option<PathBuf>> {
        // The parent is read on the window thread (`window::run`); the
        // dialog itself then runs as an ordinary future.
        parented("Save file", Some(suggested_name.to_owned()), |dialog| {
            Box::pin(async move { dialog.save_file().await.map(|h| h.path().to_path_buf()) })
        })
    }

    fn pick_files(&self) -> Task<Vec<PathBuf>> {
        parented("Choose files to send", None, |dialog| {
            Box::pin(async move {
                dialog
                    .pick_files()
                    .await
                    .unwrap_or_default()
                    .iter()
                    .map(|h| h.path().to_path_buf())
                    .collect()
            })
        })
    }

    fn pick_image(&self) -> Task<Option<PathBuf>> {
        parented("Choose a picture", None, |dialog| {
            Box::pin(async move {
                dialog
                    .add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"])
                    .pick_file()
                    .await
                    .map(|h| h.path().to_path_buf())
            })
        })
    }
}

/// The largest clipboard image the app takes, in bytes of RGBA pixels.
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 128 * 1024 * 1024;

/// What the clipboard held. `Debug` shows the kind and sizes only.
#[derive(Clone)]
pub enum ClipboardContent {
    /// A picture as 8-bit RGBA pixels.
    Image {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// `width * height * 4` bytes.
        rgba: Arc<Vec<u8>>,
    },
    /// Text.
    Text(MessageText),
    /// Nothing the app could read: empty, another kind of data, or an error.
    /// The caller falls back to the window system's own text paste.
    Unavailable,
}

impl fmt::Debug for ClipboardContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image { width, height, .. } => write!(f, "Image({width}x{height})"),
            Self::Text(_) => f.write_str("Text(<redacted>)"),
            Self::Unavailable => f.write_str("Unavailable"),
        }
    }
}

/// Reads the system clipboard.
pub trait Clipboard: Send {
    /// A task that yields what the clipboard holds now: a picture first,
    /// then text.
    fn read(&self) -> Task<ClipboardContent>;
}

/// The clipboard through `arboard`, read on a worker thread.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClipboard;

impl Clipboard for SystemClipboard {
    fn read(&self) -> Task<ClipboardContent> {
        Task::future(async {
            blocking::run(read_system)
                .await
                .unwrap_or(ClipboardContent::Unavailable)
        })
    }
}

/// `arboard`'s picture, then its text. Any error is `Unavailable`.
fn read_system() -> ClipboardContent {
    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return ClipboardContent::Unavailable;
    };
    if let Ok(image) = clipboard.get_image() {
        let (width, height) = (image.width, image.height);
        let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4));
        if let (Ok(w), Ok(h), Some(expected)) =
            (u32::try_from(width), u32::try_from(height), expected)
            && expected == image.bytes.len()
            && expected > 0
            && expected <= MAX_CLIPBOARD_IMAGE_BYTES
        {
            return ClipboardContent::Image {
                width: w,
                height: h,
                rgba: Arc::new(image.bytes.into_owned()),
            };
        }
    }
    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => ClipboardContent::Text(MessageText::from(text)),
        _ => ClipboardContent::Unavailable,
    }
}

/// Asks the system to open a saved file with its default program. `true`
/// when the handler started. Callers ask the user first, with a warning for
/// a file that may run code (see [`crate::media::may_run_code`]).
pub fn open_file(path: &Path) -> bool {
    open::that_detached(path).is_ok()
}

#[cfg(test)]
pub use fake::{RecordingClipboard, RecordingDialogs};

#[cfg(test)]
mod fake {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use iced::Task;

    use super::{Clipboard, ClipboardContent, Dialogs};

    /// A fake that records the proposed names and shows nothing. Tests send
    /// the user's answer as a message themselves.
    #[derive(Debug, Clone, Default)]
    pub struct RecordingDialogs {
        /// The names proposed so far.
        pub asked: Arc<Mutex<Vec<String>>>,
        /// How many times the file picker was opened.
        pub picked: Arc<Mutex<usize>>,
        /// How many times the picture picker was opened.
        pub images: Arc<Mutex<usize>>,
    }

    /// A fake clipboard that counts reads and yields nothing; tests send the
    /// content as a message themselves.
    #[derive(Debug, Clone, Default)]
    pub struct RecordingClipboard {
        /// How many reads were asked for.
        pub reads: Arc<Mutex<usize>>,
    }

    impl Clipboard for RecordingClipboard {
        fn read(&self) -> Task<ClipboardContent> {
            *self.reads.lock().expect("test mutex") += 1;
            Task::none()
        }
    }

    impl Dialogs for RecordingDialogs {
        fn save_file(&self, suggested_name: &str) -> Task<Option<PathBuf>> {
            self.asked
                .lock()
                .expect("test mutex")
                .push(suggested_name.to_owned());
            Task::none()
        }

        fn pick_files(&self) -> Task<Vec<PathBuf>> {
            *self.picked.lock().expect("test mutex") += 1;
            Task::none()
        }

        fn pick_image(&self) -> Task<Option<PathBuf>> {
            *self.images.lock().expect("test mutex") += 1;
            Task::none()
        }
    }
}
