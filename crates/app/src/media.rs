//! Media in the timeline: box sizes, bounded decoding of untrusted images, the
//! thumbnail cache, and the file-name rules for cards and saving (plan Q7,
//! Q8, Task 9).
//!
//! Everything here is pure, so `update` and the views can be tested without a
//! window. Image bytes come from other users through the homeserver, so they
//! are decoded here, with an explicit format allow-list and `image` limits,
//! into plain pixels. They never go to the renderer as they are.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use iced::Size;
use iced::widget::image::Handle;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder as _, ImageFormat, ImageReader, Limits, RgbaImage};
use tessera_core::api::{MediaContent, MediaKind, RoomId};

use crate::text_safety;

/// A cached item: the room and the timeline item ID.
pub type Key = (RoomId, String);

/// The widest inline box, in pixels.
pub const BOX_MAX_W: f32 = 400.0;
/// The tallest inline box, in pixels.
pub const BOX_MAX_H: f32 = 300.0;
/// The box when the sender gave no usable size.
pub const BOX_DEFAULT: Size = Size::new(320.0, 180.0);
/// The widest or tallest image the decoder accepts, in pixels.
pub const MAX_SIDE: u32 = 16_384;
/// The most memory the decoder may use for one image, in bytes.
pub const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// The byte budget of the thumbnail cache.
pub const THUMB_BUDGET: usize = 64 * 1024 * 1024;

