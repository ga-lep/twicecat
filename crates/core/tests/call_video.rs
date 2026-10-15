//! Video in calls end to end on the local stack (`scripts/stack.sh up -d
//! --wait && scripts/create-test-users.sh`), the software path (plan Phase 9
//! Task 5): alice sends the test pattern as her camera in an encrypted call
//! and carol's feed delivers it in order, every frame's stamp readable; a
//! video track from another device of a member shows as that device's, a
//! stranger's does not; leaving ends the feeds.
//!
//! The hardware path (Task 6): on a machine with LiveKit's NVENC, H.264 in
//! three layers up to 4K30 and 1440p60 (`hardware_`, human-run).
//!
//! Uses alice and carol; bob stays untouched. `TWICECAT_VIDEO_SECONDS`
//! shortens the 30 s watch (the plan's bar is for 30 s).

mod common;

use std::time::{Duration, Instant};

use common::calls::{add_member_device, allow_calls, sfu_room};
use common::video::{
    Seen, Watched, drain, ended, join, logged_in, statistics, track_where, until_unlisted, watch,
    watch_period,
};
use common::{
    TestEnv, create_encrypted_room, create_room, expect_event, raw_login, unique_room_name,
    wait_for_room,
};
use twicecat_core::api::{
    Command, CoreEvent, LocalVideo, VideoDirection, VideoKind, VideoQuality, VideoTrackStats,
};
use twicecat_perf::{Expect, record};

/// The camera test pattern's rate. A quiet machine measures 29.6 to 30 fps (on a
/// laptop and on a CI runner), so a rate more than 1 fps below it is worth a look.
const NOMINAL_FPS: f64 = 30.0;

/// A feed below this is stalled, which is a bug; between this and the nominal
/// rate it is slow, which only the `perf_` test reports (docs/testing.md, R1).
const STREAMING_FLOOR_FPS: f64 = 5.0;

/// What a watched camera call produced: the frames carol's feed delivered, and
/// the statistics of both ends.
struct CameraWatch {
    watched: Watched,
    received: VideoTrackStats,
    sent: VideoTrackStats,
}

impl CameraWatch {
    /// Frames per second at carol's feed, over the watch.
    fn delivered_fps(&self) -> f64 {
        self.watched.counters.len() as f64 / self.watched.span.as_secs_f64()
    }
}

/// alice sends the test pattern as her camera in an encrypted call, carol
/// watches it for `watch_period()`, then both leave. Shared by the behaviour
/// test and the speed indicator, so each measures the same call.
async fn watch_a_camera_call() -> CameraWatch {
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

    // The software path, also on a machine with a hardware encoder.
    common::video::prefer_software(&mut alice).await;
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
        "carol got {distinct} frames in {:.1} s ({fps:.1} fps), {} unreadable, {} repeated at a layer switch, sizes {:?}",
        watched.span.as_secs_f64(),
        watched.unreadable,
        watched.layer_repeats,
        watched.sizes
    );
    let received = carol_stats
        .iter()
        .rfind(|s| s.track == theirs.id && s.direction == VideoDirection::Received)
        .expect("carol's statistics of alice's camera")
        .clone();
    let sent = alice_stats
        .iter()
        .rfind(|s| s.track == own.id && s.direction == VideoDirection::Sent)
        .expect("alice's statistics of her camera")
        .clone();
    eprintln!("received {received:?}\nsent {sent:?}");

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

    common::video::shut_down(alice, "alice").await;
    common::video::shut_down(carol, "carol").await;
    CameraWatch {
        watched,
        received,
        sent,
    }
}

#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn a_test_pattern_reaches_the_other_member_in_order_in_an_encrypted_call() {
    let CameraWatch {
        watched,
        received,
        sent,
    } = watch_a_camera_call().await;
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
    // A stalled stream is a bug; a slow one is a number (the `perf_` test below).
    let fps = watched.counters.len() as f64 / watched.span.as_secs_f64();
    assert!(fps >= STREAMING_FLOOR_FPS, "the feed stalled: {fps:.1} fps");
    assert!(received.bitrate_kbps > 0, "{received:?}");
    assert!(sent.bitrate_kbps > 0, "{sent:?}");
}

/// How fast the camera test pattern gets through, on a quiet machine: the
/// frame rate at carol's feed and the rates in both ends' statistics. An
/// indicator (docs/testing.md): a busy runner lowers it, and nothing is wrong.
#[tokio::test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
async fn perf_a_camera_reaches_the_viewer_at_the_frame_rate() {
    let run = watch_a_camera_call().await;
    record(
        "call_video.camera.delivered_fps",
        run.delivered_fps(),
        "fps",
        Expect::AtLeast(NOMINAL_FPS - 1.0),
    );
    record(
        "call_video.camera.received_fps",
        run.received.fps as f64,
        "fps",
        Expect::AtLeast(NOMINAL_FPS - 1.0),
    );
    record(
        "call_video.camera.sent_fps",
        run.sent.fps as f64,
        "fps",
        Expect::AtLeast(NOMINAL_FPS - 1.0),
    );
    record(
        "call_video.camera.received_kbps",
        run.received.bitrate_kbps as f64,
        "kbps",
        Expect::Report,
    );
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
    common::video::shut_down(alice, "alice").await;
}

/// What one hardware run delivered.
struct Run {
    watched: common::video::Watched,
    fps: f64,
    sent: twicecat_core::api::VideoTrackStats,
    received: twicecat_core::api::VideoTrackStats,
}

