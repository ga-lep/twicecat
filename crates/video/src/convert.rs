//! Colour conversion on the CPU, for the software path and for frames that
//! arrive in CPU memory: BGRA, YUYV and I420 to NV12 (what the hardware
//! encoders take), NV12 and the others to I420 (what libwebrtc's own
//! encoder takes), and YUV to BGRA (thumbnails and tests). The heavy
//! conversions are `yuvutils-rs`'s SIMD code; repacking chroma between NV12
//! and I420 is a copy, done here.

use std::borrow::Cow;
use std::time::Duration;

use yuvutils_rs::{
    BufferStoreMut, YuvBiPlanarImage, YuvBiPlanarImageMut, YuvConversionMode, YuvPackedImage,
    YuvPlanarImage, YuvPlanarImageMut, YuvRange, YuvStandardMatrix,
};

use crate::VideoError;
use crate::frame::{ColorSpace, CpuFrame, Matrix, PixelFormat, Plane, Range};

fn range(r: Range) -> YuvRange {
    match r {
        Range::Limited => YuvRange::Limited,
        Range::Full => YuvRange::Full,
    }
}

fn matrix(m: Matrix) -> YuvStandardMatrix {
    match m {
        Matrix::Bt601 => YuvStandardMatrix::Bt601,
        Matrix::Bt709 => YuvStandardMatrix::Bt709,
    }
}

// yuvutils' messages name sizes only; ours are fixed text.
fn failed(_: yuvutils_rs::YuvError) -> VideoError {
    VideoError::Conversion
}

fn tight(data: Vec<u8>, stride: usize) -> Plane {
    Plane {
        data: data.into(),
        stride,
    }
}

/// Plane `i` of `frame` as yuvutils reads it: every row `stride` bytes,
/// the last one too. yuvutils splits a plane into rows (or row pairs) with
/// `chunks_exact` (`rgba_to_nv.rs:811`, `:885`; `yuv_to_rgba.rs:632`), so
/// a last row without its padding, which [`CpuFrame::new`] allows, is
/// skipped and its output left zero, while its size checks pass
/// (`yuv_error.rs:131`). Such a plane is copied with the padding added; a
/// full one is used as it is.
fn full_rows(frame: &CpuFrame, i: usize) -> Cow<'_, [u8]> {
    let plane = &frame.planes()[i];
    let (_, rows) = frame.format().plane_shape(i, frame.width(), frame.height());
    let full = plane.stride * rows;
    if plane.data.len() >= full {
        Cow::Borrowed(&plane.data)
    } else {
        let mut data = Vec::with_capacity(full);
        data.extend_from_slice(&plane.data);
        data.resize(full, 0);
        Cow::Owned(data)
    }
}

/// Plane `i` of `frame` with its rows packed tight, exactly as long as
/// they are, for yuvutils' checks that refuse padding: an NV12 UV plane
/// must be exactly its minimum width times its rows (`yuv_error.rs:268`),
/// a YUYV frame exactly two bytes a pixel (`yuv_error.rs:165`). A tight
/// plane is used as it is.
fn tight_rows(frame: &CpuFrame, i: usize) -> Cow<'_, [u8]> {
    let plane = &frame.planes()[i];
    let (row, rows) = frame.format().plane_shape(i, frame.width(), frame.height());
    if plane.stride == row {
        return Cow::Borrowed(&plane.data[..row * rows]);
    }
    let mut data = Vec::with_capacity(row * rows);
    for r in 0..rows {
        data.extend_from_slice(&plane.data[r * plane.stride..][..row]);
    }
    Cow::Owned(data)
}

fn u32_of(n: usize) -> Result<u32, VideoError> {
    u32::try_from(n).map_err(|_| VideoError::InvalidFrame("stride too large"))
}

fn made(
    format: PixelFormat,
    from: &CpuFrame,
    planes: Vec<Plane>,
    color: ColorSpace,
    timestamp: Duration,
) -> Result<CpuFrame, VideoError> {
    CpuFrame::new(
        format,
        from.width(),
        from.height(),
        planes,
        color,
        timestamp,
    )
}

