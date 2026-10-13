//! A synthetic video source for tests and measurements: moving content
//! (a scrolling gradient, moving chroma and a square that crosses the
//! picture, so that every frame differs and an encoder has motion to code),
//! and a stamp at the top that carries the frame's counter and its send
//! time.
//!
//! The stamp is 96 bits in 3 rows of 32 square blocks across the whole
//! width (block side = width / 32): the counter (32 bits), the send time in
//! microseconds (48 bits) and a CRC-16 of both. A bit is a block of white
//! (Y 235) or black (Y 16) with neutral chroma, large enough to survive a
//! lossy encoder and scaling down to a simulcast layer: the stamp is found
//! from the frame's own width, so a 1280 × 720 pattern still decodes at
//! 320 × 180.

use std::time::Duration;

use crate::frame::{ColorSpace, CpuFrame, PixelFormat, Plane};

/// Blocks per stamp row.
const COLUMNS: usize = 32;
/// Stamp rows.
const ROWS: usize = 3;
const BLACK: u8 = 16;
const WHITE: u8 = 235;
/// Luma period of the scrolling gradient.
const PERIOD: usize = 220;

/// What a frame's stamp says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    /// The frame's number since the pattern started.
    pub counter: u32,
    /// When it was sent, on the sender's clock (microseconds, 48 bits).
    pub sent: Duration,
}

/// Makes NV12 test frames (BT.709, limited range) of one size.
#[derive(Clone, Debug)]
pub struct TestPattern {
    width: u32,
    height: u32,
    /// The gradient, twice over, so a row is one slice of it.
    luma: Vec<u8>,
}

