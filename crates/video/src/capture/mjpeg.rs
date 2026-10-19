//! Motion-JPEG from cameras (plan Task 9, Q10): each frame decoded by
//! `zune-jpeg` to BGRA, then converted to NV12 (BT.709, limited range).
//!
//! A 4K frame takes one core about 27 ms to decode, more than a 60 fps
//! frame's 16.7 ms, so [`MjpegPool`] keeps two frames in flight on two
//! threads and hands them on in order. A frame that comes while both are
//! busy is dropped: latency never builds up behind a slow decoder.
//!
//! Many cameras leave the Huffman tables out of their frames, as Motion-JPEG
//! allows (it fixes them to the JPEG standard's, Annex K.3). zune-jpeg fills
//! those in only for frames marked as AVI Motion-JPEG (an `AVI1` APP0
//! segment, `decoder.rs:583`, `misc.rs:291`), so a frame without tables is
//! given that mark first ([`marked`]).
//!
//! JPEG data comes from the camera and is untrusted: a frame that does not
//! decode is skipped (zune-jpeg is pure Rust; its limits are its own).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use zune_jpeg::JpegDecoder;
use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::colorspace::ColorSpace as ZColorSpace;
use zune_jpeg::zune_core::options::DecoderOptions;

use crate::{ColorSpace, CpuFrame, Matrix, PixelFormat, Plane, Range, VideoError, convert};

/// What we send: BT.709, limited range.
const SENT: ColorSpace = ColorSpace {
    matrix: Matrix::Bt709,
    range: Range::Limited,
};

/// Frames decoded at once.
pub const WORKERS: usize = 2;

/// The largest side we decode: 8K. A camera's header that claims more is
/// refused before any buffer is made for it.
const MAX_SIDE: usize = 8192;

/// The `AVI1` APP0 segment: marker, length 7 (with itself), `AVI1\0`.
const AVI1: [u8; 9] = [0xFF, 0xE0, 0x00, 0x07, b'A', b'V', b'I', b'1', 0];

/// `jpeg`, marked as AVI Motion-JPEG after its start when no Huffman table
/// (DHT segment) comes before its scan, so the standard tables are used.
fn marked(jpeg: &[u8]) -> Cow<'_, [u8]> {
    if jpeg.get(..2) != Some(&[0xFF, 0xD8]) {
        return Cow::Borrowed(jpeg);
    }
    let mut i = 2;
    while let Some(&[0xFF, marker]) = jpeg.get(i..i + 2) {
        match marker {
            // A table: nothing to do.
            0xC4 => return Cow::Borrowed(jpeg),
            // The scan: no table before it.
            0xDA => break,
            // Fill bytes before a marker.
            0xFF => i += 1,
            // Markers without a length.
            0x01 | 0xD0..=0xD7 => i += 2,
            _ => {
                let Some(&[hi, lo]) = jpeg.get(i + 2..i + 4) else {
                    return Cow::Borrowed(jpeg);
                };
                i += 2 + usize::from(u16::from_be_bytes([hi, lo]));
            }
        }
    }
    let mut out = Vec::with_capacity(jpeg.len() + AVI1.len());
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&AVI1);
    out.extend_from_slice(&jpeg[2..]);
    Cow::Owned(out)
}

/// Decodes one JPEG frame to NV12.
///
/// # Errors
/// [`VideoError::InvalidBitstream`] for data that is not a JPEG image we
/// can decode (or one larger than 8K).
pub fn decode(bytes: &[u8], timestamp: Duration) -> Result<CpuFrame, VideoError> {
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ZColorSpace::BGRA)
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE);
    let bytes = marked(bytes);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(&*bytes), options);
    decoder
        .decode_headers()
        .map_err(|_| VideoError::InvalidBitstream("not a JPEG frame"))?;
    let info = decoder
        .info()
        .ok_or(VideoError::InvalidBitstream("not a JPEG frame"))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let pixels = decoder
        .decode()
        .map_err(|_| VideoError::InvalidBitstream("a JPEG frame that does not decode"))?;
    let bgra = CpuFrame::new(
        PixelFormat::Bgra,
        width,
        height,
        vec![Plane {
            data: pixels.into(),
            stride: width as usize * 4,
        }],
        SENT,
        timestamp,
    )?;
    convert::to_nv12(&bgra, SENT)
}

