//! Phase 4, Q16 (a) and (b): no secret reaches the logs or the disk in clear.
//!
//! Runs a full encrypted flow with alice and carol against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`) under a
//! global capturing subscriber, then scans the captured log text and every file
//! under the cores' data roots for the sentinels: access tokens, refresh
//! tokens, store passphrases, test passwords, the recovery key and message
//! bodies.
//!
//! This test prints only counts, sentinel KINDS and file NAMES. It never
//! prints a secret, a log line or file content, and no assertion message
//! formats one. If a check finds a leak it fails with the kind and the
//! location only; the SDK's output is never filtered.

mod common;

use std::cell::{Cell, RefCell};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    DEFAULT_TIMEOUT, EncryptedPair, TestEnv, e2ee_serial, expect_event, hex, login_and_run,
    message_with, outgoing, pump, pump_with, start_core,
};
use tessera_core::CoreHandle;
use tessera_core::api::{
    Command, CoreError, CoreEvent, CrossSigning, DeviceVerification, Password, RecoveryKey,
    RecoveryStatus, SendStatus, TimelineItemKind, VerificationPhase,
};
use tessera_core::logging::DEFAULT_FILTER;
use tessera_core::secrets::{InMemorySecretStore, SecretStore as _};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _};
use zeroize::Zeroizing;

/// An in-memory log sink.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().expect("capture lock").clone()
    }
}

impl std::io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("capture lock").extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl MakeWriter<'_> for Capture {
    type Writer = Capture;

    fn make_writer(&self) -> Capture {
        self.clone()
    }
}

/// A secret to look for. `kind` is the only thing ever printed about it.
struct Sentinel {
    kind: &'static str,
    /// The UTF-8 bytes, and the JSON-escaped form when it differs.
    needles: Vec<Zeroizing<Vec<u8>>>,
}

impl Sentinel {
    fn new(kind: &'static str, value: &str) -> Self {
        // A short value would match by chance and prove nothing.
        assert!(value.len() >= 8, "the {kind} sentinel is too short to scan");
        let mut needles = vec![Zeroizing::new(value.as_bytes().to_vec())];
        let json = serde_json::to_vec(value).expect("a string serializes");
        // Without the surrounding quotes.
        let escaped = &json[1..json.len() - 1];
        if escaped != value.as_bytes() {
            needles.push(Zeroizing::new(escaped.to_vec()));
        }
        Self { kind, needles }
    }
}

/// The kinds of the sentinels found in `haystack`.
fn scan(haystack: &[u8], sentinels: &[Sentinel]) -> Vec<&'static str> {
    sentinels
        .iter()
        .filter(|s| {
            s.needles
                .iter()
                .any(|n| haystack.windows(n.len()).any(|w| w == n.as_slice()))
        })
        .map(|s| s.kind)
        .collect()
}

/// Every file under `dir`, recursively.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![dir.to_owned()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read the data dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Scans every file under each root. Returns the number of files and bytes
/// scanned and the hits as `(kind, file name)`.
fn scan_data_roots(
    roots: &[&Path],
    sentinels: &[Sentinel],
) -> (usize, usize, Vec<(&'static str, String)>) {
    let (mut files, mut bytes, mut hits) = (0, 0, Vec::new());
    for root in roots {
        for path in files_under(root) {
            let content = std::fs::read(&path).expect("read a store file");
            files += 1;
            bytes += content.len();
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            for kind in scan(&content, sentinels) {
                hits.push((kind, name.clone()));
            }
        }
    }
    (files, bytes, hits)
}

/// Proves the scanner and the capture work, so a silent capture cannot pass:
/// a sentinel logged through `tracing::info!` into a separate, discarded buffer
/// and one written to a scratch file must both be found, and an unrelated
/// sentinel must not be.
fn scanner_self_check() {
    let value = format!("self-check-sentinel-{}", hex());
    let sentinels = [Sentinel::new("self-check", &value)];
    let unrelated = [Sentinel::new(
        "unrelated",
        &format!("unrelated-sentinel-{}", hex()),
    )];

    let discarded = Capture::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(discarded.clone())
            .with_ansi(false)
            .with_filter(EnvFilter::new("info")),
    );
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(target: "tessera_core", "self-check: {value}");
    });
    let logged = discarded.bytes();
    assert_eq!(
        scan(&logged, &sentinels),
        ["self-check"],
        "the capture or the scanner missed the self-check sentinel"
    );
    assert!(scan(&logged, &unrelated).is_empty());

    let scratch = tempfile::tempdir().expect("scratch dir");
    std::fs::create_dir(scratch.path().join("nested")).expect("scratch subdir");
    let mut file =
        std::fs::File::create(scratch.path().join("nested/file.db-wal")).expect("scratch file");
    file.write_all(b"\0\0header\0").expect("write");
    file.write_all(value.as_bytes()).expect("write");
    drop(file);
    let (files, _, hits) = scan_data_roots(&[scratch.path()], &sentinels);
    assert_eq!(files, 1);
    assert_eq!(hits, [("self-check", "nested/file.db-wal".to_owned())]);
    let (_, _, hits) = scan_data_roots(&[scratch.path()], &unrelated);
    assert!(hits.is_empty());
    println!("log hygiene: scanner self-check ok");
}

