//! The media fetcher the client uses: it reads a body in chunks and stops at
//! a cap, where the SDK's default fetcher buffers whatever the server sends.
//!
//! A media message declares its size in `info.size`, but the sender writes it
//! and the server serves what it likes, so a declared size only helps if the
//! download itself is bounded. The caller says how much it accepts for one
//! request with [`with_cap`]; a response that announces more (`Content-Length`)
//! is refused before its body is read, and one that streams more is dropped at
//! the cap, so the memory used never passes the cap (an encrypted file is
//! decrypted into a second buffer, so up to twice that while decrypting).
//! The request, the URL building and the endpoint choice follow the SDK's
//! `DefaultMediaFetcher` (matrix-sdk 0.19.1, `src/media.rs`); the SDK media
//! cache still sits in front of this (`Media::get_media_content`).
//!
//! Nothing here logs, and the errors carry no server text: they can quote
//! server data.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::io::{Cursor, Read};

use futures_util::future::BoxFuture;
use matrix_sdk::media::{MediaFetcher, MediaFormat, MediaRequestParameters};
use matrix_sdk::reqwest;
use matrix_sdk::ruma::api::auth_scheme::SendAccessToken;
use matrix_sdk::ruma::api::client::{authenticated_media, media};
use matrix_sdk::ruma::api::{Metadata, OutgoingRequestExt, SupportedVersions};
use matrix_sdk::ruma::events::room::MediaSource;
use matrix_sdk::{Client, Error};
use matrix_sdk_crypto::{AttachmentDecryptor, MediaEncryptionInfo};

use crate::media::MAX_THUMBNAIL_BYTES;

tokio::task_local! {
    static CAP: u64;
}

/// Runs `work` (a `Media::get_media_content` call) with every download in it
/// limited to `cap` bytes. Without it a download is limited to
/// [`MAX_THUMBNAIL_BYTES`], so a caller that forgets fails closed.
pub(crate) fn with_cap<F: Future>(cap: u64, work: F) -> impl Future<Output = F::Output> {
    CAP.scope(cap, work)
}

/// Why a download did not give its bytes. Fixed texts only.
#[derive(Debug, PartialEq, Eq)]
enum Fail {
    /// The request could not be built.
    Request,
    /// The server could not be reached or the body broke off.
    Network,
    /// The server refused the token.
    Unauthorized,
    /// Any other non-success status.
    Status,
    /// The body is, or says it is, over the cap.
    TooBig,
    /// The file did not decrypt.
    Decrypt,
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Request => "the media request could not be built",
            Self::Network => "the media could not be downloaded",
            Self::Unauthorized => "the media server refused the session",
            Self::Status => "the media server did not return the file",
            Self::TooBig => "the media is over the size limit",
            Self::Decrypt => "the media could not be decrypted",
        })
    }
}

impl std::error::Error for Fail {}

impl From<Fail> for Error {
    fn from(fail: Fail) -> Self {
        Self::UnknownError(Box::new(fail))
    }
}

