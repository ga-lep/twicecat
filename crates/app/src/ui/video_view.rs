//! Draws decoded video frames (Phase 9 Task 10, plan Q8): a `shader` widget
//! that uploads the planes of the newest frame of a [`VideoFeed`] to the GPU
//! and converts YUV to RGB there, so a 4K frame is never converted on the
//! CPU and the app never rebuilds its view for a frame.
//!
//! The widget reads the feed's newest frame itself when it is drawn
//! ([`VideoFeed::latest`] keeps only that one) and asks for the next redraw
//! while frames keep coming. It also tells the app how big it is on screen
//! in physical pixels, which the call server needs to pick a layer.
//!
//! iced draws shader widgets only with its wgpu renderer. [`GpuSeen`] says
//! whether one was ever drawn, so the app can tell that video has nowhere to
//! go (the software renderer) instead of showing black.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use iced::wgpu;
use iced::widget::shader::{self, Action, Viewport};
use iced::{Element, Length, Rectangle, Size, mouse, window};
use twicecat_core::api::{
    VideoFeed, VideoFrame, VideoMatrix, VideoPixelFormat, VideoRange, VideoTrackId,
};

/// The conversion and the full-screen triangle.
pub const SHADER: &str = include_str!("video.wgsl");

/// How long the widget waits for the next frame before it looks again. A
/// frame that is already there is drawn at the next display frame.
const POLL: Duration = Duration::from_millis(6);
/// Frames a picture stays on the GPU after it was last drawn.
const KEEP_FRAMES: u64 = 120;
/// A change of size smaller than this many physical pixels is not reported.
const SIZE_SLACK: f32 = 2.0;

/// Which of the places that can show a track draws it, so two copies of one
/// track (the stage and the full-screen view) keep their own pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// A tile of the call stage.
    Tile,
    /// The full-screen view.
    Fullscreen,
    /// The camera preview of the Video settings.
    Preview,
}

/// Whether iced ever drew a video widget: set by the first frame the wgpu
/// renderer prepares, never by the software renderer.
#[derive(Clone, Debug, Default)]
pub struct GpuSeen(Arc<AtomicBool>);

impl GpuSeen {
    /// The wgpu renderer drew a video widget.
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A feed on screen.
pub struct Surface<Message> {
    key: u64,
    feed: VideoFeed,
    mirror: bool,
    /// Physical pixels per logical pixel of the layout.
    scale: f32,
    seen: GpuSeen,
    on_size: Option<Arc<dyn Fn(Size<u32>) -> Message>>,
}

impl<Message> Surface<Message> {
    /// A surface for `track` in `place`, drawn at `scale` physical pixels
    /// per logical one.
    pub fn new(
        track: &VideoTrackId,
        place: Place,
        feed: VideoFeed,
        scale: f32,
        seen: GpuSeen,
    ) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        track.as_str().hash(&mut hasher);
        place.hash(&mut hasher);
        Self {
            key: hasher.finish(),
            feed,
            mirror: false,
            scale,
            seen,
            on_size: None,
        }
    }

    /// Flips the picture left to right (our own camera, as in a mirror).
    pub fn mirrored(mut self, mirror: bool) -> Self {
        self.mirror = mirror;
        self
    }

    /// The message that says how big the surface is on screen, in physical
    /// pixels, whenever that changes.
    pub fn on_size(mut self, message: impl Fn(Size<u32>) -> Message + 'static) -> Self {
        self.on_size = Some(Arc::new(message));
        self
    }
}

