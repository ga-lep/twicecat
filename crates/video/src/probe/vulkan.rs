//! The Vulkan side of the probe: every physical device, its video queues,
//! its H.264 and H.265 encode capabilities and the extensions our capture
//! import needs. Uses `ash` 0.38 (Vulkan 1.3.281 headers): its video
//! extensions are raw function tables, called here directly.
//!
//! `unsafe`: every Vulkan call is FFI into the loader and driver. Each one
//! is given handles this function made (instance, physical devices) and
//! structures that live until the call returns; the instance is destroyed
//! once, last, by [`InstanceGuard`].

use std::ffi::CStr;

use ash::vk;

use super::{Gpu, GpuKind, Vendor, VulkanEncodeCaps, VulkanVideo};

/// H.264 `level_idc` by `StdVideoH264LevelIdc` (ash `vk/native.rs`).
const H264_LEVELS: [u32; 19] = [
    10, 11, 12, 13, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52, 60, 61, 62,
];
/// H.265 `general_level_idc` (30 × level) by `StdVideoH265LevelIdc`.
const H265_LEVELS: [u32; 13] = [30, 60, 63, 90, 93, 120, 123, 150, 153, 156, 180, 183, 186];

/// `VK_KHR_video_encode_av1` (newer than our `ash`'s headers).
const AV1_ENCODE: &CStr = c"VK_KHR_video_encode_av1";

/// Destroys the instance when the probe ends, however it ends.
struct InstanceGuard(ash::Instance);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the instance was created by `probe` and nothing made from
        // it outlives the guard (physical devices need no destruction).
        unsafe { self.0.destroy_instance(None) };
    }
}

pub(super) fn probe() -> Result<Vec<Gpu>, &'static str> {
    // SAFETY: loads the system's Vulkan loader; nothing else runs code from
    // it until `create_instance`.
    let entry = unsafe { ash::Entry::load() }.map_err(|_| "no Vulkan loader")?;
    let app = vk::ApplicationInfo::default()
        .application_name(c"twicecat video probe")
        .api_version(vk::API_VERSION_1_3);
    let info = vk::InstanceCreateInfo::default().application_info(&app);
    // SAFETY: `info` and `app` live until the call returns.
    let instance = unsafe { entry.create_instance(&info, None) }
        .map_err(|_| "Vulkan instance could not be created")?;
    let instance = InstanceGuard(instance);
    let video = ash::khr::video_queue::Instance::new(&entry, &instance.0);
    // SAFETY: the instance is alive.
    let devices = unsafe { instance.0.enumerate_physical_devices() }
        .map_err(|_| "Vulkan devices could not be listed")?;
    Ok(devices
        .into_iter()
        .filter_map(|device| describe(&instance.0, &video, device))
        .collect())
}

fn describe(
    instance: &ash::Instance,
    video: &ash::khr::video_queue::Instance,
    device: vk::PhysicalDevice,
) -> Option<Gpu> {
    // SAFETY: `device` comes from this instance.
    let extensions = unsafe { instance.enumerate_device_extension_properties(device) }.ok()?;
    let has = |name: &CStr| {
        extensions
            .iter()
            .any(|e| e.extension_name_as_c_str() == Ok(name))
    };

    let mut id = vk::PhysicalDeviceIDProperties::default();
    let mut driver = vk::PhysicalDeviceDriverProperties::default();
    let mut drm = vk::PhysicalDeviceDrmPropertiesEXT::default();
    let has_drm = has(ash::ext::physical_device_drm::NAME);
    let mut props = vk::PhysicalDeviceProperties2::default()
        .push_next(&mut id)
        .push_next(&mut driver);
    if has_drm {
        props = props.push_next(&mut drm);
    }
    // SAFETY: every structure in the chain lives until the call returns and
    // is one the device knows (DRM only with its extension).
    unsafe { instance.get_physical_device_properties2(device, &mut props) };
    let base = props.properties;
    let name = base
        .device_name_as_c_str()
        .ok()?
        .to_string_lossy()
        .into_owned();
    let kind = match base.device_type {
        vk::PhysicalDeviceType::INTEGRATED_GPU => GpuKind::Integrated,
        vk::PhysicalDeviceType::DISCRETE_GPU => GpuKind::Discrete,
        _ => GpuKind::Other,
    };
    let driver_name = driver
        .driver_name_as_c_str()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let render_node =
        (has_drm && drm.has_render == vk::TRUE).then_some((drm.render_major, drm.render_minor));

    let (encode_queue, codecs) = queues(instance, device);
    let encode = |op, has_ext: bool| {
        (encode_queue && has_ext && codecs.contains(op))
            .then(|| encode_caps(video, device, op))
            .flatten()
    };
    let vulkan = VulkanVideo {
        api_version: (
            vk::api_version_major(base.api_version),
            vk::api_version_minor(base.api_version),
            vk::api_version_patch(base.api_version),
        ),
        driver: driver_name,
        encode_queue,
        h264: encode(
            vk::VideoCodecOperationFlagsKHR::ENCODE_H264,
            has(vk::KHR_VIDEO_ENCODE_H264_NAME),
        ),
        h265: encode(
            vk::VideoCodecOperationFlagsKHR::ENCODE_H265,
            has(vk::KHR_VIDEO_ENCODE_H265_NAME),
        ),
        av1_extension: has(AV1_ENCODE),
        h264_decode: codecs.contains(vk::VideoCodecOperationFlagsKHR::DECODE_H264)
            && has(vk::KHR_VIDEO_DECODE_H264_NAME),
        dma_buf_import: has(vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME)
            && has(vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME),
        external_memory_fd: has(vk::KHR_EXTERNAL_MEMORY_FD_NAME),
    };
    Some(Gpu {
        name,
        vendor: Vendor::from_pci(base.vendor_id),
        kind,
        uuid: id.device_uuid,
        render_node,
        vulkan,
        nvidia: None,
    })
}

