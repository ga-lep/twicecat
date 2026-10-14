//! Colour conversion and scaling on the GPU (plan Task 4): a frame (CPU
//! BGRA or NV12, or a DMA-BUF) goes to NV12 at up to [`MAX_OUTPUTS`]
//! sizes in one pass per size. Each size gets an R8 luma and an RG8
//! chroma storage image, read back into one NV12 frame in CPU memory.
//!
//! `unsafe`: Vulkan FFI, as in `device.rs`. Descriptor sets, staging
//! memory and imported images are reused or dropped only after the GPU
//! finished the last submission that used them: [`Converter::convert`]
//! waits on its fence before recording again.

use std::sync::Arc;

use ash::vk;

#[cfg(target_os = "linux")]
use super::device::barrier_families;
use super::{
    GpuError, VkContext,
    device::{Buffer, Commands, GpuDevice, Host, Image, Shared, barrier},
};
use crate::{ColorSpace, CpuFrame, Frame, Matrix, PixelFormat, Plane, Range, convert};

/// How many output sizes one conversion makes (the ladder's layers).
pub const MAX_OUTPUTS: usize = crate::ladder::MAX_LAYERS;

const BGRA_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bgra_to_nv12.spv"));
const NV12_SPV: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/nv12_scale.spv"));

/// Size of the shaders' push constants (`Params`).
const PARAMS_SIZE: u32 = 32;

/// Shader workgroup: 8x8 invocations, each one chroma sample.
const GROUP: u32 = 8;

/// The NV12 planes of one output size, left in `TRANSFER_SRC_OPTIMAL`.
pub(crate) struct Target {
    pub luma: Image,
    pub chroma: Image,
}

/// The source as the shaders read it.
enum Source {
    /// BGRA/RGBA in one image (uploaded or imported).
    Rgb(vk::ImageView),
    /// NV12 as R8 + RG8 images.
    Nv12(vk::ImageView, vk::ImageView),
}

/// Uploaded CPU frames: staging buffer and the images it fills.
struct Upload {
    format: PixelFormat,
    width: u32,
    height: u32,
    staging: Buffer,
    images: Vec<Image>,
}

/// GPU colour conversion and scaling to NV12.
pub struct Converter {
    shared: Arc<Shared>,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    bgra: vk::Pipeline,
    nv12: vk::Pipeline,
    pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    upload: Option<Upload>,
    #[cfg(target_os = "linux")]
    imported: Option<Image>,
    pub(crate) targets: Vec<Target>,
    /// For [`Converter::convert`] only.
    commands: Option<Commands>,
    readback: Option<Buffer>,
    /// The colour space of the last outputs.
    pub(crate) out_color: ColorSpace,
}

impl std::fmt::Debug for Converter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Converter")
            .field("outputs", &self.targets.len())
            .finish()
    }
}