/// The size of the box an image is drawn in.
///
/// The sender's width and height are scaled to fit 400 x 300 and never
/// enlarged. The size is never zero. Without both numbers (or with a zero),
/// it is 320 x 180. The box is fixed before any bytes arrive, so a loading
/// image never changes the height of its row.
///
/// ```
/// use tessera_app::media::media_box;
/// assert_eq!(media_box(Some(800), Some(300)).width, 400.0);
/// assert_eq!(media_box(None, None).height, 180.0);
/// ```
pub fn media_box(width: Option<u32>, height: Option<u32>) -> Size {
    let (Some(w), Some(h)) = (width.filter(|w| *w > 0), height.filter(|h| *h > 0)) else {
        return BOX_DEFAULT;
    };
    // f32 holds any u32 well enough here; the result is rounded to pixels.
    let (w, h) = (w as f32, h as f32);
    let scale = (BOX_MAX_W / w).min(BOX_MAX_H / h).min(1.0);
    Size::new((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

/// The limits for decoding untrusted images.
pub fn decode_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

/// Decodes untrusted bytes to pixels (first frame of a GIF), or `None`,
/// turned the way their EXIF orientation says (phone photos are often
/// stored on their side).
pub(crate) fn decode_rgba(bytes: &[u8], limits: Limits) -> Option<RgbaImage> {
    Some(decode_oriented(bytes, limits)?.0.into_rgba8())
}

/// Decodes untrusted bytes (PNG, JPEG, WebP or the first frame of a GIF)
/// and applies their EXIF orientation. Returns the picture as it is shown
/// and the orientation that was applied, or `None` for anything else.
///
/// The header is checked against `limits` before any pixel buffer exists,
/// and the RGBA size against `limits.max_alloc`.
pub(crate) fn decode_oriented(bytes: &[u8], limits: Limits) -> Option<(DynamicImage, Orientation)> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // An allow-list, not "whatever the crate was built with".
    if !matches!(
        reader.format()?,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    ) {
        return None;
    }
    let max_alloc = limits.max_alloc;
    reader.limits(limits);
    // `into_decoder` applies the limits to the header; `from_decoder` does
    // not reserve against `max_alloc`, so the RGBA size is checked here.
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (w, h) = decoder.dimensions();
    if max_alloc.is_some_and(|max| u64::from(w) * u64::from(h) * 4 > max) {
        return None;
    }
    let mut picture = DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    Some((picture, orientation))
}

/// Decodes untrusted bytes into an image handle and its size, or `None`.
///
/// Only PNG, JPEG, WebP and GIF are accepted (a GIF gives its first frame),
/// and an image over `limits` is refused from its header, before the pixel
/// buffer exists. The bytes are never given to the renderer as they are.
pub fn decode_media(bytes: &[u8], limits: Limits) -> Option<(Handle, u32, u32)> {
    let rgba = decode_rgba(bytes, limits)?;
    let (width, height) = rgba.dimensions();
    Some((
        Handle::from_rgba(width, height, rgba.into_raw()),
        width,
        height,
    ))
}

/// Pixels ready for the renderer, with their size. `Debug` shows the size
/// only.
#[derive(Clone)]
pub struct Decoded {
    /// The plain RGBA pixels.
    pub handle: Handle,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Decoded {
    /// What the pixels cost in memory, in bytes.
    pub fn cost(&self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
    }
}

impl fmt::Debug for Decoded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Decoded({}x{})", self.width, self.height)
    }
}

/// What the cache knows about one thumbnail.
#[derive(Clone, Debug)]
pub enum Thumb {
    /// Decoded and ready to draw.
    Ready(Decoded),
    /// The core had no preview, or the bytes failed validation. Not retried
    /// while the entry is cached.
    Unavailable,
}

/// What a "no preview" entry is charged, in bytes.
const UNAVAILABLE_COST: usize = 64;

/// Decodes a thumbnail for the timeline: validated like any image, then
/// scaled down to fit 400 x 300 so the renderer never holds more than a
/// box's worth of pixels. `None` when the bytes are refused.
pub fn decode_thumbnail(bytes: &[u8]) -> Option<Decoded> {
    let rgba = decode_rgba(bytes, decode_limits())?;
    let (width, height) = rgba.dimensions();
    let rgba = if width > BOX_MAX_W as u32 || height > BOX_MAX_H as u32 {
        image::DynamicImage::ImageRgba8(rgba)
            .thumbnail(BOX_MAX_W as u32, BOX_MAX_H as u32)
            .into_rgba8()
    } else {
        rgba
    };
    let (width, height) = rgba.dimensions();
    Some(Decoded {
        handle: Handle::from_rgba(width, height, rgba.into_raw()),
        width,
        height,
    })
}

/// Decodes a full-size image for the viewer, within [`decode_limits`].
pub fn decode_full(bytes: &[u8]) -> Option<Decoded> {
    let (handle, width, height) = decode_media(bytes, decode_limits())?;
    Some(Decoded {
        handle,
        width,
        height,
    })
}

/// A byte-budgeted LRU.
pub struct MediaCache {
    budget: usize,
    used: usize,
    tick: u64,
    entries: HashMap<Key, (Thumb, usize, u64)>,
}

impl MediaCache {
    /// An empty cache.
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            tick: 0,
            entries: HashMap::new(),
        }
    }

    /// Bytes used.
    pub fn used(&self) -> usize {
        self.used
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drops everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.used = 0;
    }

    /// Stores an entry.
    ///
    /// Older entries go first until the new one fits. An entry bigger than
    /// the whole budget is refused (`false`) and evicts nothing. Replacing a
    /// key frees its old cost first.
    pub fn insert(&mut self, key: Key, value: Thumb, cost: usize) -> bool {
        if cost > self.budget {
            return false;
        }
        if let Some((_, old_cost, _)) = self.entries.remove(&key) {
            self.used -= old_cost;
        }
        while self.used + cost > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used_at))| *used_at)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((_, freed, _)) = self.entries.remove(&oldest) {
                self.used -= freed;
            }
        }
        self.tick += 1;
        self.used += cost;
        self.entries.insert(key, (value, cost, self.tick));
        true
    }

    /// Marks an entry as used; false when absent.
    pub fn touch(&mut self, key: &Key) -> bool {
        self.tick += 1;
        match self.entries.get_mut(key) {
            Some(entry) => {
                entry.2 = self.tick;
                true
            }
            None => false,
        }
    }

    /// Reads without marking.
    pub fn peek(&self, key: &Key) -> Option<&Thumb> {
        self.entries.get(key).map(|(value, _, _)| value)
    }
}

/// The most thumbnails being fetched or decoded at once.
pub const MAX_PENDING: usize = 24;
/// The most saves waiting for the core's answer.
const MAX_SAVING: usize = 16;
/// The most saved paths kept for **Open**.
const MAX_SAVED: usize = 64;
/// The longest file name drawn, in characters.
pub const MAX_NAME_CHARS: usize = 96;

/// A path from the user's save dialog. `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub struct SavePath(PathBuf);

impl SavePath {
    /// Borrows the path.
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl From<PathBuf> for SavePath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl fmt::Debug for SavePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SavePath(<redacted>)")
    }
}

/// Whether `path` can be handed to `SaveMedia`: absolute, with a file name
/// that is not `..` or a root. It comes from the dialog; a relative path
/// would land wherever the process happens to run.
pub fn save_path_usable(path: &Path) -> bool {
    path.is_absolute() && path.file_name().is_some()
}

