//! Building SDK clients and mapping SDK errors to fixed, non-secret
//! [`CoreError`] messages.
//!
//! SDK error `Display` text can contain URLs, response bodies, or other data
//! we must not forward, so every mapping here returns a constant message.

use std::path::Path;

use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::{Client, ClientBuildError, Error as SdkError, HttpError};

use crate::api::CoreError;

/// Where to find the homeserver.
pub(crate) enum ServerSpec<'a> {
    /// A server name (discovery via `.well-known`) or a full homeserver URL.
    NameOrUrl(&'a str),
    /// A homeserver URL known from a stored session (no discovery).
    Url(&'a str),
}

/// Builds a client with an encrypted SQLite store at `store_dir`.
pub(crate) async fn build_client(
    store_dir: &Path,
    passphrase: &str,
    server: ServerSpec<'_>,
) -> Result<Client, CoreError> {
    let builder = Client::builder();
    let builder = match server {
        ServerSpec::NameOrUrl(s) => builder.server_name_or_homeserver_url(s),
        ServerSpec::Url(u) => builder.homeserver_url(u),
    };
    builder
        .sqlite_store(store_dir, Some(passphrase))
        .handle_refresh_tokens()
        .build()
        .await
        .map_err(|e| map_build_error(&e))
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
