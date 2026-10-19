//! Twicecat video: frames, colour conversion (CPU and Vulkan compute), the
//! test pattern, the quality ladder, H.264 bitstream helpers, the GPU probe
//! and screen capture (plan docs/plans/phase9-video.md). Encoding is
//! LiveKit's (plan amendment 2026-10-14). Knows nothing about Matrix,
//! LiveKit or the UI; the core adapts between this crate and the call.
//!
//! Frames, window titles, device serials and encoded bytes are never
//! logged.

pub mod annexb;
#[cfg(target_os = "linux")]
pub mod audio_share;
pub mod capture;
pub mod convert;
mod frame;
pub mod gpu;
pub mod ladder;
pub mod probe;
pub mod test_pattern;

pub use frame::{ColorSpace, CpuFrame, Frame, Matrix, PixelFormat, Plane, Range};
#[cfg(target_os = "linux")]
pub use frame::{DmaBufFrame, DmaBufPlane};

/// Video errors. Messages are fixed text.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum VideoError {
    /// A frame's planes do not match its format and size.
    #[error("invalid frame: {0}")]
    InvalidFrame(&'static str),
    /// The colour converter refused a frame.
    #[error("colour conversion failed")]
    Conversion,
    /// An encoded stream is malformed.
    #[error("invalid bitstream: {0}")]
    InvalidBitstream(&'static str),
}
