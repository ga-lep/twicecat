//! Sending files as media messages: validation, the attachment description
//! for the SDK, and an in-order upload queue.
//!
//! File names and bytes are untrusted user content: nothing here logs them.

use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::attachment::{
    AttachmentInfo, BaseAudioInfo, BaseFileInfo, BaseImageInfo, BaseVideoInfo, Thumbnail,
};
use matrix_sdk::ruma::{OwnedEventId, UInt};
use matrix_sdk_ui::Timeline;
use matrix_sdk_ui::timeline::{AttachmentConfig, AttachmentSource};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::{
    AttachmentData, CoreError, CoreEvent, MediaKind, OutgoingAttachment, RoomId, UploadThumbnail,
};
use crate::media::MAX_THUMBNAIL_BYTES;

/// The upload limit assumed when the server does not say.
pub(crate) const DEFAULT_MAX_UPLOAD_BYTES: u64 = 100 * 1024 * 1024;
/// Attachments waiting behind the running one; more are answered `Busy`.
const MAX_QUEUED_UPLOADS: usize = 16;
/// Deadline of the server limits request.
const LIMITS_DEADLINE: Duration = Duration::from_secs(15);

/// Parses a MIME type; a malformed one is refused.
pub(crate) fn parse_mime(s: &str) -> Result<mime::Mime, CoreError> {
    let mime: mime::Mime = s.parse().map_err(|_| CoreError::NotAllowed)?;
    // `mime` accepts "image/" and wildcards; neither names a file type.
    let concrete = |name: mime::Name<'_>| !name.as_str().is_empty() && name != mime::STAR;
    if concrete(mime.type_()) && concrete(mime.subtype()) {
        Ok(mime)
    } else {
        Err(CoreError::NotAllowed)
    }
}

/// Refuses a size over the server's limit (or the default when unknown).
pub(crate) fn check_size(len: u64, limit: Option<u64>) -> Result<(), CoreError> {
    if len > limit.unwrap_or(DEFAULT_MAX_UPLOAD_BYTES) {
        Err(CoreError::NotAllowed)
    } else {
        Ok(())
    }
}

fn thumbnail(t: &UploadThumbnail) -> Result<Thumbnail, CoreError> {
    let len = t.data.len() as u64;
    if len > MAX_THUMBNAIL_BYTES {
        return Err(CoreError::NotAllowed);
    }
    Ok(Thumbnail {
        data: t.data.as_bytes().to_vec(),
        content_type: parse_mime(&t.mimetype)?,
        height: UInt::from(t.height),
        width: UInt::from(t.width),
        size: UInt::try_from(len).map_err(|_| CoreError::NotAllowed)?,
    })
}

/// The SDK's description of an outgoing attachment of `size` bytes.
///
/// Refuses what the core cannot send: an empty file name, a bad thumbnail
/// type or an oversized thumbnail, and a malformed reply target. A thread
/// root is not part of the config: the thread timeline the job is queued on
/// adds the relation.
pub(crate) fn attachment_config(
    a: &OutgoingAttachment,
    size: u64,
) -> Result<AttachmentConfig, CoreError> {
    if a.filename.trim().is_empty() {
        return Err(CoreError::NotAllowed);
    }
    let size = Some(UInt::try_from(size).map_err(|_| CoreError::NotAllowed)?);
    let (width, height) = (a.width.map(UInt::from), a.height.map(UInt::from));
    let info = match a.kind {
        MediaKind::Image => AttachmentInfo::Image(BaseImageInfo {
            width,
            height,
            size,
            ..BaseImageInfo::default()
        }),
        MediaKind::Video => AttachmentInfo::Video(BaseVideoInfo {
            width,
            height,
            size,
            ..BaseVideoInfo::default()
        }),
        MediaKind::Audio => AttachmentInfo::Audio(BaseAudioInfo {
            size,
            ..BaseAudioInfo::default()
        }),
        MediaKind::File => AttachmentInfo::File(BaseFileInfo { size }),
    };
    // Only pictures carry the caller's preview.
    let thumbnail = match (&a.thumbnail, a.kind) {
        (Some(t), MediaKind::Image) => Some(thumbnail(t)?),
        _ => None,
    };
    let in_reply_to = match &a.reply_to {
        Some(id) => Some(OwnedEventId::try_from(id.as_str()).map_err(|_| CoreError::NotAllowed)?),
        None => None,
    };
    Ok(AttachmentConfig {
        info: Some(info),
        thumbnail,
        in_reply_to,
        ..AttachmentConfig::default()
    })
}

