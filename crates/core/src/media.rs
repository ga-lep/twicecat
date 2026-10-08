//! Media messages: what the timeline shows of them, which rendition to
//! fetch, and saving a file to disk.
//!
//! File names, captions and the bytes themselves are untrusted user content:
//! nothing here logs them, and no error text quotes server data. No MXC URI
//! or encryption key leaves this module; the API addresses media by timeline
//! item only.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::events::room::message::MessageType;
use matrix_sdk::ruma::uint;
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::api::{CoreError, CoreEvent, MediaBytes, MediaContent, MediaKind, MediaSize};

/// Fetches running at the same time (thumbnails and files together).
pub(crate) const MAX_CONCURRENT_FETCHES: usize = 4;
/// Fetches waiting for a permit or running; more are answered `Busy`, so a
/// flood of commands cannot pile up tasks.
pub(crate) const MAX_PENDING_FETCHES: usize = 128;
/// Deadline of one thumbnail fetch. The SDK download has no timeout of its
/// own (matrix-sdk 0.19.1 media.rs:818 sets `Duration::MAX`).
pub(crate) const THUMBNAIL_DEADLINE: Duration = Duration::from_secs(30);
/// Deadline of one full-file fetch.
pub(crate) const FULL_DEADLINE: Duration = Duration::from_secs(600);
/// Largest declared size for which an encrypted image without a thumbnail is
/// fetched whole to serve as its own preview. Also the largest thumbnail the
/// core hands out.
pub(crate) const MAX_THUMBNAIL_BYTES: u64 = 10 * 1024 * 1024;
/// Largest file the core fetches: the SDK holds the whole body in memory.
pub(crate) const MAX_FULL_BYTES: u64 = 512 * 1024 * 1024;
/// Suffix of the file a save is written to before it is renamed.
const PART_SUFFIX: &str = ".tessera-part";
/// Deadline of the server limits request.
const LIMITS_DEADLINE: Duration = Duration::from_secs(15);

/// The parts of a media message the core needs, borrowed from the event.
struct Parts<'a> {
    kind: MediaKind,
    filename: &'a str,
    source: &'a MediaSource,
    thumbnail_source: Option<&'a MediaSource>,
    mimetype: Option<&'a str>,
    size: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    duration_ms: Option<u64>,
}

/// Where a thumbnail comes from.
#[derive(Debug)]
pub(crate) enum ThumbnailPlan<'a> {
    /// A source scaled to a preview: the sender's thumbnail, or the image
    /// itself when it is not encrypted (the server scales it).
    Source(&'a MediaSource),
    /// The whole (small, encrypted) image, used as its own preview.
    FullFile(&'a MediaSource),
    /// No preview: show a card.
    None,
}

fn parts(msgtype: &MessageType) -> Option<Parts<'_>> {
    let uint = |v: Option<matrix_sdk::ruma::UInt>| v.map(u64::from);
    let dim = |v: Option<matrix_sdk::ruma::UInt>| v.and_then(|v| u32::try_from(u64::from(v)).ok());
    let millis = |d: Option<Duration>| d.and_then(|d| u64::try_from(d.as_millis()).ok());
    Some(match msgtype {
        MessageType::Image(c) => {
            let info = c.info.as_deref();
            Parts {
                kind: MediaKind::Image,
                filename: c.filename(),
                source: &c.source,
                thumbnail_source: info.and_then(|i| i.thumbnail_source.as_ref()),
                mimetype: info.and_then(|i| i.mimetype.as_deref()),
                size: uint(info.and_then(|i| i.size)),
                width: dim(info.and_then(|i| i.width)),
                height: dim(info.and_then(|i| i.height)),
                duration_ms: None,
            }
        }
        MessageType::Video(c) => {
            let info = c.info.as_deref();
            Parts {
                kind: MediaKind::Video,
                filename: c.filename(),
                source: &c.source,
                thumbnail_source: info.and_then(|i| i.thumbnail_source.as_ref()),
                mimetype: info.and_then(|i| i.mimetype.as_deref()),
                size: uint(info.and_then(|i| i.size)),
                width: dim(info.and_then(|i| i.width)),
                height: dim(info.and_then(|i| i.height)),
                duration_ms: millis(info.and_then(|i| i.duration)),
            }
        }
        MessageType::Audio(c) => {
            let info = c.info.as_deref();
            Parts {
                kind: MediaKind::Audio,
                filename: c.filename(),
                source: &c.source,
                thumbnail_source: None,
                mimetype: info.and_then(|i| i.mimetype.as_deref()),
                size: uint(info.and_then(|i| i.size)),
                width: None,
                height: None,
                duration_ms: millis(info.and_then(|i| i.duration)),
            }
        }
        MessageType::File(c) => {
            let info = c.info.as_deref();
            Parts {
                kind: MediaKind::File,
                filename: c.filename(),
                source: &c.source,
                thumbnail_source: info.and_then(|i| i.thumbnail_source.as_ref()),
                mimetype: info.and_then(|i| i.mimetype.as_deref()),
                size: uint(info.and_then(|i| i.size)),
                width: None,
                height: None,
                duration_ms: None,
            }
        }
        _ => return None,
    })
}

