//! Video frames: planes in CPU memory with their layout, size, colour space
//! and capture time. A frame's bytes are never logged: `Debug` shows only
//! its shape.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::VideoError;

/// How a frame's pixels are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// 4:2:0, a Y plane then one interleaved UV plane (what hardware
    /// encoders take).
    Nv12,
    /// 4:2:0, Y, U and V planes (libwebrtc's own buffers).
    I420,
    /// 4:2:2 packed as Y0 U Y1 V (webcams).
    Yuyv,
    /// 8-bit B, G, R, A per pixel, packed (screen capture).
    Bgra,
}

impl PixelFormat {
    /// How many planes a frame of this format has.
    pub fn plane_count(self) -> usize {
        match self {
            Self::Nv12 => 2,
            Self::I420 => 3,
            Self::Yuyv | Self::Bgra => 1,
        }
    }

    /// Whether width and height must be even (chroma is shared by pixel
    /// pairs).
    fn needs_even(self) -> (bool, bool) {
        match self {
            Self::Nv12 | Self::I420 => (true, true),
            Self::Yuyv => (true, false),
            Self::Bgra => (false, false),
        }
    }

    /// The smallest stride (bytes per row) and the number of rows of plane
    /// `index` for a `width` × `height` frame.
    fn plane_shape(self, index: usize, width: u32, height: u32) -> (usize, usize) {
        let (w, h) = (width as usize, height as usize);
        match (self, index) {
            (Self::Nv12 | Self::I420, 0) => (w, h),
            (Self::Nv12, 1) => (w, h / 2),
            (Self::I420, _) => (w / 2, h / 2),
            (Self::Yuyv, _) => (w * 2, h),
            (Self::Bgra, _) => (w * 4, h),
            (Self::Nv12, _) => unreachable!("NV12 has two planes"),
        }
    }
}

/// The YUV matrix of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Matrix {
    /// Standard definition and JPEG.
    Bt601,
    /// HD and above, and what we send.
    #[default]
    Bt709,
}

/// The range of a frame's YUV values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Range {
    /// Y in 16..=235, chroma in 16..=240 (video, what we send).
    #[default]
    Limited,
    /// The full 0..=255 (JPEG, some webcams).
    Full,
}

/// How YUV values map to colours. BGRA frames ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ColorSpace {
    /// The matrix.
    pub matrix: Matrix,
    /// The range.
    pub range: Range,
}

/// One plane: its bytes and how many bytes a row takes (at least the
/// visible width).
#[derive(Clone, PartialEq, Eq)]
pub struct Plane {
    /// The rows, `stride` bytes apart.
    pub data: Arc<[u8]>,
    /// Bytes per row.
    pub stride: usize,
}

impl fmt::Debug for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plane")
            .field("len", &self.data.len())
            .field("stride", &self.stride)
            .finish()
    }
}

/// A frame in CPU memory. Its planes are checked against its format and
/// size when it is made, so code that reads it can index rows safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuFrame {
    format: PixelFormat,
    width: u32,
    height: u32,
    planes: Vec<Plane>,
    color: ColorSpace,
    timestamp: Duration,
}

impl CpuFrame {
    /// A frame from its planes.
    ///
    /// # Errors
    /// [`VideoError::InvalidFrame`] when the size is zero or odd where the
    /// format needs it even, when the plane count is wrong, or when a plane
    /// is narrower or shorter than the size needs.
    pub fn new(
        format: PixelFormat,
        width: u32,
        height: u32,
        planes: Vec<Plane>,
        color: ColorSpace,
        timestamp: Duration,
    ) -> Result<Self, VideoError> {
        if width == 0 || height == 0 {
            return Err(VideoError::InvalidFrame("empty frame"));
        }
        let (even_w, even_h) = format.needs_even();
        if (even_w && !width.is_multiple_of(2)) || (even_h && !height.is_multiple_of(2)) {
            return Err(VideoError::InvalidFrame("odd size for a subsampled format"));
        }
        if planes.len() != format.plane_count() {
            return Err(VideoError::InvalidFrame("wrong number of planes"));
        }
        for (i, plane) in planes.iter().enumerate() {
            let (row, rows) = format.plane_shape(i, width, height);
            if plane.stride < row {
                return Err(VideoError::InvalidFrame("stride narrower than a row"));
            }
            // The last row need not be padded to the stride.
            if plane.data.len() < plane.stride * (rows - 1) + row {
                return Err(VideoError::InvalidFrame("plane shorter than the frame"));
            }
        }
        Ok(Self {
            format,
            width,
            height,
            planes,
            color,
            timestamp,
        })
    }

    /// A frame with tightly packed planes (stride = row width) filled
    /// with `fill[i]` for plane `i`.
    ///
    /// # Errors
    /// As [`CpuFrame::new`].
    pub fn filled(
        format: PixelFormat,
        width: u32,
        height: u32,
        fill: &[u8],
        color: ColorSpace,
        timestamp: Duration,
    ) -> Result<Self, VideoError> {
        let planes = (0..format.plane_count())
            .map(|i| {
                let (row, rows) = format.plane_shape(i, width, height);
                let value = fill.get(i).copied().unwrap_or(0);
                Plane {
                    data: vec![value; row * rows].into(),
                    stride: row,
                }
            })
            .collect();
        Self::new(format, width, height, planes, color, timestamp)
    }

