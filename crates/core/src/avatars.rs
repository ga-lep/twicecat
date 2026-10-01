//! On-demand avatar thumbnails through the SDK media API.
//!
//! An avatar ID is untrusted text (it comes from room state). It is parsed as
//! an MXC URI before anything else happens, and only the parsed value reaches
//! the SDK, which builds the request URL itself. The image bytes are never
//! logged.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use matrix_sdk::Client;
use matrix_sdk::media::{MediaFormat, MediaRequestParameters, MediaThumbnailSettings};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::ruma::{OwnedMxcUri, uint};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

use crate::api::{AvatarId, AvatarImage, CoreEvent};

/// Largest avatar the core hands out (thumbnail or original).
pub(crate) const MAX_AVATAR_BYTES: usize = 1024 * 1024;
/// Fetches running at the same time.
pub(crate) const MAX_CONCURRENT_FETCHES: usize = 4;
/// IDs looked at per `FetchAvatars` command; the rest are ignored, so a huge
/// list cannot stall the command loop.
pub(crate) const MAX_IDS_PER_COMMAND: usize = 256;
/// Longest accepted ID in bytes. Real MXC URIs are far shorter; the bound
/// also keeps the validator's length arithmetic in range.
pub(crate) const MAX_ID_LEN: usize = 255;

/// Parses an avatar ID as `mxc://server/media`; anything else is `None`.
pub(crate) fn parse_avatar_id(id: &AvatarId) -> Option<OwnedMxcUri> {
    let raw = id.as_str();
    if raw.len() > MAX_ID_LEN {
        return None;
    }
    let mxc = OwnedMxcUri::from(raw);
    // The validator accepts an empty media ID; a request for it is useless.
    match mxc.parts() {
        Ok((_, media)) if !media.is_empty() => Some(mxc),
        _ => None,
    }
}

/// Takes at most [`MAX_IDS_PER_COMMAND`] IDs and drops repeats, keeping order.
pub(crate) fn bounded_unique(ids: Vec<AvatarId>) -> Vec<AvatarId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        if out.len() == MAX_IDS_PER_COMMAND {
            break;
        }
        if !seen.contains(&id) {
            seen.insert(id.clone());
            out.push(id);
        }
    }
    out
}

/// Which rendition to request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// A 96x96 thumbnail.
    Thumbnail,
    /// The uploaded file.
    File,
}

/// Where image bytes come from. Production uses the SDK media API; tests
/// use a fake.
pub(crate) trait Fetcher: Send + Sync + 'static {
    /// Fetches one rendition of an already validated MXC URI.
    fn fetch(
        &self,
        mxc: &OwnedMxcUri,
        format: Format,
    ) -> impl Future<Output = Result<Vec<u8>, ()>> + Send;
}

/// Fetches through `Client::media().get_media_content` with the cache on.
pub(crate) struct SdkFetcher(pub(crate) Client);

impl Fetcher for SdkFetcher {
    async fn fetch(&self, mxc: &OwnedMxcUri, format: Format) -> Result<Vec<u8>, ()> {
        let format = match format {
            Format::Thumbnail => {
                MediaFormat::Thumbnail(MediaThumbnailSettings::new(uint!(96), uint!(96)))
            }
            Format::File => MediaFormat::File,
        };
        let request = MediaRequestParameters {
            source: MediaSource::Plain(mxc.clone()),
            format,
        };
        // The SDK builds the request URL from the parsed URI. Its error text
        // is dropped: it can quote server data.
        self.0
            .media()
            .get_media_content(&request, true)
            .await
            .map_err(|_| ())
    }
}

/// Thumbnail first, the file only if the thumbnail request fails; anything
/// empty or over [`MAX_AVATAR_BYTES`] is unavailable.
pub(crate) async fn fetch_capped<F: Fetcher>(f: &F, mxc: &OwnedMxcUri) -> Option<AvatarImage> {
    let bytes = match f.fetch(mxc, Format::Thumbnail).await {
        Ok(bytes) => bytes,
        Err(()) => f.fetch(mxc, Format::File).await.ok()?,
    };
    (!bytes.is_empty() && bytes.len() <= MAX_AVATAR_BYTES).then(|| AvatarImage::from(bytes))
}

