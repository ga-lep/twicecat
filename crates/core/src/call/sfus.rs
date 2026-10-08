//! Multi-SFU (Element Call's mode): one subscribe-only connection per
//! other member's SFU (their own call service, e.g. another homeserver's),
//! opened and closed as members come and go, retried with backoff.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use matrix_sdk::Client;
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinSet};

use super::jwt;
use super::media::{ConnectionShared, MediaNotice, SfuConnection};
use crate::api::CoreError;

/// Retry delays for another member's SFU: doubling from the first, capped.
const REMOTE_RETRY_FIRST: Duration = Duration::from_secs(2);
const REMOTE_RETRY_MAX: Duration = Duration::from_secs(60);

/// Call service URL → LiveKit identities expected on its SFU.
pub(super) type Expected = BTreeMap<String, BTreeSet<String>>;
/// Connected SFUs of other members, by call service URL.
pub(super) type Remotes = Arc<StdMutex<HashMap<String, SfuConnection>>>;

/// What the SFU manager needs.
pub(super) struct SfuManager {
    pub(super) client: Client,
    pub(super) room_id: String,
    pub(super) own_service: String,
    pub(super) shared: ConnectionShared,
    pub(super) remotes: Remotes,
}

/// One other member's SFU we want.
struct Remote {
    /// Who to play there.
    expected: watch::Sender<BTreeSet<String>>,
    /// A connection attempt in flight.
    connecting: Option<AbortHandle>,
    /// After a failure: when to try again, and how many attempts failed.
    retry: Option<(Instant, u32)>,
}

fn retry_delay(failures: u32) -> Duration {
    REMOTE_RETRY_FIRST
        .saturating_mul(1 << failures.min(6))
        .min(REMOTE_RETRY_MAX)
}

/// Keeps one subscribe-only connection per other member's SFU: opens them
/// as members appear, retries failures with backoff, closes the ones nobody
/// publishes to any more, and tells each connection who to play. Notices
/// from our own SFU go to the engine; a lost remote SFU is reconnected.
pub(super) async fn manage_sfus(
    m: SfuManager,
    own_expected: watch::Sender<BTreeSet<String>>,
    mut expected: watch::Receiver<Expected>,
    mut notices: mpsc::Receiver<(String, MediaNotice)>,
    to_engine: mpsc::Sender<MediaNotice>,
) {
    let mut wanted: HashMap<String, Remote> = HashMap::new();
    let mut attempts: JoinSet<(String, Result<SfuConnection, CoreError>)> = JoinSet::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        let now = expected.borrow_and_update().clone();
        let own = now.get(&m.own_service).cloned().unwrap_or_default();
        own_expected.send_if_modified(|e| {
            let changed = *e != own;
            *e = own;
            changed
        });
        // SFUs nobody publishes to any more.
        let gone: Vec<String> = wanted
            .keys()
            .filter(|url| !now.contains_key(*url))
            .cloned()
            .collect();
        for url in gone {
            if let Some(handle) = wanted.remove(&url).and_then(|r| r.connecting) {
                handle.abort();
            }
            let closed = m.remotes.lock().expect("remotes lock").remove(&url);
            if let Some(connection) = closed {
                connection.close().await;
            }
        }
        for (url, ids) in now.iter().filter(|(url, _)| **url != m.own_service) {
            let remote = wanted.entry(url.clone()).or_insert_with(|| Remote {
                expected: watch::channel(BTreeSet::new()).0,
                connecting: None,
                retry: None,
            });
            remote.expected.send_if_modified(|e| {
                let changed = e != ids;
                e.clone_from(ids);
                changed
            });
            let connected = m.remotes.lock().expect("remotes lock").contains_key(url);
            let due = remote.retry.is_none_or(|(at, _)| at <= Instant::now());
            if !connected && remote.connecting.is_none() && due {
                let client = m.client.clone();
                let room_id = m.room_id.clone();
                let url = url.clone();
                let rx = remote.expected.subscribe();
                let shared = m.shared.clone();
                remote.connecting = Some(attempts.spawn(async move {
                    let result = match jwt::subscribe_access(&client, &url, &room_id).await {
                        Ok(sfu) => {
                            SfuConnection::connect(url.clone(), &sfu, None, rx, &shared).await
                        }
                        Err(e) => Err(e),
                    };
                    (url, result)
                }));
            }
        }
        tokio::select! {
            changed = expected.changed() => if changed.is_err() { return },
            Some(done) = attempts.join_next() => {
                // An aborted attempt (its SFU no longer wanted) ends here too.
                if let Ok((url, result)) = done {
                    match (wanted.get_mut(&url), result) {
                        (Some(remote), Ok(connection)) => {
                            remote.connecting = None;
                            remote.retry = None;
                            m.remotes.lock().expect("remotes lock").insert(url, connection);
                        }
                        (Some(remote), Err(_)) => {
                            remote.connecting = None;
                            let failures = remote.retry.map_or(0, |(_, n)| n);
                            remote.retry = Some((Instant::now() + retry_delay(failures), failures + 1));
                            tracing::warn!("could not connect to another member's call server; retrying");
                        }
                        (None, Ok(connection)) => connection.close().await,
                        (None, Err(_)) => {}
                    }
                }
            }
            Some((url, notice)) = notices.recv() => {
                if url == m.own_service {
                    let _ = to_engine.send(notice).await;
                } else if notice == MediaNotice::Disconnected {
                    let lost = m.remotes.lock().expect("remotes lock").remove(&url);
                    if let Some(connection) = lost {
                        connection.close().await;
                    }
                    if let Some(remote) = wanted.get_mut(&url) {
                        remote.retry = Some((Instant::now() + REMOTE_RETRY_FIRST, 1));
                    }
                }
            }
            _ = ticker.tick() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_double_from_the_first_and_are_capped() {
        assert_eq!(retry_delay(0), REMOTE_RETRY_FIRST);
        assert_eq!(retry_delay(1), REMOTE_RETRY_FIRST * 2);
        assert_eq!(retry_delay(4), REMOTE_RETRY_FIRST * 16);
        assert_eq!(retry_delay(5), REMOTE_RETRY_MAX);
        assert_eq!(retry_delay(u32::MAX), REMOTE_RETRY_MAX);
    }
}
