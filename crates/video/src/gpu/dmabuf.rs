//! DMA-BUF import (plan Task 4): a PipeWire screen-capture buffer becomes
//! a Vulkan image the converter samples, without a copy
//! (`VK_EXT_external_memory_dma_buf`, `VK_EXT_image_drm_format_modifier`).
//! Single-plane BGRA/RGBA layouts for now (what PipeWire screen casts
//! offer); modifiers with auxiliary planes and YUV DMA-BUFs come back as
//! [`GpuError::NotSupported`]. Waiting for the producer's implicit fence
//! is Task 8's (the capture side).
//!
//! DRM fourcc and modifier values from `/usr/include/drm/drm_fourcc.h`
//! (lines 193-199 and 544).
//!
//! `unsafe`: Vulkan FFI, as in `device.rs`, plus handing a file
//! descriptor to the driver: the descriptor is a fresh duplicate, and the
//! driver owns it only once the import succeeded (it is closed by us
//! otherwise).

use std::{os::fd::IntoRawFd, sync::Arc};

use ash::vk;

use super::{
    GpuError, VkContext,
    device::{Image, Shared, view},
};
use crate::DmaBufFrame;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

/// `DRM_FORMAT_XRGB8888`: B, G, R, x in memory.
pub const XRGB8888: u32 = fourcc(b'X', b'R', b'2', b'4');
/// `DRM_FORMAT_ARGB8888`: B, G, R, A in memory.
pub const ARGB8888: u32 = fourcc(b'A', b'R', b'2', b'4');
/// `DRM_FORMAT_XBGR8888`: R, G, B, x in memory.
pub const XBGR8888: u32 = fourcc(b'X', b'B', b'2', b'4');
/// `DRM_FORMAT_ABGR8888`: R, G, B, A in memory.
pub const ABGR8888: u32 = fourcc(b'A', b'B', b'2', b'4');
/// `DRM_FORMAT_MOD_LINEAR`.
#[cfg(feature = "hw-video")]
const MOD_LINEAR: u64 = 0;

/// The Vulkan format for a DRM fourcc we import.
fn format_of(fourcc: u32) -> Option<vk::Format> {
    match fourcc {
        XRGB8888 | ARGB8888 => Some(vk::Format::B8G8R8A8_UNORM),
        XBGR8888 | ABGR8888 => Some(vk::Format::R8G8B8A8_UNORM),
        _ => None,
    }
}

/// Imports `frame` as a sampled image (one view, in the frame's format).
/// It is left in `GENERAL`, owned by the foreign (or external) queue
/// family; the converter acquires it.
pub(super) fn import(shared: &Arc<Shared>, frame: &DmaBufFrame) -> Result<Image, GpuError> {
    let dma = shared
        .dma_buf
        .as_ref()
        .ok_or(GpuError::NotSupported("DMA-BUF import"))?;
    let format = format_of(frame.fourcc).ok_or(GpuError::NotSupported("DMA-BUF format"))?;
    let [plane] = frame.planes.as_slice() else {
        return Err(GpuError::NotSupported("multi-plane DMA-BUF"));
    };
    if frame.width == 0 || frame.height == 0 {
        return Err(GpuError::Invalid("empty DMA-BUF"));
    }
    let usage = vk::ImageUsageFlags::SAMPLED;
    check_importable(shared, format, frame.modifier, usage)?;

    let layouts = [vk::SubresourceLayout {
        offset: u64::from(plane.offset),
        size: 0,
        row_pitch: u64::from(plane.stride),
        array_pitch: 0,
        depth_pitch: 0,
    }];
    let mut explicit = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
        .drm_format_modifier(frame.modifier)
        .plane_layouts(&layouts);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width: frame.width,
            height: frame.height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut explicit)
        .push_next(&mut external);
    let d = &shared.device;
    // SAFETY: the format, modifier and usage were checked importable; the
    // chain lives until the call returns.
    let image = unsafe { d.create_image(&info, None) }.vk("create DMA-BUF image")?;
    match bind_imported(shared, dma, image, plane.fd.try_clone()) {
        Ok(memory) => {
            let extent = vk::Extent2D {
                width: frame.width,
                height: frame.height,
            };
            let mut out = Image::from_parts(shared, image, memory, extent);
            out.views
                .push(view(shared, image, format, vk::ImageAspectFlags::COLOR, 0)?);
            Ok(out)
        }
        Err(e) => {
            // SAFETY: the image has no memory and was never used.
            unsafe { d.destroy_image(image, None) };
            Err(e)
        }
    }
}

