//! Our own Vulkan device (plan Task 4): one compute queue for conversion
//! and scaling, with the extensions DMA-BUF import needs where the driver
//! has them. Uses `ash` 0.38 (Vulkan 1.3.281 headers).
//!
//! `unsafe`: every Vulkan call is FFI into the loader and driver. Handles
//! passed to a call were made from this device (or its instance) and are
//! alive; structures and their `p_next` chains live until the call
//! returns. Objects are destroyed by the wrapper that made them, after the
//! work using them has finished (the converter waits on a fence before
//! reusing or dropping anything), and the device itself last, when
//! the last `Arc<Shared>` goes.

use std::{
    ffi::{CStr, c_char},
    ptr,
    sync::{Arc, Mutex},
};

use ash::vk;

use super::{GpuError, VkContext};

/// A Vulkan device for conversion, scaling and DMA-BUF import.
#[derive(Clone)]
pub struct GpuDevice {
    pub(crate) shared: Arc<Shared>,
}

impl std::fmt::Debug for GpuDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuDevice")
            .field("name", &self.shared.name)
            .field("dma_buf", &self.shared.dma_buf.is_some())
            .finish()
    }
}

/// What DMA-BUF import needs.
#[cfg_attr(
    not(target_os = "linux"),
    expect(dead_code, reason = "DMA-BUF import is Linux only")
)]
pub(crate) struct DmaBuf {
    pub memory_fd: ash::khr::external_memory_fd::Device,
    /// `VK_EXT_queue_family_foreign` is on: imported images are acquired
    /// from the foreign queue family (otherwise the external one).
    pub foreign: bool,
}

pub(crate) struct Shared {
    _entry: ash::Entry,
    pub instance: ash::Instance,
    #[cfg_attr(
        not(target_os = "linux"),
        expect(dead_code, reason = "read by the Linux-only DMA-BUF import")
    )]
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub memory: vk::PhysicalDeviceMemoryProperties,
    pub name: String,
    pub uuid: [u8; 16],
    pub compute_family: u32,
    pub compute_queue: Mutex<vk::Queue>,
    pub dma_buf: Option<DmaBuf>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // SAFETY: every object made from the device holds an `Arc<Shared>`
        // and has been dropped, so nothing is left but the device and the
        // instance; waiting for idle first covers work still in flight.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Destroys the instance if opening fails half-way.
struct InstanceGuard(Option<ash::Instance>);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Some(instance) = self.0.take() {
            // SAFETY: the instance was made by `open` and nothing made from
            // it survives (the device was not created, or failed).
            unsafe { instance.destroy_instance(None) };
        }
    }
}

