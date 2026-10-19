//! Our Vulkan code (plan Task 4): the device, conversion and scaling in
//! compute shaders, and DMA-BUF import. The frames go back to CPU memory
//! for LiveKit's encoders (plan amendment 2026-10-14).
//!
//! The compute shaders are WGSL in `shaders/`, compiled to SPIR-V and
//! validated by naga at build time (`build.rs`).

mod convert;
mod device;
#[cfg(target_os = "linux")]
mod dmabuf;

pub use convert::Converter;
pub use device::GpuDevice;
#[cfg(all(target_os = "linux", feature = "hw-video"))]
pub use dmabuf::export_for_test;
#[cfg(target_os = "linux")]
pub use dmabuf::{ABGR8888, ARGB8888, XBGR8888, XRGB8888};

use ash::vk;

/// GPU errors. Messages are fixed text (and a Vulkan result code).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GpuError {
    /// No Vulkan loader, or no device that fits.
    #[error("Vulkan unavailable: {0}")]
    NoVulkan(&'static str),
    /// The device lacks something we need.
    #[error("not supported by this GPU: {0}")]
    NotSupported(&'static str),
    /// A Vulkan call failed.
    #[error("Vulkan call failed: {what} ({result})")]
    Vulkan {
        /// What we were doing.
        what: &'static str,
        /// The `VkResult` code.
        result: i32,
    },
    /// A frame or setting we cannot use.
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    /// A queue's lock was poisoned by a panic while it was held.
    #[error("GPU queue lock poisoned")]
    Poisoned,
}

impl GpuError {
    pub(crate) fn vulkan(what: &'static str, result: vk::Result) -> Self {
        Self::Vulkan {
            what,
            result: result.as_raw(),
        }
    }
}

/// `.vk("what")` on Vulkan results.
pub(crate) trait VkContext<T> {
    fn vk(self, what: &'static str) -> Result<T, GpuError>;
}

impl<T> VkContext<T> for Result<T, vk::Result> {
    fn vk(self, what: &'static str) -> Result<T, GpuError> {
        self.map_err(|e| GpuError::vulkan(what, e))
    }
}