/// The viewer's state.
#[derive(Clone, Debug)]
enum ViewerState {
    Loading,
    Ready(Decoded),
    Failed,
}

/// The full-size viewer: one item, one decoded image at a time.
pub struct Viewer {
    /// The image being viewed.
    pub key: Key,
    state: ViewerState,
}

impl Viewer {
    /// The decoded full image, once there is one.
    pub fn decoded(&self) -> Option<&Decoded> {
        match &self.state {
            ViewerState::Ready(decoded) => Some(decoded),
            _ => None,
        }
    }

    /// The fetch or the decode failed.
    pub fn failed(&self) -> bool {
        matches!(self.state, ViewerState::Failed)
    }
}

// Manual impl: no item ID, no pixels.
impl fmt::Debug for Viewer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Viewer")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// The bar that asks "Open <file name>?" before a saved file is opened.
pub struct OpenBar {
    /// The item.
    pub key: Key,
    path: PathBuf,
    name: String,
    may_run_code: bool,
}

impl OpenBar {
    /// The question, with the sanitised name of the file that would open.
    pub fn question(&self) -> String {
        format!("Open {}?", self.name)
    }

    /// The warning under the question when the file may run code (see
    /// [`may_run_code`]).
    pub fn warning(&self) -> Option<&'static str> {
        self.may_run_code.then_some(
            "This file can run a program on your computer. Open it only if you trust the sender.",
        )
    }

    /// The saved file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// Manual impl: the path and the name are user content.
impl fmt::Debug for OpenBar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OpenBar(<redacted>)")
    }
}

/// Everything the app knows about media in the open room: decoded
/// thumbnails, fetches in flight, the viewer, saves and the open question.
pub struct MediaModel {
    cache: MediaCache,
    pending: HashSet<Key>,
    /// The full-size viewer, while open.
    pub viewer: Option<Viewer>,
    /// Saves sent to the core and not answered yet.
    saving: Vec<(Key, PathBuf)>,
    /// Files the core saved, for **Open**.
    saved: Vec<(Key, PathBuf)>,
    /// The open question, while asked.
    pub open_bar: Option<OpenBar>,
}

impl Default for MediaModel {
    fn default() -> Self {
        Self {
            cache: MediaCache::new(THUMB_BUDGET),
            pending: HashSet::new(),
            viewer: None,
            saving: Vec::new(),
            saved: Vec::new(),
            open_bar: None,
        }
    }
}

// Manual impl: counts only, no IDs, names, paths or pixels.
impl fmt::Debug for MediaModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaModel")
            .field("cached", &self.cache.len())
            .field("cached_bytes", &self.cache.used())
            .field("pending", &self.pending.len())
            .field("viewer", &self.viewer)
            .field("saving", &self.saving.len())
            .field("saved", &self.saved.len())
            .field("open_bar", &self.open_bar.is_some())
            .finish()
    }
}

impl MediaModel {
    /// What is known about the thumbnail of `key`.
    pub fn thumb(&self, key: &Key) -> Option<&Thumb> {
        self.cache.peek(key)
    }

    /// Whether a thumbnail of `key` should be requested now. When it is
    /// cached this marks it as recently used; when it is neither cached nor
    /// in flight (and not too many are), it is marked as in flight.
    pub fn needs_fetch(&mut self, key: &Key) -> bool {
        if self.cache.touch(key) || self.pending.contains(key) {
            return false;
        }
        if self.pending.len() >= MAX_PENDING {
            return false;
        }
        self.pending.insert(key.clone());
        true
    }

    /// A thumbnail of `key` is in flight.
    pub fn is_pending(&self, key: &Key) -> bool {
        self.pending.contains(key)
    }

    /// The core could not give a thumbnail.
    pub fn thumb_failed(&mut self, key: &Key) {
        self.thumb_decoded(key, None);
    }

    /// A thumbnail was decoded (`None`: refused). Ignored unless it was asked
    /// for.
    pub fn thumb_decoded(&mut self, key: &Key, decoded: Option<Decoded>) {
        if !self.pending.remove(key) {
            return;
        }
        match decoded {
            Some(decoded) => {
                let cost = decoded.cost();
                if !self.cache.insert(key.clone(), Thumb::Ready(decoded), cost) {
                    self.cache
                        .insert(key.clone(), Thumb::Unavailable, UNAVAILABLE_COST);
                }
            }
            None => {
                self.cache
                    .insert(key.clone(), Thumb::Unavailable, UNAVAILABLE_COST);
            }
        }
    }