impl GpuDevice {
    /// Opens the GPU whose Vulkan device UUID is `uuid` (from
    /// [`crate::probe`]), or the first GPU when `None`.
    ///
    /// # Errors
    /// [`GpuError::NoVulkan`] without a loader or a Vulkan 1.3 device;
    /// [`GpuError::NotSupported`] when the GPU lacks R8/RG8 storage images;
    /// [`GpuError::Vulkan`] when the driver fails.
    pub fn open(uuid: Option<[u8; 16]>) -> Result<Self, GpuError> {
        // SAFETY: loads the system's Vulkan loader; nothing runs code from
        // it before `create_instance`.
        let entry = unsafe { ash::Entry::load() }.map_err(|_| GpuError::NoVulkan("no loader"))?;
        let app = vk::ApplicationInfo::default()
            .application_name(c"twicecat")
            .api_version(vk::API_VERSION_1_3);
        let info = vk::InstanceCreateInfo::default().application_info(&app);
        // SAFETY: `info` and `app` live until the call returns.
        let instance = unsafe { entry.create_instance(&info, None) }.vk("create instance")?;
        let mut guard = InstanceGuard(Some(instance.clone()));

        // SAFETY: the instance is alive.
        let physicals = unsafe { instance.enumerate_physical_devices() }.vk("list devices")?;
        let mut found = None;
        for physical in physicals {
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
            // SAFETY: `physical` comes from this instance; the chain lives
            // until the call returns.
            unsafe { instance.get_physical_device_properties2(physical, &mut props) };
            let base = props.properties;
            if base.api_version < vk::API_VERSION_1_3 {
                continue;
            }
            if uuid.is_none_or(|u| u == id.device_uuid) {
                let name = base
                    .device_name_as_c_str()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                found = Some((physical, name, id.device_uuid));
                break;
            }
        }
        let (physical, name, device_uuid) =
            found.ok_or(GpuError::NoVulkan("no matching Vulkan 1.3 device"))?;

        let compute_family =
            compute_family(&instance, physical).ok_or(GpuError::NoVulkan("no compute queue"))?;

        // SAFETY: `physical` comes from this instance.
        let available = unsafe { instance.enumerate_device_extension_properties(physical) }
            .vk("list device extensions")?;
        let has = |name: &CStr| {
            available
                .iter()
                .any(|e| e.extension_name_as_c_str() == Ok(name))
        };
        let mut extensions: Vec<*const c_char> = Vec::new();
        let dma_buf = [
            vk::KHR_EXTERNAL_MEMORY_FD_NAME,
            vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME,
            vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME,
        ]
        .iter()
        .all(|n| has(n));
        let foreign = dma_buf && has(vk::EXT_QUEUE_FAMILY_FOREIGN_NAME);
        if dma_buf {
            extensions.extend([
                vk::KHR_EXTERNAL_MEMORY_FD_NAME.as_ptr(),
                vk::EXT_EXTERNAL_MEMORY_DMA_BUF_NAME.as_ptr(),
                vk::EXT_IMAGE_DRM_FORMAT_MODIFIER_NAME.as_ptr(),
            ]);
            if foreign {
                extensions.push(vk::EXT_QUEUE_FAMILY_FOREIGN_NAME.as_ptr());
            }
        }

        // SAFETY: `physical` comes from this instance.
        let supported = unsafe { instance.get_physical_device_features(physical) };
        if supported.shader_storage_image_extended_formats != vk::TRUE {
            return Err(GpuError::NotSupported("R8/RG8 storage images"));
        }
        let base_features =
            vk::PhysicalDeviceFeatures::default().shader_storage_image_extended_formats(true);
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default().synchronization2(true);
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .features(base_features)
            .push_next(&mut vulkan13);

        let priority = [1.0_f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(compute_family)
            .queue_priorities(&priority)];
        let info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extensions)
            .push_next(&mut features);
        // SAFETY: every extension name is one the device lists, the
        // features were checked (synchronization2 is core in 1.3), and the
        // structures live until the call returns.
        let device =
            unsafe { instance.create_device(physical, &info, None) }.vk("create device")?;
        guard.0 = None;

        // SAFETY: the device is alive and one queue of the family was
        // requested.
        let compute_queue = unsafe { device.get_device_queue(compute_family, 0) };
        let dma_buf = dma_buf.then(|| DmaBuf {
            memory_fd: ash::khr::external_memory_fd::Device::new(&instance, &device),
            foreign,
        });
        // SAFETY: `physical` comes from this instance.
        let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
        Ok(Self {
            shared: Arc::new(Shared {
                _entry: entry,
                instance,
                physical,
                device,
                memory,
                name,
                uuid: device_uuid,
                compute_family,
                compute_queue: Mutex::new(compute_queue),
                dma_buf,
            }),
        })
    }

    /// The GPU's name as the driver gives it.
    pub fn name(&self) -> &str {
        &self.shared.name
    }

    /// The Vulkan device UUID (as [`crate::probe::Gpu::uuid`]).
    pub fn uuid(&self) -> [u8; 16] {
        self.shared.uuid
    }

    /// Whether DMA-BUF frames can be imported.
    pub fn can_import_dma_buf(&self) -> bool {
        self.shared.dma_buf.is_some()
    }

    /// The DRM format modifiers of `fourcc` (a DRM fourcc such as
    /// [`crate::gpu::XRGB8888`]) this device imports; empty when it imports
    /// none.
    #[cfg(target_os = "linux")]
    pub fn dma_buf_modifiers(&self, fourcc: u32) -> Vec<u64> {
        super::dmabuf::importable_modifiers(&self.shared, fourcc)
    }

    /// Opens the GPU a desktop most likely renders on, for frames the
    /// compositor shares as DMA-BUFs: the integrated GPU of a laptop with
    /// two (compositors render there unless told otherwise), else the first.
    /// A buffer from the other GPU fails to import and the capture falls
    /// back to shared memory.
    ///
    /// # Errors
    /// As [`GpuDevice::open`].
    pub fn open_for_capture() -> Result<Self, GpuError> {
        let gpus = crate::probe::vulkan_gpus();
        let integrated = gpus
            .iter()
            .find(|g| g.kind == crate::probe::GpuKind::Integrated)
            .map(|g| g.uuid);
        Self::open(integrated)
    }
}

