//! What this machine can encode and decode, per GPU (plan Task 3 and Q6):
//! Vulkan Video's queues and H.264/H.265 capabilities, and on NVIDIA
//! cards CUDA's compute capability, NVENC's codecs and limits, NVDEC's, and
//! whether Vulkan memory can be handed to CUDA. [`choose`] picks the
//! encoder for a GPU from that.
//!
//! The probe opens the drivers' own libraries at run time (Vulkan's loader
//! through `ash`; `libcuda`, `libnvidia-encode`, `libnvcuvid` through
//! `libloading`), so a machine without them is simply reported as having
//! nothing. It takes a fraction of a second; callers run it once, off any
//! real-time thread.

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod nvidia;
mod vulkan;

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use nvidia::nvenc_session_limit;

/// Who made a GPU (PCI vendor ID).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Vendor {
    /// 0x8086.
    Intel,
    /// 0x1002.
    Amd,
    /// 0x10DE.
    Nvidia,
    /// Anything else (its vendor ID).
    Other(u32),
}

impl Vendor {
    /// From a PCI vendor ID.
    pub fn from_pci(id: u32) -> Self {
        match id {
            0x8086 => Self::Intel,
            0x1002 => Self::Amd,
            0x10DE => Self::Nvidia,
            other => Self::Other(other),
        }
    }
}

/// Whether a GPU is part of the CPU package or a card of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpuKind {
    /// Shares memory with the CPU (a laptop's iGPU).
    Integrated,
    /// Its own memory.
    Discrete,
    /// Virtual, CPU (llvmpipe) or unknown.
    Other,
}

/// What a Vulkan Video encoder can do for one codec (8-bit 4:2:0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VulkanEncodeCaps {
    /// Largest coded picture, pixels.
    pub max_width: u32,
    /// Largest coded picture, pixels.
    pub max_height: u32,
    /// The highest level, as the codec numbers it (H.264 `52` for 5.2,
    /// H.265 `153` for 5.1 × 30).
    pub max_level: u32,
    /// The most bits per second the encoder takes.
    pub max_bitrate: u64,
    /// Constant bitrate.
    pub cbr: bool,
    /// Variable bitrate.
    pub vbr: bool,
    /// Pictures the encoder can keep as references.
    pub max_dpb_slots: u32,
    /// References a picture may use at once.
    pub max_active_references: u32,
}

/// Vulkan Video on one GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VulkanVideo {
    /// The device's Vulkan version (major, minor, patch).
    pub api_version: (u32, u32, u32),
    /// The driver's name (`NVIDIA`, `Intel open-source Mesa driver`, …).
    pub driver: String,
    /// A queue family can encode video.
    pub encode_queue: bool,
    /// H.264 encode, Constrained Baseline (what we send).
    pub h264: Option<VulkanEncodeCaps>,
    /// H.265 encode, Main.
    pub h265: Option<VulkanEncodeCaps>,
    /// The driver offers `VK_KHR_video_encode_av1` (our `ash` cannot use it
    /// yet, plan Q4).
    pub av1_extension: bool,
    /// A queue family can decode H.264.
    pub h264_decode: bool,
    /// DMA-BUFs can be imported (`VK_EXT_external_memory_dma_buf` and
    /// `VK_EXT_image_drm_format_modifier`).
    pub dma_buf_import: bool,
    /// Memory can be exported as a file descriptor
    /// (`VK_KHR_external_memory_fd`), as NVENC through CUDA needs.
    pub external_memory_fd: bool,
}

/// What NVENC can do for one codec.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NvencCodecCaps {
    /// Largest picture, pixels.
    pub max_width: u32,
    /// Largest picture, pixels.
    pub max_height: u32,
    /// Macroblocks (16 × 16) per second at most, as the driver states it;
    /// 4K60 needs 1,944,000. For H.264 this follows the level's limit, not
    /// the engine's speed.
    pub max_mb_per_sec: u32,
    /// The highest level, as NVENC numbers it (`NV_ENC_LEVEL`: H.264 52
    /// for 5.2, H.265 156 for 5.2, AV1 14 for 5.2).
    pub max_level: u32,
    /// Temporal layers it can code (0 or 1: none).
    pub max_temporal_layers: u32,
    /// The bitrate can change without a new session.
    pub dynamic_bitrate: bool,
    /// Separate encoder engines on the chip.
    pub engines: u32,
}