/// Maps an image, video, audio or file message to its description; `None`
/// for every other message type.
pub(crate) fn map_media(msgtype: &MessageType) -> Option<MediaContent> {
    let parts = parts(msgtype)?;
    let has_thumbnail = !matches!(plan(&parts), ThumbnailPlan::None);
    Some(MediaContent {
        kind: parts.kind,
        filename: parts.filename.to_owned(),
        mimetype: parts.mimetype.map(str::to_owned),
        size: parts.size,
        width: parts.width,
        height: parts.height,
        duration_ms: parts.duration_ms,
        has_thumbnail,
        upload: None,
    })
}

/// The message has a caption: the body differs from the file name.
pub(crate) fn has_caption(msgtype: &MessageType) -> bool {
    match msgtype {
        MessageType::Image(c) => c.caption().is_some(),
        MessageType::Video(c) => c.caption().is_some(),
        MessageType::Audio(c) => c.caption().is_some(),
        MessageType::File(c) => c.caption().is_some(),
        _ => false,
    }
}

/// How a thumbnail of this message is made.
pub(crate) fn thumbnail_plan(msgtype: &MessageType) -> ThumbnailPlan<'_> {
    match parts(msgtype) {
        Some(parts) => plan(&parts),
        None => ThumbnailPlan::None,
    }
}

fn plan<'a>(parts: &Parts<'a>) -> ThumbnailPlan<'a> {
    if let Some(thumbnail) = parts.thumbnail_source {
        return ThumbnailPlan::Source(thumbnail);
    }
    match (parts.kind, parts.source) {
        // The server scales a plain image.
        (MediaKind::Image, MediaSource::Plain(_)) => ThumbnailPlan::Source(parts.source),
        // An encrypted image can only be scaled by whoever holds the key, so
        // a small one is its own preview.
        (MediaKind::Image, MediaSource::Encrypted(_))
            if parts.size.is_some_and(|size| size <= MAX_THUMBNAIL_BYTES) =>
        {
            ThumbnailPlan::FullFile(parts.source)
        }
        _ => ThumbnailPlan::None,
    }
}

/// The declared size allows fetching the whole file.
pub(crate) fn allowed_full(size: Option<u64>) -> bool {
    size.is_none_or(|size| size <= MAX_FULL_BYTES)
}

/// A source the core may request: a well-formed MXC URI, and a `.localhost`
/// server (the SDK's local media for pending uploads) only for a local echo.
pub(crate) fn source_permitted(source: &MediaSource, local_echo: bool) -> bool {
    let uri = match source {
        MediaSource::Plain(uri) => uri,
        MediaSource::Encrypted(file) => &file.url,
    };
    match uri.parts() {
        // The validator accepts an empty media ID; a request for it is useless.
        Ok((server, media)) if !media.is_empty() => {
            local_echo || !server.host().to_ascii_lowercase().ends_with(".localhost")
        }
        _ => false,
    }
}

/// Where content bytes come from. Production uses the SDK media API; tests
/// use a fake.
pub(crate) trait ContentFetcher: Send + Sync + 'static {
    /// Fetches (and decrypts) one rendition of already validated media, and
    /// stops reading past `cap` bytes.
    fn fetch(
        &self,
        request: &MediaRequestParameters,
        cap: u64,
    ) -> impl Future<Output = Result<Vec<u8>, ()>> + Send;
}

/// Fetches through `Client::media().get_media_content` with the cache on.
/// The client carries [`crate::media_fetch::CappedMediaFetcher`], which stops
/// reading at the cap given here.
pub(crate) struct SdkContentFetcher(pub(crate) Client);

