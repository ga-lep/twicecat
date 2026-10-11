//! Media in the timeline against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`): received
//! pictures and files are mapped, fetched, decrypted and saved.
//!
//! Prints only counts. The download tests upload with a raw SDK client; the
//! upload tests send through alice's core and read the result with carol's.

mod common;

use std::sync::Arc;

use common::{
    EncryptedPair, TestEnv, create_room_raw, e2ee_serial, expect_event, fetch_raw_event_json, hex,
    login_and_run, pump, pump_recording, raw_login, send_attachment_raw, send_attachment_with,
    send_file_declaring_size, start_core, start_core_with_upload_limit,
};
use twicecat_core::CoreHandle;
use twicecat_core::api::{
    ActionKind, AttachmentData, Command, CoreError, CoreEvent, ListDiff, MediaBytes, MediaContent,
    MediaKind, MediaSize, OutgoingAttachment, RoomId, RoomSummary, SendStatus, TimelineItem,
    TimelineItemKind, UploadThumbnail,
};
use twicecat_core::secrets::InMemorySecretStore;

const PHOTO: &[u8] = include_bytes!("fixtures/photo-1200x900.png");
const THUMB: &[u8] = include_bytes!("fixtures/small-64x48.png");

/// The first media message of `items` that is not the user's own.
fn media_item(items: &[TimelineItem], kind: MediaKind) -> Option<&TimelineItem> {
    items.iter().find(|i| {
        matches!(&i.kind, TimelineItemKind::Message { details, .. }
            if details.media.as_ref().is_some_and(|m| m.kind == kind))
    })
}

