//! What the GPU filter and the GPU bank share: the deadline a GPU call must
//! meet, the warm-up that checks it before the GPU is used, when to try a
//! GPU given up on again, and holding WebGPU's device while a graph is
//! opened (feature `onnxruntime`).

use std::time::{Duration, Instant};

use crate::AudioError;
use crate::onnx::{Accelerator, WebGpuExclusive};

/// A GPU call slower than this is a missed deadline: the whole chain has
/// 10 ms, and the GPU call must leave room for the rest.
pub(super) const GPU_DEADLINE: Duration = Duration::from_millis(4);
/// GPU calls looked back on: a second of frames.
pub(super) const GPU_WINDOW: u32 = 100;
/// Missed deadlines among the last [`GPU_WINDOW`] calls that hand the
/// frames to the CPU. A few late frames in a second (a compositor frame, a
/// clock change, a busy CPU) do not; a GPU late on every frame gives up
/// after this many (100 ms).
pub(super) const GPU_LATE_LIMIT: u32 = 10;
/// The first wait before a GPU given up on as too slow is tried again;
/// each failed try doubles it, up to [`GPU_RETRY_MAX`].
pub(super) const GPU_RETRY_FIRST: Duration = Duration::from_secs(30);
/// The longest wait between tries.
pub(super) const GPU_RETRY_MAX: Duration = Duration::from_secs(300);
/// Calls run at load time, before the deadline counts (the first ones pay
/// for kernel compilation and memory allocation on the device).
const GPU_WARM_UP_CALLS: usize = 50;

/// Counts GPU calls over [`GPU_DEADLINE`] among the last [`GPU_WINDOW`].
#[derive(Debug, Default)]
pub(super) struct Deadline {
    /// One bit per call, the newest lowest; set when it was late.
    late: u128,
}

impl Deadline {
    /// Records a call's time; true once [`GPU_LATE_LIMIT`] of the last
    /// [`GPU_WINDOW`] calls missed.
    pub(super) fn gives_up(&mut self, took: Duration) -> bool {
        let window = (1_u128 << GPU_WINDOW) - 1;
        self.late = ((self.late << 1) | u128::from(took > GPU_DEADLINE)) & window;
        self.late.count_ones() >= GPU_LATE_LIMIT
    }

    /// Forgets the calls so far (the GPU is tried afresh).
    pub(super) fn reset(&mut self) {
        self.late = 0;
    }
}

/// When to try again a GPU given up on as too slow: [`GPU_RETRY_FIRST`]
/// after giving up, then twice as long after each try that gave up again.
#[derive(Debug)]
pub(super) struct Retry {
    wait: Duration,
    at: Option<Instant>,
}

impl Default for Retry {
    fn default() -> Self {
        Self {
            wait: GPU_RETRY_FIRST,
            at: None,
        }
    }
}

impl Retry {
    /// The GPU was given up on at `now`: schedules the next try.
    pub(super) fn gave_up(&mut self, now: Instant) {
        self.at = Some(now + self.wait);
        self.wait = (self.wait * 2).min(GPU_RETRY_MAX);
    }

    /// A try due at `at` (for `DeepFilter::fall_behind`).
    #[cfg(feature = "test-fakes")]
    pub(super) fn due_at(at: Instant) -> Self {
        Self {
            at: Some(at),
            ..Self::default()
        }
    }

    /// Whether a try is due at `now`; true once per scheduled try.
    pub(super) fn due(&mut self, now: Instant) -> bool {
        if self.at.is_some_and(|at| now >= at) {
            self.at = None;
            true
        } else {
            false
        }
    }
}

/// How a warm-up went: the first call, and the median of the settled ones.
#[derive(Clone, Copy, Debug)]
pub(super) struct WarmUp {
    pub first: Duration,
    pub median: Duration,
}

impl WarmUp {
    /// Whether the GPU keeps up once warm.
    pub(super) fn keeps_up(&self) -> bool {
        self.median <= GPU_DEADLINE
    }
}

