//! Uploads: what the app decides before a file goes to the core (plan Q9,
//! Task 10).
//!
//! The pure functions here pick a MIME type, make the preview that is sent
//! with a picture, name a pasted image and check a size. The rest of the
//! module is the confirmation dialog's state. File names and bytes belong to
//! the user and to other programs: nothing here is logged, and every
//! `Debug` hides them.

use std::fmt;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone};
use iced::widget::image::Handle;
use iced::widget::text_editor::{Action, Edit};
use iced::{Task, clipboard, window};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ExtendedColorType, ImageEncoder as _};
use tessera_core::api::{
    AttachmentData, Command, CoreError, EventId, MediaBytes, MediaKind, MessageText,
    OutgoingAttachment, RoomId, UploadProgress, UploadThumbnail,
};

use crate::blocking;
use crate::composer;
use crate::dialogs::ClipboardContent;
use crate::media::{self, Decoded};
use crate::state::{App, Message, Screen};

/// The largest edge pair of a thumbnail: it fits 800 x 600.
pub const THUMB_MAX: (u32, u32) = (800, 600);
/// A picture this small (and small in bytes) is sent without a thumbnail.
pub const NO_THUMB_MAX: (u32, u32) = (640, 480);
/// A picture under this many bytes (and no larger than [`NO_THUMB_MAX`]) is
/// sent without a thumbnail.
pub const NO_THUMB_BYTES: usize = 1024 * 1024;
/// The preview in the confirmation dialog fits this box.
pub const PREVIEW_MAX: (u32, u32) = (160, 120);
/// JPEG quality of generated thumbnails.
pub const JPEG_QUALITY: u8 = 85;

/// What the app learned about a picture.
pub struct ImageInfo {
    /// Width as shown (EXIF rotation applied).
    pub width: u32,
    /// Height as shown (EXIF rotation applied).
    pub height: u32,
    /// The thumbnail to send, when one is worth sending.
    pub thumbnail: Option<UploadThumbnail>,
    /// A small picture for the dialog.
    pub preview: Decoded,
}

/// The extension table. Only passive types: nothing that runs or renders
/// active content (`html`, `svg`, `js`) is named here.
fn mime_by_extension(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        _ => return None,
    })
}

/// The picture type the first bytes show, for the formats the app decodes.
fn sniff_image(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The type sent when nothing better is known.
pub const OCTET_STREAM: &str = "application/octet-stream";

/// The MIME type to send for a file called `name` whose first bytes are
/// `head`.
///
/// The extension names the type, from a table of passive types. A picture
/// type is believed only when the first bytes show the same picture format,
/// and a picture under any other name is not believed either: both are sent
/// as a plain file, so that a renamed file is never shown as something it is
/// not. An unknown extension is `application/octet-stream`.
///
/// ```
/// use tessera_app::upload::mime_for;
/// assert_eq!(mime_for("notes.txt", b"hello"), "text/plain");
/// // A PNG that calls itself a JPEG is a plain file.
/// let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// assert_eq!(mime_for("a.jpg", &png), "application/octet-stream");
/// ```
pub fn mime_for(name: &str, head: &[u8]) -> &'static str {
    let by_name = name
        .rsplit_once('.')
        .and_then(|(_, ext)| mime_by_extension(&ext.to_ascii_lowercase()));
    let by_bytes = sniff_image(head);
    match (by_name, by_bytes) {
        (Some(named), Some(seen)) if named == seen => named,
        (Some(named), None) if !named.starts_with("image/") => named,
        _ => OCTET_STREAM,
    }
}

/// The message kind for a MIME type. Only the formats the app can check and
/// the viewer can show are sent as pictures.
pub fn kind_for(mime: &str) -> MediaKind {
    match mime {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" => MediaKind::Image,
        m if m.starts_with("video/") => MediaKind::Video,
        m if m.starts_with("audio/") => MediaKind::Audio,
        _ => MediaKind::File,
    }
}

/// Decodes `bytes` (a PNG, JPEG, WebP or GIF within the decoder limits) and
/// builds what the dialog and the message need. `None` for anything else.
///
/// The header is checked against [`crate::media::decode_limits`] before any
/// pixel buffer exists. EXIF rotation is applied, so the sides are those of
/// the picture as it is shown; a rotated photo always gets a thumbnail (the
/// thumbnail is the only copy that is turned).
pub fn inspect_image(bytes: &[u8]) -> Option<ImageInfo> {
    let (picture, orientation) = media::decode_oriented(bytes, media::decode_limits())?;
    let (width, height) = (picture.width(), picture.height());

    let small = width <= NO_THUMB_MAX.0 && height <= NO_THUMB_MAX.1;
    let light = bytes.len() < NO_THUMB_BYTES;
    let needs_thumbnail = !(small && light && orientation == Orientation::NoTransforms);
    let thumbnail = if needs_thumbnail {
        Some(encode_thumbnail(&picture)?)
    } else {
        None
    };

    let preview = picture.thumbnail(PREVIEW_MAX.0, PREVIEW_MAX.1).into_rgba8();
    let (pw, ph) = preview.dimensions();
    Some(ImageInfo {
        width,
        height,
        thumbnail,
        preview: Decoded {
            handle: Handle::from_rgba(pw, ph, preview.into_raw()),
            width: pw,
            height: ph,
        },
    })
}