fn check_importable(
    shared: &Shared,
    format: vk::Format,
    modifier: u64,
    usage: vk::ImageUsageFlags,
) -> Result<(), GpuError> {
    let mut modifier_info = vk::PhysicalDeviceImageDrmFormatModifierInfoEXT::default()
        .drm_format_modifier(modifier)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(format)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(usage)
        .push_next(&mut modifier_info)
        .push_next(&mut external_info);
    let mut external = vk::ExternalImageFormatProperties::default();
    let mut props = vk::ImageFormatProperties2::default().push_next(&mut external);
    // SAFETY: the physical device belongs to this instance; the chains
    // live until the call returns.
    unsafe {
        shared
            .instance
            .get_physical_device_image_format_properties2(shared.physical, &info, &mut props)
    }
    .map_err(|_| GpuError::NotSupported("DMA-BUF format and modifier"))?;
    if !external
        .external_memory_properties
        .external_memory_features
        .contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE)
    {
        return Err(GpuError::NotSupported("DMA-BUF import of this layout"));
    }
    Ok(())
}

fn bind_imported(
    shared: &Shared,
    dma: &super::device::DmaBuf,
    image: vk::Image,
    fd: std::io::Result<std::os::fd::OwnedFd>,
) -> Result<vk::DeviceMemory, GpuError> {
    let fd = fd.map_err(|_| GpuError::Invalid("DMA-BUF descriptor"))?;
    let d = &shared.device;
    let mut fd_props = vk::MemoryFdPropertiesKHR::default();
    // SAFETY: `fd` is an open DMA-BUF descriptor; the call only reads it.
    unsafe {
        dma.memory_fd.get_memory_fd_properties(
            vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
            std::os::fd::AsRawFd::as_raw_fd(&fd),
            &mut fd_props,
        )
    }
    .vk("read DMA-BUF memory properties")?;
    // SAFETY: the image was just made from this device.
    let requirements = unsafe { d.get_image_memory_requirements(image) };
    let bits = requirements.memory_type_bits & fd_props.memory_type_bits;
    let index = shared
        .memory_type(
            bits,
            vk::MemoryPropertyFlags::empty(),
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
        )
        .ok_or(GpuError::NotSupported("memory type for the DMA-BUF"))?;
    let raw = fd.into_raw_fd();
    let mut import = vk::ImportMemoryFdInfoKHR::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
        .fd(raw);
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let info = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(index)
        .push_next(&mut import)
        .push_next(&mut dedicated);
    // SAFETY: `raw` is our own duplicate; on success the driver owns it,
    // on failure it is closed below.
    let memory = match unsafe { d.allocate_memory(&info, None) } {
        Ok(m) => m,
        Err(e) => {
            // SAFETY: the import failed, so the descriptor is still ours.
            drop(unsafe { <std::os::fd::OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(raw) });
            return Err(GpuError::vulkan("import DMA-BUF", e));
        }
    };
    // SAFETY: the memory was imported for this image (dedicated).
    if let Err(e) = unsafe { d.bind_image_memory(image, memory, 0) } {
        // SAFETY: the memory is not bound to anything.
        unsafe { d.free_memory(memory, None) };
        return Err(GpuError::vulkan("bind DMA-BUF memory", e));
    }
    Ok(memory)
}

