//! What the GPU filter and the GPU bank share: the deadline a GPU call must
//! meet, the warm-up that checks it before the GPU is used, and holding
//! WebGPU's device while a graph is opened (feature `onnxruntime`).

use std::time::{Duration, Instant};

use crate::AudioError;
use crate::onnx::{Accelerator, WebGpuExclusive};

/// A GPU call slower than this is a missed deadline: the whole chain has
/// 10 ms, and the GPU call must leave room for the rest.
pub(super) const GPU_DEADLINE: Duration = Duration::from_millis(4);
/// Missed deadlines in a row that hand the frames to the CPU for good.
pub(super) const GPU_MISSES: u32 = 3;
/// Calls run at load time, before the deadline counts (the first ones pay
/// for kernel compilation and memory allocation on the device).
const GPU_WARM_UP_CALLS: usize = 50;

/// Counts GPU calls over [`GPU_DEADLINE`] in a row.
#[derive(Debug, Default)]
pub(super) struct Deadline {
    misses: u32,
}

impl Deadline {
    /// Records a call's time; true once [`GPU_MISSES`] in a row missed.
    pub(super) fn gives_up(&mut self, took: Duration) -> bool {
        self.misses = if took > GPU_DEADLINE {
            self.misses + 1
        } else {
            0
        };
        self.misses >= GPU_MISSES
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

    #[test]
    fn the_gpu_is_given_up_after_missed_deadlines_in_a_row_only() {
        let (late, on_time) = (GPU_DEADLINE + Duration::from_micros(1), GPU_DEADLINE);
        let mut deadline = Deadline::default();
        assert!(!deadline.gives_up(late));
        assert!(!deadline.gives_up(late));
        assert!(
            !deadline.gives_up(on_time),
            "a call in time resets the count"
        );
        assert!(!deadline.gives_up(late));
        assert!(!deadline.gives_up(late));
        assert!(deadline.gives_up(late), "{GPU_MISSES} in a row");
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