/// Scales `picture` to fit [`THUMB_MAX`] (never larger than it is) and
/// encodes it: a PNG when it has alpha, a JPEG otherwise.
fn encode_thumbnail(picture: &DynamicImage) -> Option<UploadThumbnail> {
    let fitted;
    let scaled = if picture.width() > THUMB_MAX.0 || picture.height() > THUMB_MAX.1 {
        fitted = picture.thumbnail(THUMB_MAX.0, THUMB_MAX.1);
        &fitted
    } else {
        picture
    };
    let mut out = Vec::new();
    let mimetype = if scaled.color().has_alpha() {
        let rgba = DynamicImage::ImageRgba8(scaled.to_rgba8());
        rgba.write_with_encoder(PngEncoder::new(&mut out)).ok()?;
        "image/png"
    } else {
        let rgb = DynamicImage::ImageRgb8(scaled.to_rgb8());
        rgb.write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
            .ok()?;
        "image/jpeg"
    };
    Some(UploadThumbnail {
        data: MediaBytes::from(out),
        mimetype: mimetype.to_owned(),
        width: scaled.width(),
        height: scaled.height(),
    })
}

/// The thumbnail to send with a picture, or `None` when the picture is small
/// enough to be its own preview or cannot be decoded.
pub fn make_thumbnail(bytes: &[u8]) -> Option<UploadThumbnail> {
    inspect_image(bytes)?.thumbnail
}

/// The name of an image pasted at `now`.
///
/// ```
/// use chrono::{TimeZone, Utc};
/// use tessera_app::upload::pasted_image_name;
/// let now = Utc.with_ymd_and_hms(2026, 10, 8, 9, 5, 3).unwrap();
/// assert_eq!(pasted_image_name(&now), "pasted-image-20261008-090503.png");
/// ```
pub fn pasted_image_name<Tz: TimeZone>(now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    format!("pasted-image-{}.png", now.format("%Y%m%d-%H%M%S"))
}

/// A file is over the server's limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLarge {
    /// The limit in bytes.
    pub limit: u64,
}

/// The limit assumed while the server has not said (the core's default).
pub const DEFAULT_LIMIT: u64 = 100 * 1024 * 1024;

/// Refuses `len` over `limit` (the server's, or the default 100 MiB when it
/// has not said).
///
/// ```
/// use tessera_app::upload::{check_size, TooLarge};
/// assert_eq!(check_size(10, Some(10)), Ok(()));
/// assert_eq!(check_size(11, Some(10)), Err(TooLarge { limit: 10 }));
/// ```
pub fn check_size(len: u64, limit: Option<u64>) -> Result<(), TooLarge> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if len > limit {
        Err(TooLarge { limit })
    } else {
        Ok(())
    }
}

// ---- Preparing a file -------------------------------------------------

/// How many bytes of a file are read to tell what it is.
const PEEK_BYTES: usize = 512;
/// Pictures up to this size are decoded for a preview and a thumbnail;
/// bigger ones are sent as they are, without either.
pub const MAX_INSPECT_BYTES: u64 = 64 * 1024 * 1024;
/// The most files one confirmation holds.
pub const MAX_FILES: usize = 20;
/// How long after the last dropped file the confirmation opens.
pub const DROP_DEBOUNCE: Duration = Duration::from_millis(300);

/// Where an attachment's bytes are. `Debug` shows the kind only.
#[derive(Clone)]
pub enum Source {
    /// A file the core reads when the message is sent.
    Path(PathBuf),
    /// Bytes in memory, such as a pasted picture.
    Bytes(Arc<Vec<u8>>),
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(_) => f.write_str("Path(<redacted>)"),
            Self::Bytes(bytes) => write!(f, "Bytes({})", bytes.len()),
        }
    }
}

/// Paths from the picker or a drop. `Debug` shows the count only.
#[derive(Clone, Default)]
pub struct UploadPaths(pub Vec<PathBuf>);

impl fmt::Debug for UploadPaths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UploadPaths({})", self.0.len())
    }
}

/// A file that is ready to send.
#[derive(Clone)]
pub struct Prepared {
    /// The name to send: the base name, cleaned of controls and separators.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Where the bytes are.
    pub source: Source,
    /// The MIME type to send.
    pub mime: &'static str,
    /// The message kind.
    pub kind: MediaKind,
    /// Width of a picture as shown.
    pub width: Option<u32>,
    /// Height of a picture as shown.
    pub height: Option<u32>,
    /// The thumbnail to send with a picture.
    pub thumbnail: Option<UploadThumbnail>,
    /// A small picture for the dialog.
    pub preview: Option<Decoded>,
}

impl Prepared {
    /// Whether the dialog shows the note about camera data: a JPEG photo is
    /// sent as it is, with whatever its EXIF holds.
    pub fn has_exif_note(&self) -> bool {
        self.mime == "image/jpeg"
    }
}

impl fmt::Debug for Prepared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("size", &self.size)
            .field("source", &self.source)
            .field("mime", &self.mime)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Why a file is not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Over the server's upload limit.
    TooLarge(TooLarge),
    /// It could not be opened or read.
    Unreadable,
    /// It is a folder or another thing that is not a file.
    NotAFile,
    /// The clipboard picture could not be turned into a file.
    BadPicture,
}

impl Refusal {
    /// The words the dialog shows, without the file's name.
    pub fn text(self) -> String {
        match self {
            Self::TooLarge(TooLarge { limit }) => format!(
                "Too large: the server accepts files up to {}.",
                media::size_label(Some(limit))
            ),
            Self::Unreadable => "This file could not be read.".to_owned(),
            Self::NotAFile => "Folders and special files cannot be sent.".to_owned(),
            Self::BadPicture => "The picture on the clipboard could not be used.".to_owned(),
        }
    }
}

