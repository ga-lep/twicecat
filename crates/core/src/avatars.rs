//! On-demand avatar thumbnails through the SDK media API.
//!
//! An avatar ID is untrusted text (it comes from room state). It is parsed as
//! an MXC URI before anything else happens, and only the parsed value reaches
//! the SDK, which builds the request URL itself. The image bytes are never
//! logged.

use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

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
/// Client-side deadline for one fetch. The SDK's media download has no timeout
/// of its own (matrix-sdk 0.19.1 media.rs:818 sets `Duration::MAX`).
pub(crate) const FETCH_DEADLINE: Duration = Duration::from_secs(20);
/// Most IDs remembered as in flight or answered; the set is cleared when full.
pub(crate) const MAX_SEEN: usize = 4096;
/// Longest accepted ID in bytes. Real MXC URIs are far shorter; the bound
/// also keeps the validator's length arithmetic in range.
pub(crate) const MAX_ID_LEN: usize = 255;

/// Parses an avatar ID as `mxc://server/media`; anything else is `None`.
///
/// Server names `localhost`-suffixed (`*.localhost`, any case) are refused: the
/// SDK serves `mxc://send-queue.localhost/<txn>` from the local media store
/// (matrix-sdk 0.19.1 media.rs:63, `is_local_uri` :792), i.e. the user's own
/// pending uploads. The bare name `localhost` (a local homeserver) stays valid.
pub(crate) fn parse_avatar_id(id: &AvatarId) -> Option<OwnedMxcUri> {
    let raw = id.as_str();
    if raw.len() > MAX_ID_LEN {
        return None;
    }
    let mxc = OwnedMxcUri::from(raw);
    // The validator accepts an empty media ID; a request for it is useless.
    match mxc.parts() {
        Ok((server, media)) if !media.is_empty() => {
            let host = server.host().to_ascii_lowercase();
            (!host.ends_with(".localhost")).then_some(mxc)
        }
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

/// Where image bytes come from. Production uses the SDK media API; tests
/// use a fake.
pub(crate) trait Fetcher: Send + Sync + 'static {
    /// Fetches the 96x96 thumbnail of an already validated MXC URI.
    fn fetch(&self, mxc: &OwnedMxcUri) -> impl Future<Output = Result<Vec<u8>, ()>> + Send;
}

/// Fetches through `Client::media().get_media_content` with the cache on.
pub(crate) struct SdkFetcher(pub(crate) Client);

impl Fetcher for SdkFetcher {
    async fn fetch(&self, mxc: &OwnedMxcUri) -> Result<Vec<u8>, ()> {
        // Thumbnails only: the server bounds their size, the original file
        // would be bounded by nothing.
        let request = MediaRequestParameters {
            source: MediaSource::Plain(mxc.clone()),
            format: MediaFormat::Thumbnail(MediaThumbnailSettings::new(uint!(96), uint!(96))),
        };
        // The SDK builds the request URL from the parsed URI. Its error text
        // is dropped: it can quote server data.
        // A server that sends more than an avatar can be is not read to the
        // end: the capped fetcher of the client stops at the limit.
        crate::media_fetch::with_cap(
            u64::try_from(MAX_AVATAR_BYTES).unwrap_or(u64::MAX),
            self.0.media().get_media_content(&request, true),
        )
        .await
        .map_err(|_| ())
    }
}

/// The thumbnail, within `deadline`; anything empty, over [`MAX_AVATAR_BYTES`],
/// failed or too slow is unavailable. There is no fallback to the original
/// file.
pub(crate) async fn fetch_capped<F: Fetcher>(
    f: &F,
    mxc: &OwnedMxcUri,
    deadline: Duration,
) -> Option<AvatarImage> {
    let bytes = tokio::time::timeout(deadline, f.fetch(mxc))
        .await
        .ok()?
        .ok()?;
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
    /// In flight or answered with an image. Shared with the tasks, which
    /// remove their ID when the fetch fails so that it can be asked again.
    seen: Arc<Mutex<HashSet<AvatarId>>>,
    deadline: Duration,
    tasks: JoinSet<()>,
    permits: Arc<Semaphore>,
}

impl Avatars {
    pub(crate) fn new() -> Self {
        Self::with_deadline(FETCH_DEADLINE)
    }

    pub(crate) fn with_deadline(deadline: Duration) -> Self {
        Self {
            seen: Arc::default(),
            deadline,
            tasks: JoinSet::new(),
            permits: Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES)),
        }
    }

    /// Starts answering `ids`. Never waits: every answer is sent from a task.
    ///
    /// IDs in flight or answered with an image get no second answer; an ID
    /// whose fetch failed or timed out may be asked for again. At most
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
            {
                let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
                if seen.len() >= MAX_SEEN {
                    seen.clear();
                }
                if !seen.insert(id.clone()) {
                    continue;
                }
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
            let seen = Arc::clone(&self.seen);
            let deadline = self.deadline;
            self.tasks.spawn(async move {
                // The permit covers only the fetch, not the send below.
                let image = match permits.acquire_owned().await {
                    Ok(_permit) => fetch_capped(&*fetcher, &mxc, deadline).await,
                    Err(_) => None,
                };
                if image.is_none() {
                    // Forget the failure before announcing it, so a request
                    // sent on seeing the answer is not deduplicated.
                    seen.lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(&id);
                }
                let _ = events.send(CoreEvent::Avatar { id, image }).await;
            });
        }
    }

    /// Aborts every task and forgets all answers (logout, shutdown).
    pub(crate) async fn stop(&mut self) {
        self.tasks.abort_all();
        // Aborted tasks end with a cancellation error; that is expected.
        while self.tasks.join_next().await.is_some() {}
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    #[cfg(test)]
    fn seen_len(&self) -> usize {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
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
        calls: Mutex<Vec<String>>,
        current: AtomicUsize,
        max: AtomicUsize,
        delay: Duration,
        thumb: Result<usize, ()>,
        /// Never completes (a server that drips or stalls).
        hang: bool,
    }

    impl Fake {
        fn new(thumb: Result<usize, ()>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                current: AtomicUsize::new(0),
                max: AtomicUsize::new(0),
                delay: Duration::from_millis(0),
                thumb,
                hang: false,
            })
        }
        fn hanging() -> Arc<Self> {
            let mut f = Self::new(Ok(1));
            Arc::get_mut(&mut f).unwrap().hang = true;
            f
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetcher for Fake {
        async fn fetch(&self, mxc: &OwnedMxcUri) -> Result<Vec<u8>, ()> {
            self.calls.lock().unwrap().push(mxc.to_string());
            if self.hang {
                std::future::pending::<()>().await;
            }
            let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.current.fetch_sub(1, Ordering::SeqCst);
            Ok(vec![7; self.thumb?])
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
        let fake = Fake::new(Ok(10));
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
        let mut fake = Fake::new(Ok(10));
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
        let fake = Fake::new(Ok(10));
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
        let mut fake = Fake::new(Ok(10));
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
        let fake = Fake::new(Ok(1));
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

    const DEADLINE: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn thumbnail_error_is_unavailable_and_the_file_is_never_requested() {
        let fake = Fake::new(Err(()));
        let mxc = OwnedMxcUri::from("mxc://x/a");
        assert!(fetch_capped(&*fake, &mxc, DEADLINE).await.is_none());
        // Exactly one request: the thumbnail. There is no fallback to the file.
        assert_eq!(fake.calls().len(), 1);
    }

    #[tokio::test]
    async fn size_cap_is_one_mib_inclusive() {
        let mxc = OwnedMxcUri::from("mxc://x/a");
        let at = Fake::new(Ok(MAX_AVATAR_BYTES));
        assert!(fetch_capped(&*at, &mxc, DEADLINE).await.is_some());
        let over = Fake::new(Ok(MAX_AVATAR_BYTES + 1));
        assert!(fetch_capped(&*over, &mxc, DEADLINE).await.is_none());
        let empty = Fake::new(Ok(0));
        assert!(fetch_capped(&*empty, &mxc, DEADLINE).await.is_none());
    }

    #[tokio::test]
    async fn a_fetch_that_never_completes_times_out() {
        let fake = Fake::hanging();
        let mxc = OwnedMxcUri::from("mxc://x/a");
        let started = std::time::Instant::now();
        assert!(fetch_capped(&*fake, &mxc, DEADLINE).await.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_production_deadline_is_twenty_seconds() {
        assert_eq!(FETCH_DEADLINE, Duration::from_secs(20));
    }

    #[tokio::test]
    async fn hanging_fetches_release_their_permits() {
        let fake = Fake::hanging();
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::with_deadline(DEADLINE);
        // More ids than permits: all must be answered, so no permit is held
        // forever.
        let ids: Vec<AvatarId> = (0..12)
            .map(|i| AvatarId::from(format!("mxc://x/{i}")))
            .collect();
        avatars.request(ids, &fake, &tx);
        let got = drain(&mut rx, 12).await;
        assert!(got.iter().all(|(_, i)| i.is_none()));
    }

    #[tokio::test]
    async fn a_failed_fetch_can_be_retried_but_a_success_cannot() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::new();
        let a = AvatarId::from("mxc://x/a");

        let failing = Fake::new(Err(()));
        avatars.request(vec![a.clone()], &failing, &tx);
        assert!(drain(&mut rx, 1).await[0].1.is_none());
        avatars.request(vec![a.clone()], &failing, &tx);
        assert!(drain(&mut rx, 1).await[0].1.is_none());
        assert_eq!(failing.calls().len(), 2, "a failure is retried");

        let ok = Fake::new(Ok(10));
        avatars.request(vec![a.clone()], &ok, &tx);
        assert!(drain(&mut rx, 1).await[0].1.is_some());
        avatars.request(vec![a.clone()], &ok, &tx);
        quiet(&mut rx).await;
        assert_eq!(ok.calls().len(), 1, "a success is not fetched again");
    }

    #[tokio::test]
    async fn a_timed_out_fetch_can_be_retried() {
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::with_deadline(DEADLINE);
        let a = AvatarId::from("mxc://x/a");
        let hang = Fake::hanging();
        avatars.request(vec![a.clone()], &hang, &tx);
        assert!(drain(&mut rx, 1).await[0].1.is_none());
        let ok = Fake::new(Ok(10));
        avatars.request(vec![a], &ok, &tx);
        assert!(drain(&mut rx, 1).await[0].1.is_some());
    }

    #[tokio::test]
    async fn an_invalid_id_is_not_retried() {
        let fake = Fake::new(Ok(10));
        let (tx, mut rx) = mpsc::channel(64);
        let mut avatars = Avatars::new();
        let bad = AvatarId::from("not-mxc");
        avatars.request(vec![bad.clone()], &fake, &tx);
        drain(&mut rx, 1).await;
        avatars.request(vec![bad], &fake, &tx);
        quiet(&mut rx).await;
    }

    #[tokio::test]
    async fn the_seen_set_stays_bounded() {
        let fake = Fake::new(Ok(1));
        let (tx, mut rx) = mpsc::channel(1024);
        let mut avatars = Avatars::new();
        let rounds = MAX_SEEN / MAX_IDS_PER_COMMAND + 3;
        for r in 0..rounds {
            let ids: Vec<AvatarId> = (0..MAX_IDS_PER_COMMAND)
                .map(|i| AvatarId::from(format!("mxc://x/{r}-{i}")))
                .collect();
            avatars.request(ids, &fake, &tx);
            drain(&mut rx, MAX_IDS_PER_COMMAND).await;
            assert!(avatars.seen_len() <= MAX_SEEN);
        }
    }

    #[test]
    fn parse_rejects_local_media_servers() {
        for bad in [
            "mxc://send-queue.localhost/txn",
            "mxc://SEND-QUEUE.LOCALHOST/txn",
            "mxc://send-queue.localhost:8448/txn",
            "mxc://evil.localhost/abc",
            "mxc://a.b.Localhost/abc",
            "mxc://x.localhost:1/abc",
        ] {
            assert!(parse_avatar_id(&AvatarId::from(bad)).is_none(), "{bad}");
        }
        // The bare name is the local test homeserver and stays valid, as do
        // lookalikes.
        for ok in [
            "mxc://localhost/abc",
            "mxc://localhost:8008/abc",
            "mxc://notlocalhost/abc",
            "mxc://localhost.example.org/abc",
        ] {
            assert!(parse_avatar_id(&AvatarId::from(ok)).is_some(), "{ok}");
        }
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
