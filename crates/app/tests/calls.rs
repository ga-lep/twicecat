//! A call between `twicecat-cli` (alice) and the app on the real bridge
//! (carol), against the local homeserver and call service
//! (`scripts/stack.sh up -d --wait && scripts/create-test-users.sh`; the CLI
//! binary is built by `cargo test --workspace` or `cargo build -p
//! twicecat-core --bin twicecat-cli`). Plan Task 5: the ring reaches the
//! app's card; Decline tells the caller; Accept joins and both sides see
//! each other; Leave ends it.
//!
//! Audio is virtual and silent on both sides. Nothing here prints
//! passwords, tokens, or message bodies.

use std::collections::HashMap;
use std::io::{BufRead as _, Read as _, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use iced::futures::StreamExt as _;
use twicecat_app::bridge::{BridgeEvent, events_stream};
use twicecat_app::calls::{CallMessage, CallPhase};
use twicecat_app::{App, Bridge, Message, Screen, Startup};
use twicecat_core::CoreConfig;
use twicecat_core::api::{AUDIO_FRAME_LEN, AudioBackend, RoomId, VirtualAudio};
use twicecat_core::paths::DataPaths;
use twicecat_core::secrets::InMemorySecretStore;

#[allow(dead_code)]
#[path = "../../core/tests/common/stack_env.rs"]
mod stack_env;

struct TestEnv(HashMap<String, String>);

impl TestEnv {
    fn load() -> Self {
        let values = stack_env::load();
        let env = Self(values);
        assert!(
            is_local_homeserver(env.get("HOMESERVER_URL")),
            "HOMESERVER_URL must be http on a loopback address with a port; \
             the test accounts must never be used against another server"
        );
        env
    }

    fn get(&self, key: &str) -> &str {
        self.0
            .get(key)
            .unwrap_or_else(|| panic!("{key} missing from .env.test"))
    }

    fn user(&self, n: u8) -> &str {
        self.get(&format!("TEST_USER_{n}"))
    }

    fn password(&self, n: u8) -> &str {
        self.get(&format!("TEST_PASSWORD_{n}"))
    }

    fn user_id(&self, n: u8) -> String {
        format!("@{}:{}", self.user(n), self.get("HOMESERVER_DOMAIN"))
    }
}

/// Whether `url` is plain http on a loopback address with a port (AGENTS.md
/// rule 6).
fn is_local_homeserver(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| {
        rest.strip_prefix(host)
            .and_then(|r| r.strip_prefix(':'))
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// One plain-HTTP JSON request to the local homeserver; returns the body.
fn http_json(base: &str, method: &str, path: &str, token: Option<&str>, body: &str) -> String {
    assert!(is_local_homeserver(base));
    let authority = base
        .strip_prefix("http://")
        .expect("plain http")
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(authority).expect("connect to the local homeserver");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("set read timeout");
    let auth = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.0\r\nHost: {authority}\r\n{auth}Content-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("send the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");
    assert!(
        response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200"),
        "{method} request failed: {}",
        response.lines().next().unwrap_or("")
    );
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default()
}

/// The string value of `key` in a flat JSON body.
fn json_string(body: &str, key: &str) -> String {
    body.split(&format!("\"{key}\""))
        .nth(1)
        .and_then(|rest| rest.split('"').nth(1))
        .unwrap_or_else(|| panic!("no {key} in the response"))
        .to_owned()
}

/// `!`, `:` and `@` escaped for a URL path.
fn path_part(id: &str) -> String {
    id.replace('!', "%21")
        .replace(':', "%3A")
        .replace('@', "%40")
}

/// A short-lived raw login; the values come from `.env.test` (no quotes or
/// backslashes).
fn login_token(base: &str, user: &str, password: &str) -> String {
    assert!(!user.contains(['"', '\\']) && !password.contains(['"', '\\']));
    let body = format!(
        r#"{{"type":"m.login.password","identifier":{{"type":"m.id.user","user":"{user}"}},"password":"{password}"}}"#
    );
    json_string(
        &http_json(base, "POST", "/_matrix/client/v3/login", None, &body),
        "access_token",
    )
}

fn logout(base: &str, token: &str) {
    http_json(base, "POST", "/_matrix/client/v3/logout", Some(token), "{}");
}

fn random_tag() -> String {
    use std::hash::{BuildHasher as _, Hasher as _};
    let id = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{id:016x}")
}

/// A new direct message from alice to carol, joined by both, unencrypted
/// (call encryption is covered by the core's suites). Alice's `m.direct`
/// names it, so her call rings (`ring: true`) rather than notifying.
fn direct_room(env: &TestEnv, name: &str) -> String {
    let base = env.get("HOMESERVER_URL");
    let alice = login_token(base, env.user(1), env.password(1));
    let carol = login_token(base, env.user(3), env.password(3));
    let carol_id = env.user_id(3);
    let room = json_string(
        &http_json(
            base,
            "POST",
            "/_matrix/client/v3/createRoom",
            Some(&alice),
            &format!(
                r#"{{"name":"{name}","preset":"trusted_private_chat","is_direct":true,"invite":["{carol_id}"]}}"#
            ),
        ),
        "room_id",
    );
    http_json(
        base,
        "PUT",
        &format!(
            "/_matrix/client/v3/user/{}/account_data/m.direct",
            path_part(&env.user_id(1))
        ),
        Some(&alice),
        &format!(r#"{{"{carol_id}":["{room}"]}}"#),
    );
    http_json(
        base,
        "POST",
        &format!("/_matrix/client/v3/rooms/{}/join", path_part(&room)),
        Some(&carol),
        "{}",
    );
    logout(base, &alice);
    logout(base, &carol);
    room
}

/// `twicecat-cli`, driven through its stdin like a person would.
struct Cli {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
}

impl Cli {
    fn start(env: &TestEnv) -> Self {
        // target/<profile>/deps/calls-<hash> -> target/<profile>/twicecat-cli
        let exe = std::env::current_exe().expect("the test binary's path");
        let cli: PathBuf = exe
            .parent()
            .and_then(|deps| deps.parent())
            .expect("the target directory")
            .join(format!("twicecat-cli{}", std::env::consts::EXE_SUFFIX));
        assert!(
            cli.exists(),
            "{} is missing: cargo build -p twicecat-core --bin twicecat-cli",
            cli.display()
        );
        let mut child = Command::new(cli)
            .args([
                "--ephemeral",
                "--homeserver",
                env.get("HOMESERVER_URL"),
                "--password-stdin",
                "--silent-audio",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn twicecat-cli");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
        }
    }

    fn write(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("write the CLI's stdin");
    }

    /// The next line containing `text`, within `wait`.
    fn expect_within(&mut self, text: &str, wait: Duration) -> Option<String> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).ok()?;
            self.seen.push(line.clone());
            if line.contains(text) {
                return Some(line);
            }
        }
    }

    fn expect(&mut self, text: &str) -> String {
        self.expect_within(text, Duration::from_secs(60))
            .unwrap_or_else(|| panic!("the CLI printed no {text:?}; it printed {:#?}", self.seen))
    }
}

/// Silent virtual devices: the app never opens a real microphone here.
struct Silence;

impl VirtualAudio for Silence {
    fn capture(&self, frame: &mut [f32; AUDIO_FRAME_LEN]) {
        frame.fill(0.0);
    }
    fn playback(&self, _: &[f32; AUDIO_FRAME_LEN]) {}
}

/// The app with its bridge, and a pump that feeds it the core's events.
struct AppUnderTest {
    app: App,
    pump: tokio::runtime::Runtime,
    events: iced::futures::stream::BoxStream<'static, BridgeEvent>,
    dir: PathBuf,
}

impl AppUnderTest {
    fn start(env: &TestEnv) -> Self {
        let dir = std::env::temp_dir().join(format!("twicecat-app-calls-test-{}", random_tag()));
        let (bridge, slot) = Bridge::start(
            CoreConfig {
                data: DataPaths::at(&dir),
                homeserver_override: Some(env.get("HOMESERVER_URL").to_owned()),
                room_list_page_size: twicecat_core::DEFAULT_ROOM_LIST_PAGE_SIZE,
                max_upload_bytes_override: None,
                audio: AudioBackend::Virtual(Arc::new(Silence)),
                call_service_override: None,
                video: twicecat_core::api::VideoBackend::System,
            },
            Arc::new(InMemorySecretStore::new()),
        )
        .expect("start the bridge");
        let (app, _boot) = App::new(Startup {
            sink: Box::new(bridge),
            events: slot.clone(),
        });
        let pump = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("pump runtime");
        let events = events_stream(&slot).boxed();
        Self {
            app,
            pump,
            events,
            dir,
        }
    }

    /// Applies the core's events until `done` holds, for at most `secs`.
    fn until(&mut self, what: &str, secs: u64, done: impl Fn(&App) -> bool) {
        let limit = Instant::now() + Duration::from_secs(secs);
        while !done(&self.app) {
            assert!(Instant::now() < limit, "the app: no {what} within {secs} s");
            let event = self.pump.block_on(async {
                tokio::time::timeout(Duration::from_secs(1), self.events.next()).await
            });
            match event {
                Ok(Some(BridgeEvent::Core(event))) => {
                    let _ = self.app.update(Message::Core(event));
                }
                Ok(Some(BridgeEvent::Closed)) => {
                    let _ = self.app.update(Message::CoreClosed);
                }
                Ok(None) | Err(_) => {}
            }
        }
    }

    fn login(&mut self, env: &TestEnv, n: u8) {
        self.until("login screen", 60, |app| app.screen == Screen::Login);
        let _ = self
            .app
            .update(Message::UsernameChanged(env.user(n).to_owned()));
        let _ = self.app.update(Message::PasswordChanged(
            twicecat_core::api::Password::from(env.password(n).to_owned()),
        ));
        let _ = self.app.update(Message::LoginSubmitted);
        self.until("main screen", 60, |app| app.screen == Screen::Main);
    }

    /// The arming wait of the card now showing ends. The harness drops the
    /// tasks `update` returns, so the timer that would end it never runs;
    /// until it does, the card answers nothing (clicks already on their way
    /// when it came up were meant for what was under it).
    fn arm(&mut self) {
        let seq = self.app.calls.arm_seq();
        let _ = self.app.update(Message::RingArmed(seq));
    }

    fn call(&mut self, message: CallMessage) {
        let _ = self.app.update(Message::Call(message));
    }

    fn close(mut self) {
        let _ = self.app.update(Message::CloseRequested);
        self.until("exit", 15, |app| app.exit_requested);
        let dir = self.dir.clone();
        drop(self);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
fn a_cli_call_rings_the_app_which_declines_then_answers() {
    let env = TestEnv::load();
    let name = format!("call ui {}", random_tag());
    let room_id = direct_room(&env, &name);
    let room = RoomId::from(room_id.as_str());

    let mut cli = Cli::start(&env);
    cli.write(env.user(1));
    cli.write(env.password(1));
    cli.expect(&format!("Logged in as {}", env.user_id(1)));
    let mut carol = AppUnderTest::start(&env);
    carol.login(&env, 3);
    carol.until("the DM in the room list", 60, |app| {
        app.rooms.iter().any(|r| r.id == room)
    });
    let listed = (0..30).any(|_| {
        cli.write("/rooms");
        cli.expect_within(&name, Duration::from_secs(1)).is_some()
    });
    assert!(listed, "the CLI never listed the DM");

    // 1. Alice calls; carol's card shows her; Decline tells alice.
    cli.write(&format!("/call {room_id}"));
    cli.expect("In the call in");
    carol.until("incoming-call card", 60, |app| {
        app.calls.card().is_some_and(|r| r.room_id == room)
    });
    let ring = carol.app.calls.card().expect("the card").clone();
    assert!(ring.ring, "a DM call rings");
    assert_eq!(ring.caller.as_str(), env.user_id(1));
    // A card that has just come up ignores an answer.
    carol.call(CallMessage::Decline(room.clone()));
    assert!(
        carol.app.calls.card().is_some(),
        "an answer before the card is armed does nothing"
    );
    carol.arm();
    carol.call(CallMessage::Decline(room.clone()));
    cli.expect(&format!("{} declined the call in", env.user_id(3)));
    carol.until("the ring to stop", 30, |app| {
        app.calls.ring(&room).is_none()
    });
    assert!(carol.app.calls.call.is_none());

    // 2. Alice calls again; Accept joins, both see each other.
    cli.write("/leave");
    cli.expect("Left the call in");
    cli.write(&format!("/call {room_id}"));
    cli.expect("In the call in");
    carol.until("a second card", 60, |app| {
        app.calls.card().is_some_and(|r| r.room_id == room)
    });
    carol.arm();
    carol.call(CallMessage::Accept(room.clone()));
    carol.until("connected with alice", 60, |app| {
        app.calls.call.as_ref().is_some_and(|c| {
            matches!(c.phase, CallPhase::Connected { .. })
                && c.participants.iter().any(|p| p.is_local)
                && c.participants.iter().any(|p| !p.is_local)
        })
    });
    assert!(
        carol.app.calls.card().is_none(),
        "answering closes the card"
    );
    let joined = cli.expect(" joined the call.");
    assert!(joined.contains(&env.user_id(3)), "{joined}");
    let rows = carol.app.calls.rows();
    assert!(
        rows.iter()
            .any(|r| !r.is_local && r.user_id.as_str() == env.user_id(1) && r.controls.is_some()),
        "alice has her row with controls"
    );

    // 3. Carol leaves.
    carol.call(CallMessage::Leave);
    carol.until("the call to end", 30, |app| app.calls.call.is_none());
    let left = cli.expect(" left the call.");
    assert!(left.contains(&env.user_id(3)), "{left}");
    cli.write("/leave");
    cli.write("/quit");
    let _ = cli.child.wait();
    carol.close();
}