/// The result of preparing one file.
#[derive(Clone)]
pub enum Outcome {
    /// Ready to send.
    Ready(Box<Prepared>),
    /// Not sent, with the reason.
    Refused(Refusal),
}

impl fmt::Debug for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready(prepared) => f.debug_tuple("Ready").field(prepared).finish(),
            Self::Refused(why) => f.debug_tuple("Refused").field(why).finish(),
        }
    }
}

/// The name to show and send for `path`: its last component only, cleaned
/// by [`media::suggested_file_name`].
pub fn name_of(path: &Path) -> String {
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    media::suggested_file_name(&base)
}

/// Builds the result from what was read. `picture` is the whole file when it
/// was read for inspection.
fn finish(
    name: String,
    size: u64,
    source: Source,
    head: &[u8],
    picture: Option<&[u8]>,
) -> Prepared {
    let mut mime = mime_for(&name, head);
    let mut info = None;
    if kind_for(mime) == MediaKind::Image
        && let Some(bytes) = picture
    {
        info = inspect_image(bytes);
        if info.is_none() {
            // Named and signed as a picture, but not one the app can decode
            // within its limits: sent as a plain file.
            mime = OCTET_STREAM;
        }
    }
    let kind = kind_for(mime);
    let (width, height, thumbnail, preview) = match info {
        Some(info) => (
            Some(info.width),
            Some(info.height),
            info.thumbnail,
            Some(info.preview),
        ),
        None => (None, None, None, None),
    };
    Prepared {
        name,
        size,
        source,
        mime,
        kind,
        width,
        height,
        thumbnail,
        preview,
    }
}

/// Reads a file's size and first bytes, and a whole picture when it is small
/// enough, then builds the result. Blocking: run it with
/// [`crate::blocking::run`].
pub fn prepare_path(path: PathBuf, limit: Option<u64>) -> Outcome {
    let Ok(mut file) = std::fs::File::open(&path) else {
        return Outcome::Refused(Refusal::Unreadable);
    };
    // The size and the type come from the open handle, not from the path.
    let Ok(meta) = file.metadata() else {
        return Outcome::Refused(Refusal::Unreadable);
    };
    if !meta.is_file() {
        return Outcome::Refused(Refusal::NotAFile);
    }
    let size = meta.len();
    if let Err(too_large) = check_size(size, limit) {
        return Outcome::Refused(Refusal::TooLarge(too_large));
    }
    let name = name_of(&path);
    let mut head = Vec::new();
    if (&mut file)
        .take(PEEK_BYTES as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return Outcome::Refused(Refusal::Unreadable);
    }
    let picture = if sniff_image(&head).is_some() && size <= MAX_INSPECT_BYTES {
        let mut bytes = Vec::new();
        let read = file.seek(SeekFrom::Start(0)).and_then(|_| {
            (&mut file)
                .take(MAX_INSPECT_BYTES + 1)
                .read_to_end(&mut bytes)
        });
        if read.is_err() {
            return Outcome::Refused(Refusal::Unreadable);
        }
        Some(bytes)
    } else {
        None
    };
    Outcome::Ready(Box::new(finish(
        name,
        size,
        Source::Path(path),
        &head,
        picture.as_deref(),
    )))
}

/// Builds the result for bytes that are already in memory (a pasted
/// picture). Blocking, like [`prepare_path`].
pub fn prepare_bytes(name: String, bytes: Vec<u8>, limit: Option<u64>) -> Outcome {
    let size = bytes.len() as u64;
    if let Err(too_large) = check_size(size, limit) {
        return Outcome::Refused(Refusal::TooLarge(too_large));
    }
    let bytes = Arc::new(bytes);
    let head = &bytes[..bytes.len().min(PEEK_BYTES)];
    let picture = (size <= MAX_INSPECT_BYTES).then_some(bytes.as_slice());
    Outcome::Ready(Box::new(finish(
        name,
        size,
        Source::Bytes(Arc::clone(&bytes)),
        head,
        picture,
    )))
}

/// Encodes clipboard pixels as a PNG and prepares them. `None` when the
/// pixel count does not match the size. Blocking.
pub fn prepare_pixels(
    name: String,
    width: u32,
    height: u32,
    rgba: &[u8],
    limit: Option<u64>,
) -> Outcome {
    let expected = u64::from(width) * u64::from(height) * 4;
    if width == 0 || height == 0 || expected != rgba.len() as u64 {
        return Outcome::Refused(Refusal::BadPicture);
    }
    let mut png = Vec::new();
    let encoded =
        PngEncoder::new(&mut png).write_image(rgba, width, height, ExtendedColorType::Rgba8);
    if encoded.is_err() {
        return Outcome::Refused(Refusal::BadPicture);
    }
    prepare_bytes(name, png, limit)
}

// ---- The confirmation dialog ------------------------------------------

