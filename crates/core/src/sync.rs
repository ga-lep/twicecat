//! Mapping of the SDK's sync service state to the public [`SyncState`].

use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt as _};
use matrix_sdk_ui::sync_service::State;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
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

/// Where the supervisor publishes the state. Publishing never waits, so a
/// slow consumer cannot stall supervision (and time spent blocked cannot look
/// like a stable `Running`).
pub(crate) trait StateSink {
    /// Publishes `state`; `false` when nobody listens any more.
    fn publish(&self, state: SyncState) -> bool;
}

/// A latest-value slot: intermediate states may be overwritten before the
/// forwarder ([`forward_latest`]) delivers them, the latest never is.
pub(crate) struct WatchSink(watch::Sender<SyncState>);

impl StateSink for WatchSink {
    fn publish(&self, state: SyncState) -> bool {
        self.0.send(state).is_ok()
    }
}

/// Spots the return to `Running` after the connection was lost.
#[derive(Debug, Default)]
pub(crate) struct ResumeDetector {
    /// An `Offline` or `Reconnecting` has been seen since the last `Running`.
    degraded: std::sync::atomic::AtomicBool,
}

impl ResumeDetector {
    /// Records `state`; `true` when it is a `Running` that ends an outage.
    pub(crate) fn observe(&self, state: &SyncState) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        match state {
            SyncState::Offline | SyncState::Reconnecting { .. } => {
                self.degraded.store(true, SeqCst);
                false
            }
            SyncState::Running => self.degraded.swap(false, SeqCst),
            _ => false,
        }
    }
}

/// A [`StateSink`] that calls `on_resume` (which must not block) whenever the
/// sync returns to `Running` after an outage, then publishes to `inner`.
///
/// It sees every state the supervisor publishes, unlike the latest-value slot
/// behind `inner`, which may coalesce `Offline` and `Running`.
pub(crate) struct ResumeSink<K, F> {
    inner: K,
    detector: ResumeDetector,
    on_resume: F,
}

impl<K, F> ResumeSink<K, F> {
    pub(crate) fn new(inner: K, on_resume: F) -> Self {
        Self {
            inner,
            detector: ResumeDetector::default(),
            on_resume,
        }
    }
}

impl<K: StateSink, F: Fn()> StateSink for ResumeSink<K, F> {
    fn publish(&self, state: SyncState) -> bool {
        if self.detector.observe(&state) {
            (self.on_resume)();
        }
        self.inner.publish(state)
    }
}

/// Delivers the latest published state to `events`, waiting for room there.
async fn forward_latest(mut slot: watch::Receiver<SyncState>, events: mpsc::Sender<CoreEvent>) {
    while slot.changed().await.is_ok() {
        let state = slot.borrow_and_update().clone();
        if events.send(CoreEvent::SyncState(state)).await.is_err() {
            return;
        }
    }
}

/// Asks the supervisor to end. The supervisor checks between steps; a `stop`
/// or `restart` of the service already under way is always finished, so the
/// SDK tasks never outlive an interrupted call.
#[derive(Debug)]
pub(crate) struct CancelTrigger(watch::Sender<bool>);

impl CancelTrigger {
    pub(crate) fn cancel(&self) {
        self.0.send_replace(true);
    }
}

impl Drop for CancelTrigger {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The supervisor's end of a [`CancelTrigger`].
#[derive(Debug)]
pub(crate) struct Cancel(watch::Receiver<bool>);

/// Creates a trigger and its cancel signal.
pub(crate) fn cancel_pair() -> (CancelTrigger, Cancel) {
    let (tx, rx) = watch::channel(false);
    (CancelTrigger(tx), Cancel(rx))
}

impl Cancel {
    fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }

    /// Resolves once cancelled (or when the trigger is gone).
    async fn cancelled(&mut self) {
        // An error means the sender is gone, which also counts as cancelled.
        let _ = self.0.wait_for(|cancelled| *cancelled).await;
    }
}