/// `frame` as NV12. A YUV source keeps its colour space; a BGRA one is
/// converted into `color`.
///
/// # Errors
/// [`VideoError::Conversion`] if the converter refuses the frame.
pub fn to_nv12(frame: &CpuFrame, color: ColorSpace) -> Result<CpuFrame, VideoError> {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    match frame.format() {
        PixelFormat::Nv12 => Ok(frame.clone()),
        PixelFormat::I420 => {
            let mut uv = vec![0; w * (h / 2)];
            for y in 0..h / 2 {
                let (u, v) = (frame.row(1, y), frame.row(2, y));
                for (x, pair) in uv[y * w..(y + 1) * w].chunks_exact_mut(2).enumerate() {
                    pair[0] = u[x];
                    pair[1] = v[x];
                }
            }
            let planes = vec![frame.planes()[0].clone(), tight(uv, w)];
            made(
                PixelFormat::Nv12,
                frame,
                planes,
                frame.color(),
                frame.timestamp(),
            )
        }
        PixelFormat::Yuyv => to_nv12(&to_i420(frame, color)?, color),
        PixelFormat::Bgra => {
            let bgra = &frame.planes()[0];
            let mut image = YuvBiPlanarImageMut {
                y_plane: BufferStoreMut::Owned(vec![0; w * h]),
                y_stride: u32_of(w)?,
                uv_plane: BufferStoreMut::Owned(vec![0; w.div_ceil(2) * 2 * h.div_ceil(2)]),
                uv_stride: u32_of(w.div_ceil(2) * 2)?,
                width: frame.width(),
                height: frame.height(),
            };
            yuvutils_rs::bgra_to_yuv_nv12(
                &mut image,
                &full_rows(frame, 0),
                u32_of(bgra.stride)?,
                range(color.range),
                matrix(color.matrix),
                YuvConversionMode::Balanced,
            )
            .map_err(failed)?;
            let uv_stride = image.uv_stride as usize;
            let (BufferStoreMut::Owned(y), BufferStoreMut::Owned(uv)) =
                (image.y_plane, image.uv_plane)
            else {
                unreachable!("planes were allocated above");
            };
            // An odd-sized BGRA frame is cropped to even for 4:2:0.
            let even = CpuFrame::new(
                PixelFormat::Nv12,
                frame.width() & !1,
                frame.height() & !1,
                vec![tight(y, w), tight(uv, uv_stride)],
                color,
                frame.timestamp(),
            )?;
            Ok(even)
        }
    }
}

