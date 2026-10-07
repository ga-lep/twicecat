//! Media types of the core API: what a received media message looks like and
//! what an upload carries.
//!
//! No SDK type, `mxc://` URI or encryption key appears here. A media message
//! is addressed by `(room_id, item_id)` and the core looks the item up itself.
//! File names are user content: every `Debug` impl hides them.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use super::{EventId, RoomId};

/// Bytes of a media file or thumbnail. Cloning shares the buffer; two values
/// are equal when their contents are. `Debug` shows only the length.
///
/// ```
/// use tessera_core::api::MediaBytes;
/// let a = MediaBytes::from(vec![1, 2, 3]);
/// assert_eq!(a, MediaBytes::from(vec![1, 2, 3]));
/// assert_eq!(format!("{a:?}"), "MediaBytes(<3 bytes>)");
/// assert_eq!(a.len(), 3);
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct MediaBytes(Arc<[u8]>);

impl MediaBytes {
    /// Borrows the bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when there are no bytes.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for MediaBytes {
    fn from(value: Vec<u8>) -> Self {
        Self(Arc::from(value))
    }
}

impl fmt::Debug for MediaBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MediaBytes(<{} bytes>)", self.0.len())
    }
}

/// What a media message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    /// A picture.
    Image,
    /// A video.
    Video,
    /// An audio file.
    Audio,
    /// Any other file.
    File,
}

/// Which rendition [`Command::FetchMedia`](super::Command::FetchMedia) wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaSize {
    /// A server-made or sender-supplied preview.
    Thumbnail,
    /// The whole file.
    Full,
}

/// How far an upload has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UploadProgress {
    /// Bytes sent so far.
    pub sent: u64,
    /// Bytes to send in all.
    pub total: u64,
}

/// A description of the media in a message. `Debug` hides the file name.
#[derive(Clone, PartialEq, Eq)]
pub struct MediaContent {
    /// What the media is.
    pub kind: MediaKind,
    /// The file name as the sender gave it. Untrusted: sanitise before
    /// display.
    pub filename: String,
    /// The declared MIME type.
    pub mimetype: Option<String>,
    /// The declared size in bytes.
    pub size: Option<u64>,
    /// The declared width in pixels.
    pub width: Option<u32>,
    /// The declared height in pixels.
    pub height: Option<u32>,
    /// The declared duration, for audio and video.
    pub duration_ms: Option<u64>,
    /// A thumbnail can be fetched.
    pub has_thumbnail: bool,
    /// Progress of the upload, for the user's own local echoes.
    pub upload: Option<UploadProgress>,
}

impl MediaContent {
    /// A description with only a kind and a file name; everything else is
    /// unknown.
    ///
    /// ```
    /// use tessera_core::api::{MediaContent, MediaKind};
    /// let m = MediaContent::new(MediaKind::File, "a.txt".into());
    /// assert!(m.size.is_none() && !m.has_thumbnail && m.upload.is_none());
    /// ```
    pub fn new(kind: MediaKind, filename: String) -> Self {
        Self {
            kind,
            filename,
            mimetype: None,
            size: None,
            width: None,
            height: None,
            duration_ms: None,
            has_thumbnail: false,
            upload: None,
        }
    }
}

// Manual impl: the file name is user content and must not reach logs.
impl fmt::Debug for MediaContent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaContent")
            .field("kind", &self.kind)
            .field(
                "filename",
                &format_args!("<redacted, {} bytes>", self.filename.len()),
            )
            .field("mimetype", &self.mimetype)
            .field("size", &self.size)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("duration_ms", &self.duration_ms)
            .field("has_thumbnail", &self.has_thumbnail)
            .field("upload", &self.upload)
            .finish()
    }
}

/// Where the bytes of an upload come from. `Debug` hides the path.
#[derive(Clone, PartialEq, Eq)]
pub enum AttachmentData {
    /// A file the core reads itself.
    Path(PathBuf),
    /// Bytes already in memory (a pasted image, for example).
    Bytes(MediaBytes),
}

impl fmt::Debug for AttachmentData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Path(_) => f.write_str("Path(<redacted>)"),
            Self::Bytes(bytes) => f.debug_tuple("Bytes").field(bytes).finish(),
        }
    }
}

/// A preview made by the caller and sent with an image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadThumbnail {
    /// The encoded image.
    pub data: MediaBytes,
    /// Its MIME type.
    pub mimetype: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// A file to send as a media message. `Debug` hides the file name and path.
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    /// Target room.
    pub room_id: RoomId,
    /// The file name to show.
    pub filename: String,
    /// The MIME type.
    pub mimetype: String,
    /// The bytes.
    pub data: AttachmentData,
    /// What kind of message to send.
    pub kind: MediaKind,
    /// Width of the original, for images.
    pub width: Option<u32>,
    /// Height of the original, for images.
    pub height: Option<u32>,
    /// A preview, for images.
    pub thumbnail: Option<UploadThumbnail>,
    /// The event to reply to.
    pub reply_to: Option<EventId>,
    /// Send into the thread rooted at this event.
    pub thread_root: Option<EventId>,
}

// Manual impl: the file name is user content.
impl fmt::Debug for OutgoingAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingAttachment")
            .field("room_id", &self.room_id)
            .field(
                "filename",
                &format_args!("<redacted, {} bytes>", self.filename.len()),
            )
            .field("mimetype", &self.mimetype)
            .field("data", &self.data)
            .field("kind", &self.kind)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("thumbnail", &self.thumbnail)
            .field("reply_to", &self.reply_to)
            .field("thread_root", &self.thread_root)
            .finish()
    }
}
