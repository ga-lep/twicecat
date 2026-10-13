//! End-to-end run of `twicecat-cli` against the local homeserver
//! (`scripts/stack.sh up -d --wait && scripts/create-test-users.sh`).

mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::TestEnv;

/// The bin files must reach the engine only through the public core API.
#[test]
fn cli_source_names_no_sdk_crate() {
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("list the CLI sources") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    sources(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin"),
        &mut files,
    );
    assert!(files.len() >= 4, "{files:?}");
    for file in files {
        let text = std::fs::read_to_string(&file).expect("read the CLI source");
        for needle in ["matrix_sdk", "ruma", "eyeball", "imbl"] {
            assert!(
                !text.contains(needle),
                "{} mentions {needle}",
                file.display()
            );
        }
    }
}

#[test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
fn ephemeral_password_login_and_quit() {
    let env = TestEnv::load();
    let mut child = Command::new(env!("CARGO_BIN_EXE_twicecat-cli"))
        .args(["--ephemeral", "--homeserver", env.homeserver_url()])
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn twicecat-cli");
    let input = format!("{}\n{}\n/rooms\n/quit\n", env.user(1), env.password(1));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let output = rx
        .recv_timeout(Duration::from_secs(60))
        .expect("twicecat-cli did not exit within 60 s")
        .expect("wait for twicecat-cli");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "exit status {:?}", output.status);
    let expected = format!("Logged in as {}", env.user_id(1));
    assert!(stdout.contains(&expected), "stdout lacks the login line");
    let password = env.password(1);
    assert!(!stdout.contains(password), "password in stdout");
    assert!(!stderr.contains(password), "password in stderr");
}

/// Feeds the CLI one line at a time and waits for what it prints.
struct Session {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
    seen: Vec<String>,
}

impl Session {
    fn start(args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_twicecat-cli"))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn twicecat-cli");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
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
        writeln!(self.stdin, "{line}").expect("write stdin");
    }

    /// Waits for a line containing `text`; returns it.
    fn expect(&mut self, text: &str) -> String {
        self.expect_within(text, Duration::from_secs(60))
            .unwrap_or_else(|| panic!("no line with {text:?}; printed: {:#?}", self.seen))
    }

    fn expect_within(&mut self, text: &str, wait: Duration) -> Option<String> {
        let deadline = std::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let line = self.lines.recv_timeout(left).ok()?;
            self.seen.push(line.clone());
            if line.contains(text) {
                return Some(line);
            }
        }
    }
}

/// Joins a call from the CLI, mutes, leaves and quits; the membership is
/// gone afterwards. Audio is silent and virtual (`--silent-audio`).
#[test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
fn cli_joins_mutes_and_leaves_a_call() {
    use matrix_sdk::ruma::api::client::state::get_state_events;

    let env = TestEnv::load();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let alice = runtime.block_on(common::raw_login(1));
    let name = common::unique_room_name();
    let room_id = runtime
        .block_on(common::create_room(&alice, &name, None))
        .to_string();

    let mut cli = Session::start(&[
        "--ephemeral",
        "--homeserver",
        env.homeserver_url(),
        "--password-stdin",
        "--silent-audio",
    ]);
    cli.write(env.user(1));
    cli.write(env.password(1));
    cli.expect(&format!("Logged in as {}", env.user_id(1)));
    cli.write("/leave");
    cli.expect("Not in a call.");
    // The CLI's sync must know the room before it can call there.
    let listed = (0..30).any(|_| {
        cli.write("/rooms");
        cli.expect_within(&name, Duration::from_secs(1)).is_some()
    });
    assert!(listed, "the room never showed up in /rooms");
    cli.write(&format!("/call {room_id}"));
    cli.expect("Joining the call in");
    let connected = cli.expect("In the call in");
    assert!(connected.contains("(not encrypted)"), "{connected}");
    cli.write("/mute");
    cli.expect("Microphone muted.");
    // Our own membership shows up once it has synced back.
    let me = (0..30).find_map(|_| {
        cli.write("/who");
        cli.expect_within("[you", Duration::from_secs(1))
    });
    let me = me.expect("never listed in /who");
    assert!(me.contains(env.user_id(1).as_str()), "{me}");
    assert!(me.contains("[you, muted]"), "{me}");
    cli.write("/leave");
    cli.expect("Left the call in");
    cli.write("/quit");
    let status = cli.child.wait().expect("wait for twicecat-cli");
    assert!(status.success(), "exit status {status:?}");

    // Our membership was emptied on leave.
    let request = get_state_events::v3::Request::new(room_id.as_str().try_into().unwrap());
    let state = runtime
        .block_on(async { alice.send(request).await })
        .expect("room state");
    let live: Vec<String> = state
        .room_state
        .iter()
        .filter_map(|raw| raw.deserialize_as::<serde_json::Value>().ok())
        .filter(|e| e["type"] == "org.matrix.msc3401.call.member")
        .filter(|e| e["content"].as_object().is_some_and(|c| !c.is_empty()))
        .map(|e| e["state_key"].to_string())
        .collect();
    assert!(live.is_empty(), "memberships left: {live:?}");
    let password = env.password(1);
    assert!(
        !cli.seen.iter().any(|l| l.contains(password)),
        "password in stdout"
    );
}

