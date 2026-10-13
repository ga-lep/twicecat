//! Twicecat video: frames, colour conversion, the test pattern, the quality
//! ladder and H.264 bitstream helpers; later capture and the hardware
//! encoders (plan docs/plans/phase9-video.md). Knows nothing about Matrix,
//! LiveKit or the UI; the core adapts between this crate and the call.
//!
//! Frames, window titles, device serials and encoded bytes are never
//! logged.

pub mod annexb;
pub mod convert;
mod frame;
pub mod ladder;
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
