//! Video in calls end to end on the local stack (`scripts/stack.sh up -d
//! --wait && scripts/create-test-users.sh`), the software path (plan Phase 9
//! Task 5): alice sends the test pattern as her camera in an encrypted call
//! and carol's feed delivers it in order, every frame's stamp readable; a
//! video track from another device of a member shows as that device's, a
//! stranger's does not; leaving ends the feeds.
//!
//! Uses alice and carol; bob stays untouched. `TWICECAT_VIDEO_SECONDS`
//! shortens the 30 s watch (the plan's bar is for 30 s).

mod common;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::calls::{add_member_device, allow_calls, sfu_room};
use common::{
    DEFAULT_TIMEOUT, TestEnv, create_encrypted_room, create_room, expect_event, raw_login, silence,
    start_core_with_video, unique_room_name, wait_for_room,
};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    CallIntent, CallState, Command, CoreEvent, LocalVideo, Password, RoomId, SyncState,
    VideoBackend, VideoDirection, VideoFeed, VideoFrame, VideoKind, VideoPixelFormat, VideoQuality,
    VideoTrackId, VideoTrackStats, VideoTrackView,
};
use twicecat_core::secrets::InMemorySecretStore;
use twicecat_video::test_pattern::read_stamp;
use twicecat_video::{ColorSpace, CpuFrame, PixelFormat, Plane};

