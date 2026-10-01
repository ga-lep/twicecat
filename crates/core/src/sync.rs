//! Mapping of the SDK's sync service state to the public [`SyncState`].

use std::time::Duration;

use futures_util::{Stream, StreamExt as _};
use matrix_sdk_ui::sync_service::State;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::api::{CoreEvent, SyncState};

/// Maps an SDK state to the API state.
///
/// | SDK `State`  | API `SyncState`                     |
/// |--------------|-------------------------------------|
/// | `Idle`       | `Idle`                              |
/// | `Running`    | `Running`                           |
/// | `Offline`    | `Offline`                           |
/// | `Terminated` | `Terminated`                        |
/// | `Error(_)`   | `Error("sync failed")` (fixed text) |
///
/// The SDK error text is not forwarded: it can carry server-controlled data.
pub(crate) fn map_state(state: &State) -> SyncState {
    match state {
        State::Idle => SyncState::Idle,
        State::Running => SyncState::Running,
        State::Offline => SyncState::Offline,
        State::Terminated => SyncState::Terminated,
        State::Error(_) => SyncState::Error("sync failed".into()),
    }
}

/// First reconnect delay.
const BACKOFF_BASE: Duration = Duration::from_secs(1);
/// Upper bound of the delay before jitter.
const BACKOFF_CAP: Duration = Duration::from_secs(60);
/// Jitter spread: the delay is scaled by a factor in `[1 - J, 1 + J]`.
const JITTER: f64 = 0.2;

/// Delay before reconnect attempt number `attempt` (1-based; 0 counts as 1).
///
/// The base delay doubles each attempt (1 s, 2 s, 4 s, ...) and is capped at
/// 60 s. Jitter is applied after the cap, so the result lies in
/// `[0.8 * base, 1.2 * base]`, at most 72 s. `jitter_unit` is a uniform sample
/// in `[0, 1]` (values outside are clamped); 0.5 means no jitter.
pub(crate) fn backoff_delay(attempt: u32, jitter_unit: f64) -> Duration {
    // 2^6 = 64 s already exceeds the cap, so a bounded exponent is enough and
    // cannot overflow.
    let exponent = attempt.saturating_sub(1).min(6);
    let base = (BACKOFF_BASE * 2u32.pow(exponent)).min(BACKOFF_CAP);
    let unit = if jitter_unit.is_nan() {
        0.5
    } else {
        jitter_unit.clamp(0.0, 1.0)
    };
    base.mul_f64(1.0 - JITTER + 2.0 * JITTER * unit)
}

/// What the supervisor needs from the sync service. A trait so that the loop
/// can be tested without a network.
pub(crate) trait SyncControl {
    /// Starts (or restarts) the service.
    async fn restart(&self);
    /// Waits for `delay`.
    async fn sleep(&self, delay: Duration);
    /// A uniform random sample in `[0, 1)`.
    fn jitter_unit(&self) -> f64;
}