impl<'a, Message: 'a> From<Surface<Message>> for Element<'a, Message> {
    fn from(surface: Surface<Message>) -> Self {
        iced::widget::shader(surface)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// What the widget remembers between redraws.
#[derive(Debug, Default)]
pub struct Memory {
    reported: Option<Size<u32>>,
    last: Option<usize>,
}

impl<Message> shader::Program<Message> for Surface<Message> {
    type State = Memory;
    type Primitive = Picture;

    fn update(
        &self,
        memory: &mut Memory,
        event: &iced::Event,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        let iced::Event::Window(window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        let physical = Size::new(
            (bounds.width * self.scale).round() as u32,
            (bounds.height * self.scale).round() as u32,
        );
        if let Some(on_size) = &self.on_size
            && changed(memory.reported, physical)
        {
            memory.reported = Some(physical);
            return Some(Action::publish(on_size(physical)));
        }
        let newest = self.feed.latest().map(|f| Arc::as_ptr(&f) as usize);
        if newest != memory.last {
            memory.last = newest;
            Some(Action::request_redraw())
        } else if self.feed.is_ended() {
            None
        } else {
            Some(Action::request_redraw_at(*now + POLL))
        }
    }

    fn draw(&self, _memory: &Memory, _cursor: mouse::Cursor, _bounds: Rectangle) -> Picture {
        Picture {
            key: self.key,
            frame: self.feed.latest(),
            mirror: self.mirror,
            seen: self.seen.clone(),
        }
    }
}

/// Whether `now` differs from the size last reported by more than the slack.
fn changed(reported: Option<Size<u32>>, now: Size<u32>) -> bool {
    match reported {
        None => now.width > 0 && now.height > 0,
        Some(before) => {
            (before.width as f32 - now.width as f32).abs() > SIZE_SLACK
                || (before.height as f32 - now.height as f32).abs() > SIZE_SLACK
        }
    }
}

/// One frame to draw.
pub struct Picture {
    key: u64,
    frame: Option<Arc<VideoFrame>>,
    mirror: bool,
    seen: GpuSeen,
}

// The pixels are user data: only the size is written.
impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Picture")
            .field("size", &self.frame.as_ref().map(|fr| (fr.width, fr.height)))
            .finish_non_exhaustive()
    }
}

impl shader::Primitive for Picture {
    type Pipeline = Gpu;

    fn prepare(
        &self,
        gpu: &mut Gpu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        self.seen.set();
        let Some(frame) = &self.frame else {
            return;
        };
        if !valid(frame) {
            return;
        }
        gpu.show(device, queue, self.key, frame, self.mirror, bounds.size());
    }

    fn draw(&self, gpu: &Gpu, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(tile) = gpu.tiles.get(&self.key) else {
            return true;
        };
        if !tile.ready {
            return true;
        }
        pass.set_pipeline(&gpu.pipeline);
        pass.set_bind_group(0, &tile.bind_group, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

/// The planes of a frame as the textures that hold them: the width and height
/// in texels, the bytes per texel and the row length in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneShape {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// Bytes in a texel.
    pub texel: u32,
}

/// The planes `frame`'s format has.
pub fn plane_shapes(frame: &VideoFrame) -> Vec<PlaneShape> {
    let (w, h) = (frame.width, frame.height);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    match frame.format {
        VideoPixelFormat::Nv12 => vec![
            PlaneShape {
                width: w,
                height: h,
                texel: 1,
            },
            PlaneShape {
                width: cw,
                height: ch,
                texel: 2,
            },
        ],
        VideoPixelFormat::I420 => vec![
            PlaneShape {
                width: w,
                height: h,
                texel: 1,
            },
            PlaneShape {
                width: cw,
                height: ch,
                texel: 1,
            },
            PlaneShape {
                width: cw,
                height: ch,
                texel: 1,
            },
        ],
    }
}

/// Whether the planes hold what their shapes need, so an upload cannot read
/// past them. A frame that does not is not drawn.
pub fn valid(frame: &VideoFrame) -> bool {
    if frame.width == 0 || frame.height == 0 || frame.width > MAX_SIDE || frame.height > MAX_SIDE {
        return false;
    }
    let shapes = plane_shapes(frame);
    shapes.len() == frame.planes.len()
        && shapes.iter().zip(&frame.planes).all(|(shape, plane)| {
            let row = (shape.width * shape.texel) as usize;
            let needed = plane.stride * (shape.height as usize - 1) + row;
            plane.stride >= row && plane.data.len() >= needed
        })
}

/// The largest side of a picture drawn (the GPU's own limit is at least this
/// on every adapter wgpu supports for 2D textures).
const MAX_SIDE: u32 = 8192;

/// The values the shader turns YUV into RGB with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conversion {
    /// Subtracted from Y.
    pub y_offset: f32,
    /// Y is multiplied by this.
    pub y_scale: f32,
    /// U and V (centred on 0) are multiplied by this.
    pub c_scale: f32,
    /// R = y + rv * cr.
    pub rv: f32,
    /// G = y - gu * cb - gv * cr.
    pub gu: f32,
    /// See `gu`.
    pub gv: f32,
    /// B = y + bu * cb.
    pub bu: f32,
}

impl Conversion {
    /// The standard conversion of `matrix` and `range`.
    pub fn of(matrix: VideoMatrix, range: VideoRange) -> Self {
        let (rv, gu, gv, bu) = match matrix {
            VideoMatrix::Bt601 => (1.402, 0.344_136, 0.714_136, 1.772),
            VideoMatrix::Bt709 => (1.5748, 0.187_324, 0.468_124, 1.8556),
        };
        let (y_offset, y_scale, c_scale) = match range {
            VideoRange::Limited => (16.0 / 255.0, 255.0 / 219.0, 255.0 / 224.0),
            VideoRange::Full => (0.0, 1.0, 1.0),
        };
        Self {
            y_offset,
            y_scale,
            c_scale,
            rv,
            gu,
            gv,
            bu,
        }
    }