/// Alice sends the test pattern as her camera at `quality`; carol views it
/// at `view` for the watch period. Returns what carol got and both sides'
/// last statistics. Frames come in order; their stamps are read unless the
/// view is small (the stamp needs about 360p).
async fn hardware_run(
    call: &mut common::video::Call,
    seen: &mut (Seen, Seen),
    quality: VideoQuality,
    view: (u32, u32),
) -> Run {
    let env = TestEnv::load();
    call.alice
        .commands
        .send(Command::StartCamera {
            device: None,
            quality,
        })
        .await
        .unwrap();
    let old: Vec<_> = seen.0.tracks.iter().map(|t| t.id.clone()).collect();
    let (own, _self_view) = track_where(&mut call.alice, &mut seen.0, |t| {
        t.is_local && !old.contains(&t.id)
    })
    .await;
    let alice_id = env.user_id(1);
    let old: Vec<_> = seen.1.tracks.iter().map(|t| t.id.clone()).collect();
    let (theirs, mut feed) = track_where(&mut call.carol, &mut seen.1, |t| {
        !t.is_local && t.user_id.as_str() == alice_id && !old.contains(&t.id)
    })
    .await;
    call.carol
        .commands
        .send(Command::SetVideoView {
            track: theirs.id.clone(),
            width: view.0,
            height: view.1,
            visible: true,
        })
        .await
        .unwrap();
    // The SFU moves to the layer asked for within a few frames; the run
    // counts from then on.
    let _ = drain(&mut call.carol, &mut seen.1, Duration::from_secs(3)).await;
    let (watched, carol_stats) = statistics(
        &mut call.carol,
        &mut seen.1,
        watch(&mut feed, watch_period()),
    )
    .await;
    let alice_stats = drain(&mut call.alice, &mut seen.0, Duration::from_secs(2)).await;
    let fps = watched.counters.len() as f64 / watched.span.as_secs_f64();
    let received = carol_stats
        .iter()
        .rfind(|s| s.track == theirs.id && s.direction == VideoDirection::Received)
        .cloned()
        .expect("carol's statistics of alice's camera");
    let sent = alice_stats
        .iter()
        .rfind(|s| s.track == own.id && s.direction == VideoDirection::Sent)
        .cloned()
        .expect("alice's statistics of her camera");
    eprintln!(
        "{quality:?} viewed at {view:?}: {} frames in {:.1} s ({fps:.1} fps), {} unreadable, \
         {} repeated at a layer switch, sizes {:?}\n  sent {sent:?}\n  received {received:?}",
        watched.counters.len(),
        watched.span.as_secs_f64(),
        watched.unreadable,
        watched.layer_repeats,
        watched.sizes
    );
    assert!(
        watched.counters.windows(2).all(|w| w[0] < w[1]),
        "frames out of order or repeated"
    );
    if view.1 >= 360 {
        assert_eq!(watched.unreadable, 0, "every frame's stamp reads");
    }
    Run {
        watched,
        fps,
        sent,
        received,
    }
}

/// On a machine with LiveKit's NVENC (the human's RTX 5080; plan Phase 9
/// Task 6): in an encrypted call, alice's test pattern goes out as H.264 in
/// three layers on NVENC at 4K30, 1440p60 and 1080p60, and reaches carol
/// in order at full size and rate; a small tile gets a low layer; both
/// cores stop after the call (the process then exits by itself, which
/// NVDEC made hang when a decoder outlived the call).
#[tokio::test]
#[ignore = "needs NVENC and the local homeserver (human-run)"]
async fn hardware_nvenc_sends_h264_in_three_layers_in_an_encrypted_call() {
    use twicecat_core::api::{EncoderBackend, VideoCodec};
    let mut call = common::video::alice_and_carol_in_a_call(true).await;
    let found = common::video::encoders(&mut call.alice).await;
    assert_eq!(
        found.encoders.first().map(|e| e.backend),
        Some(EncoderBackend::Nvenc),
        "needs LiveKit's NVENC (cuda.h at build time, an NVIDIA GPU): {found:?}"
    );
    let mut seen = (Seen::default(), Seen::default());
    let cases = [
        (VideoQuality::Uhd2160p30, (3840, 2160), 28.0),
        (VideoQuality::Qhd1440p60, (2560, 1440), 55.0),
        (VideoQuality::Hd1080p60, (1920, 1080), 55.0),
    ];
    for (quality, size, min_fps) in cases {
        let run = hardware_run(&mut call, &mut seen, quality, size).await;
        assert!(run.watched.sizes.contains(&size), "{:?}", run.watched.sizes);
        assert!(
            run.fps >= STREAMING_FLOOR_FPS,
            "{quality:?}: the feed stalled at {:.1} fps",
            run.fps
        );
        record(
            &format!("call_video.hardware.{quality:?}.fps"),
            run.fps,
            "fps",
            Expect::AtLeast(min_fps),
        );
        let encoder = run.sent.implementation.as_deref().unwrap_or_default();
        assert!(
            encoder.contains("NVIDIA H264 Encoder") && run.sent.hardware,
            "{:?}",
            run.sent
        );
        assert!(
            encoder.matches("NVIDIA H264 Encoder").count() == 3,
            "three layers: {encoder}"
        );
        let track = seen
            .1
            .tracks
            .iter()
            .find(|t| !t.is_local)
            .expect("alice's camera");
        assert_eq!(track.codec, Some(VideoCodec::H264), "{track:?}");
        eprintln!("  carol decodes with {:?}", run.received.implementation);
    }
    // A small tile: the SFU sends a low layer instead (360p for a camera).
    let run = hardware_run(&mut call, &mut seen, VideoQuality::Hd1080p60, (320, 180)).await;
    let largest = run.watched.sizes.iter().map(|s| s.1).max().unwrap_or(0);
    assert!(largest <= 360, "a low layer: {:?}", run.watched.sizes);

    common::video::shut_down(call.alice, "alice").await;
    common::video::shut_down(call.carol, "carol").await;
}
