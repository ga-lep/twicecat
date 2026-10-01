//! OAuth 2.0 / MAS login helpers: client metadata, the loopback redirect
//! listener, URL policy, and error mapping. Private module: may name SDK types.
//!
//! Nothing here logs or returns the authorization URL, state, code, or tokens
//! (the URL only goes out through `CoreEvent::OAuthUrl`, whose `Debug` is
//! redacted).

use std::time::Duration;

use matrix_sdk::Client;
use matrix_sdk::authentication::oauth::OAuthError;
use matrix_sdk::authentication::oauth::registration::{
    ApplicationType, ClientMetadata, Localized, OAuthGrantType,
};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::utils::local_server::{
    LocalServerBuilder, LocalServerIpAddress, LocalServerRedirectHandle,
};
use tokio::sync::{mpsc, oneshot};

use crate::api::{AuthUrl, CoreError, CoreEvent};
use crate::session::is_allowed_homeserver_url;

/// How long the user may take to finish the browser step.
const REDIRECT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Loopback redirect URIs registered with the authorization server. MAS lets
/// a native client use any port on these (RFC 8252 section 7.3).
const REDIRECT_URIS: [&str; 2] = ["http://127.0.0.1/", "http://[::1]/"];

/// The metadata Tessera registers itself with: a native client using the
/// authorization code grant with loopback redirects.
pub(crate) fn client_metadata() -> Raw<ClientMetadata> {
    let client_uri = url::Url::parse(env!("CARGO_PKG_REPOSITORY"))
        .expect("the workspace repository URL is a valid URL");
    let redirect_uris = REDIRECT_URIS
        .iter()
        .map(|u| url::Url::parse(u).expect("constant redirect URI is valid"))
        .collect();
    let mut metadata = ClientMetadata::new(
        ApplicationType::Native,
        vec![OAuthGrantType::AuthorizationCode { redirect_uris }],
        Localized::new(client_uri, []),
    );
    metadata.client_name = Some(Localized::new("Tessera".to_owned(), []));
    Raw::new(&metadata).expect("client metadata serializes")
}

/// Fixed message for an authorization URL that fails the policy.
const BAD_AUTH_URL: &str = "the authorization server gave an unsafe login URL";

/// Accepts only `https` URLs, and `http` only for loopback hosts, with no
/// userinfo. The server controls this URL and the front end opens it, so
/// anything else (`javascript:`, `file:`, cleartext) is refused. Errors carry
/// no part of the URL.
pub(crate) fn check_auth_url(raw: &str) -> Result<AuthUrl, CoreError> {
    let parsed = url::Url::parse(raw).map_err(|_| CoreError::Login(BAD_AUTH_URL.into()))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || !is_allowed_homeserver_url(raw)
    {
        return Err(CoreError::Login(BAD_AUTH_URL.into()));
    }
    Ok(AuthUrl::from(parsed.to_string()))
}

/// Maps an SDK OAuth error to a fixed, non-secret message. SDK error text can
/// carry URLs, response bodies, and codes.
pub(crate) fn map_oauth_error(e: &OAuthError) -> CoreError {
    use matrix_sdk::authentication::oauth::error::{
        OAuthClientRegistrationError, OAuthDiscoveryError,
    };
    match e {
        OAuthError::Discovery(OAuthDiscoveryError::NotSupported)
        | OAuthError::ClientRegistration(OAuthClientRegistrationError::NotSupported) => {
            CoreError::OAuthNotSupported
        }
        OAuthError::Discovery(OAuthDiscoveryError::Http(_) | OAuthDiscoveryError::Oidc(_)) => {
            CoreError::Network("could not reach the authorization server".into())
        }
        OAuthError::Discovery(_) | OAuthError::ClientRegistration(_) => {
            CoreError::Login("the authorization server could not be used".into())
        }
        OAuthError::AuthorizationCode(_) => {
            CoreError::Login("the authorization was refused or failed".into())
        }
        _ => CoreError::Login("OAuth login failed".into()),
    }
}

/// Starts the redirect listener on `127.0.0.1` only (never a wildcard or
/// public address) and returns the redirect URI with its port.
pub(crate) async fn spawn_redirect_listener()
-> Result<(url::Url, LocalServerRedirectHandle), CoreError> {
    LocalServerBuilder::new()
        .ip_address(LocalServerIpAddress::Localhostv4)
        .spawn()
        .await
        .map_err(|_| CoreError::Login("could not start the local redirect listener".into()))
}

/// Sends `event`, giving up with `Cancelled` when `cancel` fires (or its
/// sender drops) or the receiver is gone, so a full channel cannot block.
async fn send_or_cancel(
    events: &mpsc::Sender<CoreEvent>,
    event: CoreEvent,
    cancel: &mut oneshot::Receiver<()>,
) -> Result<(), CoreError> {
    tokio::select! {
        sent = events.send(event) => sent.map_err(|_| CoreError::Cancelled),
        _ = cancel => Err(CoreError::Cancelled),
    }
}

