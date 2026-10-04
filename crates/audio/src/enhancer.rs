//! What can be plugged into the voice engine to clean speech: an
//! [`Enhancer`] for one voice (the microphone, or one remote voice), a
//! [`BatchCleaner`] for several remote voices at once. Both are loaded off
//! the processing thread, handed over through [`crate::VoiceControl`], and
//! handed back there to be dropped when replaced or failed (freeing a model,
//! or a GPU context, has no place on the processing thread).

use crate::AudioError;
use crate::frame::Frame;

/// A speech enhancer plugged into the chain in place of the built-in noise
/// suppressors ([`crate::NoiseMode::Enhancer`]), or into one remote voice:
/// DeepFilterNet3, on the CPU or a GPU (plan Tasks 13, 13b, 16).
pub trait Enhancer: Send + 'static {
    /// Cleans one frame in place. An error removes the enhancer: the chain
    /// falls back to RNNoise, a remote voice plays as it came.
    fn enhance(&mut self, frame: &mut Frame) -> Result<(), AudioError>;
    /// Most attenuation applied, dB (100 or more: no limit). Enhancers
    /// without one ignore it.
    fn set_attenuation_limit(&mut self, _db: f32) {}
}

/// Cleans several remote voices in one call per frame, each in a row of its
/// own: DeepFilterNet3's batched graph on a GPU (plan Task 16).
pub trait BatchCleaner: Send + 'static {
    /// Starts `row` afresh, for a new voice.
    fn reset_row(&mut self, row: usize) -> Result<(), AudioError>;
    /// Cleans each of `frames` in place as the row at the same place in
    /// `rows`, all in one call; rows not listed run on silence. An error (a
    /// failing GPU, or one too slow frames in a row) removes the bank: its
    /// voices play as they came.
    fn clean(&mut self, frames: &mut [Frame], rows: &[usize]) -> Result<(), AudioError>;
    /// Most attenuation applied, dB (100 or more: no limit).
    fn set_attenuation_limit(&mut self, _db: f32) {}
}