    /// RGB (0..=1, gamma-encoded) of one Y, Cb, Cr triple in 0..=255: what the
    /// shader computes, for the tests.
    pub fn rgb(self, y: u8, cb: u8, cr: u8) -> [f32; 3] {
        let y = (f32::from(y) / 255.0 - self.y_offset) * self.y_scale;
        let cb = (f32::from(cb) / 255.0 - 0.5) * self.c_scale;
        let cr = (f32::from(cr) / 255.0 - 0.5) * self.c_scale;
        [
            y + self.rv * cr,
            y - self.gu * cb - self.gv * cr,
            y + self.bu * cb,
        ]
        .map(|c| c.clamp(0.0, 1.0))
    }
}

/// How much of the widget a `width` x `height` picture covers when it keeps
/// its shape: (1, 1) is all of it.
pub fn fit(picture: (u32, u32), widget: Size) -> (f32, f32) {
    let picture_aspect = picture.0 as f32 / picture.1.max(1) as f32;
    let widget_aspect = widget.width / widget.height.max(1.0);
    if !picture_aspect.is_finite() || !widget_aspect.is_finite() || widget_aspect <= 0.0 {
        return (1.0, 1.0);
    }
    if picture_aspect > widget_aspect {
        (1.0, widget_aspect / picture_aspect)
    } else {
        (picture_aspect / widget_aspect, 1.0)
    }
}

/// The parameters block of the shader, 48 bytes.
fn params_bytes(
    frame: &VideoFrame,
    mirror: bool,
    linearise: bool,
    widget: Size,
) -> [u8; PARAMS_SIZE as usize] {
    let (fit_x, fit_y) = fit((frame.width, frame.height), widget);
    let c = Conversion::of(frame.matrix, frame.range);
    let planar = frame.format == VideoPixelFormat::I420;
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let values = [
        fit_x,
        fit_y,
        flag(mirror),
        flag(planar),
        c.y_offset,
        c.y_scale,
        c.c_scale,
        flag(linearise),
        c.rv,
        c.gu,
        c.gv,
        c.bu,
    ];
    let mut bytes = [0u8; PARAMS_SIZE as usize];
    for (slot, value) in bytes.chunks_exact_mut(4).zip(values) {
        slot.copy_from_slice(&value.to_ne_bytes());
    }
    bytes
}

const PARAMS_SIZE: u64 = 48;

/// One picture on the GPU: its textures, the parameters and what binds them.
struct Tile {
    format: VideoPixelFormat,
    size: (u32, u32),
    planes: Vec<wgpu::Texture>,
    params: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    shown: Option<Arc<VideoFrame>>,
    used: u64,
    ready: bool,
}

/// The shared GPU state of every video widget: the pipeline, and the
/// pictures of the widgets that were drawn lately.
pub struct Gpu {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    dummy: wgpu::TextureView,
    linearise: bool,
    tiles: HashMap<u64, Tile>,
    frame: u64,
}

impl shader::Pipeline for Gpu {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("twicecat video"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("twicecat video layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_SIZE),
                    },
                    count: None,
                },
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("twicecat video pipeline layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("twicecat video pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("twicecat video sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        // The third plane of an NV12 frame: bound, never read.
        let dummy = make_texture(device, 1, 1, wgpu::TextureFormat::R8Unorm);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &dummy,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[128],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        Self {
            pipeline,
            layout,
            sampler,
            dummy: dummy.create_view(&wgpu::TextureViewDescriptor::default()),
            linearise: format.is_srgb(),
            tiles: HashMap::new(),
            frame: 0,
        }
    }

    fn trim(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        self.tiles
            .retain(|_, tile| frame.saturating_sub(tile.used) < KEEP_FRAMES);
    }
}

fn make_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("twicecat video plane"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

impl Gpu {
    /// Makes the picture of `key` the frame `frame`: uploads its planes when
    /// it is a new one, and writes the parameters for the widget's shape.
    fn show(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: u64,
        frame: &Arc<VideoFrame>,
        mirror: bool,
        widget: Size,
    ) {
        let same_shape = self
            .tiles
            .get(&key)
            .is_some_and(|t| t.format == frame.format && t.size == (frame.width, frame.height));
        if !same_shape {
            let tile = self.make_tile(device, frame);
            self.tiles.insert(key, tile);
        }
        let used = self.frame;
        let linearise = self.linearise;
        let Some(tile) = self.tiles.get_mut(&key) else {
            return;
        };
        tile.used = used;
        let new = tile
            .shown
            .as_ref()
            .is_none_or(|shown| !Arc::ptr_eq(shown, frame));
        if new {
            for ((plane, shape), texture) in frame
                .planes
                .iter()
                .zip(plane_shapes(frame))
                .zip(&tile.planes)
            {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &plane.data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(plane.stride as u32),
                        rows_per_image: Some(shape.height),
                    },
                    wgpu::Extent3d {
                        width: shape.width,
                        height: shape.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            tile.shown = Some(frame.clone());
            tile.ready = true;
        }
        queue.write_buffer(
            &tile.params,
            0,
            &params_bytes(frame, mirror, linearise, widget),
        );
    }

    fn make_tile(&self, device: &wgpu::Device, frame: &VideoFrame) -> Tile {
        let planes: Vec<wgpu::Texture> = plane_shapes(frame)
            .into_iter()
            .map(|shape| {
                let format = if shape.texel == 2 {
                    wgpu::TextureFormat::Rg8Unorm
                } else {
                    wgpu::TextureFormat::R8Unorm
                };
                make_texture(device, shape.width, shape.height, format)
            })
            .collect();
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("twicecat video parameters"),
            size: PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let views: Vec<wgpu::TextureView> = planes
            .iter()
            .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
            .collect();
        let third = views.get(2).unwrap_or(&self.dummy);
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("twicecat video bindings"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&views[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&views[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(third),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        Tile {
            format: frame.format,
            size: (frame.width, frame.height),
            planes,
            params,
            bind_group,
            shown: None,
            used: self.frame,
            ready: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::VideoPlane;

    use super::*;

    fn frame(format: VideoPixelFormat, width: u32, height: u32) -> VideoFrame {
        let plane = |w: u32, h: u32, texel: u32, value: u8| VideoPlane {
            data: vec![value; (w * h * texel) as usize].into(),
            stride: (w * texel) as usize,
        };
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        let planes = match format {
            VideoPixelFormat::Nv12 => vec![plane(width, height, 1, 126), plane(cw, ch, 2, 128)],
            VideoPixelFormat::I420 => vec![
                plane(width, height, 1, 126),
                plane(cw, ch, 1, 128),
                plane(cw, ch, 1, 128),
            ],
        };
        VideoFrame {
            format,
            width,
            height,
            planes,
            matrix: VideoMatrix::Bt709,
            range: VideoRange::Limited,
            timestamp_us: 0,
        }
    }

    #[test]
    fn the_shader_parses_and_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER).expect("the WGSL parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("the WGSL validates");
    }

    #[test]
    fn the_parameter_block_matches_the_shader_struct() {
        // 2 + 10 floats; the shader's struct is vec2 + 10 f32 = 48 bytes.
        let f = frame(VideoPixelFormat::Nv12, 4, 2);
        assert_eq!(
            params_bytes(&f, false, false, Size::new(4.0, 2.0)).len(),
            48
        );
        assert_eq!(PARAMS_SIZE, 48);
    }

    #[test]
    fn plane_shapes_follow_the_format() {
        let nv12 = plane_shapes(&frame(VideoPixelFormat::Nv12, 6, 4));
        assert_eq!(nv12.len(), 2);
        assert_eq!((nv12[1].width, nv12[1].height, nv12[1].texel), (3, 2, 2));
        let i420 = plane_shapes(&frame(VideoPixelFormat::I420, 6, 4));
        assert_eq!(i420.len(), 3);
        assert_eq!((i420[2].width, i420[2].height, i420[2].texel), (3, 2, 1));
    }

    #[test]
    fn a_frame_whose_planes_are_too_short_is_refused() {
        let mut f = frame(VideoPixelFormat::I420, 8, 8);
        assert!(valid(&f));
        f.planes[1].data = vec![0; 3].into();
        assert!(!valid(&f));
        let mut g = frame(VideoPixelFormat::Nv12, 8, 8);
        g.planes.pop();
        assert!(!valid(&g));
        let mut h = frame(VideoPixelFormat::Nv12, 8, 8);
        h.planes[0].stride = 4;
        assert!(!valid(&h));
        assert!(!valid(&frame(VideoPixelFormat::Nv12, 0, 0)));
    }

    #[test]
    fn a_padded_stride_is_accepted() {
        let mut f = frame(VideoPixelFormat::Nv12, 4, 4);
        // Rows 8 bytes apart for a 4-byte row: the last row needs no padding.
        f.planes[0].data = vec![0; 8 * 3 + 4].into();
        f.planes[0].stride = 8;
        assert!(valid(&f));
    }

    #[test]
    fn the_standard_conversions_give_known_colours() {
        // Limited range BT.709: black, white, and a saturated red.
        let c = Conversion::of(VideoMatrix::Bt709, VideoRange::Limited);
        let near =
            |got: [f32; 3], want: [f32; 3]| got.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.01);
        assert!(near(c.rgb(16, 128, 128), [0.0, 0.0, 0.0]));
        assert!(near(c.rgb(235, 128, 128), [1.0, 1.0, 1.0]));
        assert!(
            near(c.rgb(63, 102, 240), [1.0, 0.0, 0.0]),
            "{:?}",
            c.rgb(63, 102, 240)
        );
        // Full range: the same bytes are brighter and less saturated at the ends.
        let full = Conversion::of(VideoMatrix::Bt709, VideoRange::Full);
        assert!(near(full.rgb(0, 128, 128), [0.0, 0.0, 0.0]));
        assert!(near(full.rgb(255, 128, 128), [1.0, 1.0, 1.0]));
        // BT.601 reads the same chroma as a different colour.
        let sd = Conversion::of(VideoMatrix::Bt601, VideoRange::Limited);
        assert!(sd.rgb(63, 102, 240) != c.rgb(63, 102, 240));
    }

    #[test]
    fn a_picture_keeps_its_shape_in_the_widget() {
        // A 16:9 picture in a square widget: full width, 9/16 of the height.
        let (x, y) = fit((1920, 1080), Size::new(100.0, 100.0));
        assert!(
            (x - 1.0).abs() < 1e-6 && (y - 0.5625).abs() < 1e-4,
            "{x} {y}"
        );
        // The same picture in a tall widget.
        let (x, y) = fit((1920, 1080), Size::new(50.0, 200.0));
        assert!((x - 1.0).abs() < 1e-6 && y < 0.2, "{x} {y}");
        // A portrait picture in a wide widget: full height, narrow.
        let (x, y) = fit((1080, 1920), Size::new(200.0, 100.0));
        assert!(x < 0.3 && (y - 1.0).abs() < 1e-6, "{x} {y}");
        // Nothing to divide by: fills.
        assert_eq!(fit((0, 0), Size::new(0.0, 0.0)), (1.0, 1.0));
    }

    #[test]
    fn size_changes_under_the_slack_are_not_reported() {
        let s = |w, h| Size::new(w, h);
        assert!(changed(None, s(100, 50)));
        assert!(!changed(None, s(0, 0)));
        assert!(!changed(Some(s(100, 50)), s(101, 51)));
        assert!(changed(Some(s(100, 50)), s(104, 50)));
        assert!(changed(Some(s(100, 50)), s(100, 40)));
    }

    #[test]
    fn pictures_stay_out_of_debug() {
        let (tx, feed) = VideoFeed::channel();
        tx.send(frame(VideoPixelFormat::Nv12, 4, 4));
        let picture = Picture {
            key: 1,
            frame: feed.latest(),
            mirror: false,
            seen: GpuSeen::default(),
        };
        let text = format!("{picture:?}");
        assert!(text.contains("(4, 4)") && !text.contains("126"), "{text}");
    }

    /// Draws known frames on a real adapter and reads the pixels back. Skipped
    /// where there is none (CI): the colour tests above cover the maths.
    #[test]
    fn the_shader_draws_the_right_colours_on_an_adapter() {
        use shader::Pipeline as _;
        let Some((device, queue)) = adapter() else {
            eprintln!("no wgpu adapter: skipped");
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut gpu = Gpu::new(&device, &queue, format);
        // (frame format, Y, Cb, Cr, expected RGB bytes) in limited BT.709.
        let cases = [
            (VideoPixelFormat::Nv12, 16, 128, 128, [0, 0, 0]),
            (VideoPixelFormat::Nv12, 235, 128, 128, [255, 255, 255]),
            (VideoPixelFormat::I420, 63, 102, 240, [255, 0, 0]),
            (VideoPixelFormat::I420, 173, 42, 26, [0, 255, 0]),
        ];
        for (index, (pixel_format, y, cb, cr, want)) in cases.into_iter().enumerate() {
            let mut f = frame(pixel_format, 16, 16);
            let fill = |plane: &mut VideoPlane, value: u8| {
                plane.data = vec![value; plane.data.len()].into();
            };
            fill(&mut f.planes[0], y);
            match pixel_format {
                VideoPixelFormat::Nv12 => {
                    let uv: Vec<u8> = (0..f.planes[1].data.len())
                        .map(|i| if i % 2 == 0 { cb } else { cr })
                        .collect();
                    f.planes[1].data = uv.into();
                }
                VideoPixelFormat::I420 => {
                    fill(&mut f.planes[1], cb);
                    fill(&mut f.planes[2], cr);
                }
            }
            let f = Arc::new(f);
            gpu.show(
                &device,
                &queue,
                index as u64,
                &f,
                false,
                Size::new(16.0, 16.0),
            );
            let got = render(&device, &queue, &gpu, index as u64, format);
            for (g, w) in got.iter().zip(want) {
                assert!(
                    (i32::from(*g) - w).abs() <= 6,
                    "case {index}: got {got:?}, want {want:?}"
                );
            }
        }
    }

    fn adapter() -> Option<(wgpu::Device, wgpu::Queue)> {
        let runtime = tokio::runtime::Builder::new_current_thread().build().ok()?;
        runtime.block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .ok()?;
            adapter
                .request_device(&wgpu::DeviceDescriptor::default())
                .await
                .ok()
        })
    }

    /// The colour of the middle pixel of a 16 x 16 render of `key`.
    fn render(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu: &Gpu,
        key: u64,
        format: wgpu::TextureFormat,
    ) -> [u8; 3] {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16 * 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let tile = &gpu.tiles[&key];
            pass.set_pipeline(&gpu.pipeline);
            pass.set_bind_group(0, &tile.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(16),
                },
            },
            wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the GPU answers");
        let data = slice.get_mapped_range();
        let at = 8 * 256 + 8 * 4;
        [data[at], data[at + 1], data[at + 2]]
    }
}