/// Answers `ids` with "unavailable" (no session to fetch with).
pub(crate) async fn answer_unavailable(ids: Vec<AvatarId>, events: &mpsc::Sender<CoreEvent>) {
    for id in bounded_unique(ids) {
        // A dropped receiver just means nobody listens any more.
        let _ = events.send(CoreEvent::Avatar { id, image: None }).await;
    }
}

/// In-flight and answered avatar requests of one session.
pub(crate) struct Avatars {
    seen: HashSet<AvatarId>,
    tasks: JoinSet<()>,
    permits: Arc<Semaphore>,
}

impl Avatars {
    pub(crate) fn new() -> Self {
        Self {
            seen: HashSet::new(),
            tasks: JoinSet::new(),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
        }
    }

    /// Starts answering `ids`. Never waits: every answer is sent from a task.
    ///
    /// IDs already in flight or answered get no second answer. At most
    /// [`MAX_IDS_PER_COMMAND`] IDs are looked at.
    pub(crate) fn request<F: Fetcher>(
        &mut self,
        ids: Vec<AvatarId>,
        fetcher: &Arc<F>,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        // Reap finished tasks so the set does not grow without bound.
        while self.tasks.try_join_next().is_some() {}
        for id in bounded_unique(ids) {
            if !self.seen.insert(id.clone()) {
                continue;
            }
            let events = events.clone();
            let Some(mxc) = parse_avatar_id(&id) else {
                self.tasks.spawn(async move {
                    let _ = events.send(CoreEvent::Avatar { id, image: None }).await;
                });
                continue;
            };
            let fetcher = Arc::clone(fetcher);
            let permits = Arc::clone(&self.permits);
            self.tasks.spawn(async move {
                // The permit covers only the fetch, not the send below.
                let image = match permits.acquire_owned().await {
                    Ok(_permit) => fetch_capped(&*fetcher, &mxc).await,
                    Err(_) => None,
                };
                let _ = events.send(CoreEvent::Avatar { id, image }).await;
            });
        }
    }

