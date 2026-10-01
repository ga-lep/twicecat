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

/// Fixed, non-secret message for a login that returned another account.
pub(crate) const WRONG_ACCOUNT: &str = "the homeserver returned a different account";

/// The one server name the local test homeserver (`docker-compose.yml`,
/// `HOMESERVER_DOMAIN` in `.env.test`) serves. See [`server_name_bound`].
const LOCAL_SERVER_NAME: &str = "localhost";

/// How long the well-known lookup for a returned server name may take.
const DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether the user ID in a login response is the account the user asked for.
///
/// A homeserver must not be able to make us store a session under another
/// account's key. `username` is what the user typed: a full MXID needs exact
/// equality with `returned`; a bare localpart needs an equal localpart. Which
/// server the returned user ID belongs to is judged separately, by
/// [`server_name_bound`]. Comparison is exact on the parsed parts (no case
/// folding).
pub(crate) fn login_account_matches(username: &str, returned: &str) -> bool {
    let Some((local, server)) = returned.strip_prefix('@').and_then(|r| r.split_once(':')) else {
        return false;
    };
    if local.is_empty() || server.is_empty() {
        return false;
    }
    if username.starts_with('@') {
        return username == returned;
    }
    local == username
}

/// The server name part of a user ID (`@local:server` gives `server`).
pub(crate) fn server_name_of(user_id: &str) -> Option<&str> {
    user_id
        .strip_prefix('@')
        .and_then(|r| r.split_once(':'))
        .map(|(_, s)| s)
        .filter(|s| !s.is_empty())
}

