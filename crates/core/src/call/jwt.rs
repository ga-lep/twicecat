//! The LiveKit access token, from a homeserver's MatrixRTC authorization
//! service (lk-jwt-service), in exchange for a Matrix OpenID token.
//!
//! Element Call's compatibility mode uses the legacy `/sfu/get` endpoint for
//! the SFU it publishes to; the LiveKit identity is then `user:device`
//! (the membership's `membershipID`). For other members' SFUs, where it only
//! subscribes, it tries the Matrix 2.0 `/get_token` first.

use std::fmt;

use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::account::request_openid_token;
use serde::Deserialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::api::CoreError;

/// Where and how to connect to the SFU. `Debug` hides the token.
pub(crate) struct SfuAccess {
    /// LiveKit signalling URL (`wss://`, or `ws://` on loopback).
    pub url: String,
    pub jwt: Zeroizing<String>,
}

impl fmt::Debug for SfuAccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SfuAccess")
            .field("url", &self.url)
            .field("jwt", &format_args!("<redacted, {} bytes>", self.jwt.len()))
            .finish()
    }
}

/// `https`, or `http`/`ws` only towards loopback, as for homeservers.
fn allowed(url: &str, secure: &str, plain: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() == secure {
        return parsed.host().is_some();
    }
    parsed.scheme() == plain
        && match parsed.host() {
            Some(url::Host::Domain(d)) => d == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        }
}

pub(crate) fn service_url_allowed(url: &str) -> bool {
    allowed(url, "https", "http")
}

pub(crate) fn livekit_url_allowed(url: &str) -> bool {
    allowed(url, "wss", "ws")
}

#[derive(Deserialize)]
struct SfuReply {
    url: String,
    jwt: String,
}

/// A fresh OpenID token for us, in the shape the services expect. The
/// value holds a secret: callers keep it inside `Zeroizing` request bodies.
async fn openid_token(client: &Client) -> Result<Value, CoreError> {
    let user_id = client.user_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    let openid = client
        .send(request_openid_token::v3::Request::new(user_id))
        .await
        .map_err(|_| CoreError::Call("could not get an OpenID token".into()))?;
    Ok(json!({
        "access_token": openid.access_token,
        "token_type": openid.token_type,
        "matrix_server_name": openid.matrix_server_name,
        "expires_in": openid.expires_in.as_secs(),
    }))
}

/// Body of the legacy `/sfu/get` request (the LiveKit identity becomes
/// `user:device`).
fn legacy_body(room_id: &str, openid: Value, device_id: &str) -> Value {
    json!({ "room": room_id, "openid_token": openid, "device_id": device_id })
}

/// Body of the Matrix 2.0 `/get_token` request, as Element Call sends it
/// (`getLiveKitJWT`, slot `m.call#ROOM`).
fn get_token_body(
    room_id: &str,
    openid: Value,
    user_id: &str,
    device_id: &str,
    member_id: &str,
) -> Value {
    json!({
        "room_id": room_id,
        "slot_id": "m.call#ROOM",
        "openid_token": openid,
        "member": { "id": member_id, "claimed_user_id": user_id, "claimed_device_id": device_id },
    })
}

/// POSTs `body` to `service_url` + `path` and checks the reply.
async fn request(service_url: &str, path: &str, body: Value) -> Result<SfuAccess, CoreError> {
    if !service_url_allowed(service_url) {
        return Err(CoreError::Call("the call service must use https".into()));
    }
    let body = Zeroizing::new(body.to_string());
    let endpoint = format!("{}{path}", service_url.trim_end_matches('/'));
    let response = matrix_sdk::reqwest::Client::new()
        .post(endpoint)
        .header("content-type", "application/json")
        .body(body.as_str().to_owned())
        .send()
        .await
        .map_err(|_| CoreError::Call("the call service is unreachable".into()))?;
    if !response.status().is_success() {
        return Err(CoreError::Call(format!(
            "the call service refused the token request (HTTP {})",
            response.status().as_u16()
        )));
    }
    let text = Zeroizing::new(
        response
            .text()
            .await
            .map_err(|_| CoreError::Call("the call service reply was cut off".into()))?,
    );
    let reply: SfuReply = serde_json::from_str(&text)
        .map_err(|_| CoreError::Call("the call service sent an invalid reply".into()))?;
    if !livekit_url_allowed(&reply.url) {
        return Err(CoreError::Call(
            "the call server must use a secure connection".into(),
        ));
    }
    Ok(SfuAccess {
        url: reply.url,
        jwt: Zeroizing::new(reply.jwt),
    })
}