    /// Opens the viewer on `key`, dropping any image it showed.
    pub fn open_viewer(&mut self, key: Key) {
        self.viewer = Some(Viewer {
            key,
            state: ViewerState::Loading,
        });
    }

    /// Closes the viewer and drops the full image.
    pub fn close_viewer(&mut self) {
        self.viewer = None;
    }

    /// The viewer waits for the full file of `key`.
    pub fn viewer_wants(&self, key: &Key) -> bool {
        self.viewer
            .as_ref()
            .is_some_and(|v| v.key == *key && matches!(v.state, ViewerState::Loading))
    }

    /// The full image was decoded (`None`: failed). Ignored unless the viewer
    /// still waits for `key`.
    pub fn viewer_decoded(&mut self, key: &Key, decoded: Option<Decoded>) {
        if !self.viewer_wants(key) {
            return;
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.state = match decoded {
                Some(decoded) => ViewerState::Ready(decoded),
                None => ViewerState::Failed,
            };
        }
    }

    /// A save to `path` was sent to the core.
    pub fn begin_save(&mut self, key: Key, path: PathBuf) {
        self.saving.retain(|(k, _)| *k != key);
        if self.saving.len() >= MAX_SAVING {
            self.saving.remove(0);
        }
        self.saving.push((key, path));
    }

    /// The core saved `key`; only a save this app started counts.
    pub fn finish_save(&mut self, key: &Key) {
        let Some(at) = self.saving.iter().position(|(k, _)| k == key) else {
            return;
        };
        let (key, path) = self.saving.remove(at);
        self.saved.retain(|(k, _)| *k != key);
        if self.saved.len() >= MAX_SAVED {
            self.saved.remove(0);
        }
        self.saved.push((key, path));
    }

    /// A save in `room` failed; the core does not say which, so every
    /// waiting save of the room is forgotten.
    pub fn fail_saves(&mut self, room: &RoomId) {
        self.saving.retain(|((r, _), _)| r != room);
    }

    /// Where `key` was saved.
    pub fn saved_path(&self, key: &Key) -> Option<&Path> {
        self.saved
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, path)| path.as_path())
    }

    /// Whether **Open** is offered for `key`: it was saved in this session.
    /// Any type may be opened; the bar asks first, and warns when the file
    /// may run code.
    pub fn open_allowed(&self, key: &Key) -> bool {
        self.saved_path(key).is_some()
    }

    /// Asks "Open <file name>?" for `key`, if it was saved. The bar warns
    /// when the sender-given `filename` or the name the user saved it under
    /// may run code.
    pub fn ask_open(&mut self, key: &Key, filename: &str) {
        let Some(path) = self.saved_path(key).map(Path::to_path_buf) else {
            return;
        };
        let shown = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.open_bar = Some(OpenBar {
            key: key.clone(),
            name: card_name(&shown),
            may_run_code: may_run_code(filename) || may_run_code(&shown),
            path,
        });
    }

    /// Forgets everything (logout).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Forgets what belongs to the open room's view: the viewer and the
    /// question. Thumbnails stay cached (they are keyed by room).
    pub fn reset_view(&mut self) {
        self.viewer = None;
        self.open_bar = None;
    }
}

/// A file name for display: sanitised by `text_safety` and capped.
pub fn card_name(filename: &str) -> String {
    text_safety::name_or_placeholder(filename, MAX_NAME_CHARS)
}

/// The word for a kind of media.
pub fn kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "Image",
        MediaKind::Video => "Video",
        MediaKind::Audio => "Audio",
        MediaKind::File => "File",
    }
}

/// The type line of a card: the declared MIME type (sanitised), or the word
/// for the kind.
pub fn type_label(media: &MediaContent) -> String {
    match media.mimetype.as_deref().map(str::trim) {
        Some(mime) if !mime.is_empty() => text_safety::one_line_capped(mime, 64),
        _ => kind_label(media.kind).to_owned(),
    }
}