/// Logs a CLI in as test user `n`, with the test pattern as its camera.
fn video_cli(env: &TestEnv, n: u8) -> Session {
    let mut cli = Session::start(&[
        "--ephemeral",
        "--homeserver",
        env.homeserver_url(),
        "--password-stdin",
        "--silent-audio",
        "--test-video",
    ]);
    cli.write(env.user(n));
    cli.write(env.password(n));
    cli.expect(&format!("Logged in as {}", env.user_id(n)));
    cli
}

/// Joins `room_id`'s call once the CLI's sync knows the room.
fn join_call(cli: &mut Session, name: &str, room_id: &str) {
    let listed = (0..30).any(|_| {
        cli.write("/rooms");
        cli.expect_within(name, Duration::from_secs(1)).is_some()
    });
    assert!(listed, "the room never showed up in /rooms");
    cli.write(&format!("/call {room_id}"));
    let connected = cli.expect("In the call in");
    assert!(connected.contains("(end-to-end encrypted)"), "{connected}");
}

/// Two CLIs in an encrypted call: alice sends the test pattern as her
/// camera (`/video pattern`), carol lists it, asks for its full size and
/// receives about 30 fps of it (`/video stats`); alice's hang-up removes
/// it. Audio is silent and virtual.
#[test]
#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]
fn two_clis_send_and_receive_a_test_pattern() {
    let env = TestEnv::load();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let alice = runtime.block_on(common::raw_login(1));
    let carol = runtime.block_on(common::raw_login(3));
    let name = common::unique_room_name();
    let room_id = runtime
        .block_on(common::create_encrypted_room(
            &alice,
            &name,
            Some(&env.user_id(3)),
        ))
        .to_string();
    runtime.block_on(async {
        common::calls::allow_calls(&alice, &room_id).await;
        carol
            .join_room_by_id(room_id.as_str().try_into().unwrap())
            .await
            .unwrap();
    });

    let mut sender = video_cli(&env, 1);
    let mut viewer = video_cli(&env, 3);
    join_call(&mut sender, &name, &room_id);
    join_call(&mut viewer, &name, &room_id);
    sender.write("/video pattern 720p 30");
    sender.expect("[video] camera on, screen share off");
    viewer.expect(&format!("[video] + {} camera", env.user_id(1)));
    viewer.write("/video view 1 1280x720");
    // The SFU moves to the top layer within a few seconds. The first stats
    // at that size cover the switch (20 fps on a CI runner), so the rate is
    // read for up to 10 s more until it reaches 25 fps.
    let stats_at_full_size = |viewer: &mut Session| {
        std::thread::sleep(Duration::from_secs(1));
        viewer.write("/video stats");
        viewer.expect_within("received 1280x720", Duration::from_secs(2))
    };
    let first = (0..20).find_map(|_| stats_at_full_size(&mut viewer));
    let mut readings = vec![first.expect("never received the 1280x720 layer")];
    let fps_of = |line: &str| -> f32 {
        line.split_whitespace()
            .skip_while(|w| *w != "1280x720")
            .nth(1)
            .and_then(|w| w.parse().ok())
            .unwrap_or_else(|| panic!("no fps in {line:?}"))
    };
    for _ in 0..10 {
        if fps_of(readings.last().unwrap()) >= 25.0 {
            break;
        }
        if let Some(line) = stats_at_full_size(&mut viewer) {
            readings.push(line);
        }
    }
    assert!(
        fps_of(readings.last().unwrap()) >= 25.0,
        "never 25 fps at 1280x720: {readings:#?}"
    );

    sender.write("/leave");
    sender.expect("Left the call in");
    viewer.expect(&format!("[video] - {} camera", env.user_id(1)));
    for mut cli in [sender, viewer] {
        cli.write("/quit");
        let status = cli.child.wait().expect("wait for twicecat-cli");
        assert!(status.success(), "exit status {status:?}");
    }
}
