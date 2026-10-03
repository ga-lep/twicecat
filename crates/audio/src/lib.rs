//! Tessera audio: devices, I/O threads, and the voice engine (mixing,
//! echo cancellation, noise suppression, gain, gate). Knows nothing about Matrix or LiveKit; the core
//! adapts between this crate and the call.
//!
//! Internally everything is 48 kHz mono `f32` in 10 ms [`Frame`]s.

mod chain;
mod convert;
mod denormals;
mod devices;
mod dfn;
mod frame;
mod gate;
mod io;
mod pipeline;
#[cfg(target_os = "linux")]
mod pw;
mod resample;
mod voice;

pub use chain::{
    CaptureChain, CaptureResult, ChainSettings, ChainStats, Enhancer, LEVEL_FLOOR_DB, NoiseMode,
    NsLevel,
};
pub use denormals::flush_denormals_on_this_thread;
pub use devices::{DeviceInfo, DeviceList, DeviceWatcher, list_devices};
pub use dfn::{DeepFilter, default_model_dir};
pub use frame::{FRAME_LEN, Frame, SAMPLE_RATE, SILENCE};
pub use gate::GateMode;
pub use io::{AudioIo, IoConfig, IoEvent, IoInfo, StreamInfo, VirtualDevice};
pub use pipeline::{FrameProcessor, IoCounters, IoStats};
pub use voice::{
    Meters, OUTGOING_RING_LEN, Outgoing, SOURCE_RING_LEN, SourceId, SourceInput, VoiceControl,
    VoiceEngine, VoiceStats,
};

/// Audio errors. Messages are fixed text: backend error text is not
/// forwarded (it can name devices, which is fine, but is not stable).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AudioError {
    /// No device of the needed direction exists.
    #[error("no audio device found")]
    NoDevice,
    /// The configured device is not available.
    #[error("the audio device is not available")]
    DeviceNotFound,
    /// The device is in exclusive use elsewhere.
    #[error("the audio device is busy")]
    DeviceBusy,
    /// The OS refused access (e.g. the microphone permission on macOS).
    #[error("permission to use the audio device was denied")]
    PermissionDenied,
    /// The device's sample format is not supported.
    #[error("the audio device's format is not supported")]
    UnsupportedFormat,
    /// Devices could not be listed.
    #[error("audio devices could not be listed")]
    Enumerate,
    /// Neither capture nor playback was requested.
    #[error("nothing to open")]
    NothingToOpen,
    /// The resampler could not be set up or failed.
    #[error("audio resampling failed")]
    Resampler,
    /// A noise suppression model is missing or could not be loaded.
    #[error("the noise suppression model could not be loaded")]
    Model,
    /// Any other backend failure.
    #[error("the audio backend failed")]
    Backend,
    /// Unexpected internal failure.
    #[error("internal audio error")]
    Internal,
}