/// A size such as "1.5 MB" (decimal units), or "unknown size".
///
/// ```
/// use tessera_app::media::size_label;
/// assert_eq!(size_label(Some(1_500_000)), "1.5 MB");
/// assert_eq!(size_label(Some(999)), "999 B");
/// assert_eq!(size_label(None), "unknown size");
/// ```
pub fn size_label(size: Option<u64>) -> String {
    let Some(size) = size else {
        return "unknown size".to_owned();
    };
    if size < 1000 {
        return format!("{size} B");
    }
    let mut value = size as f64 / 1000.0;
    let mut unit = "KB";
    for next in ["MB", "GB", "TB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

/// Whether a message's text is a real caption: the core sends the file name
/// as the body when there is none.
pub fn has_caption(body: &str, media: &MediaContent) -> bool {
    !body.trim().is_empty() && body != media.filename
}

/// Extensions that are never opened (plan Q8, plus shortcuts, registry and
/// script files that start something on open).
// Programs, scripts, installers, shortcuts and links, help files, web
// pages, disk images, and their macOS counterparts.
const RUNS_CODE_EXTENSIONS: &[&str] = &[
    "exe",
    "msi",
    "bat",
    "cmd",
    "com",
    "scr",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "mjs",
    "jar",
    "jnlp",
    "wsf",
    "ws",
    "wsh",
    "hta",
    "cpl",
    "msc",
    "xll",
    "inf",
    "reg",
    "py",
    "pyw",
    "pyz",
    "pl",
    "sh",
    "run",
    "bin",
    "appimage",
    "deb",
    "rpm",
    "flatpakref",
    "flatpakrepo",
    "desktop",
    "application",
    "appref-ms",
    "lnk",
    "pif",
    "url",
    "scf",
    "settingcontent-ms",
    "library-ms",
    "search-ms",
    "chm",
    "hlp",
    "htm",
    "html",
    "xhtml",
    "mht",
    "mhtml",
    "svg",
    "iso",
    "img",
    "vhd",
    "vhdx",
    "app",
    "pkg",
    "mpkg",
    "dmg",
    "command",
    "terminal",
    "tool",
    "workflow",
    "scpt",
    "applescript",
    "fileloc",
    "webloc",
    "inetloc",
    "prefpane",
];

/// Whether a file with this name may run code when the system opens it.
///
/// The extensions of programs, scripts, installers, shortcuts and links,
/// web pages, and any name without an extension, which may be an
/// executable. Case-insensitive. Opening such a file is still allowed after
/// the confirmation; the bar warns about it.
///
/// ```
/// use tessera_app::media::may_run_code;
/// assert!(may_run_code("setup.EXE"));
/// assert!(may_run_code("README"));
/// assert!(!may_run_code("photo.png"));
/// ```
pub fn may_run_code(name: &str) -> bool {
    // Windows ignores trailing dots and spaces: "a.exe." is "a.exe".
    let name = name.trim_end_matches(['.', ' ']);
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return true;
    };
    if stem.is_empty() || extension.contains(['/', '\\']) {
        return true;
    }
    let extension = extension.to_ascii_lowercase();
    RUNS_CODE_EXTENSIONS.contains(&extension.as_str())
}

/// The name offered in the save dialog.
///
/// Path separators, control characters and bidi controls are removed, every
/// `..` goes, and surrounding blanks and dots are trimmed. An empty result is
/// `download`. The user still picks the real path in the dialog; this only
/// keeps the sender's name from steering it.
///
/// ```
/// use tessera_app::media::suggested_file_name;
/// assert_eq!(suggested_file_name("../../etc/passwd"), "etcpasswd");
/// assert_eq!(suggested_file_name(""), "download");
/// ```
pub fn suggested_file_name(name: &str) -> String {
    let mut clean: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c) && !matches!(c, '/' | '\\'))
        .collect();
    while clean.contains("..") {
        clean = clean.replace("..", "");
    }
    let trimmed = clean.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        return "download".to_owned();
    }
    let mut cut = trimmed.len().min(MAX_NAME_BYTES);
    while !trimmed.is_char_boundary(cut) {
        cut -= 1;
    }
    trimmed[..cut].to_owned()
}

/// The name offered when saving media sent at `sent`: the sender's name
/// cleaned by [`suggested_file_name`], with the sent time added when that
/// name is a generic one. Element Web names every pasted picture
/// `image.png`, so without it every save proposes the same name.
pub fn save_name<Tz: chrono::TimeZone>(filename: &str, sent: &chrono::DateTime<Tz>) -> String
where
    Tz::Offset: fmt::Display,
{
    let clean = suggested_file_name(filename);
    let (stem, extension) = match clean.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, Some(extension)),
        _ => (clean.as_str(), None),
    };
    if !GENERIC_STEMS.contains(&stem.to_ascii_lowercase().as_str()) {
        return clean;
    }
    let time = sent.format("%Y%m%d-%H%M%S");
    match extension {
        Some(extension) => format!("{stem}-{time}.{extension}"),
        None => format!("{stem}-{time}"),
    }
}