/// Forwards sync state changes as events and restarts the service after a
/// failure.
///
/// - `Running` resets the attempt count.
/// - `Error` or an unexpected `Terminated` emits `Reconnecting { attempt }`,
///   waits `backoff_delay`, then calls `restart`. (The engine's own `stop`
///   makes the service go `Idle`, and the engine aborts this task first.)
/// - `Offline` is left to the SDK's offline mode and only reported.
///
/// Returns when the stream ends or the event receiver is gone.
pub(crate) async fn supervise<C, S>(
    control: &C,
    initial: State,
    states: S,
    events: &mpsc::Sender<CoreEvent>,
) where
    C: SyncControl,
    S: Stream<Item = State> + Unpin,
{
    let mut states = states;
    let mut pending = Some(initial);
    let mut last: Option<SyncState> = None;
    let mut attempt: u32 = 0;
    loop {
        let state = match pending.take() {
            Some(state) => state,
            None => match states.next().await {
                Some(state) => state,
                None => return,
            },
        };
        let mut reconnect = false;
        let next = match state {
            State::Error(_) | State::Terminated => {
                attempt = attempt.saturating_add(1);
                reconnect = true;
                SyncState::Reconnecting { attempt }
            }
            State::Running => {
                attempt = 0;
                map_state(&state)
            }
            State::Idle | State::Offline => map_state(&state),
        };
        if last.as_ref() != Some(&next) {
            last = Some(next.clone());
            if events.send(CoreEvent::SyncState(next)).await.is_err() {
                return;
            }
        }
        if reconnect {
            let delay = backoff_delay(attempt, control.jitter_unit());
            warn!(
                attempt,
                delay_ms = delay.as_millis() as u64,
                "sync failed; reconnecting"
            );
            control.sleep(delay).await;
            control.restart().await;
            info!(attempt, "sync restart requested");
        }
    }
}
#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use matrix_sdk_ui::sync_service::Error;

    use super::*;

    #[test]
    fn maps_a_fake_state_sequence() {
        let sequence = [
            State::Idle,
            State::Running,
            State::Offline,
            State::Running,
            State::Error(Arc::new(Error::Supervisor)),
            State::Running,
            State::Terminated,
        ];
        let mapped: Vec<SyncState> = sequence.iter().map(map_state).collect();
        assert_eq!(
            mapped,
            vec![
                SyncState::Idle,
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Error("sync failed".into()),
                SyncState::Running,
                SyncState::Terminated,
            ]
        );
    }

    #[test]
    fn backoff_sequence_doubles_then_caps() {
        let secs: Vec<u64> = (1..=9).map(|a| backoff_delay(a, 0.5).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(backoff_delay(0, 0.5), Duration::from_secs(1));
        assert_eq!(backoff_delay(u32::MAX, 0.5), Duration::from_secs(60));
    }

    #[test]
    fn backoff_jitter_stays_within_twenty_percent() {
        for attempt in [1, 2, 3, 7, 50] {
            let base = backoff_delay(attempt, 0.5).as_secs_f64();
            let lo = backoff_delay(attempt, 0.0).as_secs_f64();
            let hi = backoff_delay(attempt, 1.0).as_secs_f64();
            assert!((lo - base * 0.8).abs() < 1e-6, "attempt {attempt}: {lo}");
            assert!((hi - base * 1.2).abs() < 1e-6, "attempt {attempt}: {hi}");
            for step in 0..=20 {
                let d = backoff_delay(attempt, f64::from(step) / 20.0).as_secs_f64();
                assert!(d >= lo - 1e-9 && d <= hi + 1e-9);
            }
        }
        // Out-of-range samples are clamped, never panic.
        assert_eq!(backoff_delay(1, -3.0), backoff_delay(1, 0.0));
        assert_eq!(backoff_delay(1, 9.0), backoff_delay(1, 1.0));
        assert_eq!(backoff_delay(1, f64::NAN), backoff_delay(1, 0.5));
    }

    #[derive(Default)]
    struct Fake {
        sleeps: Mutex<Vec<Duration>>,
        restarts: Mutex<u32>,
    }

    impl SyncControl for Fake {
        async fn restart(&self) {
            *self.restarts.lock().unwrap() += 1;
        }
        async fn sleep(&self, delay: Duration) {
            self.sleeps.lock().unwrap().push(delay);
        }
        fn jitter_unit(&self) -> f64 {
            0.5
        }
    }

    fn failure() -> State {
        State::Error(Arc::new(Error::Supervisor))
    }

    /// Runs the supervisor over a finite fake state sequence.
    async fn run(initial: State, rest: Vec<State>) -> (Vec<SyncState>, Fake) {
        let fake = Fake::default();
        let (tx, mut rx) = mpsc::channel(64);
        supervise(&fake, initial, futures_util::stream::iter(rest), &tx).await;
        drop(tx);
        let mut out = Vec::new();
        while let Some(CoreEvent::SyncState(s)) = rx.recv().await {
            out.push(s);
        }
        (out, fake)
    }

    #[tokio::test]
    async fn error_reconnects_after_backoff_with_increasing_attempts() {
        let (out, fake) = run(
            State::Running,
            vec![failure(), failure(), failure(), State::Running],
        )
        .await;
        assert_eq!(
            out,
            vec![
                SyncState::Running,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Reconnecting { attempt: 2 },
                SyncState::Reconnecting { attempt: 3 },
                SyncState::Running,
            ]
        );
        assert_eq!(*fake.restarts.lock().unwrap(), 3);
        let secs: Vec<u64> = fake
            .sleeps
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect();
        assert_eq!(secs, vec![1, 2, 4]);
    }

    #[tokio::test]
    async fn running_resets_the_attempt_count() {
        let (out, fake) = run(
            State::Idle,
            vec![failure(), failure(), State::Running, failure()],
        )
        .await;
        assert_eq!(
            out,
            vec![
                SyncState::Idle,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Reconnecting { attempt: 2 },
                SyncState::Running,
                SyncState::Reconnecting { attempt: 1 },
            ]
        );
        let secs: Vec<u64> = fake
            .sleeps
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect();
        assert_eq!(secs, vec![1, 2, 1]);
    }

    #[tokio::test]
    async fn unexpected_terminated_reconnects() {
        let (out, fake) = run(State::Running, vec![State::Terminated, State::Running]).await;
        assert_eq!(
            out,
            vec![
                SyncState::Running,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Running,
            ]
        );
        assert_eq!(*fake.restarts.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn offline_is_reported_but_not_restarted() {
        let (out, fake) = run(
            State::Running,
            vec![State::Offline, State::Offline, State::Running, State::Idle],
        )
        .await;
        assert_eq!(
            out,
            vec![
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Idle,
            ]
        );
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
        assert!(fake.sleeps.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn backoff_is_capped_during_a_long_outage() {
        let (_, fake) = run(State::Running, (0..10).map(|_| failure()).collect()).await;
        let sleeps = fake.sleeps.lock().unwrap();
        assert_eq!(sleeps.len(), 10);
        assert_eq!(sleeps[9], Duration::from_secs(60));
    }

    #[tokio::test]
    async fn stops_when_the_receiver_is_gone() {
        let fake = Fake::default();
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let states = futures_util::stream::iter(vec![failure(), failure()]);
        supervise(&fake, State::Running, states, &tx).await;
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
    }
}