/// One line of the dialog. `Debug` hides the name.
#[derive(Clone)]
pub struct Entry {
    /// Identifies the line while its file is being read.
    pub id: u64,
    /// The name shown before the file is read, and for a refused file.
    pub name: String,
    /// Where the line stands.
    pub state: EntryState,
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// Where a line stands.
#[derive(Debug, Clone)]
pub enum EntryState {
    /// Being read.
    Loading,
    /// Ready to send.
    Ready(Box<Prepared>),
    /// Refused.
    Refused(Refusal),
}

/// The files waiting for the user's **Send**.
#[derive(Debug, Clone)]
pub struct UploadDialog {
    /// The room they go to.
    pub room_id: RoomId,
    /// The files, in the order they will be sent.
    pub entries: Vec<Entry>,
}

impl UploadDialog {
    /// Whether any file is still being read.
    pub fn loading(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.state, EntryState::Loading))
    }

    /// How many files can be sent.
    pub fn sendable(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.state, EntryState::Ready(_)))
            .count()
    }

    /// Whether **Send** does anything now: nothing is loading and at least
    /// one file is ready.
    pub fn can_send(&self) -> bool {
        !self.loading() && self.sendable() > 0
    }
}

/// Everything the app keeps about uploads. `Debug` shows how many files wait
/// for the drop debounce, not their paths.
#[derive(Default)]
pub struct UploadModel {
    /// The server's upload limit, once it has said.
    pub max_upload_bytes: Option<u64>,
    /// Files are being dragged over the window.
    pub hovering: bool,
    /// Dropped files waiting for the debounce.
    pub dropped: Vec<PathBuf>,
    /// Counts drops, so only the timer of the last one opens the dialog.
    pub drop_seq: u64,
    /// The confirmation, while it is open.
    pub dialog: Option<UploadDialog>,
    /// The thread new uploads go into: set by the thread panel while it is
    /// open, `None` for the room's own timeline.
    pub thread_root: Option<EventId>,
    next_entry: u64,
}

impl fmt::Debug for UploadModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadModel")
            .field("max_upload_bytes", &self.max_upload_bytes)
            .field("hovering", &self.hovering)
            .field("dropped", &self.dropped.len())
            .field("drop_seq", &self.drop_seq)
            .field("dialog", &self.dialog)
            .field("thread_root", &self.thread_root)
            .field("next_entry", &self.next_entry)
            .finish()
    }
}

impl UploadModel {
    /// Forgets everything that belongs to the open room.
    pub fn reset_view(&mut self) {
        self.hovering = false;
        self.dropped.clear();
        self.drop_seq = self.drop_seq.wrapping_add(1);
        self.dialog = None;
        self.thread_root = None;
    }

    /// Forgets everything, at logout.
    pub fn clear(&mut self) {
        let seq = self.drop_seq.wrapping_add(1);
        *self = Self::default();
        self.drop_seq = seq;
    }

    /// A new line id.
    pub(crate) fn new_entry_id(&mut self) -> u64 {
        self.next_entry = self.next_entry.wrapping_add(1);
        self.next_entry
    }
}

// ---- The app's side ----------------------------------------------------

/// The notice when the core refuses to cancel an upload (the SDK may refuse
/// once the request is on the wire).
pub const CANCEL_REFUSED: &str = "It can't be cancelled now.";

/// The notice for a file the core did not accept.
pub fn failure_text(error: &CoreError) -> &'static str {
    match error {
        CoreError::NotAllowed => "The file was not sent: it is too large or not allowed.",
        CoreError::NoRoomSelected => "The file was not sent: open the room first.",
        CoreError::Busy => "The file was not sent: too many uploads are waiting.",
        _ => "The file was not sent.",
    }
}

/// How much of an upload is done, from 0 to 1.
pub fn fraction(progress: UploadProgress) -> f32 {
    if progress.total == 0 {
        return 0.0;
    }
    // Precision beyond a pixel of the bar is not needed.
    (progress.sent as f64 / progress.total as f64).clamp(0.0, 1.0) as f32
}

/// The messages the window system's file events stand for: files hovering
/// and dropping. Keys (Ctrl+U) are in `keys`.
pub fn event_message(event: &iced::Event) -> Option<Message> {
    match event {
        iced::Event::Window(window::Event::FileHovered(_)) => Some(Message::FileHovered),
        iced::Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FilesHoveredLeft),
        iced::Event::Window(window::Event::FileDropped(path)) => {
            Some(Message::FileDropped(UploadPaths(vec![path.clone()])))
        }
        _ => None,
    }
}

/// The command for one prepared file.
fn outgoing(
    prepared: Prepared,
    room_id: &RoomId,
    reply_to: Option<EventId>,
    thread_root: Option<EventId>,
) -> OutgoingAttachment {
    let data = match prepared.source {
        Source::Path(path) => AttachmentData::Path(path),
        Source::Bytes(bytes) => AttachmentData::Bytes(MediaBytes::from(
            Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()),
        )),
    };
    OutgoingAttachment {
        room_id: room_id.clone(),
        filename: prepared.name,
        mimetype: prepared.mime.to_owned(),
        data,
        kind: prepared.kind,
        width: prepared.width,
        height: prepared.height,
        thumbnail: prepared.thumbnail,
        reply_to,
        thread_root,
    }
}

impl App {
    /// The open room, when the main screen shows one: files go only there.
    pub(crate) fn upload_room(&self) -> Option<RoomId> {
        match self.screen {
            Screen::Main => self.selected.clone(),
            _ => None,
        }
    }

    /// **Attach**: the picker opens.
    pub(crate) fn attach_requested(&mut self) -> Task<Message> {
        if self.upload_room().is_none() {
            return Task::none();
        }
        self.dialogs
            .pick_files()
            .map(|paths| Message::FilesChosen(UploadPaths(paths)))
    }

