//! Avatar fetching against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).

mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{
    TestEnv, create_room, expect_event, expect_event_within, raw_login, start_core,
    unique_room_name,
};
use tessera_core::api::{AvatarId, Command, CoreEvent, ListDiff, Password, RoomSummary, SyncState};
use tessera_core::secrets::InMemorySecretStore;

const FIXTURE: &[u8] = include_bytes!("fixtures/avatar.png");

/// Paths of the regular files below `dir` that contain `needle`.
fn files_containing(dir: &Path, needle: &[u8]) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).expect("read dir").flatten() {
            let p = entry.path();
            let kind = entry.file_type().expect("file type");
            if kind.is_dir() {
                stack.push(p);
            } else if kind.is_file() {
                let data = std::fs::read(&p).expect("read file");
                if data.windows(needle.len()).any(|w| w == needle) {
                    found.push(p);
                }
            }
        }
    }
    found
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn avatar_is_fetched_once_through_the_sdk_media_cache() {
    let env = TestEnv::load();
    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    core.commands
        .send(Command::LoginPassword {
            username: env.user(1).to_owned(),
            password: Password::from(env.password(1)),
            server: None,
        })
        .await
        .unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedIn { .. })).await;
    expect_event(&mut core, |e| {
        matches!(e, CoreEvent::SyncState(SyncState::Running))
    })
    .await;

    // The helper client (alice) makes a room and uploads the fixture as its avatar.
    let alice = raw_login(1).await;
    let name = unique_room_name();
    let room_id = create_room(&alice, &name, None).await;
    let room = alice.get_room(&room_id).expect("alice knows the room");
    room.upload_avatar(&"image/png".parse().unwrap(), FIXTURE.to_vec(), None)
        .await
        .expect("upload avatar");

    // Alice's core list shows the room with an avatar ID.
    let mut rooms: Vec<RoomSummary> = Vec::new();
    let wait = async {
        loop {
            if let Some(id) = rooms
                .iter()
                .find(|r| r.name == name)
                .and_then(|r| r.avatar.clone())
            {
                return id;
            }
            if let Some(CoreEvent::RoomListChanged(diffs)) = core.events.recv().await {
                for diff in diffs {
                    ListDiff::apply(&diff, &mut rooms);
                }
            }
        }
    };
    let id: AvatarId = tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("timed out waiting for the room avatar");

    // An invalid ID is answered without a request; the real one gives bytes.
    let bad = AvatarId::from("not-mxc");
    core.commands
        .send(Command::FetchAvatars(vec![
            bad.clone(),
            id.clone(),
            id.clone(),
        ]))
        .await
        .unwrap();
    let mut bytes = None;
    for _ in 0..2 {
        let event = expect_event_within(&mut core, Duration::from_secs(30), |e| {
            matches!(e, CoreEvent::Avatar { .. })
        })
        .await;
        let CoreEvent::Avatar { id: got, image } = event else {
            unreachable!()
        };
        if got == bad {
            assert!(image.is_none());
        } else {
            assert_eq!(got, id);
            let image = image.expect("avatar bytes");
            assert!(!image.as_bytes().is_empty());
            bytes = Some(image.into_bytes());
        }
    }
    let bytes = bytes.expect("the real avatar was answered");

    // A second request for the same ID gives no second event.
    core.commands
        .send(Command::FetchAvatars(vec![id.clone()]))
        .await
        .unwrap();
    // Poll the events for a moment: only unrelated events may arrive.
    let quiet = async {
        while let Some(event) = core.events.recv().await {
            assert!(
                !matches!(event, CoreEvent::Avatar { .. }),
                "second answer for the same avatar"
            );
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(3), quiet).await;

    // Nothing but the session's SQLite files holds the image (or the
    // fixture): no plaintext cache file anywhere in the data directory.
    for needle in [&bytes[..], FIXTURE] {
        for path in files_containing(dir.path(), needle) {
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                file.contains(".sqlite"),
                "image bytes found outside the SDK store: {file}"
            );
        }
    }

    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}
