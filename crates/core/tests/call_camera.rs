//! Cameras through the core (plan Phase 9 Task 9), with a virtual camera:
//! an in-process PipeWire camera sending the stamped test pattern
//! (`twicecat_video::capture::test_camera`; no `v4l2loopback` here). It is
//! listed, its coming and going is said, its preview shows its frames, and
//! it feeds an encrypted call end to end on the local stack (`scripts/
//! stack.sh up -d --wait && scripts/create-test-users.sh`); unplugging it
//! is said in both.
//!
//! No test here opens a real camera: each names the virtual one. Listing
//! (which opens none) shows the machine's own cameras too.
//!
//! Uses alice and carol; bob stays untouched.

#![cfg(target_os = "linux")]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::video::{
    Seen, ended, join, logged_in_with, prefer_software, shut_down, statistics, track_where, watch,
    watch_period,
};
use common::{
    TestEnv, create_encrypted_room, expect_event, raw_login, start_core, unique_room_name,
    wait_for_room,
};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    Command, CoreError, CoreEvent, LocalVideo, VideoBackend, VideoDevices, VideoKind, VideoQuality,
};
use twicecat_core::secrets::InMemorySecretStore;
use twicecat_video::capture::test_camera::TestCamera;

/// The cameras the next `VideoDevices` lists.
async fn devices(core: &mut CoreHandle) -> VideoDevices {
    match expect_event(core, |e| matches!(e, CoreEvent::VideoDevices(_))).await {
        CoreEvent::VideoDevices(devices) => devices,
        _ => unreachable!("filtered by the predicate"),
    }
}

/// Waits for a `VideoDevices` that lists (or not) the camera `id`.
async fn until_listed(core: &mut CoreHandle, id: &str, listed: bool) -> VideoDevices {
    loop {
        let found = devices(core).await;
        if found.cameras.iter().any(|c| c.id.as_str() == id) == listed {
            return found;
        }
    }
}

#[tokio::test]
#[ignore = "needs a PipeWire daemon"]
async fn a_virtual_camera_is_listed_previewed_and_its_unplugging_said() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    let id = "twicecat-test-camera-core";

    // Listed: the first listing also watches for cameras coming and going.
    core.commands.send(Command::ListVideoDevices).await.unwrap();
    let before = devices(&mut core).await;
    assert!(!before.cameras.iter().any(|c| c.id.as_str() == id));
    let camera = TestCamera::start(id, 640, 360, 30).expect("the virtual camera");
    let now = until_listed(&mut core, id, true).await;
    let device = now
        .cameras
        .iter()
        .find(|c| c.id.as_str() == id)
        .expect("listed");
    assert!(
        device.name.starts_with("Twicecat Test Camera"),
        "{device:?}"
    );
    assert_eq!(
        device
            .modes
            .iter()
            .map(|m| (m.width, m.height, m.fps))
            .collect::<Vec<_>>(),
        vec![(640, 360, 30)]
    );

    // Previewed: its stamped frames, in order, at its size.
    core.commands
        .send(Command::StartCameraPreview {
            device: Some(id.into()),
            quality: VideoQuality::Hd720p30,
        })
        .await
        .unwrap();
    let e = expect_event(&mut core, |e| {
        matches!(e, CoreEvent::CameraPreview(_) | CoreEvent::Error(_))
    })
    .await;
    let CoreEvent::CameraPreview(mut feed) = e else {
        panic!("{e:?}");
    };
    let watched = watch(&mut feed, Duration::from_secs(2)).await;
    assert!(watched.counters.len() >= 20, "{:?}", watched.counters.len());
    assert!(watched.counters.windows(2).all(|w| w[0] < w[1]), "in order");
    assert_eq!(watched.unreadable, 0);
    assert_eq!(
        watched.sizes.into_iter().collect::<Vec<_>>(),
        vec![(640, 360)]
    );

    // Unplugged: said, the preview ends, and the list no longer has it.
    // The error and the new list come in either order.
    drop(camera);
    let (mut said, mut unlisted) = (false, false);
    while !(said && unlisted) {
        let e = expect_event(&mut core, |e| {
            matches!(e, CoreEvent::Error(_) | CoreEvent::VideoDevices(_))
        })
        .await;
        match e {
            CoreEvent::Error(CoreError::Call(why)) if why.contains("disconnected") => said = true,
            CoreEvent::VideoDevices(found) => {
                unlisted = !found.cameras.iter().any(|c| c.id.as_str() == id);
            }
            other => panic!("{other:?}"),
        }
    }
    ended(&mut feed, "the preview").await;

    // A preview of a camera that is not there.
    core.commands
        .send(Command::StartCameraPreview {
            device: Some(id.into()),
            quality: VideoQuality::Hd720p30,
        })
        .await
        .unwrap();
    let e = expect_event(&mut core, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(
        matches!(&e, CoreEvent::Error(CoreError::Call(why)) if why.contains("not available")),
        "{e:?}"
    );
    core.commands.send(Command::Shutdown).await.unwrap();
}