    /// Adds a line to the confirmation, opening it if needed. `None` when
    /// the confirmation is full or there is no room.
    fn add_entry(&mut self, name: String) -> Option<u64> {
        let room_id = self.upload_room()?;
        let dialog = self.uploads.dialog.get_or_insert_with(|| UploadDialog {
            room_id,
            entries: Vec::new(),
        });
        if dialog.entries.len() >= MAX_FILES {
            return None;
        }
        let id = self.uploads.new_entry_id();
        let dialog = self.uploads.dialog.as_mut()?;
        dialog.entries.push(Entry {
            id,
            name,
            state: EntryState::Loading,
        });
        Some(id)
    }

    /// Files were chosen or dropped: one line each, read in tasks.
    pub(crate) fn files_added(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        let limit = self.uploads.max_upload_bytes;
        let mut tasks = Vec::new();
        for path in paths {
            let Some(id) = self.add_entry(name_of(&path)) else {
                break;
            };
            tasks.push(Task::perform(
                async move {
                    blocking::run(move || prepare_path(path, limit))
                        .await
                        .unwrap_or(Outcome::Refused(Refusal::Unreadable))
                },
                move |outcome| Message::UploadPrepared {
                    entry: id,
                    outcome: Box::new(outcome),
                },
            ));
        }
        Task::batch(tasks)
    }

    /// A file was dropped: collect it, and open the confirmation once the
    /// drops stop for [`DROP_DEBOUNCE`].
    pub(crate) fn files_dropped(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        self.uploads.hovering = false;
        if self.upload_room().is_none() {
            return Task::none();
        }
        for path in paths {
            if self.uploads.dropped.len() < MAX_FILES {
                self.uploads.dropped.push(path);
            }
        }
        self.uploads.drop_seq = self.uploads.drop_seq.wrapping_add(1);
        self.timer(DROP_DEBOUNCE, Message::DropsDue(self.uploads.drop_seq))
    }

    /// The pause ended: if no later drop came, open the confirmation.
    pub(crate) fn drops_due(&mut self, seq: u64) -> Task<Message> {
        if seq != self.uploads.drop_seq || self.uploads.dropped.is_empty() {
            return Task::none();
        }
        let paths = std::mem::take(&mut self.uploads.dropped);
        self.files_added(paths)
    }

    /// A file was read: fill its line.
    pub(crate) fn upload_prepared(&mut self, entry: u64, outcome: Outcome) {
        let Some(line) = self
            .uploads
            .dialog
            .as_mut()
            .and_then(|dialog| dialog.entries.iter_mut().find(|e| e.id == entry))
        else {
            return;
        };
        if !matches!(line.state, EntryState::Loading) {
            return;
        }
        line.state = match outcome {
            Outcome::Ready(prepared) => {
                line.name.clone_from(&prepared.name);
                EntryState::Ready(prepared)
            }
            Outcome::Refused(why) => EntryState::Refused(why),
        };
    }

    /// Ctrl+V in the composer: read the clipboard in a task.
    pub(crate) fn paste_requested(&mut self) -> Task<Message> {
        if self.upload_room().is_none() {
            return Task::none();
        }
        self.clipboard.read().map(Message::Pasted)
    }