/// Test helper: a linear BGRA DMA-BUF on `device` holding `frame`, as a
/// capturer would hand it over (exported from Vulkan memory).
///
/// # Errors
/// [`GpuError::NotSupported`] when the device cannot export linear BGRA
/// DMA-BUFs.
#[cfg(feature = "hw-video")]
pub fn export_for_test(
    device: &super::GpuDevice,
    frame: &crate::CpuFrame,
) -> Result<DmaBufFrame, GpuError> {
    use super::device::{Buffer, Commands, Host, barrier, barrier_families};
    use crate::{DmaBufPlane, PixelFormat};
    use std::os::fd::FromRawFd;

    if frame.format() != PixelFormat::Bgra {
        return Err(GpuError::Invalid("export takes BGRA"));
    }
    let shared = &device.shared;
    let dma = shared
        .dma_buf
        .as_ref()
        .ok_or(GpuError::NotSupported("DMA-BUF export"))?;
    let d = &shared.device;
    let (w, h) = (frame.width(), frame.height());
    let modifiers = [MOD_LINEAR];
    let mut list =
        vk::ImageDrmFormatModifierListCreateInfoEXT::default().drm_format_modifiers(&modifiers);
    let mut external = vk::ExternalMemoryImageCreateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(vk::Format::B8G8R8A8_UNORM)
        .extent(vk::Extent3D {
            width: w,
            height: h,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
        .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .push_next(&mut list)
        .push_next(&mut external);
    // SAFETY: the chain lives until the call returns.
    let image = unsafe { d.create_image(&info, None) }.vk("create export image")?;
    // SAFETY: the image was just made.
    let requirements = unsafe { d.get_image_memory_requirements(image) };
    let index = shared
        .memory_type(
            requirements.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
            vk::MemoryPropertyFlags::empty(),
        )
        .ok_or(GpuError::NotSupported("memory type for export"))?;
    let mut export = vk::ExportMemoryAllocateInfo::default()
        .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
    let alloc = vk::MemoryAllocateInfo::default()
        .allocation_size(requirements.size)
        .memory_type_index(index)
        .push_next(&mut export)
        .push_next(&mut dedicated);
    // SAFETY: as above.
    let memory = unsafe { d.allocate_memory(&alloc, None) }.vk("allocate export memory")?;
    // The image owns the memory from here (freed on drop).
    let image = Image::from_parts(
        shared,
        image,
        memory,
        vk::Extent2D {
            width: w,
            height: h,
        },
    );
    // SAFETY: dedicated allocation for this image.
    unsafe { d.bind_image_memory(image.image, memory, 0) }.vk("bind export memory")?;

    let mut staging = Buffer::new(
        shared,
        u64::from(w) * u64::from(h) * 4,
        vk::BufferUsageFlags::TRANSFER_SRC,
        Some(Host::Upload),
    )?;
    {
        let dst = staging.bytes_mut();
        let row = w as usize * 4;
        for y in 0..h as usize {
            dst[y * row..(y + 1) * row].copy_from_slice(frame.row(0, y));
        }
    }
    staging.flush()?;
    let mut commands = Commands::new(shared, shared.compute_family)?;
    let cmd = commands.begin()?;
    barrier(
        shared,
        cmd,
        image.image,
        vk::ImageAspectFlags::COLOR,
        1,
        (
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        ),
        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
        (
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_WRITE,
        ),
    );
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width: w,
            height: h,
            depth: 1,
        });
    // SAFETY: `cmd` is recording; the image is in TRANSFER_DST_OPTIMAL.
    unsafe {
        d.cmd_copy_buffer_to_image(
            cmd,
            staging.buffer,
            image.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[region],
        );
    }
    let dst_family = if dma.foreign {
        vk::QUEUE_FAMILY_FOREIGN_EXT
    } else {
        vk::QUEUE_FAMILY_EXTERNAL
    };
    barrier_families(
        shared,
        cmd,
        image.image,
        vk::ImageAspectFlags::COLOR,
        1,
        (
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::GENERAL,
        ),
        (
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_WRITE,
        ),
        (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
        (shared.compute_family, dst_family),
    );
    commands.submit(&shared.compute_queue)?;
    commands.wait()?;

    let subresource =
        vk::ImageSubresource::default().aspect_mask(vk::ImageAspectFlags::MEMORY_PLANE_0_EXT);
    // SAFETY: the image uses DRM modifier tiling, so memory plane 0 is a
    // valid aspect.
    let layout = unsafe { d.get_image_subresource_layout(image.image, subresource) };
    let get = vk::MemoryGetFdInfoKHR::default()
        .memory(memory)
        .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
    // SAFETY: the memory was allocated exportable as DMA-BUF.
    let raw = unsafe { dma.memory_fd.get_memory_fd(&get) }.vk("export DMA-BUF")?;
    // SAFETY: the driver handed us a new descriptor we now own; the
    // DMA-BUF stays alive through it after the image and memory go.
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    Ok(DmaBufFrame {
        fourcc: ARGB8888,
        modifier: MOD_LINEAR,
        width: w,
        height: h,
        planes: vec![DmaBufPlane {
            fd,
            offset: u32::try_from(layout.offset).map_err(|_| GpuError::Invalid("offset"))?,
            stride: u32::try_from(layout.row_pitch).map_err(|_| GpuError::Invalid("stride"))?,
        }],
        color: crate::ColorSpace::default(),
        timestamp: frame.timestamp(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourccs_match_drm_fourcc_h() {
        // 'X' 'R' '2' '4', little endian.
        assert_eq!(XRGB8888, 0x3432_5258);
        assert_eq!(ARGB8888, 0x3432_5241);
        assert_eq!(format_of(XRGB8888), Some(vk::Format::B8G8R8A8_UNORM));
        assert_eq!(format_of(ABGR8888), Some(vk::Format::R8G8B8A8_UNORM));
        assert_eq!(format_of(fourcc(b'N', b'V', b'1', b'2')), None);
    }
}
