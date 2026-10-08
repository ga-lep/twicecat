//! The internal audio format: 48 kHz mono `f32` in 10 ms frames, the rate
//! and frame size of WebRTC audio processing and of Opus voice in LiveKit.

/// Internal sample rate, Hz.
pub const SAMPLE_RATE: u32 = 48_000;

/// Samples per 10 ms frame at [`SAMPLE_RATE`].
pub const FRAME_LEN: usize = (SAMPLE_RATE / 100) as usize;

/// One 10 ms mono frame, samples in -1.0..=1.0.
pub type Frame = [f32; FRAME_LEN];

/// A silent frame.
pub const SILENCE: Frame = [0.0; FRAME_LEN];
