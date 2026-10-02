//! The LiveKit access token, from a homeserver's MatrixRTC authorization
//! service (lk-jwt-service), in exchange for a Matrix OpenID token.
//!
//! Element Call's compatibility mode uses the legacy `/sfu/get` endpoint for
//! the SFU it publishes to; the LiveKit identity is then `user:device`
//! (the membership's `membershipID`).

use std::fmt;

use matrix_sdk::Client;
use matrix_sdk::ruma::api::client::account::request_openid_token;
use serde::Deserialize;
use serde_json::json;
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

/// Gets a LiveKit token for `room_id` from the service at `service_url`.
pub(crate) async fn sfu_access(
    client: &Client,
    service_url: &str,
    room_id: &str,
) -> Result<SfuAccess, CoreError> {
    if !service_url_allowed(service_url) {
        return Err(CoreError::Call("the call service must use https".into()));
    }
    let user_id = client.user_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    let device_id = client.device_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    let openid = client
        .send(request_openid_token::v3::Request::new(user_id))
        .await
        .map_err(|_| CoreError::Call("could not get an OpenID token".into()))?;
    let body = Zeroizing::new(
        json!({
            "room": room_id,
            "openid_token": {
                "access_token": openid.access_token,
                "token_type": openid.token_type,
                "matrix_server_name": openid.matrix_server_name,
                "expires_in": openid.expires_in.as_secs(),
            },
            "device_id": device_id,
        })
        .to_string(),
    );
    let endpoint = format!("{}/sfu/get", service_url.trim_end_matches('/'));
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