/// Whether a queue family encodes video, and every codec operation the
/// device's video queues offer.
fn queues(
    instance: &ash::Instance,
    device: vk::PhysicalDevice,
) -> (bool, vk::VideoCodecOperationFlagsKHR) {
    // SAFETY: `device` comes from this instance.
    let count = unsafe { instance.get_physical_device_queue_family_properties2_len(device) };
    let mut video = vec![vk::QueueFamilyVideoPropertiesKHR::default(); count];
    let (flags, ops): (Vec<vk::QueueFlags>, Vec<vk::VideoCodecOperationFlagsKHR>) = {
        let mut families: Vec<vk::QueueFamilyProperties2> = video
            .iter_mut()
            .map(|v| vk::QueueFamilyProperties2::default().push_next(v))
            .collect();
        // SAFETY: `families` has `count` entries, each chained to its own
        // entry of `video`, all alive until the call returns.
        unsafe { instance.get_physical_device_queue_family_properties2(device, &mut families) };
        let flags = families
            .iter()
            .map(|f| f.queue_family_properties.queue_flags)
            .collect();
        drop(families);
        (
            flags,
            video.iter().map(|v| v.video_codec_operations).collect(),
        )
    };
    let encode = flags
        .iter()
        .any(|f| f.contains(vk::QueueFlags::VIDEO_ENCODE_KHR));
    let all = ops
        .iter()
        .fold(vk::VideoCodecOperationFlagsKHR::NONE, |acc, op| acc | *op);
    (encode, all)
}

/// The encode capabilities for `op` at 8-bit 4:2:0: Constrained Baseline
/// for H.264 (what we send), Main for H.265.
fn encode_caps(
    video: &ash::khr::video_queue::Instance,
    device: vk::PhysicalDevice,
    op: vk::VideoCodecOperationFlagsKHR,
) -> Option<VulkanEncodeCaps> {
    let mut h264_profile = vk::VideoEncodeH264ProfileInfoKHR::default()
        .std_profile_idc(vk::native::StdVideoH264ProfileIdc_STD_VIDEO_H264_PROFILE_IDC_BASELINE);
    let mut h265_profile = vk::VideoEncodeH265ProfileInfoKHR::default()
        .std_profile_idc(vk::native::StdVideoH265ProfileIdc_STD_VIDEO_H265_PROFILE_IDC_MAIN);
    let mut profile = vk::VideoProfileInfoKHR::default()
        .video_codec_operation(op)
        .chroma_subsampling(vk::VideoChromaSubsamplingFlagsKHR::TYPE_420)
        .luma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8)
        .chroma_bit_depth(vk::VideoComponentBitDepthFlagsKHR::TYPE_8);
    let h264 = op == vk::VideoCodecOperationFlagsKHR::ENCODE_H264;
    profile = if h264 {
        profile.push_next(&mut h264_profile)
    } else {
        profile.push_next(&mut h265_profile)
    };

    let mut encode = vk::VideoEncodeCapabilitiesKHR::default();
    let mut h264_caps = vk::VideoEncodeH264CapabilitiesKHR::default();
    let mut h265_caps = vk::VideoEncodeH265CapabilitiesKHR::default();
    let mut caps = vk::VideoCapabilitiesKHR::default().push_next(&mut encode);
    caps = if h264 {
        caps.push_next(&mut h264_caps)
    } else {
        caps.push_next(&mut h265_caps)
    };
    // SAFETY: `profile` and `caps` and their chains live until the call
    // returns; the codec-specific structures match `op`, and the device
    // supports `op` on a video queue (checked by the caller).
    let result = unsafe {
        (video.fp().get_physical_device_video_capabilities_khr)(device, &profile, &mut caps)
    };
    if result != vk::Result::SUCCESS {
        return None;
    }
    let (max_coded_extent, max_dpb_slots, max_active_references) = (
        caps.max_coded_extent,
        caps.max_dpb_slots,
        caps.max_active_reference_pictures,
    );
    let max_level = if h264 {
        usize::try_from(h264_caps.max_level_idc)
            .ok()
            .and_then(|i| H264_LEVELS.get(i).copied())
    } else {
        usize::try_from(h265_caps.max_level_idc)
            .ok()
            .and_then(|i| H265_LEVELS.get(i).copied())
    }
    .unwrap_or(0);
    Some(VulkanEncodeCaps {
        max_width: max_coded_extent.width,
        max_height: max_coded_extent.height,
        max_level,
        max_bitrate: encode.max_bitrate,
        cbr: encode
            .rate_control_modes
            .contains(vk::VideoEncodeRateControlModeFlagsKHR::CBR),
        vbr: encode
            .rate_control_modes
            .contains(vk::VideoEncodeRateControlModeFlagsKHR::VBR),
        max_dpb_slots,
        max_active_references,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_tables_match_the_std_enums() {
        assert_eq!(
            H264_LEVELS[vk::native::StdVideoH264LevelIdc_STD_VIDEO_H264_LEVEL_IDC_5_2 as usize],
            52
        );
        assert_eq!(
            H264_LEVELS[vk::native::StdVideoH264LevelIdc_STD_VIDEO_H264_LEVEL_IDC_6_2 as usize],
            62
        );
        assert_eq!(
            H265_LEVELS[vk::native::StdVideoH265LevelIdc_STD_VIDEO_H265_LEVEL_IDC_5_1 as usize],
            153
        );
        assert_eq!(
            H265_LEVELS[vk::native::StdVideoH265LevelIdc_STD_VIDEO_H265_LEVEL_IDC_6_2 as usize],
            186
        );
    }
}
