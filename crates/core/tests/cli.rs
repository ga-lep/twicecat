//! End-to-end run of `tessera-cli` against the local homeserver
//! (`docker compose up -d --wait && scripts/create-test-users.sh`).

mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::TestEnv;

/// The bin file must reach the engine only through the public core API.
#[test]
fn cli_source_names_no_sdk_crate() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/bin/tessera-cli.rs");
    let text = std::fs::read_to_string(path).expect("read the CLI source");
    for needle in ["matrix_sdk", "ruma", "eyeball", "imbl"] {
        assert!(!text.contains(needle), "CLI source mentions {needle}");
    }
}

#[test]
#[ignore = "needs local homeserver: docker compose up -d --wait"]
fn ephemeral_password_login_and_quit() {
    let env = TestEnv::load();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tessera-cli"))
        .args(["--ephemeral", "--homeserver", env.homeserver_url()])
        .arg("--password-stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tessera-cli");
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
        .expect("tessera-cli did not exit within 60 s")
        .expect("wait for tessera-cli");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "exit status {:?}", output.status);
    let expected = format!("Logged in as {}", env.user_id(1));
    assert!(stdout.contains(&expected), "stdout lacks the login line");
    let password = env.password(1);
    assert!(!stdout.contains(password), "password in stdout");
    assert!(!stderr.contains(password), "password in stderr");
}