impl TestPattern {
    /// A pattern of `width` × `height`, rounded down to even (4:2:0), at
    /// least 64 wide and tall enough for the stamp (3 / 32 of the width).
    pub fn new(width: u32, height: u32) -> Self {
        let width = (width & !1).max(64);
        let stamp_rows = (ROWS * block(width as usize)) as u32;
        let height = (height & !1).max(stamp_rows);
        let luma = (0..width as usize + PERIOD)
            .map(|x| BLACK + (x % PERIOD) as u8)
            .collect();
        Self {
            width,
            height,
            luma,
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Frame `counter`, stamped as sent at `sent`; `sent` is also its
    /// timestamp.
    pub fn frame(&self, counter: u32, sent: Duration) -> CpuFrame {
        let (w, h) = (self.width as usize, self.height as usize);
        let n = counter as usize;
        let mut y_plane = vec![0; w * h];
        for (y, row) in y_plane.chunks_exact_mut(w).enumerate() {
            let start = (2 * y + 4 * n) % PERIOD;
            row.copy_from_slice(&self.luma[start..start + w]);
        }
        // The square: a twelfth of the width, crossing in 4 s at 60 fps.
        let side = (w / 12).max(2);
        let travel = w - side;
        let left = (n * 4 * travel / 240) % travel.max(1);
        let top = (h / 2).saturating_sub(side / 2);
        for row in y_plane.chunks_exact_mut(w).skip(top).take(side) {
            row[left..left + side].fill(WHITE);
        }
        let mut uv = vec![0; w * (h / 2)];
        for (y, row) in uv.chunks_exact_mut(w).enumerate() {
            let u = 64 + ((y + n) % 128) as u8;
            for (x, pair) in row.chunks_exact_mut(2).enumerate() {
                pair[0] = u;
                pair[1] = 192 - ((x + 2 * n) % 128) as u8;
            }
        }
        stamp(&mut y_plane, &mut uv, w, h, counter, sent);
        CpuFrame::new(
            PixelFormat::Nv12,
            self.width,
            self.height,
            vec![
                Plane {
                    data: y_plane.into(),
                    stride: w,
                },
                Plane {
                    data: uv.into(),
                    stride: w,
                },
            ],
            ColorSpace::default(),
            sent,
        )
        .expect("the pattern's planes match its size")
    }
}

/// The side of a stamp block in a frame `width` pixels wide.
fn block(width: usize) -> usize {
    (width / COLUMNS).max(2) & !1
}

fn bits(counter: u32, sent: Duration) -> [bool; COLUMNS * ROWS] {
    let micros = (sent.as_micros() as u64) & ((1 << 48) - 1);
    let mut payload = [0u8; 10];
    payload[..4].copy_from_slice(&counter.to_be_bytes());
    payload[4..].copy_from_slice(&micros.to_be_bytes()[2..]);
    let crc = crc16(&payload);
    let mut all = [0u8; 12];
    all[..10].copy_from_slice(&payload);
    all[10..].copy_from_slice(&crc.to_be_bytes());
    std::array::from_fn(|i| all[i / 8] & (0x80 >> (i % 8)) != 0)
}

fn stamp(y_plane: &mut [u8], uv: &mut [u8], w: usize, h: usize, counter: u32, sent: Duration) {
    let b = block(w);
    if b * ROWS > h {
        return;
    }
    for (i, bit) in bits(counter, sent).iter().enumerate() {
        let (col, row) = (i % COLUMNS, i / COLUMNS);
        let value = if *bit { WHITE } else { BLACK };
        for line in row * b..(row + 1) * b {
            y_plane[line * w + col * b..line * w + (col + 1) * b].fill(value);
        }
    }
    // Neutral chroma under the stamp.
    uv[..w * (ROWS * b / 2)].fill(128);
}

/// Reads the stamp of a test-pattern frame (NV12 or I420, any size the
/// pattern was scaled to). `None` when there is none or it is damaged.
pub fn read_stamp(frame: &CpuFrame) -> Option<Stamp> {
    if !matches!(frame.format(), PixelFormat::Nv12 | PixelFormat::I420) {
        return None;
    }
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let b = block(w);
    if b * ROWS > h {
        return None;
    }
    // The middle half of each block: away from its edges, which an encoder
    // and a scaler blur.
    let (from, to) = (b / 4, (3 * b / 4).max(b / 4 + 1));
    let mut bytes = [0u8; 12];
    for i in 0..COLUMNS * ROWS {
        let (col, row) = (i % COLUMNS, i / COLUMNS);
        let mut sum = 0usize;
        for line in row * b + from..row * b + to {
            sum += frame.row(0, line)[col * b + from..col * b + to]
                .iter()
                .map(|v| *v as usize)
                .sum::<usize>();
        }
        let mean = sum / ((to - from) * (to - from));
        if mean > (BLACK as usize + WHITE as usize) / 2 {
            bytes[i / 8] |= 0x80 >> (i % 8);
        }
    }
    let crc = u16::from_be_bytes([bytes[10], bytes[11]]);
    if crc16(&bytes[..10]) != crc {
        return None;
    }
    let counter = u32::from_be_bytes(bytes[..4].try_into().ok()?);
    let mut micros = [0u8; 8];
    micros[2..].copy_from_slice(&bytes[4..10]);
    Some(Stamp {
        counter,
        sent: Duration::from_micros(u64::from_be_bytes(micros)),
    })
}

/// CRC-16/CCITT-FALSE.
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFF_u16;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frame` scaled down by `factor` (box filter on both planes), as an
    /// SFU's lower simulcast layer would be.
    fn shrink(frame: &CpuFrame, factor: usize) -> CpuFrame {
        let (w, h) = (frame.width() as usize, frame.height() as usize);
        let (sw, sh) = ((w / factor) & !1, (h / factor) & !1);
        let mut y = vec![0; sw * sh];
        for oy in 0..sh {
            for ox in 0..sw {
                let mut sum = 0usize;
                for dy in 0..factor {
                    sum += frame.row(0, oy * factor + dy)[ox * factor..(ox + 1) * factor]
                        .iter()
                        .map(|v| *v as usize)
                        .sum::<usize>();
                }
                y[oy * sw + ox] = (sum / (factor * factor)) as u8;
            }
        }
        CpuFrame::new(
            PixelFormat::Nv12,
            sw as u32,
            sh as u32,
            vec![
                Plane {
                    data: y.into(),
                    stride: sw,
                },
                Plane {
                    data: vec![128; sw * sh / 2].into(),
                    stride: sw,
                },
            ],
            frame.color(),
            frame.timestamp(),
        )
        .unwrap()
    }

    #[test]
    fn the_stamp_reads_back() {
        let pattern = TestPattern::new(1280, 720);
        for (counter, micros) in [(0, 0), (1, 16_667), (u32::MAX, (1 << 48) - 1), (77, 123)] {
            let sent = Duration::from_micros(micros);
            let frame = pattern.frame(counter, sent);
            assert_eq!(frame.timestamp(), sent);
            assert_eq!(read_stamp(&frame), Some(Stamp { counter, sent }));
        }
    }

    #[test]
    fn the_stamp_survives_noise_and_scaling() {
        let pattern = TestPattern::new(1280, 720);
        let sent = Duration::from_micros(987_654_321);
        let frame = pattern.frame(4242, sent);
        // ±40 on every luma value, as a rough encoder would leave.
        let noisy_y: Vec<u8> = frame.planes()[0]
            .data
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let n = (i * 7919 % 81) as i16 - 40;
                (*v as i16 + n).clamp(0, 255) as u8
            })
            .collect();
        let noisy = CpuFrame::new(
            PixelFormat::Nv12,
            1280,
            720,
            vec![
                Plane {
                    data: noisy_y.into(),
                    stride: 1280,
                },
                frame.planes()[1].clone(),
            ],
            frame.color(),
            sent,
        )
        .unwrap();
        let expected = Some(Stamp {
            counter: 4242,
            sent,
        });
        assert_eq!(read_stamp(&noisy), expected);
        assert_eq!(read_stamp(&shrink(&frame, 2)), expected);
        assert_eq!(read_stamp(&shrink(&frame, 4)), expected);
        let i420 = crate::convert::to_i420(&frame, ColorSpace::default()).unwrap();
        assert_eq!(read_stamp(&i420), expected);
    }

    #[test]
    fn a_damaged_or_missing_stamp_reads_as_none() {
        let frame = TestPattern::new(640, 360).frame(5, Duration::from_millis(9));
        let mut y = frame.planes()[0].data.to_vec();
        // Flip one block of the first row.
        let b = block(640);
        for line in 0..b {
            for x in 0..b {
                y[line * 640 + x] = 255 - y[line * 640 + x];
            }
        }
        let damaged = CpuFrame::new(
            PixelFormat::Nv12,
            640,
            360,
            vec![
                Plane {
                    data: y.into(),
                    stride: 640,
                },
                frame.planes()[1].clone(),
            ],
            frame.color(),
            frame.timestamp(),
        )
        .unwrap();
        assert_eq!(read_stamp(&damaged), None);
        let gray = CpuFrame::filled(
            PixelFormat::Nv12,
            640,
            360,
            &[128, 128],
            ColorSpace::default(),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(read_stamp(&gray), None);
    }

    #[test]
    fn consecutive_frames_differ_outside_the_stamp() {
        let pattern = TestPattern::new(320, 180);
        let (a, b) = (
            pattern.frame(1, Duration::ZERO),
            pattern.frame(2, Duration::ZERO),
        );
        let below = 3 * block(320) * 320;
        assert_ne!(a.planes()[0].data[below..], b.planes()[0].data[below..]);
        assert_ne!(a.planes()[1].data, b.planes()[1].data);
    }

    #[test]
    fn sizes_are_made_even_and_large_enough() {
        let p = TestPattern::new(641, 7);
        assert_eq!((p.width(), p.height()), (640, 60));
        let p = TestPattern::new(10, 2);
        assert_eq!((p.width(), p.height()), (64, 6));
        assert!(read_stamp(&p.frame(3, Duration::ZERO)).is_some());
    }

    #[test]
    fn crc_matches_the_reference_value() {
        // CRC-16/CCITT-FALSE of "123456789".
        assert_eq!(crc16(b"123456789"), 0x29B1);
    }
}