/// `frame` as I420. A YUV source keeps its colour space; a BGRA one is
/// converted into `color`.
///
/// # Errors
/// [`VideoError::Conversion`] if the converter refuses the frame.
pub fn to_i420(frame: &CpuFrame, color: ColorSpace) -> Result<CpuFrame, VideoError> {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let planar = |w: usize, h: usize| YuvPlanarImageMut {
        y_plane: BufferStoreMut::Owned(vec![0; w * h]),
        y_stride: w as u32,
        u_plane: BufferStoreMut::Owned(vec![0; w.div_ceil(2) * h.div_ceil(2)]),
        u_stride: w.div_ceil(2) as u32,
        v_plane: BufferStoreMut::Owned(vec![0; w.div_ceil(2) * h.div_ceil(2)]),
        v_stride: w.div_ceil(2) as u32,
        width: w as u32,
        height: h as u32,
    };
    let finish = |image: YuvPlanarImageMut<'_, u8>, color, width, height| {
        let (ys, cs) = (image.y_stride as usize, image.u_stride as usize);
        let (BufferStoreMut::Owned(y), BufferStoreMut::Owned(u), BufferStoreMut::Owned(v)) =
            (image.y_plane, image.u_plane, image.v_plane)
        else {
            unreachable!("planes were allocated above");
        };
        CpuFrame::new(
            PixelFormat::I420,
            width,
            height,
            vec![tight(y, ys), tight(u, cs), tight(v, cs)],
            color,
            frame.timestamp(),
        )
    };
    match frame.format() {
        PixelFormat::I420 => Ok(frame.clone()),
        PixelFormat::Nv12 => {
            let (cw, ch) = (w / 2, h / 2);
            let (mut u, mut v) = (vec![0; cw * ch], vec![0; cw * ch]);
            for y in 0..ch {
                for (x, pair) in frame.row(1, y).chunks_exact(2).enumerate() {
                    u[y * cw + x] = pair[0];
                    v[y * cw + x] = pair[1];
                }
            }
            let planes = vec![frame.planes()[0].clone(), tight(u, cw), tight(v, cw)];
            made(
                PixelFormat::I420,
                frame,
                planes,
                frame.color(),
                frame.timestamp(),
            )
        }
        PixelFormat::Yuyv => {
            let mut image = planar(w, h);
            yuvutils_rs::yuyv422_to_yuv420(
                &mut image,
                &YuvPackedImage {
                    yuy: &tight_rows(frame, 0),
                    yuy_stride: u32_of(w * 2)?,
                    width: frame.width(),
                    height: frame.height(),
                },
            )
            .map_err(failed)?;
            // YUYV is already YUV: its colour space carries over.
            finish(image, frame.color(), frame.width(), frame.height() & !1)
        }
        PixelFormat::Bgra => {
            let bgra = &frame.planes()[0];
            let mut image = planar(w, h);
            yuvutils_rs::bgra_to_yuv420(
                &mut image,
                &full_rows(frame, 0),
                u32_of(bgra.stride)?,
                range(color.range),
                matrix(color.matrix),
                YuvConversionMode::Balanced,
            )
            .map_err(failed)?;
            finish(image, color, frame.width() & !1, frame.height() & !1)
        }
    }
}