struct Job {
    seq: u64,
    bytes: Vec<u8>,
    timestamp: Duration,
}

/// Decoded frames waiting for the ones before them.
struct Reorder {
    next: u64,
    done: BTreeMap<u64, Option<CpuFrame>>,
}

/// Where decoded frames go, in order.
pub(crate) type Deliver = Box<dyn FnMut(CpuFrame) + Send>;

/// Two decoders and the order they hand frames on in.
pub(crate) struct MjpegPool {
    jobs: Option<mpsc::Sender<Job>>,
    in_flight: Arc<AtomicUsize>,
    next_seq: u64,
    workers: Vec<JoinHandle<()>>,
    /// Frames dropped because both decoders were busy.
    dropped: u64,
}

impl MjpegPool {
    /// Starts the decoders; frames go to `deliver`, in the order submitted.
    pub(crate) fn new(deliver: Deliver) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let in_flight = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Mutex::new((
            Reorder {
                next: 0,
                done: BTreeMap::new(),
            },
            deliver,
        )));
        let workers = (0..WORKERS)
            .filter_map(|i| {
                let rx = rx.clone();
                let shared = shared.clone();
                let in_flight = in_flight.clone();
                std::thread::Builder::new()
                    .name(format!("twicecat-mjpeg-{i}"))
                    .spawn(move || work(&rx, &shared, &in_flight))
                    .ok()
            })
            .collect();
        Self {
            jobs: Some(tx),
            in_flight,
            next_seq: 0,
            workers,
            dropped: 0,
        }
    }

    /// Queues a frame; `false` when both decoders are busy and it is
    /// dropped.
    pub(crate) fn submit(&mut self, bytes: Vec<u8>, timestamp: Duration) -> bool {
        if self.workers.is_empty() || self.in_flight.load(Ordering::Acquire) >= self.workers.len() {
            self.dropped += 1;
            return false;
        }
        let Some(jobs) = &self.jobs else {
            return false;
        };
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let job = Job {
            seq: self.next_seq,
            bytes,
            timestamp,
        };
        self.next_seq += 1;
        if jobs.send(job).is_err() {
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }
}

