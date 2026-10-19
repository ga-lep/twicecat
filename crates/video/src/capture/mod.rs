//! Screen and camera capture (plan Tasks 8 and 9): what a capturer hands
//! over and how it says it stopped. Linux has two screen backends: the
//! ScreenCast portal with its PipeWire stream (Wayland, and X11 desktops
//! that run the portal), and our own picker data and MIT-SHM capture for X11
//! ([`x11`]). Cameras are PipeWire nodes ([`camera`]).
//!
//! A capturer converts every frame to NV12 before it hands it over, on its
//! own thread: screens to BT.709 limited range (what we send), DMA-BUFs on
//! the GPU they came from, shared memory on the CPU; cameras keep their own
//! colour space. Frames, window titles, camera identifiers and restore
//! tokens are never logged.

#[cfg(target_os = "linux")]
pub mod camera;
#[cfg(target_os = "linux")]
pub mod mjpeg;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(target_os = "linux")]
pub(crate) mod pw_video;
#[cfg(all(target_os = "linux", any(test, feature = "test-camera")))]
pub mod test_camera;
#[cfg(target_os = "linux")]
pub mod x11;

use crate::CpuFrame;

/// Why a capture stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureEnd {
    /// The person stopped it outside the app (the compositor's "stop
    /// sharing", a closed window).
    Stopped,
    /// It failed (the stream broke, the window vanished).
    Lost,
}

/// Where a capturer hands its frames. Called on the capturer's thread.
pub trait FrameSink: Send + 'static {
    /// A captured frame, NV12 in BT.709 limited range, no larger than the
    /// capture's `max_size`, at most `max_fps` a second.
    fn frame(&mut self, frame: CpuFrame);
    /// The capture ended; no frame follows. Called at most once, and not
    /// when the capture is dropped.
    fn ended(&mut self, end: CaptureEnd);
}

/// What capture can fail with. Messages are fixed text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CaptureError {
    /// The person cancelled the system's picker.
    #[error("screen sharing was cancelled")]
    Cancelled,
    /// No way to capture here (no portal, no X server, a missing feature).
    #[error("screen sharing is not available: {0}")]
    Unavailable(&'static str),
    /// The source asked for is not there (any more).
    #[error("that screen or window is not available")]
    NoSuchSource,
    /// Something failed on the way.
    #[error("screen capture failed: {0}")]
    Failed(&'static str),
}

/// How the screen is captured on this system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenBackend {
    /// The ScreenCast portal: the system shows its own picker.
    Portal,
    /// X11 without a portal session: our picker, MIT-SHM capture.
    X11,
}

impl ScreenBackend {
    /// The backend for this session, from its environment: a Wayland
    /// session uses the portal; an X11 session uses our own picker (the
    /// portal on X11 offers less, and only on some desktops). `None` when
    /// there is no display at all.
    pub fn detect() -> Option<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Self::from_env(
            var("XDG_SESSION_TYPE").as_deref(),
            var("WAYLAND_DISPLAY").is_some(),
            var("DISPLAY").is_some(),
        )
    }

    fn from_env(session_type: Option<&str>, wayland: bool, x11: bool) -> Option<Self> {
        if !cfg!(target_os = "linux") {
            return None;
        }
        match session_type {
            Some("wayland") => Some(Self::Portal),
            Some("x11") if x11 => Some(Self::X11),
            _ if wayland => Some(Self::Portal),
            _ if x11 => Some(Self::X11),
            _ => None,
        }
    }
}

/// The largest `(width, height)` with `width`×`height`'s aspect that fits
/// in `max`, both even (4:2:0); the size itself (made even) when it fits.
pub fn fit_size((width, height): (u32, u32), (max_w, max_h): (u32, u32)) -> (u32, u32) {
    let (w, h) = (u64::from(width.max(2)), u64::from(height.max(2)));
    let (mw, mh) = (u64::from(max_w.max(2)), u64::from(max_h.max(2)));
    let (w, h) = if w <= mw && h <= mh {
        (w, h)
    } else if w * mh >= h * mw {
        (mw, (h * mw / w).max(2))
    } else {
        ((w * mh / h).max(2), mh)
    };
    let even = |v: u64| u32::try_from(v & !1).unwrap_or(!1).max(2);
    (even(w), even(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backend_follows_the_session() {
        let pick = ScreenBackend::from_env;
        if cfg!(target_os = "linux") {
            assert_eq!(
                pick(Some("wayland"), true, true),
                Some(ScreenBackend::Portal)
            );
            // Xwayland's DISPLAY does not make a Wayland session X11.
            assert_eq!(
                pick(Some("wayland"), false, true),
                Some(ScreenBackend::Portal)
            );
            assert_eq!(pick(Some("x11"), false, true), Some(ScreenBackend::X11));
            assert_eq!(pick(None, true, true), Some(ScreenBackend::Portal));
            assert_eq!(pick(None, false, true), Some(ScreenBackend::X11));
            assert_eq!(pick(Some("tty"), false, false), None);
        } else {
            assert_eq!(pick(Some("wayland"), true, true), None);
        }
    }

    #[test]
    fn sizes_fit_keeping_their_shape_and_even() {
        assert_eq!(fit_size((1920, 1080), (3840, 2160)), (1920, 1080));
        assert_eq!(fit_size((3840, 2160), (1920, 1080)), (1920, 1080));
        // A tall window keeps its shape.
        assert_eq!(fit_size((1000, 3000), (1920, 1080)), (360, 1080));
        // A wide one too.
        assert_eq!(fit_size((5120, 1440), (1920, 1080)), (1920, 540));
        // Odd sizes become even.
        assert_eq!(fit_size((1365, 767), (1920, 1080)), (1364, 766));
        assert_eq!(fit_size((1, 1), (1920, 1080)), (2, 2));
    }
}