    /// The clipboard answered.
    pub(crate) fn pasted(&mut self, content: ClipboardContent) -> Task<Message> {
        match content {
            ClipboardContent::Text(text) => self.paste_text(text.expose()),
            ClipboardContent::Image {
                width,
                height,
                rgba,
            } => {
                let limit = self.uploads.max_upload_bytes;
                let Some(id) = self.add_entry("Pasted picture".to_owned()) else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        blocking::run(move || {
                            let name = pasted_image_name(&chrono::Local::now());
                            prepare_pixels(name, width, height, &rgba, limit)
                        })
                        .await
                        .unwrap_or(Outcome::Refused(Refusal::BadPicture))
                    },
                    move |outcome| Message::UploadPrepared {
                        entry: id,
                        outcome: Box::new(outcome),
                    },
                )
            }
            // The app could not read it: let the window system try text.
            ClipboardContent::Unavailable => {
                clipboard::read().map(|text| Message::PasteFallback(text.map(MessageText::from)))
            }
        }
    }

    /// The window system's text paste.
    pub(crate) fn paste_fallback(&mut self, text: Option<MessageText>) -> Task<Message> {
        match text {
            Some(text) if !text.expose().is_empty() => self.paste_text(text.expose()),
            _ => Task::none(),
        }
    }

    /// Inserts pasted text at the cursor, as the editor's own paste does.
    fn paste_text(&mut self, text: &str) -> Task<Message> {
        let action = Action::Edit(Edit::Paste(Arc::new(text.to_owned())));
        self.composer_action_at(action, Instant::now())
    }

    /// **Send**: one `SendAttachment` per ready file, in order.
    pub(crate) fn upload_confirmed(&mut self) -> Task<Message> {
        let Some(room_id) = self.upload_room() else {
            return Task::none();
        };
        let allowed = self
            .uploads
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.room_id == room_id && dialog.can_send());
        if !allowed {
            return Task::none();
        }
        let Some(dialog) = self.uploads.dialog.take() else {
            return Task::none();
        };
        // The reply target is an event; a local echo has none yet.
        let replying = match &self.composer.mode {
            composer::Mode::Reply(item) => Some(item.clone()),
            composer::Mode::Normal | composer::Mode::Edit(_) => None,
        };
        let reply_to = replying.as_ref().and_then(|item| {
            self.timeline
                .items
                .iter()
                .find(|candidate| candidate.id == *item)
                .and_then(|candidate| candidate.event_id.clone())
        });
        let thread_root = self.uploads.thread_root.clone();
        let mut tasks = Vec::new();
        for entry in dialog.entries {
            if let EntryState::Ready(prepared) = entry.state {
                let attachment =
                    outgoing(*prepared, &room_id, reply_to.clone(), thread_root.clone());
                tasks.push(self.send(Command::SendAttachment(attachment)));
            }
        }
        if replying.is_some() {
            self.composer.cancel_mode();
        }
        Task::batch(tasks)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use chrono::Utc;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::path::{Path, PathBuf};

    use super::*;

    const PHOTO: &[u8] = include_bytes!("../../core/tests/fixtures/photo-1200x900.png");
    const SMALL: &[u8] = include_bytes!("../../core/tests/fixtures/small-64x48.png");
    const ROTATED: &[u8] = include_bytes!("../../core/tests/fixtures/exif-orientation-6.jpg");

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("encode");
        out.into_inner()
    }

    fn dims(thumb: &UploadThumbnail) -> (u32, u32) {
        let image = image::load_from_memory(thumb.data.as_bytes()).expect("decodable");
        (image.width(), image.height())
    }

    #[test]
    fn mime_follows_the_extension_when_the_signature_agrees() {
        assert_eq!(mime_for("a.png", PHOTO), "image/png");
        assert_eq!(mime_for("A.PNG", PHOTO), "image/png");
        assert_eq!(mime_for("a.jpg", ROTATED), "image/jpeg");
        assert_eq!(mime_for("a.jpeg", ROTATED), "image/jpeg");
        assert_eq!(mime_for("clip.mp4", b"\0\0\0\x18ftypmp42"), "video/mp4");
        assert_eq!(mime_for("song.mp3", b"ID3"), "audio/mpeg");
        assert_eq!(mime_for("doc.pdf", b"%PDF-1.7"), "application/pdf");
        assert_eq!(mime_for("notes.txt", b"hello"), "text/plain");
    }

    #[test]
    fn an_extension_that_disagrees_with_the_picture_is_sent_as_a_file() {
        // A PNG called .jpg, a JPEG called .png, a non-picture called .png.
        assert_eq!(mime_for("a.jpg", PHOTO), "application/octet-stream");
        assert_eq!(mime_for("a.png", ROTATED), "application/octet-stream");
        assert_eq!(
            mime_for("a.png", b"not a picture"),
            "application/octet-stream"
        );
        // A picture under a name that says something else.
        assert_eq!(mime_for("a.pdf", PHOTO), "application/octet-stream");
        assert_eq!(mime_for("a.dat", PHOTO), "application/octet-stream");
    }

    #[test]
    fn unknown_or_missing_extensions_are_octet_stream() {
        for name in ["a.zzz", "noext", "", ".hidden", "a.", "archive.tar.zzz"] {
            assert_eq!(
                mime_for(name, b"abc"),
                "application/octet-stream",
                "{name:?}"
            );
        }
        // Active content types are not guessed.
        for name in ["a.html", "a.svg", "a.js", "a.exe"] {
            assert_eq!(
                mime_for(name, b"<x>"),
                "application/octet-stream",
                "{name:?}"
            );
        }
    }

    #[test]
    fn kinds_follow_the_mime_type() {
        assert_eq!(kind_for("image/png"), MediaKind::Image);
        assert_eq!(kind_for("image/webp"), MediaKind::Image);
        assert_eq!(kind_for("video/mp4"), MediaKind::Video);
        assert_eq!(kind_for("audio/ogg"), MediaKind::Audio);
        assert_eq!(kind_for("application/pdf"), MediaKind::File);
        assert_eq!(kind_for("image/svg+xml"), MediaKind::File);
        assert_eq!(kind_for("application/octet-stream"), MediaKind::File);
    }

    #[test]
    fn a_big_picture_gets_a_jpeg_thumbnail_that_fits_and_keeps_its_shape() {
        let thumb = make_thumbnail(PHOTO).expect("a thumbnail");
        assert_eq!(thumb.mimetype, "image/jpeg");
        let (w, h) = dims(&thumb);
        assert!(w <= 800 && h <= 600, "{w}x{h}");
        assert_eq!((thumb.width, thumb.height), (w, h));
        // 1200x900 is 4:3, so the box is filled exactly.
        assert_eq!((w, h), (800, 600));
    }

    #[test]
    fn a_picture_with_alpha_gets_a_png_thumbnail() {
        let image = RgbaImage::from_pixel(1000, 700, Rgba([10, 20, 30, 128]));
        let bytes = encode(&DynamicImage::ImageRgba8(image), ImageFormat::Png);
        let thumb = make_thumbnail(&bytes).expect("a thumbnail");
        assert_eq!(thumb.mimetype, "image/png");
        let (w, h) = dims(&thumb);
        assert!(w <= 800 && h <= 600);
        assert_eq!((thumb.width, thumb.height), (w, h));
    }

    #[test]
    fn a_small_light_picture_has_no_thumbnail() {
        let image = RgbaImage::from_pixel(640, 480, Rgba([1, 2, 3, 255]));
        let bytes = encode(&DynamicImage::ImageRgba8(image), ImageFormat::Png);
        assert!(bytes.len() < NO_THUMB_BYTES);
        assert!(make_thumbnail(&bytes).is_none());
        assert!(make_thumbnail(SMALL).is_none());
        // But it is still a picture, with a size and a preview.
        let info = inspect_image(&bytes).expect("a picture");
        assert_eq!((info.width, info.height), (640, 480));
        assert!(info.thumbnail.is_none());
        assert!(info.preview.width <= 160 && info.preview.height <= 120);
    }

    #[test]
    fn a_picture_just_over_the_size_gets_a_thumbnail_no_bigger_than_itself() {
        let image = RgbaImage::from_pixel(641, 100, Rgba([1, 2, 3, 255]));
        let bytes = encode(&DynamicImage::ImageRgba8(image), ImageFormat::Png);
        let thumb = make_thumbnail(&bytes).expect("a thumbnail");
        let (w, h) = dims(&thumb);
        assert!(w <= 641 && h <= 100, "{w}x{h}");
    }

    #[test]
    fn a_rotated_photo_gets_a_thumbnail_with_swapped_sides() {
        // 80x60 stored, EXIF orientation 6: shown as 60x80.
        let thumb = make_thumbnail(ROTATED).expect("a thumbnail");
        assert_eq!((thumb.width, thumb.height), (60, 80));
        assert_eq!(dims(&thumb), (60, 80));
        let info = inspect_image(ROTATED).expect("a picture");
        assert_eq!((info.width, info.height), (60, 80));
    }

    /// A PNG header that declares `w` x `h` pixels and has no pixels.
    fn png_header(w: u32, h: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xFFFF_FFFFu32;
            for byte in data {
                crc ^= u32::from(*byte);
                for _ in 0..8 {
                    crc = if crc & 1 == 1 {
                        (crc >> 1) ^ 0xEDB8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        out.extend(13u32.to_be_bytes());
        out.extend(&ihdr);
        out.extend(crc32(&ihdr).to_be_bytes());
        out
    }

    #[test]
    fn a_header_over_the_limits_is_refused_before_any_pixels_exist() {
        assert!(inspect_image(&png_header(100_000, 100_000)).is_none());
        assert!(make_thumbnail(&png_header(100_000, 100_000)).is_none());
        // The same header within the limits passes the header check, then
        // fails for having no data.
        assert!(inspect_image(&png_header(10, 10)).is_none());
    }

    #[test]
    fn junk_and_other_formats_are_not_pictures() {
        assert!(inspect_image(b"").is_none());
        assert!(inspect_image(b"GIF").is_none());
        // A bitmap header: a format the app does not decode.
        assert!(
            inspect_image(
                b"BM\x1a\0\0\0\0\0\0\0\x1a\0\0\0\x0c\0\0\0\x01\0\x01\0\x01\0\x18\0\0\0\xff"
            )
            .is_none()
        );
    }

    #[test]
    fn a_thumbnail_is_never_larger_than_the_picture() {
        for (w, h) in [(700, 20), (20, 700), (801, 601), (1600, 1200), (3000, 10)] {
            let image = RgbaImage::from_pixel(w, h, Rgba([9, 9, 9, 255]));
            let mut bytes = encode(&DynamicImage::ImageRgba8(image), ImageFormat::Png);
            // Force a thumbnail by making it heavy.
            bytes.resize(NO_THUMB_BYTES + 1, 0);
            if let Some(thumb) = make_thumbnail(&bytes) {
                assert!(thumb.width <= w && thumb.height <= h, "{w}x{h}");
                assert!(thumb.width <= 800 && thumb.height <= 600);
            }
        }
    }

    #[test]
    fn pasted_images_are_named_by_time() {
        let at = Utc
            .with_ymd_and_hms(2026, 10, 8, 9, 5, 3)
            .single()
            .expect("a date");
        assert_eq!(pasted_image_name(&at), "pasted-image-20261008-090503.png");
    }

    #[test]
    fn size_checks_use_the_server_limit_or_the_default() {
        assert_eq!(check_size(10, Some(10)), Ok(()));
        assert_eq!(check_size(11, Some(10)), Err(TooLarge { limit: 10 }));
        assert_eq!(check_size(100 * 1024 * 1024, None), Ok(()));
        assert_eq!(
            check_size(100 * 1024 * 1024 + 1, None),
            Err(TooLarge {
                limit: 100 * 1024 * 1024
            })
        );
    }

    // ---- files, events and progress ----

    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch directory that is removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "tessera-upload-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).expect("write");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ready(outcome: Outcome) -> Prepared {
        match outcome {
            Outcome::Ready(p) => *p,
            Outcome::Refused(why) => panic!("refused: {why:?}"),
        }
    }

    #[test]
    fn a_picture_file_gets_its_size_type_thumbnail_and_preview() {
        let dir = Scratch::new();
        let path = dir.file("photo.png", PHOTO);
        let p = ready(prepare_path(path.clone(), None));
        assert_eq!(p.name, "photo.png");
        assert_eq!(p.size, PHOTO.len() as u64);
        assert_eq!((p.mime, p.kind), ("image/png", MediaKind::Image));
        assert_eq!((p.width, p.height), (Some(1200), Some(900)));
        assert!(p.thumbnail.is_some() && p.preview.is_some());
        // The core reads the file itself: the bytes are not kept.
        assert!(matches!(p.source, Source::Path(ref kept) if *kept == path));
        assert!(!p.has_exif_note());
    }

    #[test]
    fn a_jpeg_carries_the_exif_note() {
        let dir = Scratch::new();
        let p = ready(prepare_path(dir.file("shot.jpg", ROTATED), None));
        assert!(p.has_exif_note());
        assert_eq!((p.width, p.height), (Some(60), Some(80)));
    }

    #[test]
    fn other_files_are_read_only_for_their_type() {
        let dir = Scratch::new();
        let p = ready(prepare_path(dir.file("notes.txt", b"hello"), None));
        assert_eq!((p.mime, p.kind), ("text/plain", MediaKind::File));
        assert_eq!(p.size, 5);
        assert!(p.thumbnail.is_none() && p.preview.is_none() && p.width.is_none());
        let p = ready(prepare_path(dir.file("what.bin", b"\x00\x01"), None));
        assert_eq!(p.mime, OCTET_STREAM);
    }

    #[test]
    fn a_renamed_picture_is_sent_as_a_file() {
        let dir = Scratch::new();
        let p = ready(prepare_path(dir.file("photo.jpg", PHOTO), None));
        assert_eq!((p.mime, p.kind), (OCTET_STREAM, MediaKind::File));
        assert!(p.thumbnail.is_none());
    }

    #[test]
    fn a_broken_picture_is_sent_as_a_file() {
        let dir = Scratch::new();
        let mut broken = PHOTO[..200].to_vec();
        broken.extend([0; 10]);
        let p = ready(prepare_path(dir.file("cut.png", &broken), None));
        assert_eq!((p.mime, p.kind), (OCTET_STREAM, MediaKind::File));
    }

    #[test]
    fn files_over_the_limit_folders_and_missing_files_are_refused() {
        let dir = Scratch::new();
        let path = dir.file("big.bin", &[0; 100]);
        assert!(matches!(
            prepare_path(path, Some(99)),
            Outcome::Refused(Refusal::TooLarge(TooLarge { limit: 99 }))
        ));
        assert!(matches!(
            prepare_path(dir.0.clone(), None),
            Outcome::Refused(Refusal::NotAFile)
        ));
        assert!(matches!(
            prepare_path(dir.0.join("missing"), None),
            Outcome::Refused(Refusal::Unreadable)
        ));
    }

    #[test]
    fn names_are_the_last_component_cleaned() {
        assert_eq!(name_of(Path::new("/a/b/c.txt")), "c.txt");
        assert_eq!(name_of(Path::new("c.txt")), "c.txt");
        assert_eq!(name_of(Path::new("/")), "download");
        assert_eq!(name_of(Path::new("/a/\u{202E}x\ny.txt")), "xy.txt");
    }

    #[test]
    fn window_events_map_to_messages() {
        let hovered = iced::Event::Window(window::Event::FileHovered(PathBuf::from("/x")));
        assert!(matches!(
            event_message(&hovered),
            Some(Message::FileHovered)
        ));
        let left = iced::Event::Window(window::Event::FilesHoveredLeft);
        assert!(matches!(
            event_message(&left),
            Some(Message::FilesHoveredLeft)
        ));
        let dropped = iced::Event::Window(window::Event::FileDropped(PathBuf::from("/x/y")));
        assert!(matches!(
            event_message(&dropped),
            Some(Message::FileDropped(UploadPaths(p))) if p == [PathBuf::from("/x/y")]
        ));
        // Keys are the shortcut table's, not this function's.
        use iced::keyboard;
        let ctrl_u = iced::Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character("u".into()),
            modified_key: keyboard::Key::Character("u".into()),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::COMMAND,
            text: None,
            repeat: false,
        });
        assert!(event_message(&ctrl_u).is_none());
    }

    #[test]
    fn progress_is_a_fraction_of_one() {
        let at = |sent, total| fraction(UploadProgress { sent, total });
        assert_eq!(at(0, 0), 0.0);
        assert_eq!(at(0, 10), 0.0);
        assert_eq!(at(5, 10), 0.5);
        assert_eq!(at(10, 10), 1.0);
        assert_eq!(at(20, 10), 1.0, "never past the end");
    }

    #[test]
    fn refusals_and_failures_have_words() {
        assert!(failure_text(&CoreError::NotAllowed).contains("not sent"));
        assert!(failure_text(&CoreError::NoRoomSelected).contains("room"));
        assert!(failure_text(&CoreError::Busy).contains("too many"));
        assert!(failure_text(&CoreError::Internal("x".into())).contains("not sent"));
        for why in [Refusal::Unreadable, Refusal::NotAFile, Refusal::BadPicture] {
            assert!(!why.text().is_empty());
        }
        assert!(!CANCEL_REFUSED.is_empty());
    }

    #[test]
    fn the_apps_side_reads_no_files_and_decodes_nothing_itself() {
        let source = include_str!("upload.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production code");
        let app_side = production.split("impl App {").nth(1).expect("the App impl");
        for forbidden in ["std::fs", "File::open", "inspect_image(", "make_thumbnail("] {
            assert!(
                !app_side.contains(forbidden),
                "`{forbidden}` in the App impl, which `update` calls"
            );
        }
        // Reading and encoding only happen inside the bounded workers.
        for call in ["prepare_path(", "prepare_pixels("] {
            let uses = app_side.matches(call).count();
            let inside = app_side
                .split("blocking::run(")
                .skip(1)
                .filter(|after| {
                    after
                        .split("unwrap_or")
                        .next()
                        .is_some_and(|b| b.contains(call))
                })
                .count();
            assert_eq!(uses, inside, "{call} outside blocking::run");
        }
    }
}