    /// Aborts every task and forgets all answers (logout, shutdown).
    pub(crate) async fn stop(&mut self) {
        self.tasks.abort_all();
        // Aborted tasks end with a cancellation error; that is expected.
        while self.tasks.join_next().await.is_some() {}
        self.seen.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    const GOOD: &str = "mxc://example.org/AbC-123_x";

    #[test]
    fn parse_accepts_only_mxc_server_media() {
        for ok in [GOOD, "mxc://127.0.0.1:8008/abc", "mxc://[::1]/abc"] {
            assert!(parse_avatar_id(&AvatarId::from(ok)).is_some(), "{ok}");
        }
        let long_media = format!("mxc://example.org/{}", "a".repeat(300));
        let long_server = format!("mxc://{}/abc", "a".repeat(250));
        for bad in [
            "",
            "not-mxc",
            "mxc://",
            "mxc://example.org",
            "mxc://example.org/",
            "mxc:///abc",
            "mxc://example.org/a/b",
            "mxc://example.org/a?x=1",
            "mxc://example.org/a#f",
            "mxc://example.org/..",
            "mxc://user@example.org/abc",
            "mxc://exa mple.org/abc",
            "mxc://example.org/abc\n",
            " mxc://example.org/abc",
            "MXC://example.org/abc",
            "https://example.org/_matrix/media/v3/download/example.org/abc",
            "http://169.254.169.254/latest",
            "file:///etc/passwd",
            long_media.as_str(),
            long_server.as_str(),
        ] {
            assert!(parse_avatar_id(&AvatarId::from(bad)).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn bounded_unique_dedupes_keeps_order_and_caps() {
        let ids: Vec<AvatarId> = ["a", "b", "a", "c", "b"]
            .into_iter()
            .map(AvatarId::from)
            .collect();
        let got: Vec<String> = bounded_unique(ids)
            .iter()
            .map(|i| i.as_str().to_owned())
            .collect();
        assert_eq!(got, ["a", "b", "c"]);

        let many: Vec<AvatarId> = (0..100_000)
            .map(|i| AvatarId::from(format!("mxc://x/{i}")))
            .collect();
        assert_eq!(bounded_unique(many).len(), MAX_IDS_PER_COMMAND);
        // Repeats do not use up the bound.
        let repeated: Vec<AvatarId> = (0..100_000).map(|_| AvatarId::from("mxc://x/a")).collect();
        assert_eq!(bounded_unique(repeated).len(), 1);
    }

    /// A fetcher that records calls and answers from a script.
    struct Fake {
        calls: Mutex<Vec<(String, Format)>>,
        current: AtomicUsize,
        max: AtomicUsize,
        delay: Duration,
        thumb: Result<usize, ()>,
        file: Result<usize, ()>,
    }

    impl Fake {
        fn new(thumb: Result<usize, ()>, file: Result<usize, ()>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                current: AtomicUsize::new(0),
                max: AtomicUsize::new(0),
                delay: Duration::from_millis(0),
                thumb,
                file,
            })
        }
        fn calls(&self) -> Vec<(String, Format)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetcher for Fake {
        async fn fetch(&self, mxc: &OwnedMxcUri, format: Format) -> Result<Vec<u8>, ()> {
            self.calls.lock().unwrap().push((mxc.to_string(), format));
            let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.current.fetch_sub(1, Ordering::SeqCst);
            let n = match format {
                Format::Thumbnail => self.thumb,
                Format::File => self.file,
            }?;
            Ok(vec![7; n])
        }
    }

    async fn drain(
        rx: &mut mpsc::Receiver<CoreEvent>,
        n: usize,
    ) -> Vec<(AvatarId, Option<AvatarImage>)> {
        let mut out = Vec::new();
        for _ in 0..n {
            let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("timed out")
                .expect("closed");
            match ev {
                CoreEvent::Avatar { id, image } => out.push((id, image)),
                other => panic!("unexpected {other:?}"),
            }
        }
        out
    }

    async fn quiet(rx: &mut mpsc::Receiver<CoreEvent>) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(rx.try_recv().is_err(), "unexpected extra event");
    }

    #[tokio::test]
    async fn answers_each_id_once_and_dedupes_later_requests() {
        let fake = Fake::new(Ok(10), Err(()));
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::new();
        let a = AvatarId::from("mxc://x/a");
        let b = AvatarId::from("mxc://x/b");
        avatars.request(vec![a.clone(), b.clone(), a.clone()], &fake, &tx);
        let mut got = drain(&mut rx, 2).await;
        got.sort_by(|l, r| l.0.cmp(&r.0));
        assert_eq!(got[0].0, a);
        assert_eq!(got[1].0, b);
        assert!(
            got.iter()
                .all(|(_, i)| i.as_ref().unwrap().as_bytes().len() == 10)
        );
        quiet(&mut rx).await;
        // Already answered: no event, no fetch.
        avatars.request(vec![a.clone()], &fake, &tx);
        quiet(&mut rx).await;
        assert_eq!(fake.calls().len(), 2);
        // Logout forgets the answers.
        avatars.stop().await;
        avatars.request(vec![a.clone()], &fake, &tx);
        drain(&mut rx, 1).await;
        assert_eq!(fake.calls().len(), 3);
    }

    #[tokio::test]
    async fn in_flight_ids_are_not_fetched_twice() {
        let mut fake = Fake::new(Ok(10), Err(()));
        Arc::get_mut(&mut fake).unwrap().delay = Duration::from_millis(100);
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::new();
        let a = AvatarId::from("mxc://x/a");
        avatars.request(vec![a.clone()], &fake, &tx);
        avatars.request(vec![a.clone()], &fake, &tx);
        drain(&mut rx, 1).await;
        quiet(&mut rx).await;
        assert_eq!(fake.calls().len(), 1);
    }

    #[tokio::test]
    async fn invalid_ids_answer_none_without_a_request() {
        let fake = Fake::new(Ok(10), Ok(10));
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::new();
        let ids: Vec<AvatarId> = ["not-mxc", "https://evil.example/x", "mxc://x/a/../b"]
            .into_iter()
            .map(AvatarId::from)
            .collect();
        avatars.request(ids.clone(), &fake, &tx);
        let got = drain(&mut rx, 3).await;
        assert!(got.iter().all(|(_, i)| i.is_none()));
        let mut answered: Vec<_> = got.into_iter().map(|(i, _)| i).collect();
        answered.sort();
        let mut want = ids;
        want.sort();
        assert_eq!(answered, want);
        assert!(fake.calls().is_empty(), "no request for an invalid id");
    }

    #[tokio::test]
    async fn at_most_four_fetches_at_once() {
        let mut fake = Fake::new(Ok(10), Err(()));
        Arc::get_mut(&mut fake).unwrap().delay = Duration::from_millis(40);
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::new();
        let ids: Vec<AvatarId> = (0..12)
            .map(|i| AvatarId::from(format!("mxc://x/{i}")))
            .collect();
        avatars.request(ids, &fake, &tx);
        drain(&mut rx, 12).await;
        assert_eq!(fake.max.load(Ordering::SeqCst), MAX_CONCURRENT_FETCHES);
    }

    #[tokio::test]
    async fn huge_id_list_is_bounded() {
        let fake = Fake::new(Ok(1), Err(()));
        let (tx, mut rx) = mpsc::channel(1024);
        let mut avatars = Avatars::new();
        let ids: Vec<AvatarId> = (0..50_000)
            .map(|i| AvatarId::from(format!("mxc://x/{i}")))
            .collect();
        avatars.request(ids, &fake, &tx);
        drain(&mut rx, MAX_IDS_PER_COMMAND).await;
        quiet(&mut rx).await;
        assert_eq!(fake.calls().len(), MAX_IDS_PER_COMMAND);
    }

    #[tokio::test]
    async fn thumbnail_error_falls_back_to_the_file() {
        let fake = Fake::new(Err(()), Ok(20));
        let mxc = OwnedMxcUri::from("mxc://x/a");
        let img = fetch_capped(&*fake, &mxc).await.unwrap();
        assert_eq!(img.as_bytes().len(), 20);
        assert_eq!(
            fake.calls().iter().map(|c| c.1).collect::<Vec<_>>(),
            [Format::Thumbnail, Format::File]
        );
    }

    #[tokio::test]
    async fn both_failing_is_unavailable() {
        let fake = Fake::new(Err(()), Err(()));
        assert!(
            fetch_capped(&*fake, &OwnedMxcUri::from("mxc://x/a"))
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn size_cap_is_one_mib_inclusive() {
        let mxc = OwnedMxcUri::from("mxc://x/a");
        let at = Fake::new(Ok(MAX_AVATAR_BYTES), Err(()));
        assert!(fetch_capped(&*at, &mxc).await.is_some());
        // An oversized thumbnail is refused and the (bigger) file is not tried.
        let over = Fake::new(Ok(MAX_AVATAR_BYTES + 1), Ok(10));
        assert!(fetch_capped(&*over, &mxc).await.is_none());
        assert_eq!(over.calls().len(), 1);
        // An oversized fallback file is refused too.
        let big_file = Fake::new(Err(()), Ok(MAX_AVATAR_BYTES + 1));
        assert!(fetch_capped(&*big_file, &mxc).await.is_none());
        // Empty is unavailable.
        let empty = Fake::new(Ok(0), Err(()));
        assert!(fetch_capped(&*empty, &mxc).await.is_none());
    }

    #[tokio::test]
    async fn answer_unavailable_is_bounded_and_unique() {
        let (tx, mut rx) = mpsc::channel(1024);
        let ids: Vec<AvatarId> = (0..10_000)
            .map(|i| AvatarId::from(format!("mxc://x/{}", i % 1000)))
            .collect();
        answer_unavailable(ids, &tx).await;
        let got = drain(&mut rx, MAX_IDS_PER_COMMAND).await;
        assert!(got.iter().all(|(_, i)| i.is_none()));
        quiet(&mut rx).await;
    }
}
