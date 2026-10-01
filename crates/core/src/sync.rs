//! Mapping of the SDK's sync service state to the public [`SyncState`].

use std::time::{Duration, Instant};

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

/// How long `Running` must last without a failure before the attempt count
/// resets.
const STABLE_RESET: Duration = Duration::from_secs(30);
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
    /// Stops the service (a no-op when it is not running).
    async fn stop(&self);
    /// A monotonic clock reading.
    fn now(&self) -> Instant;
    /// Waits for `delay`.
    async fn sleep(&self, delay: Duration);
    /// A uniform random sample in `[0, 1)`.
    fn jitter_unit(&self) -> f64;
}

/// Consecutive unstable Offline cycles after which the supervisor throttles.
const OFFLINE_CYCLE_LIMIT: u32 = 3;

/// Forwards sync state changes as events and restarts the service after a
/// failure.
///
/// The SDK sets `Running` as soon as `start` returns, before any request has
/// succeeded, so `Running` alone proves nothing. A `Running` is *stable* once
/// it has lasted [`STABLE_RESET`] without a failure; only then do the attempt
/// count and the offline-cycle count reset (checked when the next failure
/// arrives).
///
/// - `Error` or an unexpected `Terminated` increments the attempt count, emits
///   `Reconnecting { attempt }`, waits `backoff_delay`, then calls `restart`.
///   (The engine's own `stop` makes the service go `Idle`, and the engine
///   aborts this task first.)
/// - `Offline` is left to the SDK's offline mode and reported as is. But with
///   offline mode the SDK returns to `Running` and restarts at once whenever
///   `/versions` answers, so a server that answers it while sync keeps failing
///   would loop with no delay. A `Running` that did not become stable before
///   the next `Offline` is an unstable cycle; after
///   [`OFFLINE_CYCLE_LIMIT`] in a row the supervisor increments the (shared)
///   attempt count, emits `Reconnecting { attempt }`, `stop`s the service,
///   waits `backoff_delay`, then `restart`s. A real network drop that recovers
///   to a stable `Running` is never throttled.
/// - The `Idle` caused by the supervisor's own `stop` is not reported.
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
    let mut unstable_cycles: u32 = 0;
    let mut running_since: Option<Instant> = None;
    let mut expect_idle = false;
    loop {
        let state = match pending.take() {
            Some(state) => state,
            None => match states.next().await {
                Some(state) => state,
                None => return,
            },
        };
        let now = control.now();
        let mut action = Action::None;
        let next = match state {
            State::Running => {
                expect_idle = false;
                running_since.get_or_insert(now);
                map_state(&state)
            }
            State::Idle if expect_idle => {
                // Our own `stop` below; the restart follows.
                expect_idle = false;
                continue;
            }
            State::Idle => {
                running_since = None;
                map_state(&state)
            }
            State::Error(_) | State::Terminated => {
                let was_stable = take_stable(&mut running_since, now);
                if was_stable == Some(true) {
                    attempt = 0;
                    unstable_cycles = 0;
                }
                attempt = attempt.saturating_add(1);
                action = Action::Restart;
                SyncState::Reconnecting { attempt }
            }
            State::Offline => {
                match take_stable(&mut running_since, now) {
                    Some(true) => {
                        attempt = 0;
                        unstable_cycles = 0;
                    }
                    Some(false) => unstable_cycles = unstable_cycles.saturating_add(1),
                    None => {}
                }
                if unstable_cycles >= OFFLINE_CYCLE_LIMIT {
                    // Keep one cycle short of the limit so that the next
                    // unstable cycle is throttled again, with a longer delay.
                    unstable_cycles = OFFLINE_CYCLE_LIMIT - 1;
                    attempt = attempt.saturating_add(1);
                    action = Action::StopAndRestart;
                }
                map_state(&state)
            }
        };
        if last.as_ref() != Some(&next) {
            last = Some(next.clone());
            if events.send(CoreEvent::SyncState(next)).await.is_err() {
                return;
            }
        }
        if action == Action::StopAndRestart {
            // `Offline` was just reported; now say we are backing off.
            let reconnecting = SyncState::Reconnecting { attempt };
            last = Some(reconnecting.clone());
            if events
                .send(CoreEvent::SyncState(reconnecting))
                .await
                .is_err()
            {
                return;
            }
            expect_idle = true;
            control.stop().await;
        }
        if action != Action::None {
            let delay = backoff_delay(attempt, control.jitter_unit());
            warn!(
                attempt,
                delay_ms = delay.as_millis() as u64,
                "sync failing; reconnecting"
            );
            control.sleep(delay).await;
            control.restart().await;
            info!(attempt, "sync restart requested");
        }
    }
}