/// Names clients give files that had none (clipboard pastes, screenshots).
const GENERIC_STEMS: &[&str] = &[
    "blob",
    "clipboard",
    "image",
    "img",
    "photo",
    "picture",
    "screenshot",
    "untitled",
];

/// The longest suggested file name, in bytes (the usual file system limit).
const MAX_NAME_BYTES: usize = 255;

/// Bidi controls and isolates, which can reorder how a name reads.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use image::{
        Delay, Frame, ImageFormat, Rgba,
        codecs::gif::{GifEncoder, Repeat},
    };

    use super::*;

    fn encode(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let img = RgbaImage::from_pixel(width, height, Rgba([10, 200, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        // JPEG has no alpha channel.
        let img = if format == ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img).into_rgb8())
        } else {
            image::DynamicImage::ImageRgba8(img)
        };
        img.write_to(&mut out, format).expect("encode a test image");
        out.into_inner()
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in bytes {
            crc ^= u32::from(b);
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

    /// A PNG whose IHDR claims `width` x `height` (CRC fixed up); the pixel
    /// data is that of a 1x1 image, so it is a header-only lie.
    fn png_claiming(width: u32, height: u32) -> Vec<u8> {
        let mut png = encode(1, 1, ImageFormat::Png);
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    fn two_frame_gif() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).expect("repeat");
            let first = RgbaImage::from_pixel(3, 2, Rgba([255, 0, 0, 255]));
            let second = RgbaImage::from_pixel(3, 2, Rgba([0, 0, 255, 255]));
            enc.encode_frames([
                Frame::from_parts(first, 0, 0, Delay::from_numer_denom_ms(100, 1)),
                Frame::from_parts(second, 0, 0, Delay::from_numer_denom_ms(100, 1)),
            ])
            .expect("encode gif");
        }
        out
    }

    fn dims(handle: &Handle) -> (u32, u32) {
        match handle {
            Handle::Rgba { width, height, .. } => (*width, *height),
            other => panic!("expected decoded pixels, got {other:?}"),
        }
    }

    #[test]
    fn the_box_fits_400_by_300_and_is_never_upscaled() {
        assert_eq!(media_box(Some(200), Some(100)), Size::new(200.0, 100.0));
        assert_eq!(media_box(Some(800), Some(300)), Size::new(400.0, 150.0));
        assert_eq!(media_box(Some(400), Some(600)), Size::new(200.0, 300.0));
        assert_eq!(media_box(Some(4000), Some(3000)), Size::new(400.0, 300.0));
        assert_eq!(media_box(Some(1), Some(1)), Size::new(1.0, 1.0));
    }

    #[test]
    fn an_unknown_or_zero_size_gets_the_default_box() {
        assert_eq!(media_box(None, None), BOX_DEFAULT);
        assert_eq!(media_box(Some(100), None), BOX_DEFAULT);
        assert_eq!(media_box(None, Some(100)), BOX_DEFAULT);
        assert_eq!(media_box(Some(0), Some(100)), BOX_DEFAULT);
        assert_eq!(media_box(Some(100), Some(0)), BOX_DEFAULT);
    }

    #[test]
    fn the_box_is_never_zero_and_always_finite() {
        for (w, h) in [
            (1, u32::MAX),
            (u32::MAX, 1),
            (u32::MAX, u32::MAX),
            (1, 100_000),
        ] {
            let b = media_box(Some(w), Some(h));
            assert!(b.width >= 1.0 && b.height >= 1.0, "{w}x{h} gave {b:?}");
            assert!(b.width <= BOX_MAX_W && b.height <= BOX_MAX_H, "{b:?}");
        }
    }

    #[test]
    fn png_jpeg_webp_and_gif_decode() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Gif,
        ] {
            let bytes = encode(5, 4, format);
            let (handle, w, h) = decode_media(&bytes, decode_limits())
                .unwrap_or_else(|| panic!("{format:?} did not decode"));
            assert_eq!((w, h), (5, 4), "{format:?}");
            assert_eq!(dims(&handle), (5, 4), "{format:?}");
        }
    }

    #[test]
    fn a_gif_gives_its_first_frame() {
        let img = decode_rgba(&two_frame_gif(), decode_limits()).expect("gif decodes");
        assert_eq!(img.dimensions(), (3, 2));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn exif_rotation_is_applied_to_thumbnails_and_the_viewer() {
        // 80x60 stored, EXIF orientation 6 (turn 90° clockwise to show).
        const ROTATED: &[u8] = include_bytes!("../../core/tests/fixtures/exif-orientation-6.jpg");
        let stored = image::load_from_memory(ROTATED)
            .expect("decodes")
            .into_rgba8();
        assert_eq!(stored.dimensions(), (80, 60));
        let turned = image::imageops::rotate90(&stored);
        let shown = decode_rgba(ROTATED, decode_limits()).expect("decodes");
        assert_eq!(shown.dimensions(), (60, 80));
        assert!(
            shown == turned,
            "the pixels are the stored ones turned clockwise"
        );
        let thumb = decode_thumbnail(ROTATED).expect("thumbnail");
        assert_eq!((thumb.width, thumb.height), (60, 80));
        let full = decode_full(ROTATED).expect("full");
        assert_eq!((full.width, full.height), (60, 80));
    }

    #[test]
    fn other_formats_are_refused() {
        let bmp = [b"BM".as_slice(), &[0u8; 64]].concat();
        let tiff = [b"II*\0".as_slice(), &[0u8; 64]].concat();
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#;
        assert!(decode_media(&bmp, decode_limits()).is_none());
        assert!(decode_media(&tiff, decode_limits()).is_none());
        assert!(decode_media(svg, decode_limits()).is_none());
        assert!(decode_media(b"not an image at all", decode_limits()).is_none());
        assert!(decode_media(b"", decode_limits()).is_none());
    }

    #[test]
    fn a_20000_by_20000_header_is_refused() {
        assert!(decode_media(&png_claiming(20_000, 20_000), decode_limits()).is_none());
    }

    #[test]
    fn the_side_limit_is_what_refuses_a_wide_image() {
        let wide = encode(MAX_SIDE + 1, 1, ImageFormat::Png);
        assert!(decode_media(&wide, decode_limits()).is_none());
        let mut lifted = decode_limits();
        lifted.max_image_width = None;
        assert!(decode_media(&wide, lifted).is_some());
        let edge = encode(MAX_SIDE, 1, ImageFormat::Png);
        assert!(decode_media(&edge, decode_limits()).is_some());
    }

    #[test]
    fn the_allocation_limit_refuses_a_big_image() {
        let bytes = encode(64, 64, ImageFormat::Png);
        let mut tight = decode_limits();
        tight.max_alloc = Some(1024);
        assert!(decode_media(&bytes, tight).is_none());
    }

    #[test]
    fn truncated_bytes_are_refused() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Gif,
        ] {
            let bytes = encode(40, 40, format);
            let cut = &bytes[..bytes.len() / 2];
            assert!(
                decode_media(cut, decode_limits()).is_none(),
                "{format:?} cut in half"
            );
        }
    }

    fn ready() -> Thumb {
        Thumb::Ready(Decoded {
            handle: Handle::from_rgba(1, 1, vec![0; 4]),
            width: 1,
            height: 1,
        })
    }

    fn key(n: u32) -> Key {
        (RoomId::from("!r:x"), format!("item{n}"))
    }

    #[test]
    fn the_cache_evicts_the_least_recently_used_first() {
        let mut cache = MediaCache::new(100);
        assert!(cache.insert(key(1), ready(), 40));
        assert!(cache.insert(key(2), ready(), 40));
        assert!(cache.touch(&key(1)));
        assert!(cache.insert(key(3), ready(), 40));
        assert!(cache.peek(&key(1)).is_some(), "touched, so kept");
        assert!(
            cache.peek(&key(2)).is_none(),
            "least recently used, so gone"
        );
        assert!(cache.peek(&key(3)).is_some());
        assert!(!cache.touch(&key(2)));
    }

    #[test]
    fn the_cache_never_exceeds_its_budget() {
        let mut cache = MediaCache::new(100);
        for n in 0..50 {
            cache.insert(key(n), ready(), 30);
            assert!(cache.used() <= 100, "used {}", cache.used());
        }
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.used(), 90);
    }

    #[test]
    fn an_entry_over_the_budget_is_refused_and_evicts_nothing() {
        let mut cache = MediaCache::new(100);
        cache.insert(key(1), ready(), 60);
        assert!(!cache.insert(key(2), ready(), 101));
        assert!(cache.peek(&key(1)).is_some());
        assert_eq!(cache.used(), 60);
    }

    #[test]
    fn replacing_an_entry_adjusts_the_used_bytes() {
        let mut cache = MediaCache::new(100);
        cache.insert(key(1), ready(), 60);
        cache.insert(key(1), Thumb::Unavailable, 10);
        assert_eq!(cache.used(), 10);
        assert_eq!(cache.len(), 1);
        assert!(matches!(cache.peek(&key(1)), Some(Thumb::Unavailable)));
        cache.clear();
        assert!(cache.is_empty() && cache.used() == 0);
    }

    #[test]
    fn names_that_may_run_code() {
        for name in [
            "a.exe",
            "a.EXE",
            "a.Msi",
            "a.bat",
            "a.cmd",
            "a.com",
            "a.scr",
            "a.ps1",
            "a.vbs",
            "a.js",
            "a.jar",
            "a.sh",
            "a.desktop",
            "a.AppImage",
            "a.app",
            "a.pkg",
            "a.dmg",
            "a.deb",
            "a.rpm",
            "a.run",
            "a.tar.sh",
            "gnp.exe",
            "a.exe.",
            "a.exe ",
            "a.lnk",
            // Missed by the deny-list (Task 9 audit): links, help files,
            // repo files, Finder and Terminal files, and web pages.
            "a.url",
            "a.chm",
            "a.flatpakref",
            "a.flatpakrepo",
            "a.fileloc",
            "a.webloc",
            "a.inetloc",
            "a.terminal",
            "a.htm",
            "a.HTML",
            "a.xhtml",
            "a.svg",
            "a.mht",
            "a.cpl",
            "a.msc",
            "a.appref-ms",
            "a.jnlp",
            "a.iso",
            "a.workflow",
        ] {
            assert!(may_run_code(name), "{name:?}");
        }
        for name in [
            "",
            "noextension",
            "README",
            ".bashrc",
            "archive.",
            "dir.d/file",
        ] {
            assert!(may_run_code(name), "{name:?} has no extension");
        }
        for name in [
            "a.png",
            "a.PDF",
            "notes.txt",
            "a.tar.gz",
            "photo.jpeg",
            "x.mp4",
            "x.exe.txt",
            "song.flac",
            "doc.odt",
        ] {
            assert!(!may_run_code(name), "{name:?}");
        }
    }

    #[test]
    fn generic_names_get_the_sent_time_and_real_names_stay() {
        use chrono::{TimeZone, Utc};
        let sent = Utc.with_ymd_and_hms(2026, 10, 8, 15, 30, 12).unwrap();
        // Element Web names every pasted picture `image.png`.
        assert_eq!(save_name("image.png", &sent), "image-20261008-153012.png");
        assert_eq!(save_name("Image.JPG", &sent), "Image-20261008-153012.JPG");
        assert_eq!(save_name("blob", &sent), "blob-20261008-153012");
        assert_eq!(
            save_name("Screenshot.png", &sent),
            "Screenshot-20261008-153012.png"
        );
        assert_eq!(save_name("cat.png", &sent), "cat.png");
        assert_eq!(save_name("image-of-a-cat.png", &sent), "image-of-a-cat.png");
        // Cleaned first, so the time never goes into a hostile name.
        assert_eq!(
            save_name("../image.png", &sent),
            "image-20261008-153012.png"
        );
        assert_eq!(save_name("", &sent), "download");
    }

    #[test]
    fn suggested_names_lose_separators_dots_controls_and_bidi() {
        assert_eq!(suggested_file_name("photo.png"), "photo.png");
        assert_eq!(suggested_file_name("a/b\\c.txt"), "abc.txt");
        assert_eq!(suggested_file_name("../../etc/passwd"), "etcpasswd");
        assert_eq!(suggested_file_name("a..b"), "ab");
        assert_eq!(suggested_file_name("....x"), "x");
        assert_eq!(suggested_file_name("a\u{0}b\nc\td\u{7f}.txt"), "abcd.txt");
        // Right-to-left override: "gnp.exe" displays as "exe.png".
        assert_eq!(suggested_file_name("\u{202E}gnp.exe"), "gnp.exe");
        assert_eq!(suggested_file_name("a\u{2066}b\u{2069}.txt"), "ab.txt");
        assert_eq!(suggested_file_name("a\u{200E}b\u{061C}.txt"), "ab.txt");
    }

    #[test]
    fn an_empty_suggested_name_is_download() {
        for name in ["", " ", "/", "..", "...", "\u{202E}", "\n", " . "] {
            assert_eq!(suggested_file_name(name), "download", "{name:?}");
        }
    }

    #[test]
    fn a_long_suggested_name_is_cut_on_a_character_boundary() {
        let long = "é".repeat(500);
        let out = suggested_file_name(&long);
        assert!(out.len() <= 255, "{} bytes", out.len());
        assert!(out.starts_with('é'));
    }
}