/// Aborts its task when dropped.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs [`supervise`] with a non-blocking path to `events`: states go through
/// a latest-value slot drained by a forwarder task. `on_resume` is called
/// (without blocking) when `Running` returns after `Offline` or `Reconnecting`.
pub(crate) async fn run_supervised<C, S, F>(
    control: &C,
    initial: State,
    states: S,
    events: mpsc::Sender<CoreEvent>,
    mut cancel: Cancel,
    on_resume: F,
) where
    C: SyncControl,
    S: Stream<Item = State> + Unpin,
    F: Fn() + Send + Sync,
{
    let (slot_tx, slot_rx) = watch::channel(SyncState::Idle);
    let _forwarder = AbortOnDrop(tokio::spawn(forward_latest(slot_rx, events)));
    let sink = ResumeSink::new(WatchSink(slot_tx), on_resume);
    supervise(control, initial, states, &sink, &mut cancel).await;
}

/// Consecutive unstable Offline cycles after which the supervisor throttles.
const OFFLINE_CYCLE_LIMIT: u32 = 3;

/// Publishes sync state changes and restarts the service after a failure.
///
/// The SDK sets `Running` as soon as `start` returns, before any request has
/// succeeded, so `Running` alone proves nothing. A `Running` is *stable* once
/// it has lasted [`STABLE_RESET`] without a failure; only then do the attempt
/// count and the offline-cycle count reset (checked when the next failure
/// arrives).
///
/// The state stream coalesces: when several states are set before it is
/// polled, only the last is seen. The supervisor therefore reads a repeated
/// state as proof of a missed transition. The SDK sets `Offline` only after
/// leaving `Running`, and `Running` only after `Idle`, `Offline`, `Error` or
/// `Terminated` (`start` while `Running` sets nothing):
/// - an `Offline` with no `Running` seen since the last `Offline` (or restart)
///   missed a `Running`: an unstable cycle;
/// - a `Running` right after a `Running` missed an `Offline`: an unstable
///   cycle, and the stability window restarts.
///
/// Publishing goes through a [`StateSink`] that never waits, so a slow event
/// consumer cannot make a blocked period look like a stable `Running`.
///
/// - `Error` or an unexpected `Terminated` increments the attempt count,
///   publishes `Reconnecting { attempt }`, waits `backoff_delay`, then calls
///   `restart`.
/// - `Offline` is left to the SDK's offline mode and published as is. But with
///   offline mode the SDK returns to `Running` and restarts at once whenever
///   `/versions` answers, so a server that answers it while sync keeps failing
///   would loop with no delay. After [`OFFLINE_CYCLE_LIMIT`] unstable cycles
///   in a row the supervisor increments the (shared) attempt count, publishes
///   `Reconnecting { attempt }`, `stop`s the service, waits `backoff_delay`,
///   then `restart`s. A real network drop that recovers to a stable `Running`
///   is never throttled.
/// - The `Idle` caused by the supervisor's own `stop` is not reported.
///
/// `cancel` is checked between steps, never inside `stop` or `restart`, which
/// are always run to the end. Returns when the stream ends, the sink has no
/// listener, or on cancellation.
pub(crate) async fn supervise<C, S, K>(
    control: &C,
    initial: State,
    states: S,
    sink: &K,
    cancel: &mut Cancel,
) where
    C: SyncControl,
    S: Stream<Item = State> + Unpin,
    K: StateSink,
{
    let mut states = states;
    let mut pending = Some(initial);
    let mut last: Option<SyncState> = None;
    let mut attempt: u32 = 0;
    let mut unstable_cycles: u32 = 0;
    let mut running_since: Option<Instant> = None;
    let mut last_was_running = false;
    // The `Running` that our own `restart` produces is expected, not a repeat.
    let mut own_restart = false;
    let mut expect_idle = false;
    loop {
        let state = match pending.take() {
            Some(state) => state,
            None => {
                let next = tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    next = states.next() => next,
                };
                match next {
                    Some(state) => state,
                    None => return,
                }
            }
        };
        if cancel.is_cancelled() {
            return;
        }
        let now = control.now();
        let mut action = Action::None;
        let mut throttle_check = false;
        let was_running = std::mem::replace(&mut last_was_running, false);
        let next = match state {
            State::Running => {
                expect_idle = false;
                last_was_running = true;
                if std::mem::take(&mut own_restart) {
                    // Window already started when `restart` returned.
                } else if was_running {
                    // A missed Offline: count it, start a new window.
                    unstable_cycles = unstable_cycles.saturating_add(1);
                    running_since = Some(now);
                    throttle_check = true;
                } else {
                    running_since = Some(now);
                }
                map_state(&state)
            }
            State::Idle if expect_idle => {
                // Our own `stop` below; the restart follows.
                expect_idle = false;
                last_was_running = was_running;
                continue;
            }
            State::Idle => {
                own_restart = false;
                running_since = None;
                map_state(&state)
            }
            State::Error(_) | State::Terminated => {
                own_restart = false;
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
                own_restart = false;
                // No `Running` seen since the last failure: it was missed,
                // so this cycle was not stable either.
                match take_stable(&mut running_since, now) {
                    Some(true) => {
                        attempt = 0;
                        unstable_cycles = 0;
                    }
                    Some(false) | None => unstable_cycles = unstable_cycles.saturating_add(1),
                }
                throttle_check = true;
                map_state(&state)
            }
        };
        if throttle_check && unstable_cycles >= OFFLINE_CYCLE_LIMIT {
            // Keep one cycle short of the limit so that the next unstable
            // cycle is throttled again, with a longer delay.
            unstable_cycles = OFFLINE_CYCLE_LIMIT - 1;
            attempt = attempt.saturating_add(1);
            action = Action::StopAndRestart;
        }
        if last.as_ref() != Some(&next) {
            last = Some(next.clone());
            if !sink.publish(next) {
                return;
            }
        }
        if action == Action::StopAndRestart {
            // The cause was just reported; now say we are backing off.
            let reconnecting = SyncState::Reconnecting { attempt };
            last = Some(reconnecting.clone());
            if !sink.publish(reconnecting) {
                return;
            }
            expect_idle = true;
            // Runs to the end even if `cancel` fires meanwhile.
            control.stop().await;
        }
        if action != Action::None {
            let delay = backoff_delay(attempt, control.jitter_unit());
            warn!(
                attempt,
                delay_ms = delay.as_millis() as u64,
                "sync failing; reconnecting"
            );
            tokio::select! {
                biased;
                () = cancel.cancelled() => return,
                () = control.sleep(delay) => {}
            }
            if cancel.is_cancelled() {
                return;
            }
            control.restart().await;
            info!(attempt, "sync restart requested");
            // The service is `Running` now, whether or not the stream shows it.
            own_restart = true;
            running_since = Some(control.now());
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
        /// Whether `restart` and `stop` push their `Running` and `Idle`. The
        /// real subscriber coalesces them away, since the supervisor does not
        /// poll while it calls them.
        echo: bool,
        /// Cancels the supervisor while `stop` is under way.
        cancel_in_stop: Mutex<Option<CancelTrigger>>,
        stop_finished: Mutex<bool>,
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
                echo: true,
                cancel_in_stop: Mutex::default(),
                stop_finished: Mutex::default(),
            }
        }

        /// A fake whose `restart` and `stop` leave no trace in the stream.
        fn coalescing(script: Vec<(Duration, State)>) -> Self {
            Self {
                echo: false,
                ..Self::new(script)
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
            if self.echo {
                self.pushed.lock().unwrap().push_back(State::Running);
            }
        }
        async fn stop(&self) {
            *self.stops.lock().unwrap() += 1;
            if let Some(trigger) = self.cancel_in_stop.lock().unwrap().as_ref() {
                trigger.cancel();
            }
            // Give a cancelled caller every chance to give up half way.
            for _ in 0..3 {
                tokio::task::yield_now().await;
            }
            if self.echo {
                self.pushed.lock().unwrap().push_back(State::Idle);
            }
            *self.stop_finished.lock().unwrap() = true;
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

    /// Records everything published.
    struct RecSink(Mutex<Vec<SyncState>>);

    impl StateSink for RecSink {
        fn publish(&self, state: SyncState) -> bool {
            self.0.lock().unwrap().push(state);
            true
        }
    }

    /// Runs the supervisor over `initial` on `fake`, whose script is the
    /// stream (clock advance before delivery, state).
    async fn run_on(fake: &Fake, initial: State) -> Vec<SyncState> {
        let sink = RecSink(Mutex::default());
        let (_trigger, mut cancel) = cancel_pair();
        let states = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(fake.next_state()));
        supervise(fake, initial, Box::pin(states), &sink, &mut cancel).await;
        sink.0.into_inner().unwrap()
    }

    async fn run(initial: State, script: Vec<(Duration, State)>) -> (Vec<SyncState>, Fake) {
        let fake = Fake::new(script);
        let out = run_on(&fake, initial).await;
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
        struct Gone;
        impl StateSink for Gone {
            fn publish(&self, _: SyncState) -> bool {
                false
            }
        }
        let fake = Fake::new(quick(vec![failure(), failure()]));
        let (_trigger, mut cancel) = cancel_pair();
        let states = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(fake.next_state()));
        supervise(&fake, State::Running, Box::pin(states), &Gone, &mut cancel).await;
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
    }

    /// Several SDK transitions in one poll collapse to the last one; the
    /// time they took still passes.
    fn coalesce(group: Vec<(Duration, State)>) -> (Duration, State) {
        let total = group.iter().map(|(d, _)| *d).sum();
        let last = group.into_iter().last().expect("a non-empty group").1;
        (total, last)
    }

    #[tokio::test]
    async fn coalesced_running_between_offlines_still_counts_a_cycle() {
        // Offline, (Running, Offline) seen as one Offline, ... : the missed
        // Running must not hide the cycle from the throttle.
        let missed = || coalesce(vec![(QUICK, State::Running), (QUICK, State::Offline)]);
        let script = vec![(QUICK, State::Offline), missed(), missed(), missed()];
        let fake = Fake::coalescing(script);
        run_on(&fake, State::Running).await;
        assert_eq!(fake.sleeps_secs(), vec![1, 2]);
        assert_eq!(*fake.stops.lock().unwrap(), 2);
        assert_eq!(*fake.restarts.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn coalesced_offline_between_runnings_counts_a_cycle() {
        let missed = || coalesce(vec![(QUICK, State::Offline), (QUICK, State::Running)]);
        let script = vec![missed(), missed(), missed()];
        let fake = Fake::coalescing(script);
        run_on(&fake, State::Running).await;
        assert_eq!(fake.sleeps_secs(), vec![1]);
        assert_eq!(*fake.stops.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_missed_offline_does_not_make_the_window_look_stable() {
        // Running at t=2, (Offline, Running) missed at t=17, Offline at t=33:
        // 31 s after the first Running, but only 16 s after the last one.
        let script = vec![
            (QUICK, State::Offline),
            (QUICK, State::Running),
            (Duration::from_secs(15), State::Running),
            (Duration::from_secs(16), State::Offline),
        ];
        let fake = Fake::coalescing(script);
        run_on(&fake, State::Running).await;
        // Cycles: Offline #1, missed Offline #2, Offline #3 -> throttled.
        assert_eq!(fake.sleeps_secs(), vec![1]);
        assert_eq!(*fake.stops.lock().unwrap(), 1);
    }

    /// The states that `ResumeDetector` flags as a resume.
    fn resumes(states: &[SyncState]) -> Vec<usize> {
        let detector = ResumeDetector::default();
        states
            .iter()
            .enumerate()
            .filter(|(_, s)| detector.observe(s))
            .map(|(i, _)| i)
            .collect()
    }

    #[test]
    fn running_after_offline_or_reconnecting_is_a_resume() {
        use SyncState::{Idle, Offline, Reconnecting, Running};
        assert_eq!(resumes(&[Idle, Running]), Vec::<usize>::new());
        assert_eq!(resumes(&[Running, Running]), Vec::<usize>::new());
        assert_eq!(resumes(&[Running, Offline, Running]), vec![2]);
        assert_eq!(
            resumes(&[Running, Reconnecting { attempt: 1 }, Running]),
            vec![2]
        );
        // One resume per outage, however many failure states preceded it.
        assert_eq!(
            resumes(&[
                Running,
                Offline,
                Reconnecting { attempt: 1 },
                Reconnecting { attempt: 2 },
                Running,
                Running,
            ]),
            vec![4]
        );
        assert_eq!(resumes(&[Offline, Running, Offline, Running]), vec![1, 3]);
    }

    #[test]
    fn the_resume_sink_calls_back_and_still_publishes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let sink = ResumeSink::new(RecSink(Mutex::default()), || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        for state in [SyncState::Running, SyncState::Offline, SyncState::Running] {
            assert!(sink.publish(state));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(sink.inner.0.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_stalled_consumer_does_not_stop_the_backoff() {
        let fake = Fake::new(quick((0..5).map(|_| failure()).collect()));
        // Nobody ever drains this channel and it holds one event.
        let (tx, _rx) = mpsc::channel(1);
        let states = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(fake.next_state()));
        let (_trigger, cancel) = cancel_pair();
        tokio::time::timeout(
            Duration::from_secs(5),
            run_supervised(&fake, State::Running, Box::pin(states), tx, cancel, || {}),
        )
        .await
        .expect("the supervisor must not wait for the consumer");
        assert_eq!(fake.sleeps_secs(), vec![1, 2, 4, 8, 16]);
        assert_eq!(*fake.restarts.lock().unwrap(), 5);
    }

    #[tokio::test]
    async fn the_latest_state_is_delivered_after_a_stall() {
        let (slot_tx, slot_rx) = watch::channel(SyncState::Idle);
        let (tx, mut rx) = mpsc::channel(1);
        let forwarder = tokio::spawn(forward_latest(slot_rx, tx));
        let sink = WatchSink(slot_tx);
        for state in [
            SyncState::Running,
            SyncState::Offline,
            SyncState::Reconnecting { attempt: 1 },
            SyncState::Reconnecting { attempt: 2 },
            SyncState::Running,
        ] {
            assert!(sink.publish(state));
            tokio::task::yield_now().await;
        }
        let mut last = None;
        while let Ok(Some(CoreEvent::SyncState(s))) =
            tokio::time::timeout(Duration::from_millis(200), rx.recv()).await
        {
            last = Some(s);
        }
        assert_eq!(last, Some(SyncState::Running));
        forwarder.abort();
    }

    #[tokio::test]
    async fn cancelling_during_stop_still_finishes_the_stop() {
        let script = quick(vec![
            State::Offline,
            State::Running,
            State::Offline,
            State::Running,
            State::Offline, // third rapid cycle: the throttle calls `stop`
            State::Running,
        ]);
        let fake = Fake::new(script);
        let sink = RecSink(Mutex::default());
        let (trigger, mut cancel) = cancel_pair();
        *fake.cancel_in_stop.lock().unwrap() = Some(trigger);
        let states = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(fake.next_state()));
        supervise(&fake, State::Running, Box::pin(states), &sink, &mut cancel).await;
        assert!(*fake.stop_finished.lock().unwrap(), "stop was cut short");
        assert_eq!(*fake.stops.lock().unwrap(), 1);
        // Cancelled: no restart after the stop.
        assert_eq!(*fake.restarts.lock().unwrap(), 0);
    }
}
