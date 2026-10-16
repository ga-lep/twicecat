//! The update check (Phase 7 plan Task 9, Q7 a): one request to GitHub's
//! "latest release" API for this repository. Nothing is downloaded or run;
//! the app shows a link to the release page. Checks run once a day while the
//! user has them on ([`run_schedule`]); the app keeps the opt-in and the time
//! of the last check with its settings.
//!
//! Never logs the reply, and its errors never carry server text.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use matrix_sdk::reqwest;
use tokio::sync::mpsc;

use crate::api::{CoreEvent, is_newer_version, update_check_due};

/// The newest release, as GitHub names it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Release {
    /// The tag, such as `v1.2.0`.
    pub tag: String,
    /// Its page on github.com.
    pub url: String,
}

/// Why a check gave no answer (fixed texts only).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fail {
    Unreachable,
    Status(u16),
    TooLarge,
    Malformed,
}

const TIMEOUT: Duration = Duration::from_secs(20);
/// GitHub's reply for one release is a few kilobytes, more with long notes.
const REPLY_CAP: usize = 1 << 20;

/// `https://github.com/<owner>/<repo>`, from the workspace's `repository`.
fn repository() -> &'static str {
    env!("CARGO_PKG_REPOSITORY").trim_end_matches('/')
}

/// The API endpoint for this repository's latest release.
fn endpoint() -> String {
    let path = repository().trim_start_matches("https://github.com/");
    format!("https://api.github.com/repos/{path}/releases/latest")
}

