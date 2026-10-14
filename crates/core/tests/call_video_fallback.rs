//! A hardware encoder that fails mid-call falls back to software (plan
//! Phase 9 Task 6), on the local stack with LiveKit's NVENC (the human's
//! RTX 5080): with `TWICECAT_VIDEO_UNCAPPED=1` the core asks NVENC for 4K60,
//! which LiveKit's NVENC refuses today (H.264 level 3.1), so no frame is
//! encoded; the core notices within a few seconds, sends the camera again
//! with libwebrtc's VP8, and says so in `VideoEncoders`. Carol then gets
//! the pattern in VP8.
//!
//! Its own test binary: it sets the environment variable before the cores
//! start. Uses alice and carol.

mod common;

use std::time::Duration;

use common::video::{Seen, alice_and_carol_in_a_call, encoders, shut_down, track_where, watch};
use common::{DEFAULT_TIMEOUT, TestEnv, expect_event};
use twicecat_core::api::{
    Command, CoreEvent, EncoderBackend, VideoCodec, VideoEncoders, VideoQuality,
};

#[tokio::test]
#[ignore = "needs NVENC and the local homeserver (human-run)"]
async fn hardware_a_hardware_encoder_that_fails_falls_back_to_software() {
    // SAFETY: the only test in this binary, run before the cores start any
    // thread; libtest's own threads do not read the environment meanwhile.
    unsafe { std::env::set_var("TWICECAT_VIDEO_UNCAPPED", "1") };
    let env = TestEnv::load();
    let mut call = alice_and_carol_in_a_call(true).await;
    let found = encoders(&mut call.alice).await;
    assert_eq!(
        found.encoders.first().map(|e| e.backend),
        Some(EncoderBackend::Nvenc),
        "needs LiveKit's NVENC: {found:?}"
    );
    call.alice
        .commands
        .send(Command::StartCamera {
            device: None,
            quality: VideoQuality::Uhd2160p60,
        })
        .await
        .unwrap();
    let in_use = |want: EncoderBackend| move |e: &CoreEvent| matches!(e, CoreEvent::VideoEncoders(VideoEncoders { in_use, .. }) if *in_use == Some(want));
    expect_event(&mut call.alice, in_use(EncoderBackend::Nvenc)).await;
    // Three reports a second apart without an encoded frame, then the
    // software track.
    let fell_back = common::expect_event_within(
        &mut call.alice,
        DEFAULT_TIMEOUT + Duration::from_secs(10),
        in_use(EncoderBackend::Software),
    )
    .await;
    eprintln!("alice: {fell_back:?}");

    let alice_id = env.user_id(1);
    let mut seen = Seen::default();
    let (theirs, mut feed) = track_where(&mut call.carol, &mut seen, |t| {
        !t.is_local && t.user_id.as_str() == alice_id && t.codec == Some(VideoCodec::Vp8)
    })
    .await;
    call.carol
        .commands
        .send(Command::SetVideoView {
            track: theirs.id.clone(),
            width: 1920,
            height: 1080,
            visible: true,
        })
        .await
        .unwrap();
    let watched = watch(&mut feed, Duration::from_secs(5)).await;
    eprintln!("carol after the fallback: {watched:?}");
    assert!(
        watched.counters.len() >= 50,
        "VP8 frames arrive: {watched:?}"
    );
    assert_eq!(watched.unreadable, 0);

    shut_down(call.alice, "alice").await;
    shut_down(call.carol, "carol").await;
}
