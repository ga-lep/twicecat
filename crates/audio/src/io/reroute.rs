//! When a stream opened by `io::cpal` (cpal's, or on Windows our own
//! WASAPI client's) must be reopened, and where. WASAPI never moves a
//! stream itself: when its device goes away, or the default it should
//! follow changes, the stream fails or stays behind, so the processing
//! thread reopens it. Signals come from cpal's error callbacks, our WASAPI
//! streams' threads and, on Windows, the MMDevice API's notifications
//! (`crate::wasapi`); all run on other threads and only set flags here and
//! wake the processing thread, which acts between frames. A displaced
//! stream is also checked on a slow timer there, for platforms without
//! device notifications and for failures no notification follows.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread::Thread;

/// Where a direction's stream is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Placement {
    /// On the device the user chose; it stays there.
    Chosen = 0,
    /// On the default device, nothing being chosen; it follows the default.
    Default = 1,
    /// On the default device because the chosen one is gone; it goes back
    /// when that one returns, and follows the default meanwhile.
    FellBack = 2,
    /// No stream: no device to open.
    Missing = 3,
}

impl Placement {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Chosen,
            1 => Self::Default,
            2 => Self::FellBack,
            _ => Self::Missing,
        }
    }
}

#[derive(Debug)]
struct Side {
    placement: AtomicU8,
    /// Reopen at the next chance.
    requested: AtomicBool,
    /// The default device changed: reopen unless the stream is on it.
    default_moved: AtomicBool,
    /// Devices changed: see whether a displaced stream can go back.
    check: AtomicBool,
}

impl Side {
    fn new() -> Self {
        Self {
            placement: AtomicU8::new(Placement::Missing as u8),
            requested: AtomicBool::new(false),
            default_moved: AtomicBool::new(false),
            check: AtomicBool::new(false),
        }
    }
}

/// Shared between the processing thread and the threads that signal it.
#[derive(Debug)]
pub(super) struct Reroute {
    thread: Thread,
    /// Playback (0) and capture (1).
    sides: [Side; 2],
}

impl Reroute {
    /// For the processing thread `thread`, which the signals wake.
    pub(super) fn new(thread: Thread) -> Self {
        Self {
            thread,
            sides: [Side::new(), Side::new()],
        }
    }

    fn side(&self, input: bool) -> &Side {
        &self.sides[usize::from(input)]
    }

    /// Records where the stream for `input` now is.
    pub(super) fn placed(&self, input: bool, placement: Placement) {
        self.side(input)
            .placement
            .store(placement as u8, Ordering::Release);
    }

    pub(super) fn placement(&self, input: bool) -> Placement {
        Placement::from_u8(self.side(input).placement.load(Ordering::Acquire))
    }

    fn request(&self, input: bool) {
        self.side(input).requested.store(true, Ordering::Release);
        self.thread.unpark();
    }

    /// The stream's device went away, or the stream became unusable
    /// (cpal's error callback, our WASAPI stream's thread): reopen it
    /// wherever it now belongs.
    pub(super) fn lost(&self, input: bool) {
        self.request(input);
    }

    /// The default device for `input` changed: a stream on the default, or
    /// none at all, moves to the new one; one on a chosen device stays. The
    /// processing thread first checks whether the stream is already there
    /// ([`take_default_moved`]): a stream reopened because its device went
    /// away is often on the new default before this arrives.
    /// (Called by the MMDevice API's notifications, on Windows only.)
    ///
    /// [`take_default_moved`]: Self::take_default_moved
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(super) fn default_changed(&self, input: bool) {
        if self.placement(input) != Placement::Chosen {
            self.side(input)
                .default_moved
                .store(true, Ordering::Release);
            self.thread.unpark();
        }
    }

    /// Whether the default for `input` changed since the last call; clears
    /// it.
    pub(super) fn take_default_moved(&self, input: bool) -> bool {
        self.side(input).default_moved.swap(false, Ordering::AcqRel)
    }

    /// Devices came or went: a displaced stream may go back (the chosen
    /// device returned, or a device appeared where there was none). Whether
    /// it can is checked on the processing thread ([`take_check`]).
    ///
    /// [`take_check`]: Self::take_check
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(super) fn devices_changed(&self) {
        for input in [false, true] {
            if matches!(
                self.placement(input),
                Placement::FellBack | Placement::Missing
            ) {
                self.side(input).check.store(true, Ordering::Release);
                self.thread.unpark();
            }
        }
    }

    /// Whether to check if the stream for `input` can go back; clears it.
    pub(super) fn take_check(&self, input: bool) -> bool {
        self.side(input).check.swap(false, Ordering::AcqRel)
    }

    /// Asks for a reopen from the processing thread (a check succeeded).
    pub(super) fn reopen_later(&self, input: bool) {
        self.side(input).requested.store(true, Ordering::Release);
    }

    /// Whether a reopen is asked for, without clearing it.
    pub(super) fn requested(&self, input: bool) -> bool {
        self.side(input).requested.load(Ordering::Acquire)
    }

    /// Takes the request for `input`.
    pub(super) fn take_request(&self, input: bool) -> bool {
        self.side(input).requested.swap(false, Ordering::AcqRel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reroute() -> Reroute {
        Reroute::new(std::thread::current())
    }

    #[test]
    fn a_lost_stream_is_reopened_wherever_it_is() {
        for placement in [
            Placement::Chosen,
            Placement::Default,
            Placement::FellBack,
            Placement::Missing,
        ] {
            let r = reroute();
            r.placed(true, placement);
            r.lost(true);
            assert!(r.take_request(true), "{placement:?}");
            assert!(!r.take_request(true), "taken once");
            assert!(!r.requested(false), "the other direction is untouched");
        }
    }

    #[test]
    fn a_new_default_moves_all_but_a_chosen_device() {
        for (placement, moves) in [
            (Placement::Chosen, false),
            (Placement::Default, true),
            (Placement::FellBack, true),
            (Placement::Missing, true),
        ] {
            let r = reroute();
            r.placed(false, placement);
            r.default_changed(false);
            assert_eq!(r.take_default_moved(false), moves, "{placement:?}");
            assert!(!r.take_default_moved(false), "taken once");
            assert!(!r.requested(false), "the processing thread decides");
        }
    }

    #[test]
    fn device_changes_check_only_displaced_streams() {
        for (placement, checks) in [
            (Placement::Chosen, false),
            (Placement::Default, false),
            (Placement::FellBack, true),
            (Placement::Missing, true),
        ] {
            let r = reroute();
            r.placed(true, placement);
            r.placed(false, Placement::Chosen);
            r.devices_changed();
            assert_eq!(r.take_check(true), checks, "{placement:?}");
            assert!(!r.take_check(true), "taken once");
            assert!(!r.take_check(false));
            assert!(!r.requested(true), "a check is not a reopen");
        }
    }
}