fn compute_family(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<u32> {
    // SAFETY: `physical` comes from this instance.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    // A compute queue without graphics first (async compute), then any.
    let pick = |want_graphics: bool| {
        families.iter().position(|f| {
            f.queue_flags.contains(vk::QueueFlags::COMPUTE)
                && f.queue_flags.contains(vk::QueueFlags::GRAPHICS) == want_graphics
        })
    };
    pick(false)
        .or_else(|| pick(true))
        .and_then(|i| u32::try_from(i).ok())
}

impl Shared {
    /// A memory type in `bits` with `required`, preferring one that also
    /// has `preferred`.
    pub fn memory_type(
        &self,
        bits: u32,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Option<u32> {
        let types = &self.memory.memory_types[..self.memory.memory_type_count as usize];
        let find = |want: vk::MemoryPropertyFlags| {
            types
                .iter()
                .enumerate()
                .position(|(i, t)| bits & (1 << i) != 0 && t.property_flags.contains(want))
                .and_then(|i| u32::try_from(i).ok())
        };
        find(required | preferred).or_else(|| find(required))
    }

    /// Allocates memory for `requirements`.
    pub fn allocate(
        &self,
        requirements: vk::MemoryRequirements,
        required: vk::MemoryPropertyFlags,
        preferred: vk::MemoryPropertyFlags,
    ) -> Result<(vk::DeviceMemory, vk::MemoryPropertyFlags), GpuError> {
        let index = self
            .memory_type(requirements.memory_type_bits, required, preferred)
            .ok_or(GpuError::NotSupported("no suitable memory type"))?;
        let info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(index);
        // SAFETY: the device is alive; the type index is one it lists.
        let memory = unsafe { self.device.allocate_memory(&info, None) }.vk("allocate memory")?;
        Ok((
            memory,
            self.memory.memory_types[index as usize].property_flags,
        ))
    }

    /// Submits `cmd` to `queue` and signals `fence`.
    pub fn submit(
        &self,
        queue: &Mutex<vk::Queue>,
        cmd: vk::CommandBuffer,
        fence: vk::Fence,
    ) -> Result<(), GpuError> {
        let cmds = [vk::CommandBufferSubmitInfo::default().command_buffer(cmd)];
        let submit = vk::SubmitInfo2::default().command_buffer_infos(&cmds);
        let queue = queue.lock().map_err(|_| GpuError::Poisoned)?;
        // SAFETY: the queue is held for the call (external sync); the
        // command buffer is recorded and not pending; the fence is from this
        // device and unsignalled.
        unsafe { self.device.queue_submit2(*queue, &[submit], fence) }.vk("submit")
    }
}

/// A command pool with one command buffer and a fence, for one queue
/// family.
pub(crate) struct Commands {
    shared: Arc<Shared>,
    pool: vk::CommandPool,
    pub cmd: vk::CommandBuffer,
    fence: vk::Fence,
    /// Submitted and not waited for yet.
    pending: bool,
}

impl Commands {
    pub fn new(shared: &Arc<Shared>, family: u32) -> Result<Self, GpuError> {
        let device = &shared.device;
        let info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: the device is alive; `family` is one of its queue
        // families.
        let pool = unsafe { device.create_command_pool(&info, None) }.vk("create command pool")?;
        let alloc = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool was just made.
        let cmd = match unsafe { device.allocate_command_buffers(&alloc) } {
            Ok(cmds) => cmds[0],
            Err(e) => {
                // SAFETY: the pool has no buffers in use.
                unsafe { device.destroy_command_pool(pool, None) };
                return Err(GpuError::vulkan("allocate command buffer", e));
            }
        };
        let fence_info = vk::FenceCreateInfo::default();
        // SAFETY: the device is alive.
        let fence = match unsafe { device.create_fence(&fence_info, None) } {
            Ok(f) => f,
            Err(e) => {
                // SAFETY: the pool's buffer was never submitted.
                unsafe { device.destroy_command_pool(pool, None) };
                return Err(GpuError::vulkan("create fence", e));
            }
        };
        Ok(Self {
            shared: Arc::clone(shared),
            pool,
            cmd,
            fence,
            pending: false,
        })
    }

    /// Resets and begins the command buffer for one submission.
    pub fn begin(&mut self) -> Result<vk::CommandBuffer, GpuError> {
        if self.pending {
            self.wait()?;
        }
        let device = &self.shared.device;
        let info = vk::CommandBufferBeginInfo::default()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: the buffer is not pending (waited for above, or never
        // submitted).
        unsafe {
            device
                .reset_command_buffer(self.cmd, vk::CommandBufferResetFlags::empty())
                .vk("reset command buffer")?;
            device
                .begin_command_buffer(self.cmd, &info)
                .vk("begin command buffer")?;
        }
        Ok(self.cmd)
    }

    /// Ends the command buffer and submits it to `queue`.
    pub fn submit(&mut self, queue: &Mutex<vk::Queue>) -> Result<(), GpuError> {
        // SAFETY: the buffer is recording (after `begin`).
        unsafe { self.shared.device.end_command_buffer(self.cmd) }.vk("end command buffer")?;
        self.shared.submit(queue, self.cmd, self.fence)?;
        self.pending = true;
        Ok(())
    }

    /// Waits for the last submission (up to 2 s) and resets the fence.
    pub fn wait(&mut self) -> Result<(), GpuError> {
        if !self.pending {
            return Ok(());
        }
        let device = &self.shared.device;
        // SAFETY: the fence is from this device and was submitted.
        unsafe {
            device
                .wait_for_fences(&[self.fence], true, 2_000_000_000)
                .vk("wait for the GPU")?;
            self.pending = false;
            device.reset_fences(&[self.fence]).vk("reset fence")
        }
    }
}

impl Drop for Commands {
    fn drop(&mut self) {
        let device = &self.shared.device;
        // SAFETY: a pending submission is waited for (without a time
        // limit) so the buffer and fence are idle before they go.
        unsafe {
            if self.pending {
                let _ = device.wait_for_fences(&[self.fence], true, u64::MAX);
            }
            device.destroy_fence(self.fence, None);
            device.destroy_command_pool(self.pool, None);
        }
    }
}

/// How the CPU uses a mapped buffer: the memory type follows (on Intel's
/// iGPU the cached type is not coherent, and reading the coherent one,
/// write-combined, made a 4K readback take 40 ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    /// The CPU writes, the GPU reads (coherent, write-combined is fine).
    Upload,
    /// The GPU writes, the CPU reads (cached; invalidated before reads).
    Readback,
}

/// A buffer and its memory, mapped when host-visible.
pub(crate) struct Buffer {
    shared: Arc<Shared>,
    pub buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    pub size: u64,
    mapped: *mut u8,
    coherent: bool,
}

// SAFETY: the mapped pointer is only used through `&mut self` (writes) or
// `&self` (reads after the GPU finished, behind the owner's fence), and the
// Vulkan handles may be used from any thread with external sync, which the
// owners provide.
unsafe impl Send for Buffer {}

impl Buffer {
    /// A buffer of `size` bytes, mapped when `host` says how the CPU uses
    /// it.
    pub fn new(
        shared: &Arc<Shared>,
        size: u64,
        usage: vk::BufferUsageFlags,
        host: Option<Host>,
    ) -> Result<Self, GpuError> {
        let device = &shared.device;
        let info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the device is alive; `info` lives until the call returns.
        let buffer = unsafe { device.create_buffer(&info, None) }.vk("create buffer")?;
        // SAFETY: the buffer was just made.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
        let (required, preferred) = match host {
            Some(Host::Upload) => (
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_COHERENT,
            ),
            Some(Host::Readback) => (
                vk::MemoryPropertyFlags::HOST_VISIBLE,
                vk::MemoryPropertyFlags::HOST_CACHED,
            ),
            None => (
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                vk::MemoryPropertyFlags::empty(),
            ),
        };
        let (memory, flags) = match shared.allocate(requirements, required, preferred) {
            Ok(m) => m,
            Err(e) => {
                // SAFETY: the buffer has no memory and was never used.
                unsafe { device.destroy_buffer(buffer, None) };
                return Err(e);
            }
        };
        let mut out = Self {
            shared: Arc::clone(shared),
            buffer,
            memory,
            size,
            mapped: ptr::null_mut(),
            coherent: flags.contains(vk::MemoryPropertyFlags::HOST_COHERENT),
        };
        // SAFETY: the memory was allocated for this buffer's requirements.
        unsafe { device.bind_buffer_memory(buffer, memory, 0) }.vk("bind buffer memory")?;
        if host.is_some() {
            // SAFETY: host-visible memory, mapped once for its lifetime.
            let p = unsafe {
                device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            }
            .vk("map memory")?;
            out.mapped = p.cast();
        }
        Ok(out)
    }

    /// The mapped bytes, for writing before a submission.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        assert!(!self.mapped.is_null(), "buffer is not host-visible");
        let len = usize::try_from(self.size).unwrap_or(usize::MAX);
        // SAFETY: `mapped` covers the whole buffer (WHOLE_SIZE), stays
        // mapped while `self` lives, and `&mut self` makes this the only
        // view.
        unsafe { std::slice::from_raw_parts_mut(self.mapped, len) }
    }

    /// Makes host writes visible to the device (non-coherent memory).
    pub fn flush(&self) -> Result<(), GpuError> {
        if self.coherent {
            return Ok(());
        }
        let range = vk::MappedMemoryRange::default()
            .memory(self.memory)
            .size(vk::WHOLE_SIZE);
        // SAFETY: the memory is mapped.
        unsafe { self.shared.device.flush_mapped_memory_ranges(&[range]) }.vk("flush memory")
    }

    /// The mapped bytes, for reading after the GPU wrote them and the
    /// owner waited on its fence.
    pub fn bytes(&self) -> Result<&[u8], GpuError> {
        assert!(!self.mapped.is_null(), "buffer is not host-visible");
        if !self.coherent {
            let range = vk::MappedMemoryRange::default()
                .memory(self.memory)
                .size(vk::WHOLE_SIZE);
            // SAFETY: the memory is mapped.
            unsafe { self.shared.device.invalidate_mapped_memory_ranges(&[range]) }
                .vk("invalidate memory")?;
        }
        let len = usize::try_from(self.size).unwrap_or(usize::MAX);
        // SAFETY: as in `bytes_mut`; the GPU is done with the buffer
        // (caller's fence), so nothing writes while this borrow lives.
        Ok(unsafe { std::slice::from_raw_parts(self.mapped, len) })
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        // SAFETY: the owner waited for the GPU to finish with the buffer.
        unsafe {
            self.shared.device.destroy_buffer(self.buffer, None);
            self.shared.device.free_memory(self.memory, None);
        }
    }
}

/// An image, its memory and its views.
pub(crate) struct Image {
    shared: Arc<Shared>,
    pub image: vk::Image,
    memory: vk::DeviceMemory,
    pub views: Vec<vk::ImageView>,
    pub extent: vk::Extent2D,
}

impl Image {
    /// An image in device memory with one view per entry of `views`
    /// (format, aspect, array layer).
    pub fn new(
        shared: &Arc<Shared>,
        info: &vk::ImageCreateInfo<'_>,
        views: &[(vk::Format, vk::ImageAspectFlags, u32)],
    ) -> Result<Self, GpuError> {
        let device = &shared.device;
        // SAFETY: the device is alive; `info` and its chain live until the
        // call returns.
        let image = unsafe { device.create_image(info, None) }.vk("create image")?;
        // SAFETY: the image was just made.
        let requirements = unsafe { device.get_image_memory_requirements(image) };
        let memory = match shared.allocate(
            requirements,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
            vk::MemoryPropertyFlags::empty(),
        ) {
            Ok((m, _)) => m,
            Err(e) => {
                // SAFETY: the image has no memory and was never used.
                unsafe { device.destroy_image(image, None) };
                return Err(e);
            }
        };
        let mut out = Self {
            shared: Arc::clone(shared),
            image,
            memory,
            views: Vec::new(),
            extent: vk::Extent2D {
                width: info.extent.width,
                height: info.extent.height,
            },
        };
        // SAFETY: the memory was allocated for this image's requirements.
        unsafe { device.bind_image_memory(image, memory, 0) }.vk("bind image memory")?;
        for &(format, aspect, layer) in views {
            out.views.push(view(shared, image, format, aspect, layer)?);
        }
        Ok(out)
    }

    /// An image that owns memory made elsewhere (DMA-BUF import).
    #[cfg(target_os = "linux")]
    pub fn from_parts(
        shared: &Arc<Shared>,
        image: vk::Image,
        memory: vk::DeviceMemory,
        extent: vk::Extent2D,
    ) -> Self {
        Self {
            shared: Arc::clone(shared),
            image,
            memory,
            views: Vec::new(),
            extent,
        }
    }
}

/// A 2D view of one array layer of `image`.
pub(crate) fn view(
    shared: &Shared,
    image: vk::Image,
    format: vk::Format,
    aspect: vk::ImageAspectFlags,
    layer: u32,
) -> Result<vk::ImageView, GpuError> {
    let info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .level_count(1)
                .base_array_layer(layer)
                .layer_count(1),
        );
    // SAFETY: `image` is alive and was made from this device with a
    // format the view's is compatible with.
    unsafe { shared.device.create_image_view(&info, None) }.vk("create image view")
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: the owner waited for the GPU to finish with the image.
        unsafe {
            for v in self.views.drain(..) {
                self.shared.device.destroy_image_view(v, None);
            }
            self.shared.device.destroy_image(self.image, None);
            self.shared.device.free_memory(self.memory, None);
        }
    }
}