/// Reads a response body in chunks, giving up past `cap` bytes.
async fn read_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>, Fail> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Fail::Unauthorized);
    }
    if !status.is_success() {
        return Err(Fail::Status);
    }
    if response.content_length().is_some_and(|len| len > cap) {
        return Err(Fail::TooBig);
    }
    // Starts small: a lying Content-Length must not reserve memory either.
    let mut body = Vec::with_capacity(usize::try_from(cap.min(256 * 1024)).unwrap_or(0));
    while let Some(chunk) = response.chunk().await.map_err(|_| Fail::Network)? {
        let total = u64::try_from(body.len()).unwrap_or(u64::MAX);
        let next = u64::try_from(chunk.len()).unwrap_or(u64::MAX);
        if total.saturating_add(next) > cap {
            return Err(Fail::TooBig);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Builds one `reqwest` request from a ruma request, with the access token.
/// A macro, not a function: the media endpoints have different request types
/// and the bounds that would unify them are the SDK's private ones.
macro_rules! build {
    ($request:expr, $base:expr, $token:expr, $versions:expr) => {
        $request
            .map_err(|_| Fail::Request)?
            .try_into_http_request::<Vec<u8>>(
                $base,
                SendAccessToken::Always($token),
                Cow::Borrowed($versions),
            )
            .map_err(|_| Fail::Request)
            .and_then(|http| reqwest::Request::try_from(http).map_err(|_| Fail::Request))
    };
}

/// The HTTP request for `request`, on the authenticated media endpoints when
/// the server has them (as the SDK chooses).
fn http_request(
    request: &MediaRequestParameters,
    base: &str,
    token: &str,
    versions: &SupportedVersions,
) -> Result<reqwest::Request, Fail> {
    let auth = authenticated_media::get_content::v1::Request::PATH_BUILDER.is_supported(versions);
    #[allow(deprecated)] // the pre-1.11 endpoints, for servers without authenticated media
    match &request.source {
        MediaSource::Encrypted(file) => {
            if auth {
                build!(
                    authenticated_media::get_content::v1::Request::from_uri(&file.url),
                    base,
                    token,
                    versions
                )
            } else {
                build!(
                    media::get_content::v3::Request::from_url(&file.url),
                    base,
                    token,
                    versions
                )
            }
        }
        MediaSource::Plain(uri) => match &request.format {
            MediaFormat::Thumbnail(settings) => {
                if auth {
                    build!(
                        authenticated_media::get_content_thumbnail::v1::Request::from_uri(
                            uri,
                            settings.width,
                            settings.height,
                        )
                        .map(|mut request| {
                            request.method = Some(settings.method.clone());
                            request.animated = Some(settings.animated);
                            request
                        }),
                        base,
                        token,
                        versions
                    )
                } else {
                    build!(
                        media::get_content_thumbnail::v3::Request::from_url(
                            uri,
                            settings.width,
                            settings.height,
                        )
                        .map(|mut request| {
                            request.method = Some(settings.method.clone());
                            request.animated = Some(settings.animated);
                            request
                        }),
                        base,
                        token,
                        versions
                    )
                }
            }
            _ if auth => build!(
                authenticated_media::get_content::v1::Request::from_uri(uri),
                base,
                token,
                versions
            ),
            _ => build!(
                media::get_content::v3::Request::from_url(uri),
                base,
                token,
                versions
            ),
        },
    }
}

/// Downloads `request`, within `cap` bytes, and decrypts it when it is an
/// encrypted file. A refused token is refreshed once and the request resent.
async fn download(
    client: &Client,
    request: &MediaRequestParameters,
    cap: u64,
) -> Result<Vec<u8>, Fail> {
    let versions = client
        .supported_versions()
        .await
        .map_err(|_| Fail::Network)?;
    let base = client.homeserver().to_string();
    let mut refreshed = false;
    let body = loop {
        let token = client.access_token().ok_or(Fail::Unauthorized)?;
        let http = http_request(request, &base, &token, &versions)?;
        let response = client
            .http_client()
            .execute(http)
            .await
            .map_err(|_| Fail::Network)?;
        match read_capped(response, cap).await {
            Err(Fail::Unauthorized) if !refreshed => {
                refreshed = true;
                client
                    .refresh_access_token()
                    .await
                    .map_err(|_| Fail::Unauthorized)?;
            }
            other => break other?,
        }
    };
    let MediaSource::Encrypted(file) = &request.source else {
        return Ok(body);
    };
    let info = MediaEncryptionInfo::from(file.as_ref().clone());
    // Decrypting is CPU work on up to the cap: off the async threads.
    tokio::task::spawn_blocking(move || {
        let mut cursor = Cursor::new(body);
        let mut reader = AttachmentDecryptor::new(&mut cursor, info).map_err(|_| Fail::Decrypt)?;
        let mut plain = Vec::new();
        reader.read_to_end(&mut plain).map_err(|_| Fail::Decrypt)?;
        Ok(plain)
    })
    .await
    .map_err(|_| Fail::Decrypt)?
}

/// The fetcher to give `ClientBuilder::media_fetcher`.
#[derive(Debug)]
pub(crate) struct CappedMediaFetcher;

impl MediaFetcher for CappedMediaFetcher {
    fn fetch_media_content<'a>(
        &'a self,
        client: &'a Client,
        request: &'a MediaRequestParameters,
    ) -> BoxFuture<'a, Result<Vec<u8>, Error>> {
        Box::pin(async move {
            let cap = CAP.try_with(|cap| *cap).unwrap_or(MAX_THUMBNAIL_BYTES);
            Ok(download(client, request, cap).await?)
        })
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// Serves one canned HTTP response, then closes. `Some(len)` writes a
    /// `Content-Length` header, `None` streams the body unframed until close.
    async fn serve(status: &str, announced: Option<u64>, body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}/", listener.local_addr().expect("addr"));
        let head = match announced {
            Some(len) => {
                format!("HTTP/1.1 {status}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n")
            }
            None => format!("HTTP/1.1 {status}\r\nConnection: close\r\n\r\n"),
        };
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut request = [0_u8; 1024];
            let _ = socket.read(&mut request).await;
            let _ = socket.write_all(head.as_bytes()).await;
            // The client may hang up at the cap: a failed write is fine.
            for chunk in body.chunks(8192) {
                if socket.write_all(chunk).await.is_err() {
                    return;
                }
            }
        });
        url
    }

    async fn get(url: &str) -> reqwest::Response {
        reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("response")
    }

    #[tokio::test]
    async fn a_body_within_the_cap_is_read_whole() {
        let url = serve("200 OK", Some(1000), vec![7; 1000]).await;
        assert_eq!(read_capped(get(&url).await, 1000).await, Ok(vec![7; 1000]));
    }

    #[tokio::test]
    async fn a_body_over_the_cap_is_dropped_at_the_cap() {
        // No Content-Length: only the stream tells.
        let url = serve("200 OK", None, vec![7; 1_000_000]).await;
        assert_eq!(
            read_capped(get(&url).await, 100_000).await,
            Err(Fail::TooBig)
        );
    }

    #[tokio::test]
    async fn an_announced_length_over_the_cap_is_refused_before_the_body() {
        // The body never comes: reading it would hang, so the refusal must
        // not wait for it.
        let url = serve("200 OK", Some(5_000_000), Vec::new()).await;
        assert_eq!(read_capped(get(&url).await, 1000).await, Err(Fail::TooBig));
    }

    #[tokio::test]
    async fn a_lying_small_length_cannot_smuggle_a_big_body() {
        // A server that announces 10 bytes and sends more is stopped by the
        // HTTP layer or by the cap; either way the caller gets no more than
        // the cap.
        let url = serve("200 OK", Some(10), vec![7; 1_000_000]).await;
        let got = read_capped(get(&url).await, 100).await;
        assert!(
            matches!(got, Ok(ref b) if b.len() <= 100) || got.is_err(),
            "{got:?}"
        );
    }

    #[tokio::test]
    async fn statuses_map_to_fixed_failures() {
        let url = serve("401 Unauthorized", Some(0), Vec::new()).await;
        assert_eq!(
            read_capped(get(&url).await, 10).await,
            Err(Fail::Unauthorized)
        );
        let url = serve("404 Not Found", Some(0), Vec::new()).await;
        assert_eq!(read_capped(get(&url).await, 10).await, Err(Fail::Status));
    }

    #[tokio::test]
    async fn the_cap_defaults_closed_and_follows_the_scope() {
        assert!(CAP.try_with(|_| ()).is_err(), "no scope outside with_cap");
        let inside = with_cap(42, async { CAP.try_with(|cap| *cap) }).await;
        assert_eq!(inside, Ok(42));
    }

    #[test]
    fn failures_carry_no_server_text() {
        for fail in [
            Fail::Request,
            Fail::Network,
            Fail::Unauthorized,
            Fail::Status,
            Fail::TooBig,
            Fail::Decrypt,
        ] {
            assert!(!fail.to_string().is_empty());
        }
    }
}