impl ContentFetcher for SdkContentFetcher {
    async fn fetch(&self, request: &MediaRequestParameters, cap: u64) -> Result<Vec<u8>, ()> {
        // The SDK's error text is dropped: it can quote server data.
        crate::media_fetch::with_cap(cap, self.0.media().get_media_content(request, true))
            .await
            .map_err(|_| ())
    }
}

/// Extra bytes allowed over the declared size, for senders and bridges that
/// count a little differently from the file the server stores.
const DECLARED_SIZE_SLACK: u64 = 1024 * 1024;

/// The most one download may read: [`MAX_THUMBNAIL_BYTES`] for a preview;
/// for the file, the sender's declared size (plus a little slack) when given
/// and at most [`MAX_FULL_BYTES`], else [`MAX_FULL_BYTES`]. The declared size
/// is only the sender's word, so it can lower the cap but never raise it.
fn read_cap(msgtype: &MessageType, size: MediaSize) -> u64 {
    match size {
        MediaSize::Thumbnail => MAX_THUMBNAIL_BYTES,
        MediaSize::Full => parts(msgtype)
            .and_then(|p| p.size)
            .map_or(MAX_FULL_BYTES, |declared| {
                declared
                    .saturating_add(DECLARED_SIZE_SLACK)
                    .min(MAX_FULL_BYTES)
            }),
    }
}

/// The request for a rendition, or the reason there is none.
fn request_for(
    msgtype: &MessageType,
    size: MediaSize,
    local_echo: bool,
) -> Result<MediaRequestParameters, CoreError> {
    let parts = parts(msgtype).ok_or(CoreError::NotAllowed)?;
    let (source, format) = match size {
        MediaSize::Full => {
            if !allowed_full(parts.size) {
                return Err(CoreError::Internal("the file is too large to fetch".into()));
            }
            (parts.source, MediaFormat::File)
        }
        MediaSize::Thumbnail => match thumbnail_plan(msgtype) {
            ThumbnailPlan::Source(source) => (
                source,
                MediaFormat::Thumbnail(MediaThumbnailSettings::new(uint!(800), uint!(600))),
            ),
            ThumbnailPlan::FullFile(source) => (source, MediaFormat::File),
            ThumbnailPlan::None => {
                return Err(CoreError::Internal("there is no preview".into()));
            }
        },
    };
    if !source_permitted(source, local_echo) {
        return Err(CoreError::NotAllowed);
    }
    Ok(MediaRequestParameters {
        source: source.clone(),
        format,
    })
}

/// Fetches one rendition: at most [`MAX_CONCURRENT_FETCHES`] at a time, each
/// within `deadline`. A thumbnail over [`MAX_THUMBNAIL_BYTES`] is refused.
async fn fetch_vec<F: ContentFetcher>(
    fetcher: &F,
    msgtype: &MessageType,
    size: MediaSize,
    local_echo: bool,
    permits: &Semaphore,
    deadline: Duration,
) -> Result<Vec<u8>, CoreError> {
    let request = request_for(msgtype, size, local_echo)?;
    let _permit = permits
        .acquire()
        .await
        .map_err(|_| CoreError::Internal("media fetching has stopped".into()))?;
    let unavailable = || CoreError::Network("the media could not be downloaded".into());
    let cap = read_cap(msgtype, size);
    let bytes = tokio::time::timeout(deadline, fetcher.fetch(&request, cap))
        .await
        .map_err(|_| unavailable())?
        .map_err(|()| unavailable())?;
    let too_big = match size {
        MediaSize::Thumbnail => u64::try_from(bytes.len()).is_ok_and(|n| n > MAX_THUMBNAIL_BYTES),
        MediaSize::Full => u64::try_from(bytes.len()).is_ok_and(|n| n > MAX_FULL_BYTES),
    };
    if too_big {
        return Err(CoreError::Internal("the media is too large".into()));
    }
    Ok(bytes)
}

/// Fetches the thumbnail or the file of a media message.
pub(crate) async fn fetch_media<F: ContentFetcher>(
    fetcher: &F,
    msgtype: &MessageType,
    size: MediaSize,
    local_echo: bool,
    permits: &Semaphore,
    deadline: Duration,
) -> Result<MediaBytes, CoreError> {
    let bytes = fetch_vec(fetcher, msgtype, size, local_echo, permits, deadline).await?;
    Ok(MediaBytes::from(bytes))
}