/// Runs the browser part of an OAuth login on `client`: registers, emits
/// `OAuthUrl`, waits for the redirect, and finishes the login.
///
/// Ends with `Err(Cancelled)` when `cancel` fires (or its sender is dropped)
/// or `REDIRECT_TIMEOUT` passes; the pending authorization is then aborted and
/// the listener shut down (it stops when its handle drops).
pub(crate) async fn authorize(
    client: &Client,
    events: &mpsc::Sender<CoreEvent>,
    mut cancel: oneshot::Receiver<()>,
) -> Result<(), CoreError> {
    let oauth = client.oauth();
    let start = async {
        let (redirect_uri, handle) = spawn_redirect_listener().await?;
        let data = oauth
            .login(redirect_uri, None, Some(client_metadata().into()), None)
            .build()
            .await
            .map_err(|e| map_oauth_error(&e))?;
        Ok::<_, CoreError>((data, handle))
    };
    let (data, handle) = tokio::select! {
        r = start => r?,
        _ = &mut cancel => return Err(CoreError::Cancelled),
    };

    // Policy before the URL leaves the core. The state is dropped on failure
    // with the client.
    let url = match check_auth_url(data.url.as_str()) {
        Ok(url) => url,
        Err(e) => {
            oauth.abort_login(&data.state).await;
            return Err(e);
        }
    };
    // A front end that stopped draining events must not block cancel.
    if let Err(e) = send_or_cancel(events, CoreEvent::OAuthUrl(url), &mut cancel).await {
        oauth.abort_login(&data.state).await;
        return Err(e);
    }

    let query = tokio::select! {
        q = tokio::time::timeout(REDIRECT_TIMEOUT, handle) => q,
        _ = &mut cancel => {
            oauth.abort_login(&data.state).await;
            return Err(CoreError::Cancelled);
        }
    };
    match query {
        // Not raced against `cancel`: dropping it could leave a device logged
        // in on the server. A cancel in this window is handled by the caller,
        // which logs the finished login out; shutdown bounds the wait.
        Ok(Some(query)) => oauth.finish_login(query.into()).await.map_err(|e| match e {
            matrix_sdk::Error::OAuth(e) => map_oauth_error(&e),
            _ => CoreError::Login("OAuth login failed".into()),
        }),
        Ok(None) => {
            oauth.abort_login(&data.state).await;
            Err(CoreError::Login(
                "the login redirect was not received".into(),
            ))
        }
        Err(_) => {
            oauth.abort_login(&data.state).await;
            Err(CoreError::Login("login timed out, try again".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::authentication::oauth::OAuthError;
    use matrix_sdk::authentication::oauth::error::{
        OAuthClientRegistrationError, OAuthDiscoveryError,
    };

    use super::*;
    use crate::api::CoreError;

    #[test]
    fn client_metadata_has_the_required_fields() {
        let json: serde_json::Value = serde_json::from_str(client_metadata().json().get()).unwrap();
        assert_eq!(json["application_type"], "native");
        assert_eq!(json["client_name"], "Tessera");
        assert_eq!(json["client_uri"], "https://github.com/ga-lep/tessera");
        let grants: Vec<&str> = json["grant_types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(grants.contains(&"authorization_code"), "{grants:?}");
        let uris: Vec<&str> = json["redirect_uris"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(uris, ["http://127.0.0.1/", "http://[::1]/"]);
    }

    #[test]
    fn auth_url_policy() {
        for ok in [
            "https://auth.example.org/authorize?state=s",
            "http://127.0.0.1:8080/authorize",
            "http://localhost/authorize",
        ] {
            assert!(check_auth_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://auth.example.org/authorize",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "ftp://auth.example.org/",
            "https://user:pw@auth.example.org/",
            "not a url",
            "",
        ] {
            let err = check_auth_url(bad).expect_err(bad);
            let CoreError::Login(msg) = &err else {
                panic!("{err:?}")
            };
            assert!(!msg.contains("pw@"), "{msg}");
        }
    }

    #[test]
    fn unsupported_servers_map_to_oauth_not_supported() {
        let discovery = OAuthError::Discovery(OAuthDiscoveryError::NotSupported);
        assert_eq!(map_oauth_error(&discovery), CoreError::OAuthNotSupported);
        let registration =
            OAuthError::ClientRegistration(OAuthClientRegistrationError::NotSupported);
        assert_eq!(map_oauth_error(&registration), CoreError::OAuthNotSupported);
    }

    #[test]
    fn other_errors_map_to_fixed_messages() {
        let err = map_oauth_error(&OAuthError::NotRegistered);
        assert!(matches!(err, CoreError::Login(_)), "{err:?}");
        assert_ne!(err, CoreError::OAuthNotSupported);
    }

    #[tokio::test]
    async fn redirect_listener_binds_ipv4_loopback_only() {
        let (uri, _handle) = spawn_redirect_listener().await.unwrap();
        assert_eq!(uri.scheme(), "http");
        assert_eq!(
            uri.host(),
            Some(url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST))
        );
    }

    #[tokio::test]
    async fn send_or_cancel_gives_up_when_the_consumer_stalls() {
        let (tx, _rx) = mpsc::channel(1);
        tx.send(CoreEvent::LoggedOut).await.unwrap(); // the channel is now full
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        cancel_tx.send(()).unwrap();
        let sent = tokio::time::timeout(
            Duration::from_secs(5),
            send_or_cancel(&tx, CoreEvent::LoggedOut, &mut cancel_rx),
        )
        .await
        .expect("must not block on a full channel once cancelled");
        assert_eq!(sent, Err(CoreError::Cancelled));
    }

    #[tokio::test]
    async fn send_or_cancel_sends_when_there_is_room() {
        let (tx, mut rx) = mpsc::channel(1);
        let (_cancel_tx, mut cancel_rx) = oneshot::channel();
        send_or_cancel(&tx, CoreEvent::LoggedOut, &mut cancel_rx)
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(CoreEvent::LoggedOut)));
    }
}