/// Checks everything that can be checked without the bytes.
pub(crate) fn validate(a: &OutgoingAttachment) -> Result<mime::Mime, CoreError> {
    let mime = parse_mime(&a.mimetype)?;
    attachment_config(a, 0)?;
    Ok(mime)
}

fn read_err(_: std::io::Error) -> CoreError {
    CoreError::Storage("could not read the file".into())
}

/// One attachment to queue.
pub(crate) struct Job {
    pub(crate) timeline: Arc<Timeline>,
    pub(crate) attachment: OutgoingAttachment,
    pub(crate) mime: mime::Mime,
}

/// Reads the bytes (refusing an oversized file before reading it), then hands
/// the attachment to the send queue. Returns once it is queued: the upload and
/// its progress continue in the SDK.
async fn run_job(job: Job, client: &Client, limit_override: Option<u64>) -> Result<(), CoreError> {
    let Job {
        timeline,
        attachment: a,
        mime,
    } = job;
    let limit = match limit_override {
        Some(limit) => Some(limit),
        None => tokio::time::timeout(LIMITS_DEADLINE, client.load_or_fetch_max_upload_size())
            .await
            .ok()
            .and_then(Result::ok)
            .map(u64::from),
    };
    let bytes = match &a.data {
        AttachmentData::Path(path) => {
            let meta = tokio::fs::metadata(path).await.map_err(read_err)?;
            if !meta.is_file() {
                return Err(CoreError::NotAllowed);
            }
            check_size(meta.len(), limit)?;
            tokio::fs::read(path).await.map_err(read_err)?
        }
        AttachmentData::Bytes(bytes) => bytes.as_bytes().to_vec(),
    };
    let len = bytes.len() as u64;
    check_size(len, limit)?;
    let config = attachment_config(&a, len)?;
    timeline
        .send_attachment(
            AttachmentSource::Data {
                bytes,
                filename: a.filename,
            },
            mime,
            config,
        )
        .use_send_queue()
        .await
        .map_err(|_| CoreError::Internal("the file could not be queued".into()))
}

/// Queues attachments one at a time, in the order they were submitted, so
/// that several files appear in the timeline in order.
pub(crate) struct UploadQueue {
    jobs: mpsc::Sender<Job>,
    task: JoinHandle<()>,
}

