//! Our screen and camera as captured (plan Phase 9 Tasks 8 and 9): what
//! the connection that publishes a track holds while it sends one, the sink
//! the capturer hands frames to, and the camera preview's.
//!
//! A capture starts before its track is published: the portal's picker
//! must be answered first, and the track takes the captured size. Its frames
//! wait in a [`PusherSlot`] until the track exists (until then they are
//! dropped). A capture that ends by itself (the person stopped sharing from
//! the compositor, the window closed) says so with its serial, so the
//! engine stops that share, never a newer one.
//!
//! Frames and window titles are never logged.

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

#[cfg(target_os = "linux")]
use tokio::sync::mpsc;
#[cfg(target_os = "linux")]
use twicecat_video::CpuFrame;
use twicecat_video::capture::CaptureEnd;
#[cfg(target_os = "linux")]
use twicecat_video::capture::FrameSink;

use super::video::FramePusher;
use crate::api::VideoKind;

/// A running capture.
pub(crate) enum Capture {
    /// Through the ScreenCast portal (Wayland).
    #[cfg(target_os = "linux")]
    Portal(twicecat_video::capture::portal::ScreenShare),
    /// Through MIT-SHM (X11).
    #[cfg(target_os = "linux")]
    X11(twicecat_video::capture::x11::X11Capture),
    /// A camera, through PipeWire.
    #[cfg(target_os = "linux")]
    Camera(twicecat_video::capture::camera::Camera),
}

impl Capture {
    /// Stops the frames (and closes the portal session), off the async
    /// threads where it waits for the capture thread.
    pub(crate) async fn stop(self) {
        match self {
            #[cfg(target_os = "linux")]
            Self::Portal(share) => share.close().await,
            #[cfg(target_os = "linux")]
            Self::X11(capture) => {
                let _ = tokio::task::spawn_blocking(move || drop(capture)).await;
            }
            #[cfg(target_os = "linux")]
            Self::Camera(camera) => {
                let _ = tokio::task::spawn_blocking(move || drop(camera)).await;
            }
        }
    }
}

/// Where a capture's frames go once its track is published.
#[derive(Clone, Default)]
pub(crate) struct PusherSlot(Arc<StdMutex<Option<FramePusher>>>);

impl PusherSlot {
    /// Frames go to `pusher` from now on.
    pub(crate) fn set(&self, pusher: FramePusher) {
        *self.0.lock().expect("pusher slot lock") = Some(pusher);
    }

    /// No more frames go anywhere (the track is being taken down).
    pub(crate) fn clear(&self) {
        self.0.lock().expect("pusher slot lock").take();
    }
}

/// Identifies one capture, to match its end to the share it belongs to.
pub(crate) type CaptureSerial = u64;

/// A new serial.
#[cfg(target_os = "linux")]
pub(crate) fn next_serial() -> CaptureSerial {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A capture's end, for the engine.
pub(crate) type CaptureEnded = (VideoKind, CaptureSerial, CaptureEnd);

/// The capturer's side: frames into the slot, the end to the engine.
#[cfg(target_os = "linux")]
pub(crate) struct CaptureSink {
    pub slot: PusherSlot,
    pub kind: VideoKind,
    pub serial: CaptureSerial,
    pub ended: mpsc::UnboundedSender<CaptureEnded>,
}

#[cfg(target_os = "linux")]
impl FrameSink for CaptureSink {
    fn frame(&mut self, frame: CpuFrame) {
        if let Some(pusher) = self.slot.0.lock().expect("pusher slot lock").as_mut() {
            pusher.push(&frame);
        }
    }

    fn ended(&mut self, end: CaptureEnd) {
        let _ = self.ended.send((self.kind, self.serial, end));
    }
}

/// The camera preview's side: frames to its feed, BT.709 limited range as
/// the self-view's, the end to the engine.
#[cfg(target_os = "linux")]
pub(crate) struct PreviewSink {
    pub feed: crate::api::VideoFeedSender,
    pub serial: CaptureSerial,
    pub ended: mpsc::UnboundedSender<CaptureEnded>,
}

#[cfg(target_os = "linux")]
impl FrameSink for PreviewSink {
    fn frame(&mut self, frame: CpuFrame) {
        use super::video::{clock_us, is_sent_as_is, to_api_frame, to_sent};
        let frame = if is_sent_as_is(&frame) {
            frame
        } else {
            match to_sent(&frame) {
                Some(frame) => frame,
                None => return,
            }
        };
        self.feed.send(to_api_frame(&frame, clock_us()));
    }

    fn ended(&mut self, end: CaptureEnd) {
        let _ = self.ended.send((VideoKind::Camera, self.serial, end));
    }
}

/// A started capture, ready to be published.
pub(crate) struct Captured {
    pub capture: Capture,
    pub slot: PusherSlot,
    pub serial: CaptureSerial,
    /// What is captured, in physical pixels, when known.
    pub size: Option<(u32, u32)>,
}
