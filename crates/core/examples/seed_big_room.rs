//! Seeds a room with thousands of messages on the LOCAL test homeserver, for
//! the human checkpoint (scrolling a 10,000-item timeline). It is compiled by
//! `cargo test` and never run in CI.
//!
//! ```text
//! docker compose up -d --wait && scripts/create-test-users.sh
//! cargo run -p twicecat-core --example seed_big_room -- 10000
//! ```
//!
//! It reads `.env.test` (variables of the same name win), logs in as test
//! user 1, refuses any homeserver that is not plain http on a loopback
//! address, invites test user 3, sends the messages in concurrent batches and
//! prints only the room ID.

use std::collections::HashMap;
use std::path::Path;

use matrix_sdk::Client;
use matrix_sdk::ruma::TransactionId;
use matrix_sdk::ruma::api::client::message::send_message_event::v3::Request as SendRequest;
use matrix_sdk::ruma::api::client::room::create_room::v3::Request as CreateRequest;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;

/// Messages sent at the same time.
const BATCH: usize = 20;
/// Messages sent when no count is given.
const DEFAULT_COUNT: usize = 10_000;

/// Whether `url` is plain http on a loopback address with a port.
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

/// `KEY=VALUE` lines of `.env.test`, overridden by environment variables.
fn load_env() -> Result<HashMap<String, String>, String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.env.test");
    let text = std::fs::read_to_string(&path).map_err(|_| "cannot read .env.test".to_owned())?;
    let mut values: HashMap<String, String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    for (key, value) in &mut values {
        if let Ok(over) = std::env::var(key) {
            *value = over;
        }
    }
    Ok(values)
}

/// An error mapper that keeps only a fixed message (errors may name URLs).
fn fail<E>(what: &'static str) -> impl FnOnce(E) -> String {
    move |_| what.to_owned()
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let count = match std::env::args().nth(1) {
        Some(arg) => arg
            .parse::<usize>()
            .map_err(|_| "the message count must be a number".to_owned())?,
        None => DEFAULT_COUNT,
    };
    let env = load_env()?;
    let get = |key: &str| {
        env.get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("{key} is missing from .env.test"))
    };
    let url = get("HOMESERVER_URL")?;
    if !is_local_homeserver(url) {
        return Err(
            "refusing to run: HOMESERVER_URL must be http://127.0.0.1:<port>, \
             http://localhost:<port> or http://[::1]:<port>"
                .to_owned(),
        );
    }

    let client = Client::builder()
        .homeserver_url(url)
        .build()
        .await
        .map_err(fail("cannot reach the homeserver"))?;
    client
        .matrix_auth()
        .login_username(get("TEST_USER_1")?, get("TEST_PASSWORD_1")?)
        .initial_device_display_name("twicecat-seed-big-room")
        .await
        .map_err(fail("login failed"))?;

    let mut request = CreateRequest::new();
    request.name = Some(format!("twicecat-big-room-{count}"));
    let invitee = format!("@{}:{}", get("TEST_USER_3")?, get("HOMESERVER_DOMAIN")?);
    request
        .invite
        .push(invitee.try_into().map_err(fail("bad invitee user ID"))?);
    let room_id = client
        .create_room(request)
        .await
        .map_err(fail("cannot create the room"))?
        .room_id()
        .to_owned();

    let mut sent = 0;
    while sent < count {
        let end = (sent + BATCH).min(count);
        let batch = (sent..end).map(|n| {
            let client = client.clone();
            let room_id = room_id.clone();
            async move {
                let content = RoomMessageEventContent::text_plain(format!("message {}", n + 1));
                let request = SendRequest::new(room_id, TransactionId::new(), &content)
                    .map_err(|_| "cannot serialize a message".to_owned())?;
                client
                    .send(request)
                    .await
                    .map(drop)
                    .map_err(|_| "a message could not be sent".to_owned())
            }
        });
        for result in futures_util::future::join_all(batch).await {
            result?;
        }
        sent = end;
    }
    println!("{room_id}");
    Ok(())
}
