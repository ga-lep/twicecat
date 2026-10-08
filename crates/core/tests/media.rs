//! Media in the timeline against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`): received
//! pictures and files are mapped, fetched, decrypted and saved.
//!
//! Prints only counts. Alice uploads with a raw SDK client (the core cannot
//! upload yet); carol is the receiving core.

mod common;

use std::sync::Arc;

use common::{
    EncryptedPair, TestEnv, create_room_raw, e2ee_serial, expect_event, fetch_raw_event_json, hex,
    pump, raw_login, send_attachment_raw, send_attachment_with, send_file_declaring_size,
    start_core,
};
use tessera_core::CoreHandle;
use tessera_core::api::{
    ActionKind, Command, CoreError, CoreEvent, MediaBytes, MediaKind, MediaSize, RoomId,
    RoomSummary, TimelineItem, TimelineItemKind,
};
use tessera_core::secrets::InMemorySecretStore;

const PHOTO: &[u8] = include_bytes!("fixtures/photo-1200x900.png");

/// The first media message of `items` that is not the user's own.
fn media_item(items: &[TimelineItem], kind: MediaKind) -> Option<&TimelineItem> {
    items.iter().find(|i| {
        matches!(&i.kind, TimelineItemKind::Message { details, .. }
            if details.media.as_ref().is_some_and(|m| m.kind == kind))
    })
}

fn media_of(item: &TimelineItem) -> &tessera_core::api::MediaContent {
    let TimelineItemKind::Message { details, .. } = &item.kind else {
        panic!("not a message");
    };
    details.media.as_deref().expect("a media message")
}

/// Asks for a rendition and waits for the answer.
async fn fetch(
    core: &mut CoreHandle,
    room: &str,
    item: &TimelineItem,
    size: MediaSize,
) -> Result<MediaBytes, CoreError> {
    core.commands
        .send(Command::FetchMedia {
            room_id: RoomId::from(room),
            item_id: item.id.clone(),
            size,
        })
        .await
        .unwrap();
    match expect_event(core, |e| matches!(e, CoreEvent::Media { .. })).await {
        CoreEvent::Media { result, .. } => result,
        _ => unreachable!(),
    }
}