async fn logged_in(dir: &std::path::Path, n: u8) -> CoreHandle {
    let mut core = start_core_with_video(
        dir,
        Arc::new(InMemorySecretStore::new()),
        silence(),
        None,
        VideoBackend::TestPattern,
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

async fn join(core: &mut CoreHandle, room_id: &str, encrypted: bool) {
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
struct Seen {
    tracks: Vec<VideoTrackView>,
    feeds: HashMap<VideoTrackId, VideoFeed>,
    /// Every track ever listed.
    ever: Vec<VideoTrackView>,
}

impl Seen {
    fn take(&mut self, event: CoreEvent) {
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
async fn track_where(
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
async fn until_unlisted(
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
fn cpu_frame(frame: &VideoFrame) -> CpuFrame {
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
struct Watched {
    /// The stamps' counters, in arrival order.
    counters: Vec<u32>,
    /// Frames whose stamp did not read.
    unreadable: usize,
    sizes: BTreeSet<(u32, u32)>,
    /// From the first frame to the end of the watch.
    span: Duration,
}

/// Takes every frame `feed` delivers for `period` after its first one.
async fn watch(feed: &mut VideoFeed, period: Duration) -> Watched {
    let mut out = Watched::default();
    let first = tokio::time::timeout(DEFAULT_TIMEOUT, feed.changed())
        .await
        .expect("a first frame");
    assert!(first, "the feed ended before its first frame");
    let start = Instant::now();
    let end = start + period;
    loop {
        if let Some(frame) = feed.latest() {
            out.sizes.insert((frame.width, frame.height));
            match read_stamp(&cpu_frame(&frame)) {
                Some(stamp) => out.counters.push(stamp.counter),
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
async fn statistics(
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
async fn drain(core: &mut CoreHandle, seen: &mut Seen, period: Duration) -> Vec<VideoTrackStats> {
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
async fn ended(feed: &mut VideoFeed, who: &str) {
    let wait = async { while feed.changed().await {} };
    tokio::time::timeout(DEFAULT_TIMEOUT, wait)
        .await
        .unwrap_or_else(|_| panic!("{who}'s feed did not end"));
}

fn watch_period() -> Duration {
    let seconds = std::env::var("TWICECAT_VIDEO_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    Duration::from_secs(seconds)
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_test_pattern_reaches_the_other_member_in_order_in_an_encrypted_call() {
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_encrypted_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();

    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in(dir_a.path(), 1).await;
    let mut carol = logged_in(dir_c.path(), 3).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    join(&mut alice, &room_id, true).await;
    join(&mut carol, &room_id, true).await;

    alice
        .commands
        .send(Command::StartCamera {
            device: None,
            quality: VideoQuality::Hd720p30,
        })
        .await
        .unwrap();
    let local = expect_event(&mut alice, |e| {
        matches!(e, CoreEvent::LocalVideo(_) | CoreEvent::Error(_))
    })
    .await;
    assert!(
        matches!(
            &local,
            CoreEvent::LocalVideo(LocalVideo {
                camera: Some(_),
                screen: None,
                ..
            })
        ),
        "{local:?}"
    );
    let mut alice_seen = Seen::default();
    let (own, mut self_view) = track_where(&mut alice, &mut alice_seen, |t| t.is_local).await;
    assert_eq!(
        (own.kind, own.width, own.height, own.fps),
        (VideoKind::Camera, 1280, 720, Some(30)),
        "{own:?}"
    );

    let alice_id = env.user_id(1);
    let mut carol_seen = Seen::default();
    let (theirs, mut feed) = track_where(&mut carol, &mut carol_seen, |t| {
        !t.is_local && t.user_id.as_str() == alice_id
    })
    .await;
    assert_eq!(theirs.kind, VideoKind::Camera, "{theirs:?}");
    assert_eq!(
        theirs.participant, own.participant,
        "the same participant on both sides"
    );
    carol
        .commands
        .send(Command::SetVideoView {
            track: theirs.id.clone(),
            width: 1280,
            height: 720,
            visible: true,
        })
        .await
        .unwrap();

    let period = watch_period();
    let (watched, carol_stats) =
        statistics(&mut carol, &mut carol_seen, watch(&mut feed, period)).await;
    let alice_stats = drain(&mut alice, &mut alice_seen, Duration::from_secs(2)).await;
    let distinct = watched.counters.len();
    let fps = distinct as f64 / watched.span.as_secs_f64();
    eprintln!(
        "carol got {distinct} frames in {:.1} s ({fps:.1} fps), {} unreadable, sizes {:?}",
        watched.span.as_secs_f64(),
        watched.unreadable,
        watched.sizes
    );
    assert!(
        watched.counters.windows(2).all(|w| w[0] < w[1]),
        "frames out of order or repeated: {:?}",
        watched.counters.windows(2).find(|w| w[0] >= w[1])
    );
    assert_eq!(watched.unreadable, 0, "every frame's stamp reads");
    assert!(
        watched.sizes.contains(&(1280, 720)),
        "the top layer: {:?}",
        watched.sizes
    );
    assert!(fps >= 28.0, "{fps:.1} fps");

    let received = carol_stats
        .iter()
        .rfind(|s| s.track == theirs.id && s.direction == VideoDirection::Received)
        .expect("carol's statistics of alice's camera");
    let sent = alice_stats
        .iter()
        .rfind(|s| s.track == own.id && s.direction == VideoDirection::Sent)
        .expect("alice's statistics of her camera");
    eprintln!("received {received:?}\nsent {sent:?}");
    assert!(
        received.fps > 20.0 && received.bitrate_kbps > 0,
        "{received:?}"
    );
    assert!(sent.fps > 20.0 && sent.bitrate_kbps > 0, "{sent:?}");

    alice.commands.send(Command::LeaveCall).await.unwrap();
    ended(&mut self_view, "alice's self-view").await;
    ended(&mut feed, "carol's view of alice").await;
    until_unlisted(&mut carol, &mut carol_seen, |t| t.id == theirs.id).await;
    // Alice is told that no track is left.
    expect_event(
        &mut alice,
        |e| matches!(e, CoreEvent::VideoTracks { tracks, .. } if tracks.is_empty()),
    )
    .await;

    for core in [&alice, &carol] {
        core.commands.send(Command::LeaveCall).await.unwrap();
        core.commands.send(Command::Shutdown).await.unwrap();
    }
}

/// Publishes a grey 640 × 360 picture at 15 fps on `room` as `source`, until
/// the room closes.
async fn publish_picture(
    room: &livekit::prelude::Room,
    source: livekit::prelude::TrackSource,
) -> (
    livekit::prelude::LocalTrackPublication,
    livekit::prelude::LocalVideoTrack,
) {
    use livekit::options::TrackPublishOptions;
    use livekit::prelude::{LocalTrack, LocalVideoTrack};
    use livekit::webrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
    use livekit::webrtc::video_source::native::NativeVideoSource;
    use livekit::webrtc::video_source::{RtcVideoSource, VideoResolution};

    let video = NativeVideoSource::new(
        VideoResolution {
            width: 640,
            height: 360,
        },
        source == livekit::prelude::TrackSource::Screenshare,
    );
    let track =
        LocalVideoTrack::create_video_track("picture", RtcVideoSource::Native(video.clone()));
    let publication = room
        .local_participant()
        .publish_track(
            LocalTrack::Video(track.clone()),
            TrackPublishOptions {
                source,
                ..TrackPublishOptions::default()
            },
        )
        .await
        .expect("publishes a picture");
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(66));
        let start = Instant::now();
        loop {
            ticker.tick().await;
            let mut buffer = I420Buffer::new(640, 360);
            let (y, u, v) = buffer.data_mut();
            y.fill(128);
            u.fill(128);
            v.fill(128);
            let frame = VideoFrame {
                rotation: VideoRotation::VideoRotation0,
                timestamp_us: i64::try_from(start.elapsed().as_micros()).unwrap(),
                frame_metadata: None,
                buffer,
            };
            video.capture_frame(&frame);
        }
    });
    (publication, track)
}

/// Another client of carol's shares a screen: alice lists it as that
/// device's screen share with a feed, sees it turned off and back on, and
/// it goes when unpublished. A stranger on the SFU (a token, no call
/// membership) sending a camera is never listed.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_video_track_from_another_device_shows_as_that_device() {
    use livekit::prelude::TrackSource;
    const DEVICE: &str = "SCREENER";
    let env = TestEnv::load();
    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();

    let dir_a = tempfile::tempdir().unwrap();
    let mut alice = logged_in(dir_a.path(), 1).await;
    wait_for_room(&mut alice, &room_id).await;
    join(&mut alice, &room_id, false).await;

    let carol_id = env.user_id(3);
    add_member_device(&carol_raw, &room_id, &env.user_id(1), DEVICE).await;
    let screener = sfu_room(&carol_raw, &room_id, DEVICE).await;
    let stranger = sfu_room(&carol_raw, &room_id, "STRANGER").await;
    publish_picture(&stranger, TrackSource::Camera).await;
    let (share, track) = publish_picture(&screener, TrackSource::Screenshare).await;

    let mut seen = Seen::default();
    let (view, mut feed) = track_where(&mut alice, &mut seen, |t| !t.is_local).await;
    assert_eq!(view.user_id.as_str(), carol_id, "{view:?}");
    assert_eq!(view.device_id.as_str(), DEVICE, "{view:?}");
    assert_eq!(view.participant.as_str(), format!("{carol_id}:{DEVICE}"));
    assert_eq!(view.kind, VideoKind::Screen, "{view:?}");
    assert!(!view.muted, "{view:?}");
    alice
        .commands
        .send(Command::SetVideoView {
            track: view.id.clone(),
            width: 640,
            height: 360,
            visible: true,
        })
        .await
        .unwrap();
    let frames = statistics(
        &mut alice,
        &mut seen,
        watch(&mut feed, Duration::from_secs(3)),
    )
    .await
    .0;
    assert!(
        frames.sizes.contains(&(640, 360)),
        "the picture arrives: {frames:?}"
    );

    track.mute();
    let _ = track_where(&mut alice, &mut seen, |t| t.id == view.id && t.muted).await;
    track.unmute();
    let _ = track_where(&mut alice, &mut seen, |t| t.id == view.id && !t.muted).await;

    screener
        .local_participant()
        .unpublish_track(&share.sid())
        .await
        .expect("stops sharing");
    until_unlisted(&mut alice, &mut seen, |t| t.id == view.id).await;
    ended(&mut feed, "the screen share").await;
    assert!(
        seen.ever.iter().all(|t| t.device_id.as_str() != "STRANGER"),
        "a stranger's camera was listed"
    );

    let _ = screener.close().await;
    let _ = stranger.close().await;
    alice.commands.send(Command::LeaveCall).await.unwrap();
    alice.commands.send(Command::Shutdown).await.unwrap();
}