/// An image barrier for `cmd` (synchronization2).
#[allow(clippy::too_many_arguments)]
pub(crate) fn barrier(
    shared: &Shared,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    layers: u32,
    (old, new): (vk::ImageLayout, vk::ImageLayout),
    (src_stage, src_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    (dst_stage, dst_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
) {
    barrier_families(
        shared,
        cmd,
        image,
        aspect,
        layers,
        (old, new),
        (src_stage, src_access),
        (dst_stage, dst_access),
        (vk::QUEUE_FAMILY_IGNORED, vk::QUEUE_FAMILY_IGNORED),
    );
}

/// As [`barrier`], with a queue family transfer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn barrier_families(
    shared: &Shared,
    cmd: vk::CommandBuffer,
    image: vk::Image,
    aspect: vk::ImageAspectFlags,
    layers: u32,
    (old, new): (vk::ImageLayout, vk::ImageLayout),
    (src_stage, src_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    (dst_stage, dst_access): (vk::PipelineStageFlags2, vk::AccessFlags2),
    (src_family, dst_family): (u32, u32),
) {
    let barriers = [vk::ImageMemoryBarrier2::default()
        .image(image)
        .old_layout(old)
        .new_layout(new)
        .src_stage_mask(src_stage)
        .src_access_mask(src_access)
        .dst_stage_mask(dst_stage)
        .dst_access_mask(dst_access)
        .src_queue_family_index(src_family)
        .dst_queue_family_index(dst_family)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .level_count(1)
                .layer_count(layers),
        )];
    let info = vk::DependencyInfo::default().image_memory_barriers(&barriers);
    // SAFETY: `cmd` is recording; `image` is alive.
    unsafe { shared.device.cmd_pipeline_barrier2(cmd, &info) };
}