#[tokio::test]
#[ignore = "needs local homeserver (scripts/stack.sh up -d --wait) and a PipeWire daemon"]
async fn a_virtual_camera_feeds_an_encrypted_call_and_its_unplugging_is_said() {
    let env = TestEnv::load();
    let id = "twicecat-test-camera-call";
    let camera = TestCamera::start(id, 1280, 720, 30).expect("the virtual camera");

    let alice_raw = raw_login(1).await;
    let carol_raw = raw_login(3).await;
    let room_id = create_encrypted_room(&alice_raw, &unique_room_name(), Some(&env.user_id(3)))
        .await
        .to_string();
    common::calls::allow_calls(&alice_raw, &room_id).await;
    carol_raw
        .join_room_by_id(room_id.as_str().try_into().unwrap())
        .await
        .unwrap();
    let (dir_a, dir_c) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut alice = logged_in_with(dir_a.path(), 1, VideoBackend::System).await;
    let mut carol = logged_in_with(dir_c.path(), 3, VideoBackend::TestPattern).await;
    wait_for_room(&mut alice, &room_id).await;
    wait_for_room(&mut carol, &room_id).await;
    join(&mut alice, &room_id, true).await;
    join(&mut carol, &room_id, true).await;

    prefer_software(&mut alice).await;
    alice
        .commands
        .send(Command::StartCamera {
            device: Some(id.into()),
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
                ..
            })
        ),
        "{local:?}"
    );
    let mut alice_seen = Seen::default();
    let (own, _self_view) = track_where(&mut alice, &mut alice_seen, |t| t.is_local).await;
    assert_eq!(
        (own.kind, own.width, own.height),
        (VideoKind::Camera, 1280, 720),
        "{own:?}"
    );

    let alice_id = env.user_id(1);
    let mut carol_seen = Seen::default();
    let (theirs, mut feed) = track_where(&mut carol, &mut carol_seen, |t| {
        !t.is_local && t.user_id.as_str() == alice_id
    })
    .await;
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
    // The camera's frames, through the encoder, the SFU, the decryption
    // and the decoder: their stamps read, in order.
    let period = watch_period().min(Duration::from_secs(10));
    // Carol's events are read meanwhile, as in the other video tests.
    let (watched, _) = statistics(&mut carol, &mut carol_seen, watch(&mut feed, period)).await;
    let fps = watched.counters.len() as f64 / watched.span.as_secs_f64();
    eprintln!(
        "carol got {} frames in {:.1} s ({fps:.1} fps), {} unreadable, sizes {:?}",
        watched.counters.len(),
        watched.span.as_secs_f64(),
        watched.unreadable,
        watched.sizes
    );
    assert!(fps >= 5.0, "{fps:.1} fps");
    assert!(watched.counters.windows(2).all(|w| w[0] < w[1]), "in order");
    assert!(watched.sizes.contains(&(1280, 720)), "{:?}", watched.sizes);

    // Unplugged: alice is told, her camera is off, carol's feed ends.
    drop(camera);
    let e = expect_event(&mut alice, |e| matches!(e, CoreEvent::Error(_))).await;
    assert!(
        matches!(&e, CoreEvent::Error(CoreError::Call(why)) if why.contains("disconnected")),
        "{e:?}"
    );
    ended(&mut feed, "carol's view of alice's camera").await;

    alice.commands.send(Command::LeaveCall).await.unwrap();
    shut_down(alice, "alice").await;
    shut_down(carol, "carol").await;
}