fn media_of(item: &TimelineItem) -> &twicecat_core::api::MediaContent {
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
            password: twicecat_core::api::Password::from(env.password(n)),
            server: None,
        })
        .await
        .unwrap();
    let (mut running, mut limit) = (false, None);
    let wait = async {
        while !running || limit.is_none() {
            match core.events.recv().await {
                Some(CoreEvent::SyncState(twicecat_core::api::SyncState::Running)) => {
                    running = true
                }
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

/// Two cores in one room: alice sends, carol receives.
struct Pair {
    room_id: matrix_sdk::ruma::OwnedRoomId,
    alice: CoreHandle,
    carol: CoreHandle,
    alice_rooms: Vec<RoomSummary>,
    carol_rooms: Vec<RoomSummary>,
    alice_items: Vec<TimelineItem>,
    carol_items: Vec<TimelineItem>,
    _dirs: Vec<tempfile::TempDir>,
}

impl Pair {
    fn room(&self) -> String {
        self.room_id.to_string()
    }

    /// The pair of the encrypted-room helper.
    fn encrypted(p: EncryptedPair) -> Self {
        Self {
            room_id: p.room_id,
            alice: p.alice,
            carol: p.carol,
            alice_rooms: p.alice_rooms,
            carol_rooms: p.carol_rooms,
            alice_items: p.alice_items,
            carol_items: p.carol_items,
            _dirs: p._dirs.into(),
        }
    }

    /// A fresh plain room with alice's core and carol's core in it. Alice's
    /// core sees `alice_limit` as the server's upload limit when given.
    async fn plain(alice_limit: Option<u64>) -> Self {
        let env = TestEnv::load();
        let room_id =
            create_room_raw(1, &common::unique_room_name(), false, &[&env.user_id(3)]).await;
        let carol_raw = raw_login(3).await;
        carol_raw
            .join_room_by_id(&room_id)
            .await
            .expect("carol joins");
        carol_raw.logout().await.expect("carol raw logout");
        let dirs = vec![tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let secrets = || Arc::new(InMemorySecretStore::new());
        let mut alice = match alice_limit {
            Some(limit) => start_core_with_upload_limit(dirs[0].path(), secrets(), limit),
            None => start_core(dirs[0].path(), secrets()),
        };
        let mut carol = start_core(dirs[1].path(), secrets());
        login_and_run(&mut alice, &env, 1).await;
        login_and_run(&mut carol, &env, 3).await;
        let room = room_id.to_string();
        let mut pair = Self {
            room_id,
            alice,
            carol,
            alice_rooms: Vec::new(),
            carol_rooms: Vec::new(),
            alice_items: Vec::new(),
            carol_items: Vec::new(),
            _dirs: dirs,
        };
        pump(
            &mut pair.alice,
            &room,
            &mut pair.alice_rooms,
            &mut pair.alice_items,
            |r, _| r.iter().any(|x| x.id.as_str() == room),
        )
        .await;
        pump(
            &mut pair.carol,
            &room,
            &mut pair.carol_rooms,
            &mut pair.carol_items,
            |r, _| r.iter().any(|x| x.id.as_str() == room),
        )
        .await;
        for core in [&mut pair.alice, &mut pair.carol] {
            core.commands
                .send(Command::SelectRoom(RoomId::from(room.as_str())))
                .await
                .unwrap();
        }
        pair
    }

    async fn finish(mut self) {
        for core in [&mut self.alice, &mut self.carol] {
            core.commands.send(Command::Logout).await.unwrap();
            expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
        }
    }
}

/// The message with this file name, its media and its send status.
fn find_named<'a>(
    items: &'a [TimelineItem],
    name: &str,
) -> Option<(&'a TimelineItem, &'a MediaContent, SendStatus)> {
    items.iter().find_map(|i| match &i.kind {
        TimelineItemKind::Message { details, sent, .. } => details
            .media
            .as_deref()
            .filter(|m| m.filename == name)
            .map(|m| (i, m, *sent)),
        _ => None,
    })
}

fn outgoing_file(room: &str, name: &str, data: AttachmentData) -> OutgoingAttachment {
    OutgoingAttachment {
        room_id: RoomId::from(room),
        filename: name.to_owned(),
        mimetype: "application/octet-stream".into(),
        data,
        kind: MediaKind::File,
        width: None,
        height: None,
        thumbnail: None,
        reply_to: None,
        thread_root: None,
    }
}

async fn send_attachment(core: &mut CoreHandle, attachment: OutgoingAttachment) {
    core.commands
        .send(Command::SendAttachment(attachment))
        .await
        .unwrap();
}

/// Alice sends a picture with a thumbnail and a random file through her core;
/// carol's core fetches both.
async fn upload_round_trip(p: &mut Pair, encrypted: bool) {
    let room = p.room();
    let photo_name = format!("photo-{}.png", hex());
    let file_name = format!("blob-{}.bin", hex());
    let file_bytes = random_bytes(300 * 1024);
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("source.bin");
    std::fs::write(&file_path, &file_bytes).unwrap();

    send_attachment(
        &mut p.alice,
        OutgoingAttachment {
            kind: MediaKind::Image,
            mimetype: "image/png".into(),
            width: Some(1200),
            height: Some(900),
            thumbnail: Some(UploadThumbnail {
                data: MediaBytes::from(THUMB.to_vec()),
                mimetype: "image/png".into(),
                width: 64,
                height: 48,
            }),
            ..outgoing_file(
                &room,
                &photo_name,
                AttachmentData::Bytes(MediaBytes::from(PHOTO.to_vec())),
            )
        },
    )
    .await;
    send_attachment(
        &mut p.alice,
        outgoing_file(&room, &file_name, AttachmentData::Path(file_path)),
    )
    .await;

    // The local echo carries the media before the message is sent.
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| {
            find_named(i, &photo_name).is_some_and(|(_, m, sent)| {
                sent == SendStatus::Sending
                    && m.kind == MediaKind::Image
                    && m.size == Some(PHOTO.len() as u64)
                    && (m.width, m.height) == (Some(1200), Some(900))
            })
        },
    )
    .await;
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| {
            [&photo_name, &file_name]
                .iter()
                .all(|n| find_named(i, n).is_some_and(|(_, _, sent)| sent == SendStatus::Sent))
        },
    )
    .await;

    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| find_named(i, &photo_name).is_some() && find_named(i, &file_name).is_some(),
    )
    .await;
    let photo = find_named(&p.carol_items, &photo_name).unwrap().0.clone();
    let file = find_named(&p.carol_items, &file_name).unwrap().0.clone();
    assert_eq!(media_of(&file).size, Some(file_bytes.len() as u64));
    assert!(media_of(&photo).has_thumbnail);

    let full = fetch(&mut p.carol, &room, &photo, MediaSize::Full)
        .await
        .expect("full picture");
    assert_eq!(full.as_bytes(), PHOTO);
    let got = fetch(&mut p.carol, &room, &file, MediaSize::Full)
        .await
        .expect("full file");
    assert_eq!(got.as_bytes(), file_bytes.as_slice());
    let thumb = fetch(&mut p.carol, &room, &photo, MediaSize::Thumbnail)
        .await
        .expect("thumbnail");
    if encrypted {
        // The SDK encrypted the thumbnail and decrypts it again: the same bytes.
        assert_eq!(thumb.as_bytes(), THUMB);
    } else {
        assert!(thumb.as_bytes().starts_with(b"\x89PNG\r\n\x1a\n"));
    }
    println!(
        "{} upload: picture {} bytes, thumbnail {} bytes, file {} bytes",
        if encrypted { "encrypted" } else { "plain" },
        full.len(),
        thumb.len(),
        got.len()
    );
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn core_uploads_round_trip_in_a_plain_room() {
    let _serial = e2ee_serial().await;
    let mut p = Pair::plain(None).await;
    upload_round_trip(&mut p, false).await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn core_uploads_round_trip_in_an_encrypted_room() {
    let _serial = e2ee_serial().await;
    let mut p = Pair::encrypted(EncryptedPair::new().await);
    upload_round_trip(&mut p, true).await;
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn upload_progress_reaches_the_local_echo() {
    let _serial = e2ee_serial().await;
    let mut p = Pair::plain(None).await;
    let room = p.room();
    let name = format!("big-{}.bin", hex());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.bin");
    std::fs::write(&path, random_bytes(4 * 1024 * 1024)).unwrap();
    send_attachment(
        &mut p.alice,
        outgoing_file(&room, &name, AttachmentData::Path(path)),
    )
    .await;

    let mut recorded = Vec::new();
    pump_recording(
        &mut p.alice,
        &room,
        common::DEFAULT_TIMEOUT,
        &mut p.alice_items,
        &mut recorded,
        |_| {},
        |i| find_named(i, &name).is_some_and(|(_, _, sent)| sent == SendStatus::Sent),
    )
    .await;
    let mut shown = Vec::new();
    for diff in &recorded {
        let items: Vec<&TimelineItem> = match diff {
            ListDiff::Append { values } => values.iter().collect(),
            ListDiff::PushFront { value }
            | ListDiff::PushBack { value }
            | ListDiff::Insert { value, .. }
            | ListDiff::Set { value, .. } => vec![value],
            _ => Vec::new(),
        };
        for item in items {
            if let TimelineItemKind::Message { details, .. } = &item.kind
                && let Some(m) = details.media.as_deref()
                && m.filename == name
                && let Some(up) = m.upload
            {
                shown.push(up);
            }
        }
    }
    assert!(
        shown.iter().any(|u| u.sent < u.total),
        "no partial progress in {} updates",
        shown.len()
    );
    println!("{} progress updates seen", shown.len());
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn a_file_over_the_advertised_limit_is_refused_and_nothing_is_sent() {
    let _serial = e2ee_serial().await;
    let mut p = Pair::plain(Some(10_000)).await;
    let room = p.room();
    let (big, small) = (format!("big-{}.bin", hex()), format!("small-{}.bin", hex()));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.bin");
    std::fs::write(&path, vec![7u8; 10_001]).unwrap();

    // Bytes and a path are both held to the limit.
    let sources = [
        AttachmentData::Path(path),
        AttachmentData::Bytes(MediaBytes::from(vec![7u8; 10_001])),
    ];
    for data in sources {
        send_attachment(&mut p.alice, outgoing_file(&room, &big, data)).await;
        let failed = expect_event(&mut p.alice, |e| {
            matches!(e, CoreEvent::ActionFailed { .. })
        })
        .await;
        assert!(
            matches!(
                failed,
                CoreEvent::ActionFailed {
                    action: ActionKind::Upload,
                    error: CoreError::NotAllowed,
                    ..
                }
            ),
            "{failed:?}"
        );
    }
    // A file at the limit still goes through, behind the refused ones.
    send_attachment(
        &mut p.alice,
        outgoing_file(
            &room,
            &small,
            AttachmentData::Bytes(MediaBytes::from(vec![1u8; 10_000])),
        ),
    )
    .await;
    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| find_named(i, &small).is_some(),
    )
    .await;
    assert!(
        find_named(&p.carol_items, &big).is_none(),
        "nothing was sent"
    );
    assert!(find_named(&p.alice_items, &big).is_none(), "no local echo");
    p.finish().await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn discarding_an_echo_queued_behind_a_running_upload_removes_it() {
    let _serial = e2ee_serial().await;
    let mut p = Pair::plain(None).await;
    let room = p.room();
    let (first, second) = (
        format!("first-{}.bin", hex()),
        format!("second-{}.bin", hex()),
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("first.bin");
    std::fs::write(&path, random_bytes(16 * 1024 * 1024)).unwrap();
    send_attachment(
        &mut p.alice,
        outgoing_file(&room, &first, AttachmentData::Path(path)),
    )
    .await;
    send_attachment(
        &mut p.alice,
        outgoing_file(
            &room,
            &second,
            AttachmentData::Bytes(MediaBytes::from(vec![5u8; 2048])),
        ),
    )
    .await;
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| find_named(i, &second).is_some_and(|(_, _, sent)| sent == SendStatus::Sending),
    )
    .await;
    let echo = find_named(&p.alice_items, &second).unwrap().0.clone();
    p.alice
        .commands
        .send(Command::DiscardSend {
            room_id: RoomId::from(room.as_str()),
            item_id: echo.id,
        })
        .await
        .unwrap();
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| find_named(i, &second).is_none(),
    )
    .await;
    // The first upload completes and the second never arrives.
    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| find_named(i, &first).is_some(),
    )
    .await;
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| find_named(i, &first).is_some_and(|(_, _, sent)| sent == SendStatus::Sent),
    )
    .await;
    assert!(find_named(&p.alice_items, &second).is_none());
    assert!(find_named(&p.carol_items, &second).is_none());
    p.finish().await;
}