/// Runs `call` [`GPU_WARM_UP_CALLS`] times and times it.
pub(super) fn warm_up(
    mut call: impl FnMut() -> Result<(), AudioError>,
) -> Result<WarmUp, AudioError> {
    let mut times = Vec::with_capacity(GPU_WARM_UP_CALLS);
    for _ in 0..GPU_WARM_UP_CALLS {
        let start = Instant::now();
        call()?;
        times.push(start.elapsed());
    }
    let first = times[0];
    let settled = &mut times[GPU_WARM_UP_CALLS / 2..];
    settled.sort();
    Ok(WarmUp {
        first,
        median: settled[settled.len() / 2],
    })
}

/// WebGPU's device, held while a graph is opened and warmed up when
/// WebGPU is among `prefer` (see `crate::onnx`).
pub(super) fn hold_webgpu(prefer: &[Accelerator]) -> Option<WebGpuExclusive> {
    prefer
        .contains(&Accelerator::WebGpu)
        .then(WebGpuExclusive::take)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LATE: Duration = GPU_DEADLINE.saturating_add(Duration::from_micros(1));

    #[test]
    fn a_few_late_calls_in_a_second_are_tolerated() {
        let mut deadline = Deadline::default();
        // Three late calls in a row, as the human's log showed, then a
        // late call every 20 frames: never 10 in the last 100.
        for _ in 0..3 {
            assert!(!deadline.gives_up(LATE));
        }
        for i in 0..2000 {
            let took = if i % 20 == 0 { LATE } else { GPU_DEADLINE };
            assert!(!deadline.gives_up(took), "call {i}");
        }
    }

    #[test]
    fn the_gpu_is_given_up_after_too_many_late_calls_in_a_second() {
        let mut deadline = Deadline::default();
        for _ in 1..GPU_LATE_LIMIT {
            assert!(!deadline.gives_up(LATE));
        }
        assert!(deadline.gives_up(LATE), "{GPU_LATE_LIMIT} late in a row");

        // Spread out: the 10th late call within 100 gives up.
        let mut deadline = Deadline::default();
        for i in 0..(GPU_LATE_LIMIT - 1) * 10 {
            let took = if i % 10 == 0 { LATE } else { GPU_DEADLINE };
            assert!(!deadline.gives_up(took));
        }
        assert!(deadline.gives_up(LATE));

        // Calls older than the window no longer count.
        let mut deadline = Deadline::default();
        for _ in 1..GPU_LATE_LIMIT {
            assert!(!deadline.gives_up(LATE));
        }
        for _ in 0..GPU_WINDOW {
            assert!(!deadline.gives_up(GPU_DEADLINE));
        }
        assert!(!deadline.gives_up(LATE));
        deadline.reset();
        assert_eq!(deadline.late, 0);
    }

    #[test]
    fn a_gpu_given_up_on_is_tried_again_later_and_later() {
        let start = Instant::now();
        let mut retry = Retry::default();
        assert!(!retry.due(start + GPU_RETRY_MAX), "nothing scheduled");
        retry.gave_up(start);
        assert!(!retry.due(start + GPU_RETRY_FIRST - Duration::from_millis(1)));
        assert!(retry.due(start + GPU_RETRY_FIRST));
        assert!(!retry.due(start + GPU_RETRY_FIRST), "once per try");
        let mut waits = Vec::new();
        for _ in 0..6 {
            retry.gave_up(start);
            waits.push(retry.at.unwrap() - start);
            assert!(retry.due(start + GPU_RETRY_MAX));
        }
        let secs: Vec<u64> = waits.iter().map(Duration::as_secs).collect();
        assert_eq!(secs, [60, 120, 240, 300, 300, 300]);
    }

    #[test]
    fn a_warm_up_reports_the_first_call_and_the_settled_median() {
        let mut calls = 0;
        let warm = warm_up(|| {
            calls += 1;
            if calls == 1 {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(calls, GPU_WARM_UP_CALLS);
        assert!(warm.first >= Duration::from_millis(20));
        assert!(warm.median < Duration::from_millis(20));
        assert!(warm.keeps_up());
        let failing = warm_up(|| Err(AudioError::Internal));
        assert!(failing.is_err());
    }
}