impl Converter {
    /// Builds the pipelines on `device`.
    ///
    /// # Errors
    /// [`GpuError::Vulkan`] when the driver refuses a pipeline.
    pub fn new(device: &GpuDevice) -> Result<Self, GpuError> {
        let shared = Arc::clone(&device.shared);
        let d = &shared.device;
        let bindings: Vec<_> = [
            vk::DescriptorType::SAMPLED_IMAGE,
            vk::DescriptorType::SAMPLED_IMAGE,
            vk::DescriptorType::STORAGE_IMAGE,
            vk::DescriptorType::STORAGE_IMAGE,
        ]
        .iter()
        .enumerate()
        .map(|(i, &ty)| {
            vk::DescriptorSetLayoutBinding::default()
                .binding(i as u32)
                .descriptor_type(ty)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
        })
        .collect();
        let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: the device is alive; `bindings` lives until the call
        // returns.
        let set_layout = unsafe { d.create_descriptor_set_layout(&info, None) }
            .vk("create descriptor set layout")?;
        let mut out = Self {
            shared: Arc::clone(&shared),
            set_layout,
            layout: vk::PipelineLayout::null(),
            bgra: vk::Pipeline::null(),
            nv12: vk::Pipeline::null(),
            pool: vk::DescriptorPool::null(),
            sets: Vec::new(),
            upload: None,
            #[cfg(target_os = "linux")]
            imported: None,
            targets: Vec::new(),
            commands: None,
            readback: None,
            out_color: ColorSpace::default(),
        };
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .size(PARAMS_SIZE)];
        let layouts = [set_layout];
        let info = vk::PipelineLayoutCreateInfo::default()
            .set_layouts(&layouts)
            .push_constant_ranges(&ranges);
        // SAFETY: the set layout is alive; arrays live until the call
        // returns.
        out.layout =
            unsafe { d.create_pipeline_layout(&info, None) }.vk("create pipeline layout")?;
        out.bgra = pipeline(&shared, out.layout, BGRA_SPV)?;
        out.nv12 = pipeline(&shared, out.layout, NV12_SPV)?;
        let sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(2 * MAX_OUTPUTS as u32),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(2 * MAX_OUTPUTS as u32),
        ];
        let info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(MAX_OUTPUTS as u32)
            .pool_sizes(&sizes);
        // SAFETY: the device is alive.
        out.pool = unsafe { d.create_descriptor_pool(&info, None) }.vk("create descriptor pool")?;
        let layouts = [set_layout; MAX_OUTPUTS];
        let info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(out.pool)
            .set_layouts(&layouts);
        // SAFETY: the pool has room for MAX_OUTPUTS sets of this layout.
        out.sets = unsafe { d.allocate_descriptor_sets(&info) }.vk("allocate descriptor sets")?;
        Ok(out)
    }

    /// Converts `frame` to NV12 at each of `sizes` (even, at most
    /// [`MAX_OUTPUTS`]) and reads the results back. BGRA input is
    /// converted to `color`; NV12 input keeps its own colour space.
    ///
    /// # Errors
    /// [`GpuError::Invalid`] for a size or frame the converter cannot
    /// take, [`GpuError::Vulkan`] when the driver fails.
    pub fn convert(
        &mut self,
        frame: &Frame,
        sizes: &[(u32, u32)],
        color: ColorSpace,
    ) -> Result<Vec<CpuFrame>, GpuError> {
        let mut commands = match self.commands.take() {
            Some(c) => c,
            None => Commands::new(&self.shared, self.shared.compute_family)?,
        };
        let result = self.convert_with(&mut commands, frame, sizes, color);
        self.commands = Some(commands);
        result
    }

    fn convert_with(
        &mut self,
        commands: &mut Commands,
        frame: &Frame,
        sizes: &[(u32, u32)],
        color: ColorSpace,
    ) -> Result<Vec<CpuFrame>, GpuError> {
        commands.wait()?;
        let total: u64 = sizes
            .iter()
            .map(|&(w, h)| u64::from(w) * u64::from(h) * 3 / 2)
            .sum();
        if self.readback.as_ref().is_none_or(|b| b.size < total) {
            self.readback = None;
            self.readback = Some(Buffer::new(
                &self.shared,
                total,
                vk::BufferUsageFlags::TRANSFER_DST,
                Some(Host::Readback),
            )?);
        }
        let cmd = commands.begin()?;
        self.record(cmd, frame, sizes, color)?;
        let readback = self.readback.as_ref().map(|b| b.buffer).unwrap_or_default();
        let mut offset = 0;
        for (target, &(w, h)) in self.targets.iter().zip(sizes) {
            for (image, (pw, ph)) in [(&target.luma, (w, h)), (&target.chroma, (w / 2, h / 2))] {
                let region = vk::BufferImageCopy::default()
                    .buffer_offset(offset)
                    .image_subresource(color_layer())
                    .image_extent(vk::Extent3D {
                        width: pw,
                        height: ph,
                        depth: 1,
                    });
                // SAFETY: `cmd` is recording; the image is in
                // TRANSFER_SRC_OPTIMAL (end of `record`) and the buffer
                // holds `total` bytes.
                unsafe {
                    self.shared.device.cmd_copy_image_to_buffer(
                        cmd,
                        image.image,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                        readback,
                        &[region],
                    );
                }
                let bpp = if ph == h { 1 } else { 2 };
                offset += u64::from(pw) * u64::from(ph) * bpp;
            }
        }
        let queue = &self.shared.compute_queue;
        commands.submit(queue)?;
        commands.wait()?;

        let bytes = self.readback.as_ref().map(Buffer::bytes).transpose()?;
        let bytes = bytes.unwrap_or_default();
        let mut out = Vec::with_capacity(sizes.len());
        let mut at = 0usize;
        for &(w, h) in sizes {
            let luma = (w * h) as usize;
            let y: Arc<[u8]> = bytes[at..at + luma].into();
            let uv: Arc<[u8]> = bytes[at + luma..at + luma * 3 / 2].into();
            at += luma * 3 / 2;
            out.push(
                CpuFrame::new(
                    PixelFormat::Nv12,
                    w,
                    h,
                    vec![
                        Plane {
                            data: y,
                            stride: w as usize,
                        },
                        Plane {
                            data: uv,
                            stride: w as usize,
                        },
                    ],
                    self.out_color,
                    frame.timestamp(),
                )
                .map_err(|_| GpuError::Invalid("output size"))?,
            );
        }
        Ok(out)
    }

    /// Records the upload or import of `frame` and the conversion to each
    /// of `sizes` into `cmd`. Afterwards `self.targets[i]` holds size `i`
    /// in `TRANSFER_SRC_OPTIMAL`. The caller has waited for the previous
    /// submission that used this converter.
    fn record(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: &Frame,
        sizes: &[(u32, u32)],
        color: ColorSpace,
    ) -> Result<(), GpuError> {
        if sizes.is_empty() || sizes.len() > MAX_OUTPUTS {
            return Err(GpuError::Invalid("one to three output sizes"));
        }
        if sizes
            .iter()
            .any(|&(w, h)| w == 0 || h == 0 || !w.is_multiple_of(2) || !h.is_multiple_of(2))
        {
            return Err(GpuError::Invalid("output sizes must be even"));
        }
        self.ensure_targets(sizes)?;
        let (source, src_color) = match frame {
            Frame::Cpu(f) => self.upload(cmd, f, color)?,
            #[cfg(target_os = "linux")]
            Frame::DmaBuf(f) => {
                let image = super::dmabuf::import(&self.shared, f)?;
                let view = image.views[0];
                // Acquired from the producer's side; GENERAL keeps the
                // contents (UNDEFINED would allow the driver to drop them).
                let src_family = match &self.shared.dma_buf {
                    Some(d) if d.foreign => vk::QUEUE_FAMILY_FOREIGN_EXT,
                    _ => vk::QUEUE_FAMILY_EXTERNAL,
                };
                barrier_families(
                    &self.shared,
                    cmd,
                    image.image,
                    vk::ImageAspectFlags::COLOR,
                    1,
                    (
                        vk::ImageLayout::GENERAL,
                        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    ),
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_SAMPLED_READ,
                    ),
                    (src_family, self.shared.compute_family),
                );
                self.imported = Some(image);
                (Source::Rgb(view), color)
            }
        };
        self.out_color = src_color;
        let d = &self.shared.device;
        let (pipeline, src_size) = match source {
            Source::Rgb(_) => (self.bgra, (frame.width(), frame.height())),
            Source::Nv12(..) => (self.nv12, (frame.width(), frame.height())),
        };
        let (a, b) = match source {
            Source::Rgb(v) => (v, v),
            Source::Nv12(y, uv) => (y, uv),
        };
        for (i, target) in self.targets.iter().enumerate() {
            let images = [
                [vk::DescriptorImageInfo::default()
                    .image_view(a)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)],
                [vk::DescriptorImageInfo::default()
                    .image_view(b)
                    .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)],
                [vk::DescriptorImageInfo::default()
                    .image_view(target.luma.views[0])
                    .image_layout(vk::ImageLayout::GENERAL)],
                [vk::DescriptorImageInfo::default()
                    .image_view(target.chroma.views[0])
                    .image_layout(vk::ImageLayout::GENERAL)],
            ];
            let writes: Vec<_> = images
                .iter()
                .enumerate()
                .map(|(binding, info)| {
                    vk::WriteDescriptorSet::default()
                        .dst_set(self.sets[i])
                        .dst_binding(binding as u32)
                        .descriptor_type(if binding < 2 {
                            vk::DescriptorType::SAMPLED_IMAGE
                        } else {
                            vk::DescriptorType::STORAGE_IMAGE
                        })
                        .image_info(info)
                })
                .collect();
            // SAFETY: the set is not in use by the GPU (the caller waited)
            // and every view is alive.
            unsafe { d.update_descriptor_sets(&writes, &[]) };
            for image in [&target.luma, &target.chroma] {
                barrier(
                    &self.shared,
                    cmd,
                    image.image,
                    vk::ImageAspectFlags::COLOR,
                    1,
                    (vk::ImageLayout::UNDEFINED, vk::ImageLayout::GENERAL),
                    (vk::PipelineStageFlags2::NONE, vk::AccessFlags2::NONE),
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                );
            }
        }
        // SAFETY: `cmd` is recording; pipeline, layout and sets are alive
        // and every set was written above.
        unsafe { d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline) };
        for (i, target) in self.targets.iter().enumerate() {
            let size = (target.luma.extent.width, target.luma.extent.height);
            let params = Params::new(src_size, size, src_color);
            // SAFETY: as above; the push constant range covers `Params`.
            unsafe {
                d.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::COMPUTE,
                    self.layout,
                    0,
                    &[self.sets[i]],
                    &[],
                );
                d.cmd_push_constants(
                    cmd,
                    self.layout,
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    &params.bytes(),
                );
                d.cmd_dispatch(
                    cmd,
                    (size.0 / 2).div_ceil(GROUP),
                    (size.1 / 2).div_ceil(GROUP),
                    1,
                );
            }
        }
        for target in &self.targets {
            for image in [&target.luma, &target.chroma] {
                barrier(
                    &self.shared,
                    cmd,
                    image.image,
                    vk::ImageAspectFlags::COLOR,
                    1,
                    (
                        vk::ImageLayout::GENERAL,
                        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    ),
                    (
                        vk::PipelineStageFlags2::COMPUTE_SHADER,
                        vk::AccessFlags2::SHADER_STORAGE_WRITE,
                    ),
                    (
                        vk::PipelineStageFlags2::COPY,
                        vk::AccessFlags2::TRANSFER_READ,
                    ),
                );
            }
        }
        Ok(())
    }

    fn ensure_targets(&mut self, sizes: &[(u32, u32)]) -> Result<(), GpuError> {
        let same = self.targets.len() == sizes.len()
            && self
                .targets
                .iter()
                .zip(sizes)
                .all(|(t, &(w, h))| t.luma.extent.width == w && t.luma.extent.height == h);
        if same {
            return Ok(());
        }
        self.targets.clear();
        for &(w, h) in sizes {
            let luma = plane_image(&self.shared, vk::Format::R8_UNORM, w, h, true)?;
            let chroma = plane_image(&self.shared, vk::Format::R8G8_UNORM, w / 2, h / 2, true)?;
            self.targets.push(Target { luma, chroma });
        }
        Ok(())
    }

    /// Copies a CPU frame into staging memory and records its upload.
    fn upload(
        &mut self,
        cmd: vk::CommandBuffer,
        frame: &CpuFrame,
        color: ColorSpace,
    ) -> Result<(Source, ColorSpace), GpuError> {
        let converted;
        let frame = match frame.format() {
            PixelFormat::Bgra | PixelFormat::Nv12 => frame,
            // YUYV and I420 go through the CPU converter first.
            PixelFormat::I420 | PixelFormat::Yuyv => {
                converted = convert::to_nv12(frame, frame.color())
                    .map_err(|_| GpuError::Invalid("frame conversion"))?;
                &converted
            }
        };
        let (format, w, h) = (frame.format(), frame.width(), frame.height());
        if format == PixelFormat::Nv12 && (!w.is_multiple_of(2) || !h.is_multiple_of(2)) {
            return Err(GpuError::Invalid("NV12 sizes must be even"));
        }
        let reuse = self
            .upload
            .as_ref()
            .is_some_and(|u| u.format == format && u.width == w && u.height == h);
        if !reuse {
            self.upload = None;
            let (size, images) = match format {
                PixelFormat::Bgra => (
                    u64::from(w) * u64::from(h) * 4,
                    vec![plane_image(
                        &self.shared,
                        vk::Format::B8G8R8A8_UNORM,
                        w,
                        h,
                        false,
                    )?],
                ),
                _ => (
                    u64::from(w) * u64::from(h) * 3 / 2,
                    vec![
                        plane_image(&self.shared, vk::Format::R8_UNORM, w, h, false)?,
                        plane_image(&self.shared, vk::Format::R8G8_UNORM, w / 2, h / 2, false)?,
                    ],
                ),
            };
            let staging = Buffer::new(
                &self.shared,
                size,
                vk::BufferUsageFlags::TRANSFER_SRC,
                Some(Host::Upload),
            )?;
            self.upload = Some(Upload {
                format,
                width: w,
                height: h,
                staging,
                images,
            });
        }
        let Some(upload) = self.upload.as_mut() else {
            return Err(GpuError::Invalid("upload"));
        };
        // Planes are packed tightly in staging memory.
        let shapes: Vec<(usize, usize)> = (0..format.plane_count())
            .map(|i| format.plane_shape(i, w, h))
            .collect();
        {
            let dst = upload.staging.bytes_mut();
            let mut at = 0;
            for (i, &(row, rows)) in shapes.iter().enumerate() {
                for y in 0..rows {
                    dst[at..at + row].copy_from_slice(frame.row(i, y));
                    at += row;
                }
            }
        }
        upload.staging.flush()?;
        let mut offset = 0u64;
        for (image, &(row, rows)) in upload.images.iter().zip(&shapes) {
            barrier(
                &self.shared,
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
                .buffer_offset(offset)
                .image_subresource(color_layer())
                .image_extent(vk::Extent3D {
                    width: image.extent.width,
                    height: image.extent.height,
                    depth: 1,
                });
            // SAFETY: `cmd` is recording; the staging buffer holds the
            // packed planes and the image is in TRANSFER_DST_OPTIMAL.
            unsafe {
                self.shared.device.cmd_copy_buffer_to_image(
                    cmd,
                    upload.staging.buffer,
                    image.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            offset += (row * rows) as u64;
            barrier(
                &self.shared,
                cmd,
                image.image,
                vk::ImageAspectFlags::COLOR,
                1,
                (
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                ),
                (
                    vk::PipelineStageFlags2::COPY,
                    vk::AccessFlags2::TRANSFER_WRITE,
                ),
                (
                    vk::PipelineStageFlags2::COMPUTE_SHADER,
                    vk::AccessFlags2::SHADER_SAMPLED_READ,
                ),
            );
        }
        Ok(match format {
            PixelFormat::Bgra => (Source::Rgb(upload.images[0].views[0]), color),
            _ => (
                Source::Nv12(upload.images[0].views[0], upload.images[1].views[0]),
                frame.color(),
            ),
        })
    }
}

impl Drop for Converter {
    fn drop(&mut self) {
        // Pending standalone work first (its Drop waits), then images and
        // buffers, then the pipeline objects.
        self.commands = None;
        self.targets.clear();
        self.upload = None;
        #[cfg(target_os = "linux")]
        {
            self.imported = None;
        }
        self.readback = None;
        let d = &self.shared.device;
        // SAFETY: `convert` waited for every submission that used these
        // objects (and dropping `commands` waits for a pending one); null handles are ignored by Vulkan.
        unsafe {
            d.destroy_descriptor_pool(self.pool, None);
            d.destroy_pipeline(self.bgra, None);
            d.destroy_pipeline(self.nv12, None);
            d.destroy_pipeline_layout(self.layout, None);
            d.destroy_descriptor_set_layout(self.set_layout, None);
        }
    }
}

fn color_layer() -> vk::ImageSubresourceLayers {
    vk::ImageSubresourceLayers::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .layer_count(1)
}

/// An R8, RG8 or BGRA image: a shader output (`storage`) or an upload.
fn plane_image(
    shared: &Arc<Shared>,
    format: vk::Format,
    width: u32,
    height: u32,
    storage: bool,
) -> Result<Image, GpuError> {
    let usage = if storage {
        vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC
    } else {
        vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST
    };
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    Image::new(shared, &info, &[(format, vk::ImageAspectFlags::COLOR, 0)])
}

fn pipeline(
    shared: &Shared,
    layout: vk::PipelineLayout,
    spv: &[u8],
) -> Result<vk::Pipeline, GpuError> {
    let words: Vec<u32> = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let d = &shared.device;
    let info = vk::ShaderModuleCreateInfo::default().code(&words);
    // SAFETY: `words` is SPIR-V that naga wrote and validated at build
    // time.
    let module = unsafe { d.create_shader_module(&info, None) }.vk("create shader module")?;
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(c"main");
    let info = vk::ComputePipelineCreateInfo::default()
        .stage(stage)
        .layout(layout);
    // SAFETY: module and layout are alive; the shader's bindings and push
    // constants match the layout.
    let result = unsafe { d.create_compute_pipelines(vk::PipelineCache::null(), &[info], None) };
    // SAFETY: a module may go once its pipelines are made.
    unsafe { d.destroy_shader_module(module, None) };
    match result {
        Ok(p) => Ok(p[0]),
        Err((_, e)) => Err(GpuError::vulkan("create compute pipeline", e)),
    }
}

/// The shaders' push constants (`Params` in `shaders/*.wgsl`).
#[derive(Clone, Copy)]
struct Params {
    src_size: [u32; 2],
    dst_size: [u32; 2],
    kr: f32,
    kb: f32,
    full_range: u32,
}

impl Params {
    fn new(src: (u32, u32), dst: (u32, u32), color: ColorSpace) -> Self {
        let (kr, kb) = match color.matrix {
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt709 => (0.2126, 0.0722),
        };
        Self {
            src_size: [src.0, src.1],
            dst_size: [dst.0, dst.1],
            kr,
            kb,
            full_range: u32::from(color.range == Range::Full),
        }
    }

    /// The WGSL layout: two vec2<u32>, two f32, two u32 (32 bytes).
    fn bytes(self) -> [u8; PARAMS_SIZE as usize] {
        let words = [
            self.src_size[0],
            self.src_size[1],
            self.dst_size[0],
            self.dst_size[1],
            self.kr.to_bits(),
            self.kb.to_bits(),
            self.full_range,
            0,
        ];
        let mut out = [0; PARAMS_SIZE as usize];
        for (chunk, w) in out.chunks_exact_mut(4).zip(words) {
            chunk.copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_match_the_wgsl_layout() {
        let p = Params::new(
            (3840, 2160),
            (1920, 1080),
            ColorSpace {
                matrix: Matrix::Bt709,
                range: Range::Full,
            },
        );
        let b = p.bytes();
        assert_eq!(&b[0..4], &3840u32.to_le_bytes());
        assert_eq!(&b[12..16], &1080u32.to_le_bytes());
        assert_eq!(f32::from_le_bytes([b[16], b[17], b[18], b[19]]), 0.2126);
        assert_eq!(&b[24..28], &1u32.to_le_bytes());
    }
}