/// Fetches the file and writes it to `path` (see [`write_atomically`]).
pub(crate) async fn save_media<F: ContentFetcher>(
    fetcher: &F,
    msgtype: &MessageType,
    local_echo: bool,
    permits: &Semaphore,
    deadline: Duration,
    path: PathBuf,
) -> Result<(), CoreError> {
    let bytes = fetch_vec(
        fetcher,
        msgtype,
        MediaSize::Full,
        local_echo,
        permits,
        deadline,
    )
    .await?;
    // Blocking file work stays off the async threads.
    tokio::task::spawn_blocking(move || write_atomically(&path, &bytes))
        .await
        .map_err(|_| CoreError::Internal("background task failed".into()))?
        .map_err(|_| CoreError::Storage("could not write the file".into()))
}

/// Writes `bytes` to `<path>.tessera-part`, syncs it and renames it to
/// `path`. On any failure the part file is removed.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if path.file_name().is_none() {
        return Err(std::io::ErrorKind::InvalidInput.into());
    }
    let mut part = path.as_os_str().to_owned();
    part.push(PART_SUFFIX);
    let part = PathBuf::from(part);
    // A stale part file of an earlier attempt; removing a link removes the
    // link, never its target.
    let _ = std::fs::remove_file(&part);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&part, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&part);
        return written;
    }
    // Make the rename durable. Best effort: not every platform can open a
    // directory.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty())
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The media work of one session: the permits shared by all fetches and the
/// tasks that run them.
pub(crate) struct MediaTasks {
    pub(crate) permits: std::sync::Arc<Semaphore>,
    tasks: JoinSet<()>,
}

impl MediaTasks {
    pub(crate) fn new() -> Self {
        Self {
            permits: std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
            tasks: JoinSet::new(),
        }
    }

    /// Runs `work` in the background. `false` when too many are pending.
    pub(crate) fn spawn(&mut self, work: impl Future<Output = ()> + Send + 'static) -> bool {
        while self.tasks.try_join_next().is_some() {}
        if self.tasks.len() >= MAX_PENDING_FETCHES {
            return false;
        }
        self.tasks.spawn(work);
        true
    }

    /// Reports the server's upload limit once.
    pub(crate) fn send_limits(&mut self, client: &Client, events: &mpsc::Sender<CoreEvent>) {
        let client = client.clone();
        let events = events.clone();
        self.spawn(async move {
            let limit =
                tokio::time::timeout(LIMITS_DEADLINE, client.load_or_fetch_max_upload_size())
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .map(u64::from);
            // A dropped receiver just means nobody listens any more.
            let _ = events
                .send(CoreEvent::MediaLimits {
                    max_upload_bytes: limit,
                })
                .await;
        });
    }