impl Drop for MjpegPool {
    fn drop(&mut self) {
        if self.dropped > 0 {
            tracing::debug!(
                dropped = self.dropped,
                "camera frames dropped while both MJPEG decoders were busy"
            );
        }
        // Closing the queue ends the workers once their frame is done.
        self.jobs.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn work(
    jobs: &Mutex<mpsc::Receiver<Job>>,
    shared: &Mutex<(Reorder, Deliver)>,
    in_flight: &AtomicUsize,
) {
    loop {
        let job = {
            let Ok(rx) = jobs.lock() else {
                return;
            };
            match rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };
        let frame = decode(&job.bytes, job.timestamp).ok();
        let Ok(mut shared) = shared.lock() else {
            return;
        };
        let (reorder, deliver) = &mut *shared;
        reorder.done.insert(job.seq, frame);
        while let Some(frame) = reorder.done.remove(&reorder.next) {
            reorder.next += 1;
            in_flight.fetch_sub(1, Ordering::AcqRel);
            if let Some(frame) = frame {
                deliver(frame);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use twicecat_perf::{Expect, record};

    use super::*;

    const SMALL: &[u8] = include_bytes!("../../tests/fixtures/camera-360p.jpg");
    const LARGE: &[u8] = include_bytes!("../../tests/fixtures/camera-2160p.jpg");

    /// `jpeg` without its Huffman tables (DHT segments), as many cameras
    /// send MJPEG: the decoder must use the standard ones.
    fn without_huffman_tables(jpeg: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(jpeg.len());
        let mut i = 0;
        while i + 4 <= jpeg.len() {
            if jpeg[i] == 0xFF && jpeg[i + 1] == 0xDA {
                // Start of scan: the rest is entropy-coded data.
                break;
            }
            if jpeg[i] == 0xFF && (jpeg[i + 1] == 0xD8 || jpeg[i + 1] == 0x01) {
                out.extend_from_slice(&jpeg[i..i + 2]);
                i += 2;
                continue;
            }
            let len = usize::from(u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]));
            if jpeg[i + 1] != 0xC4 {
                out.extend_from_slice(&jpeg[i..i + 2 + len]);
            }
            i += 2 + len;
        }
        out.extend_from_slice(&jpeg[i..]);
        out
    }

    #[test]
    fn a_camera_frame_decodes_to_nv12_at_its_size() {
        let frame = decode(SMALL, Duration::from_millis(3)).expect("decodes");
        assert_eq!(frame.format(), PixelFormat::Nv12);
        assert_eq!((frame.width(), frame.height()), (640, 360));
        assert_eq!(frame.timestamp(), Duration::from_millis(3));
        // The test pattern is colourful, not one flat value.
        let y = &frame.planes()[0].data;
        let (min, max) = (y.iter().min().unwrap(), y.iter().max().unwrap());
        assert!(max - min > 100, "{min}..{max}");
    }

    #[test]
    fn a_frame_without_huffman_tables_decodes_the_same() {
        let stripped = without_huffman_tables(SMALL);
        assert!(stripped.len() < SMALL.len(), "the tables were removed");
        assert_eq!(
            decode(&stripped, Duration::ZERO).expect("decodes"),
            decode(SMALL, Duration::ZERO).expect("decodes")
        );
    }

    #[test]
    fn only_frames_without_tables_are_marked() {
        assert!(matches!(marked(SMALL), Cow::Borrowed(_)));
        let stripped = without_huffman_tables(SMALL);
        let Cow::Owned(marked) = marked(&stripped) else {
            panic!("not marked");
        };
        assert_eq!(&marked[2..11], &AVI1);
        assert_eq!(&marked[11..], &stripped[2..]);
        // Not a JPEG, or one cut short: left as it is.
        assert!(matches!(super::marked(b"nope"), Cow::Borrowed(_)));
        assert!(matches!(super::marked(&SMALL[..5]), Cow::Borrowed(_)));
    }

    #[test]
    fn broken_frames_are_errors_not_panics() {
        assert!(decode(&[], Duration::ZERO).is_err());
        assert!(decode(b"not a jpeg", Duration::ZERO).is_err());
        for cut in [2, 20, 200, 2000] {
            let _ = decode(&SMALL[..cut], Duration::ZERO);
        }
    }

    #[test]
    fn the_pool_hands_frames_on_in_order_and_drops_when_both_are_busy() {
        let (tx, rx) = mpsc::channel();
        let mut pool = MjpegPool::new(Box::new(move |frame: CpuFrame| {
            let _ = tx.send(frame.timestamp());
        }));
        let mut sent = Vec::new();
        for i in 0..40_u64 {
            let at = Duration::from_millis(i);
            if pool.submit(SMALL.to_vec(), at) {
                sent.push(at);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(pool);
        let got: Vec<Duration> = rx.try_iter().collect();
        assert_eq!(got, sent, "every frame queued comes out, in order");
        assert!(!got.is_empty());
    }

    /// The plan's bar: 4K MJPEG at 60 fps or more with two decoders (and
    /// the conversion to NV12), measured on the machine that runs it. An
    /// indicator, not a gate. The bar holds for a release build only:
    /// zune-jpeg's decoder is generic over its reader, so it is compiled
    /// with this crate, unoptimised in a debug build (about 6 fps), which
    /// is reported and decodes fewer frames.
    #[test]
    fn perf_two_decoders_keep_up_with_4k_at_60_fps() {
        let (frames, expect) = if cfg!(debug_assertions) {
            (12, Expect::Report)
        } else {
            (240, Expect::AtLeast(60.0))
        };
        let (tx, rx) = mpsc::channel();
        let mut pool = MjpegPool::new(Box::new(move |_frame: CpuFrame| {
            let _ = tx.send(());
        }));
        let start = Instant::now();
        let mut queued = 0;
        while queued < frames {
            if pool.submit(LARGE.to_vec(), Duration::ZERO) {
                queued += 1;
            } else {
                std::thread::sleep(Duration::from_micros(200));
            }
        }
        drop(pool);
        let elapsed = start.elapsed();
        assert_eq!(rx.try_iter().count(), frames);
        let fps = frames as f64 / elapsed.as_secs_f64();
        record("video.mjpeg.4k_two_decoders", fps, "fps", expect);
    }
}