    /// The pixel layout.
    pub fn format(&self) -> PixelFormat {
        self.format
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The planes, in the format's order (Y first).
    pub fn planes(&self) -> &[Plane] {
        &self.planes
    }

    /// How YUV values map to colours.
    pub fn color(&self) -> ColorSpace {
        self.color
    }

    /// When the frame was captured, on the capturer's clock.
    pub fn timestamp(&self) -> Duration {
        self.timestamp
    }

    /// Row `y` of plane `index`, without its padding.
    pub(crate) fn row(&self, index: usize, y: usize) -> &[u8] {
        let plane = &self.planes[index];
        let (row, _) = self.format.plane_shape(index, self.width, self.height);
        &plane.data[y * plane.stride..y * plane.stride + row]
    }
}

/// A frame as a capturer hands it over: in CPU memory, or (Linux) a
/// DMA-BUF that stays on the GPU.
#[derive(Debug)]
pub enum Frame {
    /// Planes in CPU memory.
    Cpu(CpuFrame),
    /// A GPU buffer shared by file descriptor.
    #[cfg(target_os = "linux")]
    DmaBuf(DmaBufFrame),
}

impl Frame {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        match self {
            Self::Cpu(f) => f.width(),
            #[cfg(target_os = "linux")]
            Self::DmaBuf(f) => f.width,
        }
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        match self {
            Self::Cpu(f) => f.height(),
            #[cfg(target_os = "linux")]
            Self::DmaBuf(f) => f.height,
        }
    }

    /// When the frame was captured, on the capturer's clock.
    pub fn timestamp(&self) -> Duration {
        match self {
            Self::Cpu(f) => f.timestamp(),
            #[cfg(target_os = "linux")]
            Self::DmaBuf(f) => f.timestamp,
        }
    }
}

/// One plane of a DMA-BUF.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct DmaBufPlane {
    /// The buffer holding the plane (planes may share one).
    pub fd: std::os::fd::OwnedFd,
    /// Where the plane starts in the buffer, bytes.
    pub offset: u32,
    /// Bytes per row.
    pub stride: u32,
}

/// A frame that stays in GPU memory, as PipeWire hands it over (imported
/// by the Vulkan code of Task 4).
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub struct DmaBufFrame {
    /// DRM fourcc of the layout.
    pub fourcc: u32,
    /// DRM format modifier (tiling).
    pub modifier: u64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The planes.
    pub planes: Vec<DmaBufPlane>,
    /// How YUV values map to colours (YUV layouts only).
    pub color: ColorSpace,
    /// When the frame was captured, on the capturer's clock.
    pub timestamp: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(len: usize, stride: usize) -> Plane {
        Plane {
            data: vec![0; len].into(),
            stride,
        }
    }

    #[test]
    fn frames_are_checked_against_their_format() {
        let ok = |format, w, h, planes| {
            CpuFrame::new(format, w, h, planes, ColorSpace::default(), Duration::ZERO).is_ok()
        };
        assert!(ok(PixelFormat::Nv12, 4, 2, vec![plane(8, 4), plane(4, 4)]));
        // Padded rows, the last one without its padding.
        assert!(ok(PixelFormat::Nv12, 4, 2, vec![plane(12, 8), plane(4, 8)]));
        assert!(!ok(PixelFormat::Nv12, 4, 2, vec![plane(8, 4)]));
        assert!(!ok(PixelFormat::Nv12, 3, 2, vec![plane(6, 3), plane(4, 4)]));
        assert!(!ok(PixelFormat::Nv12, 4, 2, vec![plane(7, 4), plane(4, 4)]));
        assert!(!ok(PixelFormat::Nv12, 4, 2, vec![plane(8, 3), plane(4, 4)]));
        assert!(ok(
            PixelFormat::I420,
            4,
            2,
            vec![plane(8, 4), plane(2, 2), plane(2, 2)]
        ));
        assert!(ok(PixelFormat::Yuyv, 2, 3, vec![plane(12, 4)]));
        assert!(!ok(PixelFormat::Yuyv, 3, 3, vec![plane(18, 6)]));
        assert!(ok(PixelFormat::Bgra, 3, 3, vec![plane(36, 12)]));
        assert!(!ok(PixelFormat::Bgra, 0, 3, vec![plane(0, 0)]));
    }

    #[test]
    fn debug_shows_the_shape_not_the_bytes() {
        let frame = CpuFrame::filled(
            PixelFormat::Nv12,
            4,
            2,
            &[0xAB, 0xCD],
            ColorSpace::default(),
            Duration::ZERO,
        )
        .unwrap();
        let text = format!("{frame:?}");
        assert!(
            text.contains("len: 8") && text.contains("stride: 4"),
            "{text}"
        );
        assert!(!text.contains("171") && !text.contains("AB"), "{text}");
    }
}