/// `frame` as BGRA, opaque.
///
/// # Errors
/// [`VideoError::Conversion`] if the converter refuses the frame.
pub fn to_bgra(frame: &CpuFrame) -> Result<CpuFrame, VideoError> {
    let (w, h) = (frame.width(), frame.height());
    let stride = w as usize * 4;
    let mut out = vec![0; stride * h as usize];
    let color = frame.color();
    match frame.format() {
        PixelFormat::Bgra => return Ok(frame.clone()),
        PixelFormat::Nv12 => {
            let y = &frame.planes()[0];
            yuvutils_rs::yuv_nv12_to_bgra(
                &YuvBiPlanarImage {
                    y_plane: &full_rows(frame, 0),
                    y_stride: u32_of(y.stride)?,
                    uv_plane: &tight_rows(frame, 1),
                    uv_stride: u32_of(w as usize)?,
                    width: w,
                    height: h,
                },
                &mut out,
                u32_of(stride)?,
                range(color.range),
                matrix(color.matrix),
                YuvConversionMode::Balanced,
            )
            .map_err(failed)?;
        }
        PixelFormat::I420 => {
            let p = frame.planes();
            yuvutils_rs::yuv420_to_bgra(
                &YuvPlanarImage {
                    y_plane: &full_rows(frame, 0),
                    y_stride: u32_of(p[0].stride)?,
                    u_plane: &full_rows(frame, 1),
                    u_stride: u32_of(p[1].stride)?,
                    v_plane: &full_rows(frame, 2),
                    v_stride: u32_of(p[2].stride)?,
                    width: w,
                    height: h,
                },
                &mut out,
                u32_of(stride)?,
                range(color.range),
                matrix(color.matrix),
            )
            .map_err(failed)?;
        }
        PixelFormat::Yuyv => return to_bgra(&to_i420(frame, color)?),
    }
    CpuFrame::new(
        PixelFormat::Bgra,
        w,
        h,
        vec![tight(out, stride)],
        color,
        frame.timestamp(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color(range: Range) -> ColorSpace {
        ColorSpace {
            matrix: Matrix::Bt709,
            range,
        }
    }

    /// A BGRA frame with a different colour in each quarter and a gradient.
    fn bgra(w: u32, h: u32) -> CpuFrame {
        let stride = w as usize * 4;
        let mut data = vec![0; stride * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                let px = &mut data[y * stride + x * 4..][..4];
                let (b, g, r) = match (x * 2 / w as usize, y * 2 / h as usize) {
                    (0, 0) => (200, 40, 40),
                    (1, 0) => (40, 200, 40),
                    (0, _) => (40, 40, 200),
                    _ => ((x * 255 / w as usize) as u8, 128, 128),
                };
                px.copy_from_slice(&[b, g, r, 255]);
            }
        }
        CpuFrame::new(
            PixelFormat::Bgra,
            w,
            h,
            vec![tight(data, stride)],
            ColorSpace::default(),
            Duration::from_millis(7),
        )
        .unwrap()
    }

    fn max_diff(a: &CpuFrame, b: &CpuFrame) -> u8 {
        let (pa, pb) = (&a.planes()[0], &b.planes()[0]);
        let mut worst = 0;
        for y in 0..a.height() as usize {
            for x in 0..a.width() as usize * 4 {
                if x % 4 == 3 {
                    continue;
                }
                let d = pa.data[y * pa.stride + x].abs_diff(pb.data[y * pb.stride + x]);
                worst = worst.max(d);
            }
        }
        worst
    }

    #[test]
    fn nv12_and_i420_repack_without_loss() {
        let source = to_i420(&bgra(64, 32), color(Range::Limited)).unwrap();
        let nv12 = to_nv12(&source, color(Range::Full)).unwrap();
        assert_eq!(nv12.format(), PixelFormat::Nv12);
        // A YUV source keeps its own colour space.
        assert_eq!(nv12.color(), color(Range::Limited));
        assert_eq!(nv12.timestamp(), Duration::from_millis(7));
        assert_eq!(to_i420(&nv12, color(Range::Full)).unwrap(), source);
    }

    #[test]
    fn bgra_round_trips_through_nv12_and_i420_in_both_ranges() {
        let source = bgra(64, 32);
        for r in [Range::Limited, Range::Full] {
            let nv12 = to_nv12(&source, color(r)).unwrap();
            assert_eq!(nv12.color(), color(r));
            let i420 = to_i420(&source, color(r)).unwrap();
            // 4:2:0 shares chroma between 2 × 2 pixels; each quarter is one
            // colour on even boundaries, so only rounding is left, except
            // along the gradient.
            for back in [to_bgra(&nv12).unwrap(), to_bgra(&i420).unwrap()] {
                let d = max_diff(&source, &back);
                assert!(d <= 6, "{r:?}: off by {d}");
            }
        }
    }

    #[test]
    fn yuyv_becomes_4_2_0_with_its_values() {
        // Two pixels per 4 bytes: Y0 U Y1 V; two rows with the same chroma.
        let row = [100, 90, 110, 160, 120, 90, 130, 160];
        let yuyv = CpuFrame::new(
            PixelFormat::Yuyv,
            4,
            2,
            vec![tight([row, row].concat(), 8)],
            color(Range::Full),
            Duration::ZERO,
        )
        .unwrap();
        let i420 = to_i420(&yuyv, ColorSpace::default()).unwrap();
        assert_eq!(i420.color(), color(Range::Full));
        assert_eq!(i420.row(0, 0), [100, 110, 120, 130]);
        assert_eq!(i420.row(1, 0), [90, 90]);
        assert_eq!(i420.row(2, 0), [160, 160]);
        let nv12 = to_nv12(&yuyv, ColorSpace::default()).unwrap();
        assert_eq!(nv12.row(1, 0), [90, 160, 90, 160]);
    }

    #[test]
    fn padded_rows_are_read_by_their_stride() {
        let tight_frame = to_nv12(&bgra(16, 8), ColorSpace::default()).unwrap();
        let pad = |p: &Plane, rows: usize, row: usize| {
            let mut data = vec![0xEE; (row + 8) * rows];
            for r in 0..rows {
                data[r * (row + 8)..r * (row + 8) + row]
                    .copy_from_slice(&p.data[r * p.stride..r * p.stride + row]);
            }
            tight(data, row + 8)
        };
        let padded = CpuFrame::new(
            PixelFormat::Nv12,
            16,
            8,
            vec![
                pad(&tight_frame.planes()[0], 8, 16),
                pad(&tight_frame.planes()[1], 4, 16),
            ],
            tight_frame.color(),
            Duration::ZERO,
        )
        .unwrap();
        let a = to_i420(&tight_frame, ColorSpace::default()).unwrap();
        let b = to_i420(&padded, ColorSpace::default()).unwrap();
        assert_eq!(a.planes()[1], b.planes()[1]);
        assert_eq!(a.planes()[2], b.planes()[2]);
    }

    /// `frame` with each plane's rows 16 bytes apart more than they need,
    /// and the last row unpadded (the buffer ends with its pixels), as
    /// `CpuFrame::new` allows and a capture's buffer may end.
    fn last_row_unpadded(frame: &CpuFrame) -> CpuFrame {
        let planes = frame
            .planes()
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (row, rows) = frame.format().plane_shape(i, frame.width(), frame.height());
                let stride = row + 16;
                let mut data = vec![0xEE; stride * (rows - 1) + row];
                for r in 0..rows {
                    data[r * stride..r * stride + row]
                        .copy_from_slice(&p.data[r * p.stride..r * p.stride + row]);
                }
                Plane {
                    data: data.into(),
                    stride,
                }
            })
            .collect();
        CpuFrame::new(
            frame.format(),
            frame.width(),
            frame.height(),
            planes,
            frame.color(),
            frame.timestamp(),
        )
        .expect("a layout CpuFrame::new allows")
    }

    /// Plane contents, row by row, without the padding.
    fn rows_of(frame: &CpuFrame) -> Vec<Vec<u8>> {
        (0..frame.planes().len())
            .flat_map(|i| {
                let (_, rows) = frame.format().plane_shape(i, frame.width(), frame.height());
                (0..rows).map(move |r| frame.row(i, r).to_vec())
            })
            .collect()
    }

    #[test]
    fn an_unpadded_last_row_converts_like_a_padded_one() {
        let sent = ColorSpace::default();
        // Wide enough for the SIMD paths, odd row pairs included.
        let source = bgra(64, 6);
        let nv12 = to_nv12(&source, sent).unwrap();
        let i420 = to_i420(&source, sent).unwrap();
        let yuyv =
            CpuFrame::filled(PixelFormat::Yuyv, 64, 6, &[0x80], sent, Duration::ZERO).unwrap();
        type Convert = fn(&CpuFrame) -> CpuFrame;
        let cases: [(&str, &CpuFrame, Convert); 7] = [
            ("BGRA to NV12", &source, |f| {
                to_nv12(f, ColorSpace::default()).unwrap()
            }),
            ("BGRA to I420", &source, |f| {
                to_i420(f, ColorSpace::default()).unwrap()
            }),
            ("NV12 to BGRA", &nv12, |f| to_bgra(f).unwrap()),
            ("I420 to BGRA", &i420, |f| to_bgra(f).unwrap()),
            ("NV12 to I420", &nv12, |f| {
                to_i420(f, ColorSpace::default()).unwrap()
            }),
            ("I420 to NV12", &i420, |f| {
                to_nv12(f, ColorSpace::default()).unwrap()
            }),
            ("YUYV to I420", &yuyv, |f| {
                to_i420(f, ColorSpace::default()).unwrap()
            }),
        ];
        for (name, frame, convert) in cases {
            let expected = rows_of(&convert(frame));
            let got = rows_of(&convert(&last_row_unpadded(frame)));
            assert_eq!(got, expected, "{name}");
        }
    }
}
