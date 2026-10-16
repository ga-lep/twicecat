//! Video call helpers shared by the video stack tests: logging in with the
//! test pattern, joining a call, waiting for tracks and feeds, watching a
//! feed's frames and collecting statistics.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use twicecat_core::CoreHandle;
use twicecat_core::api::{
    CallIntent, CallState, Command, CoreEvent, Password, RoomId, SyncState, VideoBackend,
    VideoEncoders, VideoFeed, VideoFrame, VideoPixelFormat, VideoTrackId, VideoTrackStats,
    VideoTrackView,
};
use twicecat_core::secrets::InMemorySecretStore;
use twicecat_video::test_pattern::read_stamp;
use twicecat_video::{ColorSpace, CpuFrame, PixelFormat, Plane};

use super::{DEFAULT_TIMEOUT, TestEnv, expect_event, silence, start_core_with_video};

pub async fn logged_in(dir: &std::path::Path, n: u8) -> CoreHandle {
    logged_in_with(dir, n, VideoBackend::TestPattern).await
}

/// Like [`logged_in`], with `video` as its cameras and screens.
pub async fn logged_in_with(dir: &std::path::Path, n: u8, video: VideoBackend) -> CoreHandle {
    let mut core = start_core_with_video(
        dir,
        Arc::new(InMemorySecretStore::new()),
        silence(),
        None,
        video,
    );
    let env = TestEnv::load();
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    expect_event(&mut core, |e| {
        matches!(e, CoreEvent::SyncState(SyncState::Running))
    })
    .await;
    core
}

pub async fn join(core: &mut CoreHandle, room_id: &str, encrypted: bool) {
    core.commands
        .send(Command::JoinCall {
            room_id: RoomId::from(room_id),
            intent: CallIntent::Video,
        })
        .await
        .unwrap();
    let state = expect_event(core, |e| {
        matches!(
            e,
            CoreEvent::CallState(CallState::Connected { .. } | CallState::Left { .. })
        )
    })
    .await;
    assert!(
        matches!(state, CoreEvent::CallState(CallState::Connected { encrypted: e, .. }) if e == encrypted),
        "{state:?}"
    );
}

/// The video tracks and feeds a core reported.
#[derive(Default)]
pub struct Seen {
    pub tracks: Vec<VideoTrackView>,
    pub feeds: HashMap<VideoTrackId, VideoFeed>,
    /// Every track ever listed.
    pub ever: Vec<VideoTrackView>,
}

impl Seen {
    pub fn take(&mut self, event: CoreEvent) {
        match event {
            CoreEvent::VideoTracks { tracks, .. } => {
                self.ever.extend(tracks.iter().cloned());
                self.tracks = tracks;
            }
            CoreEvent::VideoFeed { track, feed } => {
                self.feeds.insert(track, feed);
            }
            _ => {}
        }
    }
}