/// Asks `endpoint` for the latest release; its page must be
/// `<repository>/releases/tag/<tag>`.
pub(crate) async fn latest_release(endpoint: &str, repository: &str) -> Result<Release, Fail> {
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        // GitHub refuses API requests without a user agent.
        .user_agent(concat!("twicecat/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| Fail::Unreachable)?;
    let mut response = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| Fail::Unreachable)?;
    let status = response.status();
    if !status.is_success() {
        return Err(Fail::Status(status.as_u16()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Fail::Unreachable)? {
        if body.len() + chunk.len() > REPLY_CAP {
            return Err(Fail::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    parse_release(&body, repository)
}

/// `tag_name` and `html_url` from GitHub's release object.
pub(crate) fn parse_release(body: &[u8], repository: &str) -> Result<Release, Fail> {
    let value: serde_json::Value = serde_json::from_slice(body).map_err(|_| Fail::Malformed)?;
    let field = |name: &str| value.get(name).and_then(|v| v.as_str()).map(str::to_owned);
    let (Some(tag), Some(url)) = (field("tag_name"), field("html_url")) else {
        return Err(Fail::Malformed);
    };
    // The only link ever shown: this repository's page for that tag, as
    // GitHub writes it.
    let tag_ok = !tag.is_empty()
        && tag.len() <= 64
        && tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    let page = format!("{}/releases/tag/{tag}", repository.trim_end_matches('/'));
    if !tag_ok || url != page {
        return Err(Fail::Malformed);
    }
    Ok(Release { tag, url })
}

/// The event for a check's outcome against `current`; `None` when it failed
/// (logged without detail; the app tries again the next day).
pub(crate) fn outcome(result: Result<Release, Fail>, current: &str) -> Option<CoreEvent> {
    match result {
        Ok(release) if is_newer_version(&release.tag, current) => {
            let version = release.tag.trim_start_matches('v').to_owned();
            Some(CoreEvent::UpdateAvailable {
                version,
                url: release.url,
                installable: false,
            })
        }
        Ok(_) => Some(CoreEvent::UpToDate),
        Err(fail) => {
            tracing::warn!(?fail, "the update check got no answer");
            None
        }
    }
}

/// The check the engine runs: an AppImage that can update itself reads the
/// signed manifest of the latest release (installable), any other copy asks
/// GitHub's API (a link only).
pub(crate) async fn check() -> Option<CoreEvent> {
    #[cfg(target_os = "linux")]
    if crate::appimage_update::available() {
        let checked = tokio::task::spawn_blocking(crate::appimage_update::check_blocking).await;
        return match checked {
            Ok(Ok(Some(update))) => Some(CoreEvent::UpdateAvailable {
                url: format!("{}/releases/tag/v{}", repository(), update.version),
                version: update.version,
                installable: true,
            }),
            Ok(Ok(None)) => Some(CoreEvent::UpToDate),
            Ok(Err(fail)) => {
                tracing::warn!(?fail, "the AppImage's update check got no answer");
                None
            }
            Err(_) => None,
        };
    }
    let result = latest_release(&endpoint(), repository()).await;
    outcome(result, env!("CARGO_PKG_VERSION"))
}

/// `Command::InstallUpdate`: the AppImage's updater, or a failure anywhere
/// else.
pub(crate) async fn install() -> CoreEvent {
    #[cfg(target_os = "linux")]
    {
        match tokio::task::spawn_blocking(crate::appimage_update::install_blocking).await {
            Ok(Ok(version)) => CoreEvent::UpdateInstalled { version },
            Ok(Err(fail)) => {
                tracing::warn!(?fail, "the AppImage update was not installed");
                CoreEvent::UpdateInstallFailed
            }
            Err(_) => CoreEvent::UpdateInstallFailed,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CoreEvent::UpdateInstallFailed
    }
}

/// Seconds since the Unix epoch on the wall clock.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The longest sleep between two looks at the wall clock: a sleep counts
/// monotonic time, which stops while the computer sleeps, and the date can
/// change.
const LOOK_EVERY: Duration = Duration::from_secs(60 * 60);

/// Checks once a day until aborted: the first time when a check is due after
/// `last`, announced by `UpdateChecked` (so the app keeps the time even when
/// GitHub gives no answer), then `check`'s answer. Ends when the event
/// channel closes.
pub(crate) async fn run_schedule<C, F>(
    mut last: u64,
    now: fn() -> u64,
    check: C,
    events: mpsc::Sender<CoreEvent>,
) where
    C: Fn() -> F,
    F: Future<Output = Option<CoreEvent>>,
{
    loop {
        let at = now();
        if update_check_due(last, at) {
            last = at;
            if events.send(CoreEvent::UpdateChecked { at }).await.is_err() {
                return;
            }
            if let Some(event) = check().await
                && events.send(event).await.is_err()
            {
                return;
            }
        }
        tokio::time::sleep(LOOK_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const REPO: &str = "https://github.com/ga-lep/twicecat";

    fn release(tag: &str, url: &str) -> Vec<u8> {
        format!(r#"{{"tag_name":"{tag}","html_url":"{url}","body":"notes","assets":[]}}"#)
            .into_bytes()
    }

    #[test]
    fn the_endpoint_is_this_repositorys_latest_release() {
        assert_eq!(
            endpoint(),
            "https://api.github.com/repos/ga-lep/twicecat/releases/latest"
        );
    }

    #[test]
    fn a_release_must_link_to_this_repositorys_releases() {
        let ok = release(
            "v0.2.0",
            "https://github.com/ga-lep/twicecat/releases/tag/v0.2.0",
        );
        assert_eq!(
            parse_release(&ok, REPO),
            Ok(Release {
                tag: "v0.2.0".into(),
                url: "https://github.com/ga-lep/twicecat/releases/tag/v0.2.0".into()
            })
        );
        for (tag, url) in [
            (
                "v0.2.0",
                "https://github.com/someone-else/twicecat/releases/tag/v0.2.0",
            ),
            (
                "v0.2.0",
                "https://github.com/ga-lep/twicecat-evil/releases/tag/v0.2.0",
            ),
            (
                "v0.2.0",
                "http://github.com/ga-lep/twicecat/releases/tag/v0.2.0",
            ),
            (
                "v0.2.0",
                "https://github.com/ga-lep/twicecat/releases/tag/v0.3.0",
            ),
            (
                "v0.2.0",
                "https://github.com/ga-lep/twicecat/releases/tag/v0.2.0?x=1",
            ),
            (
                "../../x",
                "https://github.com/ga-lep/twicecat/releases/tag/../../x",
            ),
            ("", "https://github.com/ga-lep/twicecat/releases/tag/"),
        ] {
            assert_eq!(
                parse_release(&release(tag, url), REPO),
                Err(Fail::Malformed),
                "{url}"
            );
        }
        assert_eq!(parse_release(b"not json", REPO), Err(Fail::Malformed));
        assert_eq!(
            parse_release(br#"{"tag_name":"v1.0.0"}"#, REPO),
            Err(Fail::Malformed)
        );
        assert_eq!(
            parse_release(br#"{"tag_name":1,"html_url":2}"#, REPO),
            Err(Fail::Malformed)
        );
    }

    #[test]
    fn only_a_newer_release_is_announced() {
        let url = "https://github.com/ga-lep/twicecat/releases/tag/v0.2.0".to_owned();
        let newer = Release {
            tag: "v0.2.0".into(),
            url: url.clone(),
        };
        assert!(matches!(
            outcome(Ok(newer), "0.1.0"),
            Some(CoreEvent::UpdateAvailable { version, url: u, installable: false })
                if version == "0.2.0" && u == url
        ));
        let same = Release {
            tag: "v0.1.0".into(),
            url: url.clone(),
        };
        assert!(matches!(
            outcome(Ok(same), "0.1.0"),
            Some(CoreEvent::UpToDate)
        ));
        let odd = Release {
            tag: "nightly".into(),
            url,
        };
        assert!(matches!(
            outcome(Ok(odd), "0.1.0"),
            Some(CoreEvent::UpToDate)
        ));
        assert!(outcome(Err(Fail::Unreachable), "0.1.0").is_none());
    }

    use std::sync::atomic::{AtomicU64, Ordering};

    /// The schedule's wall clock in the test below.
    static CLOCK: AtomicU64 = AtomicU64::new(0);

    fn clock() -> u64 {
        CLOCK.load(Ordering::SeqCst)
    }

    /// Moves the wall clock and tokio's paused clock together, then lets the
    /// schedule run.
    async fn pass(seconds: u64) {
        CLOCK.fetch_add(seconds, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(seconds)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    fn drain(rx: &mut mpsc::Receiver<CoreEvent>) -> Vec<String> {
        let mut seen = Vec::new();
        while let Ok(event) = rx.try_recv() {
            seen.push(match event {
                CoreEvent::UpdateChecked { at } => format!("checked {at}"),
                CoreEvent::UpToDate => "up to date".to_owned(),
                other => format!("{other:?}"),
            });
        }
        seen
    }

    #[tokio::test(start_paused = true)]
    async fn the_schedule_checks_when_due_then_once_a_day() {
        let day = 24 * 60 * 60;
        let start = 1_800_000_000;
        CLOCK.store(start, Ordering::SeqCst);
        let (tx, mut rx) = mpsc::channel(16);
        // Last checked 23 hours ago: the first check is an hour away.
        let task = tokio::spawn(run_schedule(
            start - 23 * 3_600,
            clock,
            || async { Some(CoreEvent::UpToDate) },
            tx,
        ));
        pass(0).await;
        assert!(drain(&mut rx).is_empty());
        pass(3_600).await;
        assert_eq!(
            drain(&mut rx),
            [
                format!("checked {}", start + 3_600),
                "up to date".to_owned()
            ]
        );
        // Nothing more for a day, then the next one.
        for _ in 0..23 {
            pass(3_600).await;
        }
        assert!(drain(&mut rx).is_empty());
        pass(3_600).await;
        assert_eq!(
            drain(&mut rx),
            [
                format!("checked {}", start + 3_600 + day),
                "up to date".to_owned()
            ]
        );
        // The app turned the check off.
        task.abort();
        pass(day).await;
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_check_still_counts_and_a_closed_channel_ends_it() {
        static CLOCK2: AtomicU64 = AtomicU64::new(1_900_000_000);
        fn clock2() -> u64 {
            CLOCK2.load(Ordering::SeqCst)
        }
        let at = clock2();
        let (tx, mut rx) = mpsc::channel(16);
        // Never checked; GitHub gives no answer.
        let task = tokio::spawn(run_schedule(0, clock2, || async { None }, tx));
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::UpdateChecked { at: t }) if t == at));
        assert!(rx.try_recv().is_err());
        // The app is gone: the next check finds the channel closed and ends.
        drop(rx);
        CLOCK2.fetch_add(25 * 3_600, Ordering::SeqCst);
        tokio::time::advance(Duration::from_secs(25 * 3_600)).await;
        assert!(task.await.is_ok());
    }

    /// One HTTP reply from a server on localhost; returns its URL.
    async fn serve(status: &str, body: Vec<u8>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let status = status.to_owned();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let n = socket.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_lowercase();
            // The headers GitHub needs.
            assert!(request.contains("user-agent: twicecat/"));
            assert!(request.contains("accept: application/vnd.github+json"));
            let head = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        format!("http://{addr}/releases/latest")
    }

    #[tokio::test]
    async fn a_check_reads_the_reply_and_its_failures() {
        let page = "https://github.com/ga-lep/twicecat/releases/tag/v9.0.0";
        let url = serve("200 OK", release("v9.0.0", page)).await;
        assert_eq!(
            latest_release(&url, REPO).await.map(|r| r.tag),
            Ok("v9.0.0".to_owned())
        );

        let url = serve("404 Not Found", br#"{"message":"Not Found"}"#.to_vec()).await;
        assert_eq!(latest_release(&url, REPO).await, Err(Fail::Status(404)));

        let url = serve("200 OK", vec![b' '; REPLY_CAP + 1]).await;
        assert_eq!(latest_release(&url, REPO).await, Err(Fail::TooLarge));

        // Nothing listens on port 9 of localhost here.
        assert_eq!(
            latest_release("http://127.0.0.1:9/", REPO).await,
            Err(Fail::Unreachable)
        );
    }
}
