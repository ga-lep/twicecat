//! Building SDK clients and mapping SDK errors to fixed, non-secret
//! [`CoreError`] messages.
//!
//! SDK error `Display` text can contain URLs, response bodies, or other data
//! we must not forward, so every mapping here returns a constant message.

use std::path::Path;

use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::{Client, ClientBuildError, Error as SdkError, HttpError};

use crate::api::CoreError;
use crate::session::is_allowed_homeserver_url;

/// Fixed, non-secret message for a homeserver that fails the transport policy.
const HTTPS_REQUIRED: &str = "the homeserver must use https";

/// Applies the transport policy (see [`is_allowed_homeserver_url`]).
fn ensure_https(url: &str) -> Result<(), CoreError> {
    if is_allowed_homeserver_url(url) {
        Ok(())
    } else {
        Err(CoreError::Login(HTTPS_REQUIRED.into()))
    }
}

/// Re-applies the transport policy to the client's current homeserver. Call
/// after login and before anything is stored or sent, as defence in depth
/// against the homeserver changing during login.
pub(crate) fn ensure_homeserver_after_login(client: &Client) -> Result<(), CoreError> {
    ensure_https(client.homeserver().as_str())
}

/// Where to find the homeserver.
pub(crate) enum ServerSpec<'a> {
    /// A server name (discovery via `.well-known`) or a full homeserver URL.
    NameOrUrl(&'a str),
    /// A homeserver URL known from a stored session (no discovery).
    Url(&'a str),
}

/// Builds a client with an encrypted SQLite store at `store_dir`.
///
/// For [`ServerSpec::NameOrUrl`] the https/loopback policy is enforced both on
/// an explicit URL (before any request, including discovery) and on the
/// homeserver the SDK resolved (`.well-known` may point to plain http), so no
/// credential can be sent over cleartext. Every login path (password, and
/// OAuth later) must go through here.
pub(crate) async fn build_client(
    store_dir: &Path,
    passphrase: &str,
    server: ServerSpec<'_>,
) -> Result<Client, CoreError> {
    let builder = Client::builder();
    let check_resolved = matches!(server, ServerSpec::NameOrUrl(_));
    let builder = match server {
        ServerSpec::NameOrUrl(s) => {
            // An explicit http(s) URL is judged before the SDK contacts it.
            if let Ok(url) = url::Url::parse(s)
                && matches!(url.scheme(), "http" | "https")
            {
                ensure_https(s)?;
            }
            builder.server_name_or_homeserver_url(s)
        }
        ServerSpec::Url(u) => builder.homeserver_url(u),
    };
    let client = builder
        .sqlite_store(store_dir, Some(passphrase))
        .handle_refresh_tokens()
        // A login response's `well_known` must not move the client to another
        // (possibly cleartext) homeserver after the policy check above.
        .respect_login_well_known(false)
        .build()
        .await
        .map_err(|e| map_build_error(&e))?;
    if check_resolved {
        ensure_https(client.homeserver().as_str())?;
    }
    Ok(client)
}

/// Maps a client build failure to a fixed message.
pub(crate) fn map_build_error(e: &ClientBuildError) -> CoreError {
    match e {
        ClientBuildError::Http(_) | ClientBuildError::AutoDiscovery(_) => {
            CoreError::Network("could not reach or discover the homeserver".into())
        }
        ClientBuildError::InvalidServerName
        | ClientBuildError::MissingHomeserver
        | ClientBuildError::Url(_) => CoreError::Network("invalid homeserver address".into()),
        ClientBuildError::SqliteStore(_) => {
            CoreError::Storage("could not open the session store".into())
        }
        _ => CoreError::Internal("could not set up the client".into()),
    }
}

/// Maps a login failure to a fixed message.
pub(crate) fn map_login_error(e: &SdkError) -> CoreError {
    match e.client_api_error_kind() {
        Some(ErrorKind::Forbidden) => CoreError::Login("invalid username or password".into()),
        Some(ErrorKind::UserDeactivated) => {
            CoreError::Login("this account has been deactivated".into())
        }
        Some(ErrorKind::LimitExceeded(_)) => {
            CoreError::Login("too many attempts, try again later".into())
        }
        Some(_) => CoreError::Login("the homeserver rejected the login".into()),
        None => match e {
            SdkError::Http(http) if matches!(**http, HttpError::Reqwest(_)) => {
                CoreError::Network("could not reach the homeserver".into())
            }
            _ => CoreError::Login("login failed".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn client_for(url: &str) -> Client {
        // `homeserver_url` does no discovery, so this contacts nobody.
        Client::builder()
            .homeserver_url(url)
            .build()
            .await
            .expect("client builds without network")
    }

    #[tokio::test]
    async fn recheck_rejects_cleartext_non_loopback_homeserver() {
        let client = client_for("http://evil.example.org").await;
        let err = ensure_homeserver_after_login(&client).expect_err("must be refused");
        assert!(matches!(&err, CoreError::Login(m) if m == HTTPS_REQUIRED));
    }

    #[tokio::test]
    async fn recheck_accepts_https_and_loopback_http() {
        for url in ["https://matrix.example.org", "http://127.0.0.1:8008"] {
            let client = client_for(url).await;
            ensure_homeserver_after_login(&client).expect("allowed");
        }
    }
}