impl UploadQueue {
    /// Starts the worker. `limit_override` replaces the server's limit.
    pub(crate) fn start(
        client: Client,
        events: mpsc::Sender<CoreEvent>,
        limit_override: Option<u64>,
    ) -> Self {
        let (jobs, mut rx) = mpsc::channel::<Job>(MAX_QUEUED_UPLOADS);
        let task = tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let room_id: RoomId = job.attachment.room_id.clone();
                if let Err(error) = run_job(job, &client, limit_override).await
                    && events
                        .send(CoreEvent::ActionFailed {
                            room_id,
                            action: crate::api::ActionKind::Upload,
                            error,
                        })
                        .await
                        .is_err()
                {
                    return;
                }
            }
        });
        Self { jobs, task }
    }

    /// Queues `job`; `Busy` when too many wait already.
    pub(crate) fn submit(&self, job: Job) -> Result<(), CoreError> {
        self.jobs.try_send(job).map_err(|_| CoreError::Busy)
    }

    /// Ends the worker; it holds a clone of the client.
    pub(crate) async fn stop(self) {
        self.task.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{AttachmentData, EventId, MediaBytes, MediaKind, RoomId, UploadThumbnail};
    use matrix_sdk::attachment::AttachmentInfo as Info;

    fn attachment(kind: MediaKind) -> OutgoingAttachment {
        OutgoingAttachment {
            room_id: RoomId::from("!r:x"),
            filename: "a.png".into(),
            mimetype: "image/png".into(),
            data: AttachmentData::Bytes(MediaBytes::from(vec![1, 2, 3])),
            kind,
            width: Some(1200),
            height: Some(900),
            thumbnail: None,
            reply_to: None,
            thread_root: None,
        }
    }

    fn thumb() -> UploadThumbnail {
        UploadThumbnail {
            data: MediaBytes::from(vec![9; 10]),
            mimetype: "image/jpeg".into(),
            width: 64,
            height: 48,
        }
    }

    #[test]
    fn mime_parsing() {
        assert_eq!(parse_mime("image/png").unwrap().essence_str(), "image/png");
        for bad in [
            "",
            "png",
            "image/",
            "a b/c",
            "image/png;;;=",
            "*/*",
            "image/*",
        ] {
            assert_eq!(parse_mime(bad), Err(CoreError::NotAllowed), "{bad:?}");
        }
    }

    #[test]
    fn size_check() {
        assert_eq!(check_size(10, Some(10)), Ok(()));
        assert_eq!(check_size(11, Some(10)), Err(CoreError::NotAllowed));
        assert_eq!(check_size(DEFAULT_MAX_UPLOAD_BYTES, None), Ok(()));
        assert_eq!(
            check_size(DEFAULT_MAX_UPLOAD_BYTES + 1, None),
            Err(CoreError::NotAllowed)
        );
    }

    #[test]
    fn image_config_carries_dimensions_size_and_thumbnail() {
        let mut a = attachment(MediaKind::Image);
        a.thumbnail = Some(thumb());
        let config = attachment_config(&a, 3).unwrap();
        let Some(Info::Image(info)) = config.info else {
            panic!("image info");
        };
        assert_eq!(info.width.map(u64::from), Some(1200));
        assert_eq!(info.height.map(u64::from), Some(900));
        assert_eq!(info.size.map(u64::from), Some(3));
        let t: Thumbnail = config.thumbnail.expect("a thumbnail");
        assert_eq!(t.data, vec![9; 10]);
        assert_eq!(t.content_type.essence_str(), "image/jpeg");
        assert_eq!((u64::from(t.width), u64::from(t.height)), (64, 48));
        assert_eq!(u64::from(t.size), 10);
        assert!(config.in_reply_to.is_none());
    }

    #[test]
    fn other_kinds_get_matching_info_and_no_thumbnail() {
        for kind in [MediaKind::Video, MediaKind::Audio, MediaKind::File] {
            let mut a = attachment(kind);
            a.thumbnail = Some(thumb());
            let config = attachment_config(&a, 7).unwrap();
            assert!(config.thumbnail.is_none(), "{kind:?}");
            let ok = match (kind, config.info) {
                (MediaKind::Video, Some(Info::Video(i))) => i.size.map(u64::from) == Some(7),
                (MediaKind::Audio, Some(Info::Audio(i))) => i.size.map(u64::from) == Some(7),
                (MediaKind::File, Some(Info::File(i))) => i.size.map(u64::from) == Some(7),
                _ => false,
            };
            assert!(ok, "{kind:?}");
        }
    }

    #[test]
    fn reply_target_is_parsed() {
        let mut a = attachment(MediaKind::File);
        a.reply_to = Some(EventId::from("$e:x"));
        let config = attachment_config(&a, 1).unwrap();
        assert_eq!(
            config.in_reply_to.as_deref().map(|e| e.as_str()),
            Some("$e:x")
        );
        a.reply_to = Some(EventId::from("not an event id"));
        assert_eq!(attachment_config(&a, 1).unwrap_err(), CoreError::NotAllowed);
    }

    #[test]
    fn bad_thumbnails_are_refused() {
        let mut a = attachment(MediaKind::Image);
        let mut t = thumb();
        t.mimetype = "nope".into();
        a.thumbnail = Some(t);
        assert_eq!(attachment_config(&a, 1).unwrap_err(), CoreError::NotAllowed);
        let mut a = attachment(MediaKind::Image);
        a.thumbnail = Some(UploadThumbnail {
            data: MediaBytes::from(vec![0; 10 * 1024 * 1024 + 1]),
            ..thumb()
        });
        assert_eq!(attachment_config(&a, 1).unwrap_err(), CoreError::NotAllowed);
    }

    #[test]
    fn a_thread_attachment_is_valid_the_engine_picks_the_thread_timeline() {
        let mut a = attachment(MediaKind::File);
        a.thread_root = Some(EventId::from("$root:x"));
        assert!(attachment_config(&a, 1).is_ok());
        assert!(validate(&a).is_ok());
    }
}