/// Whether two URLs have the same origin: scheme, host and port (default
/// ports filled in). Hosts are already lower-cased by the URL parser.
pub(crate) fn same_origin(a: &url::Url, b: &url::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host() == b.host()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Whether `url` is a plain-http loopback URL (the local test homeserver).
fn is_loopback_http(url: &url::Url) -> bool {
    url.scheme() == "http" && is_allowed_homeserver_url(url.as_str())
}

/// Whether the homeserver actually used (`homeserver`) is the one that serves
/// the server name of the returned user ID (`returned_server`).
///
/// Without this a malicious homeserver could return `@alice:matrix.org` and
/// make us store (and later overwrite) the record of that account.
///
/// Accepted when:
/// 1. `typed_server` is `Some(name)` (the client was built by discovery from
///    the bare server name the user typed) and `name == returned_server`: the
///    homeserver is by construction the one that name delegates to; or
/// 2. narrow local-test exception: `homeserver` is a loopback http URL and
///    `returned_server` is [`LOCAL_SERVER_NAME`]; or
/// 3. `discover(returned_server)` yields a homeserver URL of the same origin
///    as `homeserver` (delegation: `example.org` served by
///    `matrix-client.example.org`).
///
/// Discovery failure (`None`) refuses. `discover` is injected so tests never
/// touch the network.
pub(crate) async fn server_name_bound<F, Fut>(
    typed_server: Option<&str>,
    homeserver: &url::Url,
    returned_server: &str,
    discover: F,
) -> bool
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Option<url::Url>>,
{
    if typed_server == Some(returned_server) {
        return true;
    }
    if is_loopback_http(homeserver) && returned_server == LOCAL_SERVER_NAME {
        return true;
    }
    match discover(returned_server.to_owned()).await {
        Some(found) => same_origin(&found, homeserver),
        None => false,
    }
}

/// Whether the user ID in a login response (`returned`) may be adopted.
///
/// It must match what the user typed (`username`, password login only) and
/// its server name must be bound to the homeserver used. An OAuth login has no
/// `username`, so there the only account binding is [`server_name_bound`]: a
/// user ID from a server name that neither equals the typed name nor
/// delegates to the homeserver used is refused.
pub(crate) async fn account_bound<F, Fut>(
    returned: Option<&str>,
    username: Option<&str>,
    typed_server: Option<&str>,
    homeserver: &url::Url,
    discover: F,
) -> bool
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Option<url::Url>>,
{
    match returned {
        Some(r) if username.is_none_or(|u| login_account_matches(u, r)) => {
            match server_name_of(r) {
                Some(name) => server_name_bound(typed_server, homeserver, name, discover).await,
                None => false,
            }
        }
        _ => false,
    }
}

/// Resolves the homeserver of `server_name` by `.well-known` discovery, using
/// a throwaway client with the SDK's default in-memory store (nothing is
/// persisted). `ClientBuilder::server_name` (matrix-sdk-0.19.1
/// `src/client/builder/mod.rs:235`) always performs the lookup
/// (`homeserver_config.rs:63-87`, `discover_homeserver` at `:193`) and has no
/// fallback to treating the name as the homeserver. The result must also pass
/// the transport policy. `None` on any failure.
pub(crate) async fn discover_homeserver_url(server_name: String) -> Option<url::Url> {
    let name = matrix_sdk::ServerName::parse(&server_name).ok()?;
    let build = Client::builder().server_name(&name).build();
    let client = tokio::time::timeout(DISCOVERY_TIMEOUT, build)
        .await
        .ok()?
        .ok()?;
    let url = client.homeserver();
    is_allowed_homeserver_url(url.as_str()).then_some(url)
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

    #[test]
    fn full_mxid_needs_exact_equality() {
        assert!(login_account_matches("@a:x.org", "@a:x.org"));
        assert!(!login_account_matches("@a:x.org", "@a:matrix.org"));
        assert!(!login_account_matches("@a:x.org", "@b:x.org"));
        assert!(!login_account_matches("@a:x.org", "@A:x.org"));
    }

    #[test]
    fn localpart_needs_equal_localpart() {
        assert!(login_account_matches("a", "@a:x.org"));
        assert!(!login_account_matches("a", "@b:x.org"));
        assert!(!login_account_matches("a", "@A:x.org"));
    }

    #[test]
    fn malformed_returned_ids_never_match() {
        assert!(!login_account_matches("a", "a"));
        assert!(!login_account_matches("a", "@a"));
        assert!(!login_account_matches("a", ""));
        assert!(!login_account_matches("a", "@a:"));
    }

    #[test]
    fn server_name_of_splits_on_first_colon() {
        assert_eq!(server_name_of("@a:x.org"), Some("x.org"));
        assert_eq!(server_name_of("@a:x.org:8448"), Some("x.org:8448"));
        assert_eq!(server_name_of("@a:"), None);
        assert_eq!(server_name_of("a:x.org"), None);
    }

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[test]
    fn same_origin_compares_scheme_host_and_port() {
        assert!(same_origin(&u("https://h.org"), &u("https://h.org/")));
        assert!(same_origin(&u("https://h.org"), &u("https://H.ORG:443/x")));
        assert!(same_origin(&u("http://h.org"), &u("http://h.org:80")));
        assert!(!same_origin(&u("https://h.org"), &u("http://h.org")));
        assert!(!same_origin(&u("https://h.org"), &u("https://h.org:8448")));
        assert!(!same_origin(&u("https://h.org"), &u("https://evil.org")));
        assert!(!same_origin(&u("https://h.org"), &u("https://sub.h.org")));
    }

    /// A discovery stub; panics if called when `expect_call` is false.
    async fn bound(
        typed: Option<&str>,
        homeserver: &str,
        returned: &str,
        found: Option<&str>,
        expect_call: bool,
    ) -> bool {
        let found = found.map(u);
        server_name_bound(typed, &u(homeserver), returned, |name| async move {
            assert!(expect_call, "discovery must not run");
            assert!(!name.is_empty());
            found
        })
        .await
    }

    /// OAuth has no typed username: the account binding rests on
    /// `server_name_bound` alone, so a user ID from another server name must be
    /// refused unless that name delegates to the homeserver used.
    #[tokio::test]
    async fn oauth_account_from_another_server_name_is_refused() {
        let no_discovery = |_| async {
            unreachable!("discovery must not run");
            #[allow(unreachable_code)] // gives the async block its output type
            None
        };
        // Bare name typed, but the server returned another server's account.
        assert!(
            !account_bound(
                Some("@alice:matrix.org"),
                None,
                Some("evil.example"),
                &u("https://evil.example"),
                |_| async { Some(u("https://matrix-client.matrix.org")) },
            )
            .await
        );
        // Same with a URL typed (no bare name) and failing discovery.
        assert!(
            !account_bound(
                Some("@alice:matrix.org"),
                None,
                None,
                &u("https://evil.example"),
                |_| async { None },
            )
            .await
        );
        // The matching server name is accepted without discovery.
        assert!(
            account_bound(
                Some("@alice:example.org"),
                None,
                Some("example.org"),
                &u("https://matrix-client.example.org"),
                no_discovery,
            )
            .await
        );
        // No user ID at all is refused.
        assert!(
            !account_bound(
                None,
                None,
                Some("example.org"),
                &u("https://h.org"),
                |_| async { None }
            )
            .await
        );
    }

    #[tokio::test]
    async fn typed_server_name_equal_to_returned_is_accepted_without_discovery() {
        assert!(
            bound(
                Some("example.org"),
                "https://matrix-client.example.org",
                "example.org",
                None,
                false
            )
            .await
        );
    }

    #[tokio::test]
    async fn gap_a_localpart_with_evil_url_is_refused() {
        // "alice" + https://evil.example, which returns @alice:matrix.org.
        // No typed server name (a URL was used); matrix.org does not
        // delegate to evil.example.
        assert!(
            !bound(
                None,
                "https://evil.example",
                "matrix.org",
                Some("https://matrix-client.matrix.org"),
                true
            )
            .await
        );
        assert!(!bound(None, "https://evil.example", "matrix.org", None, true).await);
    }

    #[tokio::test]
    async fn gap_b_full_mxid_with_other_server_is_refused() {
        // "@alice:matrix.org" typed, server evil.example used, echoing it.
        assert!(
            !bound(
                Some("evil.example"),
                "https://evil.example",
                "matrix.org",
                Some("https://matrix-client.matrix.org"),
                true
            )
            .await
        );
        // Discovery failing refuses too.
        assert!(
            !bound(
                Some("evil.example"),
                "https://evil.example",
                "matrix.org",
                None,
                true
            )
            .await
        );
    }

    #[tokio::test]
    async fn delegated_homeserver_is_accepted_after_discovery() {
        assert!(
            bound(
                None,
                "https://matrix-client.example.org",
                "example.org",
                Some("https://matrix-client.example.org/"),
                true
            )
            .await
        );
        // Also when typed another name that delegates to the same server.
        assert!(
            bound(
                Some("alias.example.net"),
                "https://matrix-client.example.org",
                "example.org",
                Some("https://matrix-client.example.org:443"),
                true
            )
            .await
        );
    }

    #[tokio::test]
    async fn discovery_to_another_origin_is_refused() {
        for found in [
            "https://other.example.org",
            "http://matrix-client.example.org",
            "https://matrix-client.example.org:8448",
        ] {
            assert!(
                !bound(
                    None,
                    "https://matrix-client.example.org",
                    "example.org",
                    Some(found),
                    true
                )
                .await,
                "{found}"
            );
        }
    }

    #[tokio::test]
    async fn local_test_homeserver_exception_is_narrow() {
        // Loopback http + the configured local name: no discovery.
        for hs in ["http://localhost:8008", "http://127.0.0.1:8008"] {
            assert!(bound(None, hs, "localhost", None, false).await, "{hs}");
        }
        // Loopback http but another server name: needs discovery (fails).
        assert!(!bound(None, "http://localhost:8008", "matrix.org", None, true).await);
        // The name "localhost" on a remote https server: not exempt.
        assert!(!bound(None, "https://evil.example", "localhost", None, true).await);
    }

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
