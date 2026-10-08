//! CPU and file work off iced's executor, a few jobs at a time.
//!
//! Decoding untrusted images and reading big files must not run on the
//! executor's threads (they also drive the UI's tasks), and a burst of
//! images must not run in parallel without bound (each decode may use up to
//! [`crate::media::MAX_ALLOC`]). [`run`] starts each job on its own thread
//! once one of [`MAX_PARALLEL`] permits is free. Iced's executor is not a
//! tokio runtime, so `spawn_blocking` is not available here; the permits are
//! a tokio semaphore, which needs no runtime.

use std::sync::{Arc, LazyLock};

use tokio::sync::{Semaphore, oneshot};

/// Most jobs running at once.
pub const MAX_PARALLEL: usize = 2;

static PERMITS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(MAX_PARALLEL)));

/// Runs `work` on a thread of its own when a permit is free, and yields its
/// result. `None` when the thread could not start or `work` panicked.
///
/// The permit is held by the thread until `work` returns, even when the
/// caller stops waiting.
pub async fn run<T, F>(work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let permit = PERMITS.clone().acquire_owned().await.ok()?;
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("tessera-blocking".to_owned())
        .spawn(move || {
            let result = work();
            drop(permit);
            // The caller may have gone away; nobody needs the result then.
            let _ = tx.send(result);
        })
        .ok()?;
    rx.await.ok()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn it_returns_the_result_off_the_calling_thread() {
        let caller = std::thread::current().id();
        let got = iced::futures::executor::block_on(run(move || std::thread::current().id()));
        assert!(got.is_some_and(|id| id != caller));
    }

    #[test]
    fn a_panicking_job_gives_none() {
        let got: Option<u8> = iced::futures::executor::block_on(run(|| panic!("boom")));
        assert!(got.is_none());
    }

    #[test]
    fn at_most_max_parallel_jobs_run_at_once() {
        static RUNNING: AtomicUsize = AtomicUsize::new(0);
        static PEAK: AtomicUsize = AtomicUsize::new(0);
        let jobs = (0..8).map(|_| {
            run(|| {
                let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
                PEAK.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                RUNNING.fetch_sub(1, Ordering::SeqCst);
            })
        });
        let all = iced::futures::future::join_all(jobs);
        let done = iced::futures::executor::block_on(all);
        assert!(done.iter().all(Option::is_some));
        assert!(PEAK.load(Ordering::SeqCst) <= MAX_PARALLEL);
        assert!(PEAK.load(Ordering::SeqCst) >= 1);
    }
}