/// A token to publish on our own service, through the legacy `/sfu/get`
/// (Element Call's compatibility mode does the same for its own SFU).
pub(crate) async fn sfu_access(
    client: &Client,
    service_url: &str,
    room_id: &str,
) -> Result<SfuAccess, CoreError> {
    let device_id = client.device_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    let openid = openid_token(client).await?;
    request(
        service_url,
        "/sfu/get",
        legacy_body(room_id, openid, device_id.as_str()),
    )
    .await
}

/// A token to subscribe on another member's service. As Element Call for
/// SFUs it does not publish to: `/get_token` first, then `/sfu/get`. A
/// service for another homeserver grants subscribe-only access.
pub(crate) async fn subscribe_access(
    client: &Client,
    service_url: &str,
    room_id: &str,
) -> Result<SfuAccess, CoreError> {
    let user_id = client.user_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    let device_id = client.device_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    let member_id = super::membership::member_id(user_id.as_str(), device_id.as_str());
    let body = get_token_body(
        room_id,
        openid_token(client).await?,
        user_id.as_str(),
        device_id.as_str(),
        &member_id,
    );
    match request(service_url, "/get_token", body).await {
        Ok(access) => Ok(access),
        Err(_) => {
            let openid = openid_token(client).await?;
            request(
                service_url,
                "/sfu/get",
                legacy_body(room_id, openid, device_id.as_str()),
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy() {
        assert!(service_url_allowed("https://rtc.example.org/livekit/jwt"));
        assert!(service_url_allowed("http://127.0.0.1:8080"));
        assert!(service_url_allowed("http://localhost:8080"));
        assert!(!service_url_allowed("http://rtc.example.org"));
        assert!(!service_url_allowed("ftp://x"));
        assert!(livekit_url_allowed("wss://sfu.example.org"));
        assert!(livekit_url_allowed("ws://127.0.0.1:7880"));
        assert!(livekit_url_allowed("ws://[::1]:7880"));
        assert!(!livekit_url_allowed("ws://sfu.example.org"));
        assert!(!livekit_url_allowed("https://sfu.example.org"));
    }

    #[test]
    fn token_request_bodies_match_element_call() {
        let openid = json!({ "access_token": "t" });
        assert_eq!(
            legacy_body("!r:x", openid.clone(), "DEV"),
            json!({ "room": "!r:x", "openid_token": { "access_token": "t" }, "device_id": "DEV" })
        );
        assert_eq!(
            get_token_body("!r:x", openid, "@a:x", "DEV", "@a:x:DEV"),
            json!({
                "room_id": "!r:x",
                "slot_id": "m.call#ROOM",
                "openid_token": { "access_token": "t" },
                "member": { "id": "@a:x:DEV", "claimed_user_id": "@a:x", "claimed_device_id": "DEV" },
            })
        );
    }

    #[test]
    fn debug_hides_the_token() {
        let access = SfuAccess {
            url: "wss://sfu".into(),
            jwt: Zeroizing::new("secret-jwt-x".into()),
        };
        let dbg = format!("{access:?}");
        assert!(!dbg.contains("secret-jwt-x"), "{dbg}");
        assert!(dbg.contains("wss://sfu"));
    }
}