/// What NVDEC can decode for one codec (8-bit 4:2:0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NvdecCodecCaps {
    /// Largest picture, pixels.
    pub max_width: u32,
    /// Largest picture, pixels.
    pub max_height: u32,
    /// Decoder engines that can take it.
    pub engines: u32,
}

/// NVIDIA's own video and compute APIs on one GPU.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Nvidia {
    /// CUDA compute capability (8.9 is Ada).
    pub compute_capability: (u32, u32),
    /// The CUDA version the driver supports (`12080` for 12.8).
    pub cuda_driver_version: u32,
    /// The highest NVENC API version the driver offers (major, minor);
    /// `None` without `libnvidia-encode`.
    pub nvenc_api: Option<(u32, u32)>,
    /// NVENC, H.264.
    pub nvenc_h264: Option<NvencCodecCaps>,
    /// NVENC, H.265.
    pub nvenc_h265: Option<NvencCodecCaps>,
    /// NVENC, AV1.
    pub nvenc_av1: Option<NvencCodecCaps>,
    /// NVDEC, H.264.
    pub nvdec_h264: Option<NvdecCodecCaps>,
    /// NVDEC, AV1.
    pub nvdec_av1: Option<NvdecCodecCaps>,
    /// Vulkan memory exported as a file descriptor was imported into CUDA
    /// and read back the same (`None`: not tried).
    pub vulkan_interop: Option<bool>,
}

/// One GPU and its video engines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gpu {
    /// The device's name.
    pub name: String,
    /// Who made it.
    pub vendor: Vendor,
    /// Integrated or discrete.
    pub kind: GpuKind,
    /// Vulkan's device UUID (CUDA reports the same one for NVIDIA cards).
    pub uuid: [u8; 16],
    /// The DRM render node (major, minor), to match the GPU a DMA-BUF
    /// comes from (Linux, `VK_EXT_physical_device_drm`).
    pub render_node: Option<(i64, i64)>,
    /// Vulkan Video.
    pub vulkan: VulkanVideo,
    /// NVENC, NVDEC and CUDA (NVIDIA cards with the proprietary driver).
    pub nvidia: Option<Nvidia>,
}

/// Every GPU found, in Vulkan's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// The GPUs.
    pub gpus: Vec<Gpu>,
    /// Why Vulkan could not be asked, if it could not (no loader, no
    /// driver); fixed text.
    pub vulkan_error: Option<&'static str>,
}

/// The encoder [`choose`] picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// NVENC through CUDA.
    Nvenc,
    /// Vulkan Video.
    VulkanVideo,
}

/// Ada Lovelace's compute capability: from here on NVIDIA cards use NVENC
/// (plan Q6, the human's choice).
pub const ADA: (u32, u32) = (8, 9);

/// The H.264 encoder for `gpu` (plan Q6):
/// - NVIDIA Ada and newer: NVENC;
/// - Intel, AMD and NVIDIA before Ada: Vulkan Video;
/// - an NVIDIA GPU before Ada without Vulkan Video encode: NVENC;
/// - otherwise none (the software path).
///
/// Vulkan Video counts only with a bitrate mode (CBR or VBR): WebRTC sets
/// the bitrate, and an encoder without rate control cannot follow it
/// (Mesa 26.2's hidden ANV encoder offers only a constant quantizer).
pub fn choose(gpu: &Gpu) -> Option<Backend> {
    let nvenc = gpu.nvidia.as_ref().is_some_and(|n| n.nvenc_h264.is_some());
    let modern = gpu
        .nvidia
        .as_ref()
        .is_some_and(|n| n.compute_capability >= ADA);
    let vulkan = gpu.vulkan.encode_queue
        && gpu
            .vulkan
            .h264
            .as_ref()
            .is_some_and(|caps| caps.cbr || caps.vbr);
    if nvenc && modern {
        Some(Backend::Nvenc)
    } else if vulkan {
        Some(Backend::VulkanVideo)
    } else if nvenc {
        Some(Backend::Nvenc)
    } else {
        None
    }
}

/// Probes every GPU. Never fails: what cannot be asked is left out, and
/// [`Probe::vulkan_error`] says why Vulkan could not.
pub fn probe() -> Probe {
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "windows")),
        expect(unused_mut, reason = "only the NVIDIA probe adds to it")
    )]
    let mut out = match vulkan::probe() {
        Ok(gpus) => Probe {
            gpus,
            vulkan_error: None,
        },
        Err(why) => Probe {
            gpus: Vec::new(),
            vulkan_error: Some(why),
        },
    };
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    nvidia::probe(&mut out.gpus);
    out
}