/// Reads `core`'s events until a listed track matches `wanted` and its feed
/// has come; returns it.
pub async fn track_where(
    core: &mut CoreHandle,
    seen: &mut Seen,
    wanted: impl Fn(&VideoTrackView) -> bool,
) -> (VideoTrackView, VideoFeed) {
    let wait = async {
        loop {
            if let Some(view) = seen.tracks.iter().find(|t| wanted(t)).cloned()
                && let Some(feed) = seen.feeds.get(&view.id)
            {
                return (view, feed.clone());
            }
            match core.events.recv().await {
                Some(event) => seen.take(event),
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for a video track")
}

/// Reads `core`'s events until no listed track matches `gone`.
pub async fn until_unlisted(
    core: &mut CoreHandle,
    seen: &mut Seen,
    gone: impl Fn(&VideoTrackView) -> bool,
) {
    let wait = async {
        while seen.tracks.iter().any(&gone) {
            match core.events.recv().await {
                Some(event) => seen.take(event),
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for a video track to go");
}

/// The feed's frame as the video crate's, for its stamp reader.
pub fn cpu_frame(frame: &VideoFrame) -> CpuFrame {
    let format = match frame.format {
        VideoPixelFormat::I420 => PixelFormat::I420,
        VideoPixelFormat::Nv12 => PixelFormat::Nv12,
    };
    let planes = frame
        .planes
        .iter()
        .map(|p| Plane {
            data: p.data.clone(),
            stride: p.stride,
        })
        .collect();
    CpuFrame::new(
        format,
        frame.width,
        frame.height,
        planes,
        ColorSpace::default(),
        Duration::ZERO,
    )
    .expect("a well-formed frame")
}

/// What a feed delivered while it was watched.
#[derive(Debug, Default)]
pub struct Watched {
    /// The stamps' counters, in arrival order.
    pub counters: Vec<u32>,
    /// Frames whose stamp did not read.
    pub unreadable: usize,
    /// Frames that repeated the previous frame's capture at another size:
    /// when the SFU moves the feed to another layer, that layer's first frame
    /// can be the capture the old layer just gave. Not in `counters`.
    pub layer_repeats: usize,
    pub sizes: BTreeSet<(u32, u32)>,
    /// From the first frame to the end of the watch.
    pub span: Duration,
}

/// Takes every frame `feed` delivers for `period` after its first one.
pub async fn watch(feed: &mut VideoFeed, period: Duration) -> Watched {
    let mut out = Watched::default();
    let first = tokio::time::timeout(DEFAULT_TIMEOUT, feed.changed())
        .await
        .expect("a first frame");
    assert!(first, "the feed ended before its first frame");
    let start = Instant::now();
    let end = start + period;
    let mut last: Option<(u32, (u32, u32))> = None;
    loop {
        if let Some(frame) = feed.latest() {
            let size = (frame.width, frame.height);
            out.sizes.insert(size);
            match read_stamp(&cpu_frame(&frame)) {
                Some(stamp) => {
                    if last.is_some_and(|(counter, was)| counter == stamp.counter && was != size) {
                        out.layer_repeats += 1;
                    } else {
                        out.counters.push(stamp.counter);
                    }
                    last = Some((stamp.counter, size));
                }
                None => out.unreadable += 1,
            }
        }
        let left = end.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, feed.changed()).await {
            Ok(true) => {}
            Ok(false) => panic!("the feed ended while watched"),
            Err(_) => break,
        }
    }
    out.span = start.elapsed();
    out
}

/// Takes `core`'s video statistics until `stop` resolves, keeping the rest.
pub async fn statistics(
    core: &mut CoreHandle,
    seen: &mut Seen,
    stop: impl Future<Output = Watched>,
) -> (Watched, Vec<VideoTrackStats>) {
    let mut stats = Vec::new();
    tokio::pin!(stop);
    loop {
        tokio::select! {
            watched = &mut stop => return (watched, stats),
            event = core.events.recv() => match event {
                Some(CoreEvent::VideoStats(s)) => stats.extend(s.tracks),
                Some(event) => seen.take(event),
                None => panic!("event channel closed"),
            },
        }
    }
}

/// Drains `core`'s events into `seen` and statistics for `period`.
pub async fn drain(
    core: &mut CoreHandle,
    seen: &mut Seen,
    period: Duration,
) -> Vec<VideoTrackStats> {
    let mut stats = Vec::new();
    let _ = tokio::time::timeout(period, async {
        loop {
            match core.events.recv().await {
                Some(CoreEvent::VideoStats(s)) => stats.extend(s.tracks),
                Some(event) => seen.take(event),
                None => panic!("event channel closed"),
            }
        }
    })
    .await;
    stats
}

/// Waits until `feed` ends.
pub async fn ended(feed: &mut VideoFeed, who: &str) {
    let wait = async { while feed.changed().await {} };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .unwrap_or_else(|_| panic!("{who}'s feed did not end"));
}

pub fn watch_period() -> Duration {
    let seconds = std::env::var("TWICECAT_VIDEO_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(seconds)
}

/// Alice and carol in a call in a new room of alice's (encrypted or not),
/// both connected.
pub struct Call {
    pub alice: CoreHandle,
    pub carol: CoreHandle,
    pub room_id: String,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

/// Sets up a [`Call`].
pub async fn alice_and_carol_in_a_call(encrypted: bool) -> Call {
    let env = TestEnv::load();
    let alice_raw = super::raw_login(1).await;
    let carol_raw = super::raw_login(3).await;
    let name = super::unique_room_name();
    let room_id = if encrypted {
        super::create_encrypted_room(&alice_raw, &name, Some(&env.user_id(3))).await
    } else {
        super::create_room(&alice_raw, &name, Some(&env.user_id(3))).await
    }
    .to_string();
    super::calls::allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();
    let dirs = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dirs.0.path(), 1).await;
    let mut carol = logged_in(dirs.1.path(), 3).await;
    super::wait_for_room(&mut alice, &room_id).await;
    super::wait_for_room(&mut carol, &room_id).await;
    join(&mut alice, &room_id, encrypted).await;
    join(&mut carol, &room_id, encrypted).await;
    Call {
        alice,
        carol,
        room_id,
        _dirs: dirs,
    }
}

/// The encoders `core` reports for `ProbeVideoEncoders`.
pub async fn encoders(core: &mut CoreHandle) -> VideoEncoders {
    core.commands
        .send(Command::ProbeVideoEncoders)
        .await
        .unwrap();
    match expect_event(core, |e| matches!(e, CoreEvent::VideoEncoders(_))).await {
        CoreEvent::VideoEncoders(e) => e,
        _ => unreachable!("filtered by the predicate"),
    }
}

/// Leaves the call, shuts `core` down and waits until it has stopped (its
/// event channel closes), as the app does before it exits.
pub async fn shut_down(mut core: CoreHandle, who: &str) {
    let _ = core.commands.send(Command::LeaveCall).await;
    let _ = core.commands.send(Command::Shutdown).await;
    let wait = async { while core.events.recv().await.is_some() {} };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .unwrap_or_else(|_| panic!("{who}'s core did not stop"));
}

/// Makes `core` send video on the software path (VP8) whatever this
/// machine's GPU: the tests of that path ask for it.
pub async fn prefer_software(core: &mut CoreHandle) {
    use twicecat_core::api::{EncoderBackend, EncoderChoice, VideoSettings};
    let settings = VideoSettings {
        encoder: EncoderChoice::Prefer(EncoderBackend::Software),
        ..VideoSettings::default()
    };
    core.commands
        .send(Command::UpdateVideoSettings(settings))
        .await
        .unwrap();
    expect_event(core, |e| matches!(e, CoreEvent::VideoSettings(_))).await;
}