    /// Ends every task; they hold clones of the client.
    pub(crate) async fn stop(&mut self) {
        self.tasks.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::{Value, json};

    use super::*;

    fn msg(value: Value) -> MessageType {
        serde_json::from_value(value).expect("a valid message type")
    }

    fn enc_file(url: &str) -> Value {
        json!({
            "url": url,
            "key": {
                "kty": "oct",
                "key_ops": ["encrypt", "decrypt"],
                "alg": "A256CTR",
                "k": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "ext": true
            },
            "iv": "AAAAAAAAAAAAAAAAAAAAAA",
            "hashes": {"sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"},
            "v": "v2"
        })
    }

    fn plain_image(info: Value) -> MessageType {
        msg(json!({"msgtype": "m.image", "body": "a.png", "url": "mxc://x/img", "info": info}))
    }

    fn encrypted_image(info: Value) -> MessageType {
        msg(json!({
            "msgtype": "m.image", "body": "a.png",
            "file": enc_file("mxc://x/enc"), "info": info
        }))
    }

    #[test]
    fn plain_image_maps_all_declared_fields() {
        let m = map_media(&plain_image(
            json!({"w": 640, "h": 480, "mimetype": "image/png", "size": 1234}),
        ))
        .expect("image is media");
        assert_eq!(m.kind, MediaKind::Image);
        assert_eq!(m.filename, "a.png");
        assert_eq!(m.mimetype.as_deref(), Some("image/png"));
        assert_eq!(
            (m.size, m.width, m.height),
            (Some(1234), Some(640), Some(480))
        );
        assert_eq!(m.duration_ms, None);
        assert!(m.has_thumbnail, "the server scales a plain image");
        assert!(m.upload.is_none());
    }

    #[test]
    fn caption_versus_file_name() {
        let captioned = msg(json!({
            "msgtype": "m.image", "body": "look at this", "filename": "cat.png",
            "url": "mxc://x/img"
        }));
        assert_eq!(map_media(&captioned).unwrap().filename, "cat.png");
        assert!(has_caption(&captioned));
        let same = msg(json!({
            "msgtype": "m.image", "body": "cat.png", "filename": "cat.png",
            "url": "mxc://x/img"
        }));
        assert_eq!(map_media(&same).unwrap().filename, "cat.png");
        assert!(!has_caption(&same));
        // Legacy: no `filename`, the body is the name.
        let legacy = plain_image(json!({}));
        assert_eq!(map_media(&legacy).unwrap().filename, "a.png");
        assert!(!has_caption(&legacy));
        assert!(!has_caption(&msg(
            json!({"msgtype": "m.text", "body": "hi"})
        )));
    }

    #[test]
    fn missing_info_leaves_everything_unknown() {
        let bare = msg(json!({"msgtype": "m.file", "body": "x.bin", "url": "mxc://x/f"}));
        let m = map_media(&bare).unwrap();
        assert_eq!(m.kind, MediaKind::File);
        assert_eq!(
            (m.mimetype, m.size, m.width, m.height, m.duration_ms),
            (None, None, None, None, None)
        );
        assert!(!m.has_thumbnail);
        let bare_image = msg(json!({"msgtype": "m.image", "body": "x.png", "url": "mxc://x/i"}));
        assert!(map_media(&bare_image).unwrap().has_thumbnail);
    }

    #[test]
    fn out_of_range_dimensions_are_dropped() {
        let m = map_media(&plain_image(json!({"w": 5_000_000_000u64, "h": 3}))).unwrap();
        assert_eq!((m.width, m.height), (None, Some(3)));
    }

    #[test]
    fn encrypted_image_thumbnail_rules() {
        // Small and no thumbnail: the file is its own preview.
        let small = encrypted_image(json!({"size": 5}));
        assert!(map_media(&small).unwrap().has_thumbnail);
        assert!(matches!(thumbnail_plan(&small), ThumbnailPlan::FullFile(_)));
        // Exactly at the limit still counts as small.
        let edge = encrypted_image(json!({"size": MAX_THUMBNAIL_BYTES}));
        assert!(matches!(thumbnail_plan(&edge), ThumbnailPlan::FullFile(_)));
        // Too big, or of unknown size: no preview.
        for info in [json!({"size": MAX_THUMBNAIL_BYTES + 1}), json!({})] {
            let big = encrypted_image(info);
            assert!(!map_media(&big).unwrap().has_thumbnail);
            assert!(matches!(thumbnail_plan(&big), ThumbnailPlan::None));
        }
        // A sender thumbnail wins at any size.
        let with_thumb = encrypted_image(json!({
            "size": 900_000_000u64, "thumbnail_file": enc_file("mxc://x/thumb")
        }));
        assert!(map_media(&with_thumb).unwrap().has_thumbnail);
        assert!(matches!(
            thumbnail_plan(&with_thumb),
            ThumbnailPlan::Source(_)
        ));
    }

    #[test]
    fn plain_thumbnail_source_is_preferred() {
        let m = plain_image(json!({"thumbnail_url": "mxc://x/thumb"}));
        let ThumbnailPlan::Source(MediaSource::Plain(uri)) = thumbnail_plan(&m) else {
            panic!("expected the sender's thumbnail");
        };
        assert_eq!(uri.as_str(), "mxc://x/thumb");
    }

    #[test]
    fn video_audio_and_file_thumbnails() {
        let video = msg(json!({
            "msgtype": "m.video", "body": "v.mp4", "url": "mxc://x/v",
            "info": {"w": 1920, "h": 1080, "duration": 61_500, "mimetype": "video/mp4",
                     "size": 99}
        }));
        let m = map_media(&video).unwrap();
        assert_eq!(m.kind, MediaKind::Video);
        assert_eq!(m.duration_ms, Some(61_500));
        assert_eq!((m.width, m.height), (Some(1920), Some(1080)));
        assert!(!m.has_thumbnail, "the server cannot thumbnail a video");
        let video_thumb = msg(json!({
            "msgtype": "m.video", "body": "v.mp4", "url": "mxc://x/v",
            "info": {"thumbnail_url": "mxc://x/vt"}
        }));
        assert!(map_media(&video_thumb).unwrap().has_thumbnail);
        let audio = msg(json!({
            "msgtype": "m.audio", "body": "s.ogg", "url": "mxc://x/a",
            "info": {"duration": 3000, "size": 7, "mimetype": "audio/ogg"}
        }));
        let m = map_media(&audio).unwrap();
        assert_eq!(m.kind, MediaKind::Audio);
        assert_eq!((m.duration_ms, m.size), (Some(3000), Some(7)));
        assert!(!m.has_thumbnail);
        assert!(matches!(thumbnail_plan(&audio), ThumbnailPlan::None));
        let file_thumb = msg(json!({
            "msgtype": "m.file", "body": "d.pdf", "url": "mxc://x/d",
            "info": {"thumbnail_file": enc_file("mxc://x/dt")}
        }));
        assert!(map_media(&file_thumb).unwrap().has_thumbnail);
    }

    #[test]
    fn other_message_types_are_not_media() {
        for value in [
            json!({"msgtype": "m.text", "body": "hi"}),
            json!({"msgtype": "m.notice", "body": "hi"}),
            json!({"msgtype": "m.emote", "body": "hi"}),
            json!({"msgtype": "m.location", "body": "here", "geo_uri": "geo:1,2"}),
            json!({"msgtype": "x.unknown", "body": "?"}),
        ] {
            let m = msg(value);
            assert!(map_media(&m).is_none());
            assert!(matches!(thumbnail_plan(&m), ThumbnailPlan::None));
        }
    }

    #[test]
    fn media_debug_hides_the_file_name() {
        let m = map_media(&msg(json!({
            "msgtype": "m.file", "body": "secret-name-x.png", "url": "mxc://x/f"
        })))
        .unwrap();
        assert!(!format!("{m:?}").contains("secret-name-x"));
    }

    #[test]
    fn full_size_limit_table() {
        assert!(allowed_full(None));
        assert!(allowed_full(Some(0)));
        assert!(allowed_full(Some(MAX_FULL_BYTES)));
        assert!(!allowed_full(Some(MAX_FULL_BYTES + 1)));
        assert!(!allowed_full(Some(u64::MAX)));
    }

    #[test]
    fn source_permission_table() {
        let plain = |u: &str| MediaSource::Plain(u.into());
        let enc = |u: &str| {
            let file: matrix_sdk::ruma::events::room::EncryptedFile =
                serde_json::from_value(enc_file(u)).unwrap();
            MediaSource::Encrypted(Box::new(file))
        };
        // (source, local echo, permitted)
        let table = [
            (plain("mxc://example.org/abc"), false, true),
            (plain("mxc://localhost/abc"), false, true),
            (plain("mxc://send-queue.localhost/txn"), false, false),
            (plain("mxc://SEND-QUEUE.LOCALHOST/txn"), false, false),
            (plain("mxc://send-queue.localhost/txn"), true, true),
            (enc("mxc://send-queue.localhost/txn"), false, false),
            (enc("mxc://example.org/abc"), false, true),
            (plain("https://example.org/abc"), true, false),
            (plain("mxc://example.org/"), true, false),
            (plain("not a uri"), true, false),
        ];
        for (source, local, expected) in table {
            assert_eq!(source_permitted(&source, local), expected, "{source:?}");
        }
    }

    /// Records requests and answers with fixed bytes.
    struct Fake {
        reply: Result<Vec<u8>, ()>,
        calls: Mutex<Vec<String>>,
        stall: bool,
    }

    impl Fake {
        fn ok(bytes: &[u8]) -> Self {
            Self {
                reply: Ok(bytes.to_vec()),
                calls: Mutex::new(Vec::new()),
                stall: false,
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ContentFetcher for Fake {
        async fn fetch(&self, request: &MediaRequestParameters, _cap: u64) -> Result<Vec<u8>, ()> {
            let what = match &request.format {
                MediaFormat::File => "file".to_owned(),
                MediaFormat::Thumbnail(s) => format!("thumb {}x{}", s.width, s.height),
            };
            self.calls.lock().unwrap().push(what);
            if self.stall {
                std::future::pending::<()>().await;
            }
            self.reply.clone()
        }
    }

    const SHORT: Duration = Duration::from_millis(50);

    async fn run(
        fake: &Fake,
        m: &MessageType,
        size: MediaSize,
        local: bool,
    ) -> Result<MediaBytes, CoreError> {
        fetch_media(fake, m, size, local, &Semaphore::new(4), SHORT).await
    }

    #[tokio::test]
    async fn fetch_uses_the_right_rendition() {
        let fake = Fake::ok(b"bytes");
        let m = plain_image(json!({"size": 10}));
        let thumb = run(&fake, &m, MediaSize::Thumbnail, false).await.unwrap();
        assert_eq!(thumb, MediaBytes::from(b"bytes".to_vec()));
        run(&fake, &m, MediaSize::Full, false).await.unwrap();
        assert_eq!(fake.calls(), ["thumb 800x600", "file"]);
        // An encrypted image without a thumbnail uses the whole file.
        let fake = Fake::ok(b"png");
        let enc = encrypted_image(json!({"size": 3}));
        run(&fake, &enc, MediaSize::Thumbnail, false).await.unwrap();
        assert_eq!(fake.calls(), ["file"]);
    }

    #[test]
    fn the_read_cap_follows_the_declared_size_but_never_exceeds_the_limits() {
        let file = |size: Value| {
            msg(json!({
                "msgtype": "m.file", "body": "b", "url": "mxc://x/f", "info": size
            }))
        };
        // A preview always gets the thumbnail limit.
        assert_eq!(
            read_cap(&file(json!({"size": 5})), MediaSize::Thumbnail),
            MAX_THUMBNAIL_BYTES
        );
        // The file: the declared size plus the slack, under the hard limit.
        assert_eq!(
            read_cap(&file(json!({"size": 5000})), MediaSize::Full),
            5000 + DECLARED_SIZE_SLACK
        );
        assert_eq!(
            read_cap(&file(json!({"size": MAX_FULL_BYTES})), MediaSize::Full),
            MAX_FULL_BYTES
        );
        // No size, or none that can be read: the hard limit, never more.
        assert_eq!(read_cap(&file(json!({})), MediaSize::Full), MAX_FULL_BYTES);
        assert_eq!(
            // The largest integer a Matrix event can carry (2^53 - 1).
            read_cap(
                &file(json!({"size": 9_007_199_254_740_991_u64})),
                MediaSize::Full
            ),
            MAX_FULL_BYTES
        );
    }

    /// Remembers the cap it was given.
    struct CapFake(Mutex<Option<u64>>);

    impl ContentFetcher for CapFake {
        async fn fetch(&self, _: &MediaRequestParameters, cap: u64) -> Result<Vec<u8>, ()> {
            *self.0.lock().unwrap() = Some(cap);
            Ok(vec![0; 3])
        }
    }

    #[tokio::test]
    async fn a_fetch_passes_the_cap_for_its_rendition_to_the_fetcher() {
        let fake = CapFake(Mutex::new(None));
        let file = msg(json!({
            "msgtype": "m.file", "body": "b", "url": "mxc://x/f", "info": {"size": 1000}
        }));
        fetch_media(
            &fake,
            &file,
            MediaSize::Full,
            false,
            &Semaphore::new(4),
            SHORT,
        )
        .await
        .expect("fetches");
        assert_eq!(*fake.0.lock().unwrap(), Some(1000 + DECLARED_SIZE_SLACK));
        let image = plain_image(json!({"size": 1000}));
        fetch_media(
            &fake,
            &image,
            MediaSize::Thumbnail,
            false,
            &Semaphore::new(4),
            SHORT,
        )
        .await
        .expect("fetches");
        assert_eq!(*fake.0.lock().unwrap(), Some(MAX_THUMBNAIL_BYTES));
    }

    #[tokio::test]
    async fn refused_fetches_never_reach_the_network() {
        let fake = Fake::ok(b"x");
        let huge = msg(json!({
            "msgtype": "m.file", "body": "b", "url": "mxc://x/f",
            "info": {"size": MAX_FULL_BYTES + 1}
        }));
        assert!(matches!(
            run(&fake, &huge, MediaSize::Full, false).await,
            Err(CoreError::Internal(_))
        ));
        let no_preview = encrypted_image(json!({}));
        assert!(
            run(&fake, &no_preview, MediaSize::Thumbnail, false)
                .await
                .is_err()
        );
        let forged = msg(json!({
            "msgtype": "m.image", "body": "b", "url": "mxc://send-queue.localhost/t"
        }));
        assert!(run(&fake, &forged, MediaSize::Full, false).await.is_err());
        let text = msg(json!({"msgtype": "m.text", "body": "hi"}));
        assert_eq!(
            run(&fake, &text, MediaSize::Full, false).await,
            Err(CoreError::NotAllowed)
        );
        assert!(fake.calls().is_empty());
        // The same forged source is fine for the user's own local echo.
        assert!(run(&fake, &forged, MediaSize::Full, true).await.is_ok());
    }

    #[tokio::test]
    async fn failures_empty_and_oversized_thumbnails_and_deadlines() {
        let m = plain_image(json!({}));
        let failing = Fake {
            reply: Err(()),
            ..Fake::ok(b"")
        };
        let err = run(&failing, &m, MediaSize::Full, false).await.unwrap_err();
        assert!(matches!(err, CoreError::Network(_)));
        let big = Fake::ok(&vec![0; MAX_THUMBNAIL_BYTES as usize + 1]);
        assert!(run(&big, &m, MediaSize::Thumbnail, false).await.is_err());
        assert!(run(&big, &m, MediaSize::Full, false).await.is_ok());
        let stalled = Fake {
            stall: true,
            ..Fake::ok(b"")
        };
        let err = run(&stalled, &m, MediaSize::Full, false).await.unwrap_err();
        assert!(matches!(err, CoreError::Network(_)));
    }

    #[tokio::test]
    async fn fetches_wait_for_a_permit() {
        let fake = Fake::ok(b"x");
        let m = plain_image(json!({}));
        let none = Semaphore::new(0);
        // Waiting for a permit is not bounded by the fetch deadline.
        let waited = tokio::time::timeout(
            SHORT,
            fetch_media(&fake, &m, MediaSize::Full, false, &none, SHORT),
        )
        .await;
        assert!(waited.is_err());
        assert!(fake.calls().is_empty());
    }

    fn part_of(path: &Path) -> PathBuf {
        let mut p = path.as_os_str().to_owned();
        p.push(PART_SUFFIX);
        PathBuf::from(p)
    }

    #[tokio::test]
    async fn save_writes_the_bytes_and_leaves_no_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.bin");
        let fake = Fake::ok(b"file contents");
        let m = msg(json!({"msgtype": "m.file", "body": "f", "url": "mxc://x/f"}));
        let permits = Semaphore::new(1);
        save_media(&fake, &m, false, &permits, SHORT, path.clone())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"file contents");
        assert!(!part_of(&path).exists());
        // An existing file is replaced; a stale part file does not matter.
        std::fs::write(part_of(&path), b"stale").unwrap();
        let fake = Fake::ok(b"new");
        save_media(&fake, &m, false, &permits, SHORT, path.clone())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!part_of(&path).exists());
    }

    #[tokio::test]
    async fn failed_saves_leave_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let m = msg(json!({"msgtype": "m.file", "body": "f", "url": "mxc://x/f"}));
        let permits = Semaphore::new(1);
        // The rename fails: the target is a directory.
        let target = dir.path().join("taken");
        std::fs::create_dir(&target).unwrap();
        let err = save_media(&Fake::ok(b"x"), &m, false, &permits, SHORT, target.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Storage(_)));
        assert!(!part_of(&target).exists());
        assert!(target.is_dir());
        // The directory does not exist.
        let missing = dir.path().join("no").join("such").join("f");
        assert!(
            save_media(&Fake::ok(b"x"), &m, false, &permits, SHORT, missing)
                .await
                .is_err()
        );
        // The fetch fails: nothing is written.
        let path = dir.path().join("never");
        let failing = Fake {
            reply: Err(()),
            ..Fake::ok(b"")
        };
        let err = save_media(&failing, &m, false, &permits, SHORT, path.clone())
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Network(_)));
        assert!(!path.exists() && !part_of(&path).exists());
        // A path with no file name.
        assert!(write_atomically(Path::new("/"), b"x").is_err());
        // Only the directory the test made is left.
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn deadlines_follow_the_plan() {
        assert_eq!(THUMBNAIL_DEADLINE, Duration::from_secs(30));
        assert_eq!(FULL_DEADLINE, Duration::from_secs(600));
        assert_eq!(MAX_CONCURRENT_FETCHES, 4);
        assert_eq!(MAX_THUMBNAIL_BYTES, 10 * 1024 * 1024);
        assert_eq!(MAX_FULL_BYTES, 512 * 1024 * 1024);
    }

    #[test]
    fn pending_fetch_cap() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut tasks = MediaTasks::new();
            for _ in 0..MAX_PENDING_FETCHES {
                assert!(tasks.spawn(std::future::pending()));
            }
            assert!(!tasks.spawn(std::future::pending()));
            tasks.stop().await;
        });
    }
}