/// The Vulkan GPUs only, without opening NVIDIA's libraries (to pick the
/// device capture imports on); empty when Vulkan cannot be asked.
pub(crate) fn vulkan_gpus() -> Vec<Gpu> {
    vulkan::probe().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gpu(vendor: Vendor, vulkan_h264: bool, nvidia: Option<((u32, u32), bool)>) -> Gpu {
        let caps = VulkanEncodeCaps {
            max_width: 4096,
            max_height: 4096,
            max_level: 52,
            max_bitrate: 100_000_000,
            cbr: true,
            vbr: true,
            max_dpb_slots: 16,
            max_active_references: 2,
        };
        let nvenc = NvencCodecCaps {
            max_width: 4096,
            max_height: 4096,
            max_mb_per_sec: 2_000_000,
            max_level: 52,
            max_temporal_layers: 4,
            dynamic_bitrate: true,
            engines: 1,
        };
        Gpu {
            name: "test".into(),
            vendor,
            kind: GpuKind::Discrete,
            uuid: [0; 16],
            render_node: None,
            vulkan: VulkanVideo {
                encode_queue: vulkan_h264,
                h264: vulkan_h264.then_some(caps),
                ..VulkanVideo::default()
            },
            nvidia: nvidia.map(|(cc, has_nvenc)| Nvidia {
                compute_capability: cc,
                nvenc_h264: has_nvenc.then_some(nvenc),
                ..Nvidia::default()
            }),
        }
    }

    #[test]
    fn nvidia_ada_and_newer_use_nvenc() {
        for cc in [(8, 9), (9, 0), (10, 0), (12, 0)] {
            let g = gpu(Vendor::Nvidia, true, Some((cc, true)));
            assert_eq!(choose(&g), Some(Backend::Nvenc), "{cc:?}");
        }
    }

    #[test]
    fn older_nvidia_uses_vulkan_video_and_falls_back_to_nvenc() {
        for cc in [(7, 5), (8, 6), (6, 1)] {
            let g = gpu(Vendor::Nvidia, true, Some((cc, true)));
            assert_eq!(choose(&g), Some(Backend::VulkanVideo), "{cc:?}");
            let no_vulkan = gpu(Vendor::Nvidia, false, Some((cc, true)));
            assert_eq!(choose(&no_vulkan), Some(Backend::Nvenc), "{cc:?}");
        }
    }

    #[test]
    fn intel_and_amd_use_vulkan_video_or_nothing() {
        for vendor in [Vendor::Intel, Vendor::Amd] {
            assert_eq!(choose(&gpu(vendor, true, None)), Some(Backend::VulkanVideo));
            assert_eq!(choose(&gpu(vendor, false, None)), None);
        }
    }

    #[test]
    fn an_ada_card_without_nvenc_uses_vulkan_video() {
        // nouveau or NVK: no libnvidia-encode, so no NVENC.
        let g = gpu(Vendor::Nvidia, true, Some(((8, 9), false)));
        assert_eq!(choose(&g), Some(Backend::VulkanVideo));
        let g = gpu(Vendor::Nvidia, true, None);
        assert_eq!(choose(&g), Some(Backend::VulkanVideo));
    }

    #[test]
    fn vulkan_video_without_rate_control_does_not_count() {
        let mut g = gpu(Vendor::Intel, true, None);
        if let Some(caps) = g.vulkan.h264.as_mut() {
            caps.cbr = false;
            caps.vbr = false;
        }
        assert_eq!(choose(&g), None);
        if let Some(caps) = g.vulkan.h264.as_mut() {
            caps.vbr = true;
        }
        assert_eq!(choose(&g), Some(Backend::VulkanVideo));
    }

    #[test]
    fn vendors_come_from_pci_ids() {
        assert_eq!(Vendor::from_pci(0x10DE), Vendor::Nvidia);
        assert_eq!(Vendor::from_pci(0x8086), Vendor::Intel);
        assert_eq!(Vendor::from_pci(0x1002), Vendor::Amd);
        assert_eq!(Vendor::from_pci(0x5143), Vendor::Other(0x5143));
    }
}