/// Logs in and waits for the sync to run and for the upload limit, in
/// whichever order they arrive. Returns the limit.
async fn login_with_limits(core: &mut CoreHandle, env: &TestEnv, n: u8) -> Option<u64> {
    core.commands
        .send(Command::LoginPassword {
            username: env.user(n).to_owned(),
            password: tessera_core::api::Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    let (mut running, mut limit) = (false, None);
    let wait = async {
        while !running || limit.is_none() {
            match core.events.recv().await {
                Some(CoreEvent::SyncState(tessera_core::api::SyncState::Running)) => running = true,
                Some(CoreEvent::MediaLimits { max_upload_bytes }) => limit = Some(max_upload_bytes),
                Some(_) => {}
                None => panic!("event channel closed"),
            }
        }
    };
    tokio::time::timeout(common::DEFAULT_TIMEOUT, wait)
        .await
        .expect("timed out waiting for login");
    limit.flatten()
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    rand::fill(bytes.as_mut_slice());
    bytes
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn plain_image_is_mapped_fetched_and_thumbnailed() {
    // The encrypted test resets carol's devices; this one must not overlap it.
    let _serial = e2ee_serial().await;
    let env = TestEnv::load();
    let room_id = create_room_raw(1, &common::unique_room_name(), false, &[&env.user_id(3)]).await;
    let carol_raw = raw_login(3).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    let room = room_id.to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    let limit = login_with_limits(&mut core, &env, 3).await;
    assert!(
        limit.is_some_and(|l| l > 0),
        "the server states an upload limit"
    );
    let (mut rooms, mut items): (Vec<RoomSummary>, Vec<TimelineItem>) = (Vec::new(), Vec::new());
    pump(&mut core, &room, &mut rooms, &mut items, |r, _| {
        r.iter().any(|x| x.id.as_str() == room)
    })
    .await;
    core.commands
        .send(Command::SelectRoom(RoomId::from(room.as_str())))
        .await
        .unwrap();

    let name = format!("photo-{}.png", hex());
    send_attachment_raw(1, &room_id, &name, "image/png", PHOTO.to_vec()).await;
    pump(&mut core, &room, &mut rooms, &mut items, |_, i| {
        media_item(i, MediaKind::Image).is_some()
    })
    .await;

    let item = media_item(&items, MediaKind::Image)
        .expect("arrived")
        .clone();
    let media = media_of(&item);
    assert_eq!(media.filename, name);
    assert_eq!((media.width, media.height), (Some(1200), Some(900)));
    assert_eq!(media.mimetype.as_deref(), Some("image/png"));
    assert_eq!(media.size, Some(PHOTO.len() as u64));
    assert!(media.has_thumbnail);
    assert!(!item.id.is_empty());
    // With no caption the body is the file name.
    let TimelineItemKind::Message { body, .. } = &item.kind else {
        unreachable!()
    };
    assert_eq!(*body, name);

    let full = fetch(&mut core, &room, &item, MediaSize::Full)
        .await
        .expect("full file");
    assert_eq!(full.as_bytes(), PHOTO, "the full file is identical");
    let thumb = fetch(&mut core, &room, &item, MediaSize::Thumbnail)
        .await
        .expect("thumbnail");
    let signature = thumb.as_bytes();
    assert!(
        signature.starts_with(b"\x89PNG\r\n\x1a\n") || signature.starts_with(&[0xff, 0xd8, 0xff]),
        "the thumbnail is a PNG or a JPEG"
    );
    println!(
        "plain image: full {} bytes, thumbnail {} bytes",
        full.len(),
        thumb.len()
    );

    // An item the timeline does not have.
    let ghost = TimelineItem {
        id: "no-such-item".into(),
        ..item.clone()
    };
    assert_eq!(
        fetch(&mut core, &room, &ghost, MediaSize::Full).await,
        Err(CoreError::ItemNotFound)
    );
    // A message that is not media is not a file either.
    let other = items.iter().find(
        |i| matches!(&i.kind, TimelineItemKind::Message { details, .. } if details.media.is_none()),
    );
    if let Some(other) = other {
        assert_eq!(
            fetch(&mut core, &room, other, MediaSize::Full).await,
            Err(CoreError::NotAllowed)
        );
    }
    // A room that is not the selected one.
    assert_eq!(
        fetch(&mut core, "!other:localhost", &item, MediaSize::Full).await,
        Err(CoreError::NoRoomSelected)
    );
    carol_raw.logout().await.expect("carol raw logout");
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_file_bigger_than_it_declares_is_refused_while_reading() {
    // The sender's `info.size` is its own word: a file that streams past the
    // declared size (plus a little slack) is cut off, not buffered whole.
    let _serial = e2ee_serial().await;
    let env = TestEnv::load();
    let room_id = create_room_raw(1, &common::unique_room_name(), false, &[&env.user_id(3)]).await;
    let carol_raw = raw_login(3).await;
    carol_raw
        .join_room_by_id(&room_id)
        .await
        .expect("carol joins");
    let room = room_id.to_string();

    let dir = tempfile::tempdir().unwrap();
    let mut core = start_core(dir.path(), Arc::new(InMemorySecretStore::new()));
    login_with_limits(&mut core, &env, 3).await;
    let (mut rooms, mut items): (Vec<RoomSummary>, Vec<TimelineItem>) = (Vec::new(), Vec::new());
    pump(&mut core, &room, &mut rooms, &mut items, |r, _| {
        r.iter().any(|x| x.id.as_str() == room)
    })
    .await;
    core.commands
        .send(Command::SelectRoom(RoomId::from(room.as_str())))
        .await
        .unwrap();

    // 3 MiB sent as a file that says it has 10 bytes.
    let name = format!("liar-{}.bin", hex());
    let real = vec![0x5a_u8; 3 * 1024 * 1024];
    send_file_declaring_size(1, &room_id, &name, real, 10).await;
    pump(&mut core, &room, &mut rooms, &mut items, |_, i| {
        media_item(i, MediaKind::File).is_some()
    })
    .await;
    let item = media_item(&items, MediaKind::File)
        .expect("arrived")
        .clone();
    assert_eq!(media_of(&item).size, Some(10));
    let result = fetch(&mut core, &room, &item, MediaSize::Full).await;
    assert!(
        matches!(result, Err(CoreError::Network(_))),
        "a body past its declared size is refused, got {result:?}"
    );

    // The same sender telling the truth is fetched whole.
    let honest = vec![0x5a_u8; 3 * 1024 * 1024];
    let honest_name = format!("honest-{}.bin", hex());
    send_attachment_raw(
        1,
        &room_id,
        &honest_name,
        "application/octet-stream",
        honest.clone(),
    )
    .await;
    pump(&mut core, &room, &mut rooms, &mut items, |_, i| {
        i.iter().any(|x| {
            matches!(&x.kind, TimelineItemKind::Message { details, .. }
                if details.media.as_ref().is_some_and(|m| m.filename == honest_name))
        })
    })
    .await;
    let honest_item = items
        .iter()
        .find(|x| {
            matches!(&x.kind, TimelineItemKind::Message { details, .. }
                if details.media.as_ref().is_some_and(|m| m.filename == honest_name))
        })
        .expect("arrived")
        .clone();
    let bytes = fetch(&mut core, &room, &honest_item, MediaSize::Full)
        .await
        .expect("an honest file");
    assert_eq!(bytes.as_bytes(), honest.as_slice());
    println!(
        "cap held: lying file refused, honest file {} bytes",
        bytes.len()
    );

    carol_raw.logout().await.expect("carol raw logout");
    core.commands.send(Command::Logout).await.unwrap();
    expect_event(&mut core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn encrypted_image_and_file_come_back_identical_and_save_to_disk() {
    let _serial = e2ee_serial().await;
    let mut p = EncryptedPair::new().await;
    let room = p.room();
    let file_bytes = random_bytes(300 * 1024);
    let file_name = format!("blob-{}.bin", hex());

    let alice_raw = raw_login(1).await;
    let image_event = send_attachment_with(
        &alice_raw,
        &p.room_id,
        "secret-name-x.png",
        "image/png",
        PHOTO.to_vec(),
    )
    .await;
    let file_event = send_attachment_with(
        &alice_raw,
        &p.room_id,
        &file_name,
        "application/octet-stream",
        file_bytes.clone(),
    )
    .await;
    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| {
            media_item(i, MediaKind::Image).is_some() && media_item(i, MediaKind::File).is_some()
        },
    )
    .await;

    let image = media_item(&p.carol_items, MediaKind::Image)
        .expect("image")
        .clone();
    let file = media_item(&p.carol_items, MediaKind::File)
        .expect("file")
        .clone();
    assert_eq!(media_of(&file).filename, file_name);
    assert_eq!(media_of(&file).size, Some(file_bytes.len() as u64));
    assert!(!media_of(&file).has_thumbnail);
    assert!(media_of(&image).has_thumbnail, "a small encrypted image");

    let got_image = fetch(&mut p.carol, &room, &image, MediaSize::Full)
        .await
        .expect("image bytes");
    assert_eq!(got_image.as_bytes(), PHOTO);
    let got_file = fetch(&mut p.carol, &room, &file, MediaSize::Full)
        .await
        .expect("file bytes");
    assert_eq!(got_file.as_bytes(), file_bytes.as_slice());
    // The preview of a small encrypted image is the decrypted file itself.
    let preview = fetch(&mut p.carol, &room, &image, MediaSize::Thumbnail)
        .await
        .expect("preview");
    assert_eq!(preview.as_bytes(), PHOTO);
    // No preview for a file without a thumbnail.
    assert!(
        fetch(&mut p.carol, &room, &file, MediaSize::Thumbnail)
            .await
            .is_err()
    );

    // On the server both events are encrypted, and the stored file is not the
    // original.
    for event in [&image_event, &file_event] {
        let json = fetch_raw_event_json(&alice_raw, &p.room_id, event).await;
        assert_eq!(json["type"], "m.room.encrypted");
        assert!(json["content"]["ciphertext"].is_string());
        assert!(json["content"].get("url").is_none());
    }
    // The media itself: read what the server stores for the file.
    let encrypted_url = encrypted_media_url(&alice_raw, &p, &file_event).await;
    let stored = download_raw(&alice_raw, &encrypted_url).await;
    assert_ne!(stored, file_bytes, "the server holds ciphertext");
    assert_eq!(stored.len() % 16, 0, "a block cipher output size");
    println!(
        "encrypted: image {} bytes, file {} bytes identical",
        got_image.len(),
        got_file.len()
    );

    // Save the file into a temp directory.
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("saved.bin");
    p.carol
        .commands
        .send(Command::SaveMedia {
            room_id: RoomId::from(room.as_str()),
            item_id: file.id.clone(),
            path: path.clone(),
        })
        .await
        .unwrap();
    let saved = expect_event(&mut p.carol, |e| {
        matches!(e, CoreEvent::MediaSaved { .. })
            || matches!(
                e,
                CoreEvent::ActionFailed {
                    action: ActionKind::Media,
                    ..
                }
            )
    })
    .await;
    assert!(matches!(saved, CoreEvent::MediaSaved { .. }), "{saved:?}");
    assert_eq!(std::fs::read(&path).unwrap(), file_bytes);
    let names: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["saved.bin"], "no part file is left");

    // A save to a place that cannot be written fails and leaves nothing.
    p.carol
        .commands
        .send(Command::SaveMedia {
            room_id: RoomId::from(room.as_str()),
            item_id: file.id.clone(),
            path: out.path().join("missing-dir").join("x.bin"),
        })
        .await
        .unwrap();
    let failed = expect_event(&mut p.carol, |e| {
        matches!(
            e,
            CoreEvent::MediaSaved { .. } | CoreEvent::ActionFailed { .. }
        )
    })
    .await;
    assert!(
        matches!(
            failed,
            CoreEvent::ActionFailed {
                action: ActionKind::Media,
                ..
            }
        ),
        "{failed:?}"
    );
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 1);

    alice_raw.logout().await.expect("alice raw logout");
    p.finish().await;
}

/// The `file.url` of the encrypted attachment in `event`. The raw client of
/// the sender made the room key, so it can read its own event.
async fn encrypted_media_url(
    alice: &matrix_sdk::Client,
    p: &EncryptedPair,
    event: &matrix_sdk::ruma::EventId,
) -> matrix_sdk::ruma::OwnedMxcUri {
    let timeline_event = alice
        .get_room(&p.room_id)
        .expect("alice knows the room")
        .event(event, None)
        .await
        .expect("alice reads the event");
    let value: serde_json::Value =
        serde_json::from_str(timeline_event.raw().json().get()).expect("event JSON");
    let url = value["content"]["file"]["url"]
        .as_str()
        .expect("a decrypted attachment has a file url")
        .to_owned();
    url.into()
}

/// Downloads media bytes straight from the server.
async fn download_raw(client: &matrix_sdk::Client, uri: &matrix_sdk::ruma::MxcUri) -> Vec<u8> {
    use matrix_sdk::ruma::api::client::authenticated_media::get_content::v1::Request;
    client
        .send(Request::from_uri(uri).expect("a valid mxc uri"))
        .await
        .expect("download media")
        .file
}