/// Installs the global subscriber: two layers over one flow, one at `info`
/// (everything, stricter than the default) and one at [`DEFAULT_FILTER`].
fn install_capture() -> (Capture, Capture) {
    let (info, default) = (Capture::default(), Capture::default());
    let layer = |capture: &Capture, filter: &str| {
        tracing_subscriber::fmt::layer()
            .with_writer(capture.clone())
            .with_ansi(false)
            .with_filter(EnvFilter::new(filter))
            .boxed()
    };
    let layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync>,
    > = vec![layer(&info, "info"), layer(&default, DEFAULT_FILTER)];
    tracing_subscriber::registry()
        .with(layers)
        .try_init()
        .expect("no other global subscriber in this test binary");
    (info, default)
}

/// Collects the string values stored under `key` anywhere in `value`.
fn collect_strings(value: &serde_json::Value, key: &str, out: &mut Vec<Zeroizing<String>>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                match v {
                    serde_json::Value::String(s) if k == key => {
                        out.push(Zeroizing::new(s.clone()));
                    }
                    _ => collect_strings(v, key, out),
                }
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_strings(v, key, out);
            }
        }
        _ => {}
    }
}

/// The token and passphrase sentinels from the session record `secrets` holds
/// for `user_id`. Fails if the record or a required field is missing.
fn record_sentinels(secrets: &InMemorySecretStore, user_id: &str) -> Vec<Sentinel> {
    let bytes = secrets
        .get(user_id)
        .expect("read the secret store")
        .expect("a stored session record");
    let record: serde_json::Value = serde_json::from_slice(&bytes).expect("the record is JSON");
    let mut out = Vec::new();
    for (kind, key, required) in [
        ("access token", "access_token", true),
        ("refresh token", "refresh_token", false),
        ("store passphrase", "store_passphrase", true),
    ] {
        let mut values = Vec::new();
        collect_strings(&record, key, &mut values);
        assert!(
            !required || !values.is_empty(),
            "no {kind} in the stored record"
        );
        out.extend(values.iter().map(|v| Sentinel::new(kind, v)));
    }
    out
}

async fn send(core: &mut CoreHandle, command: Command) {
    core.commands.send(command).await.expect("send a command");
}

/// Waits for the emoji (`emoji`) or the end (`!emoji`) of flow `id`.
async fn phase(core: &mut CoreHandle, id: &tessera_core::api::VerificationId, emoji: bool) {
    expect_event(core, |e| {
        matches!(e, CoreEvent::Verification { id: i, phase }
        if i == id && if emoji {
            matches!(phase, VerificationPhase::Emojis(_))
        } else {
            matches!(phase, VerificationPhase::Done)
        })
    })
    .await;
}

async fn logout(core: &mut CoreHandle) {
    send(core, Command::Logout).await;
    expect_event(core, |e| matches!(e, CoreEvent::LoggedOut)).await;
}