/// What to do after reporting a state.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    None,
    /// Back off, then restart.
    Restart,
    /// Stop the service, back off, then restart.
    StopAndRestart,
}

/// Ends the current `Running` period, if any: `Some(true)` when it lasted at
/// least [`STABLE_RESET`], `Some(false)` when it did not, `None` when the
/// service was not running.
fn take_stable(running_since: &mut Option<Instant>, now: Instant) -> Option<bool> {
    running_since
        .take()
        .map(|since| now.saturating_duration_since(since) >= STABLE_RESET)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
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

    /// A fake service with a virtual clock. Like the real SDK, `restart`
    /// pushes `Running` and `stop` pushes `Idle` into the state stream, ahead
    /// of the scripted states.
    struct Fake {
        base: Instant,
        elapsed: Mutex<Duration>,
        script: Mutex<VecDeque<(Duration, State)>>,
        pushed: Mutex<VecDeque<State>>,
        sleeps: Mutex<Vec<Duration>>,
        restarts: Mutex<u32>,
        stops: Mutex<u32>,
    }

    impl Fake {
        fn new(script: Vec<(Duration, State)>) -> Self {
            Self {
                base: Instant::now(),
                elapsed: Mutex::default(),
                script: Mutex::new(script.into()),
                pushed: Mutex::default(),
                sleeps: Mutex::default(),
                restarts: Mutex::default(),
                stops: Mutex::default(),
            }
        }

        /// The next state: pushed ones first, else the next scripted one,
        /// after advancing the clock by its delay.
        fn next_state(&self) -> Option<State> {
            if let Some(state) = self.pushed.lock().unwrap().pop_front() {
                return Some(state);
            }
            let (advance, state) = self.script.lock().unwrap().pop_front()?;
            *self.elapsed.lock().unwrap() += advance;
            Some(state)
        }

        fn sleeps_secs(&self) -> Vec<u64> {
            self.sleeps
                .lock()
                .unwrap()
                .iter()
                .map(Duration::as_secs)
                .collect()
        }
    }

    impl SyncControl for Fake {
        async fn restart(&self) {
            *self.restarts.lock().unwrap() += 1;
            self.pushed.lock().unwrap().push_back(State::Running);
        }
        async fn stop(&self) {
            *self.stops.lock().unwrap() += 1;
            self.pushed.lock().unwrap().push_back(State::Idle);
        }
        fn now(&self) -> Instant {
            self.base + *self.elapsed.lock().unwrap()
        }
        async fn sleep(&self, delay: Duration) {
            self.sleeps.lock().unwrap().push(delay);
            *self.elapsed.lock().unwrap() += delay;
        }
        fn jitter_unit(&self) -> f64 {
            0.5
        }
    }

    fn failure() -> State {
        State::Error(Arc::new(Error::Supervisor))
    }

    const QUICK: Duration = Duration::from_secs(1);
    const STABLE: Duration = Duration::from_secs(31);

    /// Runs the supervisor over `initial` and a script of (clock advance
    /// before delivery, state) pairs.
    async fn run(initial: State, script: Vec<(Duration, State)>) -> (Vec<SyncState>, Fake) {
        let fake = Fake::new(script);
        let (tx, mut rx) = mpsc::channel(64);
        let states = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(fake.next_state()));
        supervise(&fake, initial, Box::pin(states), &tx).await;
        drop(tx);
        let mut out = Vec::new();
        while let Some(CoreEvent::SyncState(s)) = rx.recv().await {
            out.push(s);
        }
        (out, fake)
    }

    fn quick(states: Vec<State>) -> Vec<(Duration, State)> {
        states.into_iter().map(|s| (QUICK, s)).collect()
    }

    #[tokio::test]
    async fn error_reconnects_with_growing_backoff_when_running_is_unstable() {
        // The real SDK reports `Running` as soon as `start` returns, before any
        // request succeeded; the fake does the same on `restart`.
        let (out, fake) = run(State::Running, quick(vec![failure(), failure(), failure()])).await;
        assert_eq!(
            out,
            vec![
                SyncState::Running,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Running,
                SyncState::Reconnecting { attempt: 2 },
                SyncState::Running,
                SyncState::Reconnecting { attempt: 3 },
                SyncState::Running,
            ]
        );
        assert_eq!(*fake.restarts.lock().unwrap(), 3);
        assert_eq!(fake.sleeps_secs(), vec![1, 2, 4]);
    }

    #[tokio::test]
    async fn a_running_that_stays_stable_resets_the_attempt_count() {
        let script = vec![(QUICK, failure()), (QUICK, failure()), (STABLE, failure())];
        let (out, fake) = run(State::Idle, script).await;
        assert_eq!(
            out,
            vec![
                SyncState::Idle,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Running,
                SyncState::Reconnecting { attempt: 2 },
                SyncState::Running,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Running,
            ]
        );
        assert_eq!(fake.sleeps_secs(), vec![1, 2, 1]);
    }

    #[tokio::test]
    async fn a_running_just_short_of_the_window_does_not_reset() {
        let almost = STABLE_RESET - Duration::from_secs(1);
        let script = vec![(QUICK, failure()), (almost, failure())];
        let (_, fake) = run(State::Running, script).await;
        assert_eq!(fake.sleeps_secs(), vec![1, 2]);
    }

    #[tokio::test]
    async fn unexpected_terminated_reconnects() {
        let (out, fake) = run(State::Running, quick(vec![State::Terminated])).await;
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
    async fn a_genuine_network_drop_is_reported_and_not_throttled() {
        let script = vec![
            (STABLE, State::Offline),
            (QUICK, State::Running),
            (STABLE, State::Offline),
            (QUICK, State::Running),
            (STABLE, State::Idle),
        ];
        let (out, fake) = run(State::Running, script).await;
        assert_eq!(
            out,
            vec![
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Idle,
            ]
        );
        assert_eq!(*fake.stops.lock().unwrap(), 0);
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
        assert!(fake.sleeps.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rapid_offline_cycles_are_throttled_with_growing_delays() {
        // Offline -> Running -> Offline without the Running staying stable:
        // /versions answers but sync keeps failing.
        let off = State::Offline;
        let run_ = State::Running;
        let states = vec![
            off.clone(),
            run_.clone(),
            off.clone(),
            run_,
            off.clone(),
            off.clone(),
            off,
        ];
        let (out, fake) = run(State::Running, quick(states)).await;
        assert_eq!(
            out,
            vec![
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Offline,
                SyncState::Running,
                SyncState::Offline,
                SyncState::Reconnecting { attempt: 1 },
                SyncState::Running,
                SyncState::Offline,
                SyncState::Reconnecting { attempt: 2 },
                SyncState::Running,
                SyncState::Offline,
                SyncState::Reconnecting { attempt: 3 },
                SyncState::Running,
            ]
        );
        assert_eq!(fake.sleeps_secs(), vec![1, 2, 4]);
        assert_eq!(*fake.stops.lock().unwrap(), 3);
        assert_eq!(*fake.restarts.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn a_stable_running_after_a_throttle_resets_everything() {
        let script = vec![
            (QUICK, State::Offline),
            (QUICK, State::Running),
            (QUICK, State::Offline),
            (QUICK, State::Running),
            (QUICK, State::Offline),  // third rapid cycle: throttled (1 s)
            (STABLE, State::Offline), // genuine drop after a stable Running
            (QUICK, State::Running),
            (QUICK, failure()), // counts from 1 again
        ];
        let (_, fake) = run(State::Running, script).await;
        assert_eq!(fake.sleeps_secs(), vec![1, 1]);
        assert_eq!(*fake.stops.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn backoff_is_capped_during_a_long_outage() {
        let (_, fake) = run(State::Running, quick((0..10).map(|_| failure()).collect())).await;
        let sleeps = fake.sleeps.lock().unwrap();
        assert_eq!(sleeps.len(), 10);
        assert_eq!(sleeps[9], Duration::from_secs(60));
    }

    #[tokio::test]
    async fn stops_when_the_receiver_is_gone() {
        let fake = Fake::new(quick(vec![failure(), failure()]));
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        let states = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(fake.next_state()));
        supervise(&fake, State::Running, Box::pin(states), &tx).await;
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
    }
}