#[tokio::test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
async fn no_secret_reaches_the_logs_or_the_disk() {
    scanner_self_check();
    let (info_log, default_log) = install_capture();

    let _serial = e2ee_serial().await;
    let env = TestEnv::load();
    let mut p = EncryptedPair::new().await;
    let room = p.room();

    // Each core looks the other user up first: the SDK needs the peer's device
    // keys before a send or a verification can go through (Task 6 notes).
    for (core, rooms, items, peer) in [
        (
            &mut p.alice,
            &mut p.alice_rooms,
            &mut p.alice_items,
            env.user_id(3),
        ),
        (
            &mut p.carol,
            &mut p.carol_rooms,
            &mut p.carol_items,
            env.user_id(1),
        ),
    ] {
        let peer = tessera_core::api::UserId::from(peer.as_str());
        send(
            core,
            Command::WatchUserTrust {
                user_id: peer.clone(),
            },
        )
        .await;
        let seen = Cell::new(false);
        pump_with(
            core,
            &room,
            DEFAULT_TIMEOUT,
            rooms,
            items,
            |e| {
                if matches!(e, CoreEvent::UserTrust { user_id, .. } if *user_id == peer) {
                    seen.set(true)
                }
            },
            |_, _| seen.get(),
        )
        .await;
    }

    // Sentinel bodies, both ways, in the encrypted room.
    println!("log hygiene: step bodies");
    let from_alice = format!("secret-body-{}", hex());
    let from_carol = format!("secret-body-{}", hex());
    send(&mut p.alice, Command::Send(outgoing(&room, &from_alice))).await;
    send(&mut p.carol, Command::Send(outgoing(&room, &from_carol))).await;
    let sent = |items: &[tessera_core::api::TimelineItem], body: &str, own: bool| {
        matches!(
            message_with(items, body, own).map(|m| &m.kind),
            Some(TimelineItemKind::Message {
                sent: SendStatus::Sent,
                ..
            })
        ) || (!own && message_with(items, body, false).is_some())
    };
    pump(
        &mut p.alice,
        &room,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |_, i| sent(i, &from_alice, true) && sent(i, &from_carol, false),
    )
    .await;
    pump(
        &mut p.carol,
        &room,
        &mut p.carol_rooms,
        &mut p.carol_items,
        |_, i| sent(i, &from_carol, true) && sent(i, &from_alice, false),
    )
    .await;

    // Alice's core 1 becomes the account's first device (password reset).
    // The reset runs through a pump so the open room's diffs are not lost.
    println!("log hygiene: step reset");
    send(
        &mut p.alice,
        Command::ResetIdentity {
            password: Some(Password::from(env.password(1))),
        },
    )
    .await;
    let reset_done = Cell::new(false);
    pump_with(
        &mut p.alice,
        &room,
        DEFAULT_TIMEOUT,
        &mut p.alice_rooms,
        &mut p.alice_items,
        |e| {
            if let CoreEvent::Security(s) = e
                && s.cross_signing == CrossSigning::Complete
                && s.device == DeviceVerification::Verified
                && s.recovery == RecoveryStatus::Disabled
            {
                reset_done.set(true);
            }
        },
        |_, _| reset_done.get(),
    )
    .await;

    // Recovery on alice's core 1; the key reaches the test once. `Busy` (the
    // reset task is still ending) is retried.
    println!("log hygiene: step recovery");
    let recovery_key: RefCell<Option<Zeroizing<String>>> = RefCell::new(None);
    loop {
        let enabled = Cell::new(false);
        let busy = Cell::new(false);
        send(&mut p.alice, Command::EnableRecovery).await;
        pump_with(
            &mut p.alice,
            &room,
            Duration::from_secs(60),
            &mut p.alice_rooms,
            &mut p.alice_items,
            |e| match e {
                CoreEvent::RecoveryKeyReady(k) => {
                    *recovery_key.borrow_mut() = Some(Zeroizing::new(k.expose().to_owned()));
                }
                CoreEvent::Security(s) if s.recovery == RecoveryStatus::Enabled => {
                    enabled.set(true);
                }
                CoreEvent::ActionFailed {
                    error: CoreError::Busy,
                    ..
                } => busy.set(true),
                CoreEvent::ActionFailed { action, .. } => {
                    panic!("enabling recovery failed ({action:?})")
                }
                _ => {}
            },
            |_, _| busy.get() || (recovery_key.borrow().is_some() && enabled.get()),
        )
        .await;
        if !busy.get() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let recovery_key = recovery_key.take().expect("the recovery key arrived");

    println!("log hygiene: step second device");
    // Alice's second device logs in, is verified by SAS against core 1, then
    // restores the secrets with the recovery key.
    let dir2 = tempfile::tempdir().expect("data dir");
    let secrets2 = Arc::new(InMemorySecretStore::new());
    let mut second = start_core(dir2.path(), secrets2.clone());
    login_and_run(&mut second, &env, 1).await;
    common::expect_security(&mut second, |s| {
        s.device == DeviceVerification::Unverified && s.has_devices_to_verify_against
    })
    .await;
    send(&mut second, Command::RequestSelfVerification).await;
    let id = match expect_event(&mut p.alice, |e| {
        matches!(e, CoreEvent::VerificationRequested { .. })
    })
    .await
    {
        CoreEvent::VerificationRequested { id, .. } => id,
        _ => unreachable!("the predicate only accepts VerificationRequested"),
    };
    send(&mut p.alice, Command::AcceptVerification { id: id.clone() }).await;
    tokio::join!(
        phase(&mut p.alice, &id, true),
        phase(&mut second, &id, true)
    );
    send(&mut p.alice, Command::ConfirmSas { id: id.clone() }).await;
    send(&mut second, Command::ConfirmSas { id: id.clone() }).await;
    tokio::join!(
        phase(&mut p.alice, &id, false),
        phase(&mut second, &id, false)
    );
    send(
        &mut second,
        Command::Recover {
            key: RecoveryKey::from(recovery_key.as_str()),
        },
    )
    .await;
    expect_event(&mut second, |e| matches!(e, CoreEvent::Recovered)).await;

    println!("log hygiene: step rename");
    // Device rename on core 2.
    send(&mut second, Command::ListDevices).await;
    let own = match expect_event(
        &mut second,
        |e| matches!(e, CoreEvent::Devices(d) if d.iter().any(|x| x.current)),
    )
    .await
    {
        CoreEvent::Devices(d) => d
            .into_iter()
            .find(|x| x.current)
            .expect("the current device"),
        _ => unreachable!("the predicate only accepts Devices"),
    };
    send(
        &mut second,
        Command::RenameDevice {
            device_id: own.device_id,
            name: "log hygiene device".into(),
        },
    )
    .await;
    expect_event(&mut second, |e| {
        matches!(e, CoreEvent::Devices(d)
            if d.iter().any(|x| x.current && x.display_name.as_deref() == Some("log hygiene device")))
    })
    .await;

    // The sentinels. The stored records are read before logout deletes them.
    let mut sentinels = Vec::new();
    sentinels.extend(record_sentinels(&p.secrets[0], &env.user_id(1)));
    sentinels.extend(record_sentinels(&p.secrets[1], &env.user_id(3)));
    sentinels.extend(record_sentinels(&secrets2, &env.user_id(1)));
    sentinels.push(Sentinel::new("alice password", env.password(1)));
    sentinels.push(Sentinel::new("carol password", env.password(3)));
    sentinels.push(Sentinel::new("recovery key", &recovery_key));
    let compact: String = recovery_key.split_whitespace().collect();
    sentinels.push(Sentinel::new("recovery key", &compact));
    sentinels.push(Sentinel::new("message body", &from_alice));
    sentinels.push(Sentinel::new("message body", &from_carol));
    println!("log hygiene: {} sentinel values", sentinels.len());

    // (b) Data at rest, while the sessions are live (the SQLite -wal and -shm
    // files are scanned too).
    let roots = [p._dirs[0].path(), p._dirs[1].path(), dir2.path()];
    let (files, bytes, hits) = scan_data_roots(&roots, &sentinels);
    println!("log hygiene: scanned {files} files, {bytes} bytes under 3 data roots");
    assert!(files >= 3, "the stores were not found on disk");
    assert!(
        hits.is_empty(),
        "secrets found on disk (kind, file): {hits:?}"
    );

    logout(&mut second).await;
    logout(&mut p.alice).await;
    logout(&mut p.carol).await;

    // (a) Logs, after the whole flow including the logouts.
    for (name, log) in [("info", &info_log), ("default filter", &default_log)] {
        let text = log.bytes();
        let hits = scan(&text, &sentinels);
        println!(
            "log hygiene: {name}: {} bytes, {} lines captured",
            text.len(),
            text.iter().filter(|b| **b == b'\n').count()
        );
        assert!(
            hits.is_empty(),
            "secrets found in the {name} log (kinds): {hits:?}"
        );
    }
    assert!(
        !info_log.bytes().is_empty(),
        "nothing was captured at info; the capture is broken"
    );
}
