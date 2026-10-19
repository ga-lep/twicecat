//! X11 screen capture (plan Task 8, Q9): the monitors (RandR) and the
//! windows the window manager lists (`_NET_CLIENT_LIST`), with thumbnails,
//! for our own picker; and a capture loop that reads a monitor's area of
//! the root window, or one window, through MIT-SHM.
//!
//! The shared segment is a file in `/dev/shm`, unlinked at once, that the
//! X server maps; we read it back with `pread` (one kernel copy, no
//! `mmap`, no `unsafe`). Without MIT-SHM (a remote display) plain
//! `GetImage` is used. The cursor is not drawn in, and a window that other
//! windows cover shows them: X11 sharing is best effort (plan Q9).
//!
//! Window titles are never logged.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState, Window,
};
use x11rb::rust_connection::RustConnection;

use super::{CaptureEnd, CaptureError, FrameSink};
use crate::{ColorSpace, CpuFrame, Matrix, PixelFormat, Plane, Range, convert};

/// What we send: BT.709, limited range.
const SENT: ColorSpace = ColorSpace {
    matrix: Matrix::Bt709,
    range: Range::Limited,
};

/// A monitor or a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X11SourceKind {
    /// An area of the root window.
    Monitor,
    /// One top-level window.
    Window,
}

/// Something [`X11Capture::start`] can capture.
#[derive(Clone, PartialEq, Eq)]
pub struct X11Source {
    /// Identifier for [`X11Capture::start`].
    pub id: String,
    /// A monitor or a window.
    pub kind: X11SourceKind,
    /// The monitor's name or the window's title.
    pub title: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// A small picture of it (BGRA), when one could be taken.
    pub thumbnail: Option<CpuFrame>,
}

impl std::fmt::Debug for X11Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Source")
            .field("kind", &self.kind)
            .field("title", &format_args!("<{} bytes>", self.title.len()))
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// What an identifier names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// An area of the root window.
    Area {
        x: i16,
        y: i16,
        w: u16,
        h: u16,
    },
    Window(Window),
}

fn monitor_id(x: i16, y: i16, w: u16, h: u16) -> String {
    format!("x11-monitor:{x},{y},{w}x{h}")
}

fn window_id(window: Window) -> String {
    format!("x11-window:{window}")
}

fn parse_id(id: &str) -> Option<Target> {
    if let Some(window) = id.strip_prefix("x11-window:") {
        return window.parse().ok().map(Target::Window);
    }
    let area = id.strip_prefix("x11-monitor:")?;
    let (pos, size) = area.split_once(',').and_then(|(x, rest)| {
        let (y, size) = rest.split_once(',')?;
        Some(((x, y), size))
    })?;
    let (w, h) = size.split_once('x')?;
    let target = Target::Area {
        x: pos.0.parse().ok()?,
        y: pos.1.parse().ok()?,
        w: w.parse().ok()?,
        h: h.parse().ok()?,
    };
    match target {
        Target::Area { w, h, .. } if w > 0 && h > 0 => Some(target),
        _ => None,
    }
}

/// A connection, its root window, and a check that its images are what we
/// read: 32 bits per pixel, little-endian (B, G, R, x).
struct Display {
    conn: RustConnection,
    root: Window,
}

impl Display {
    fn open() -> Result<Self, CaptureError> {
        let (conn, screen) =
            x11rb::connect(None).map_err(|_| CaptureError::Unavailable("no X server"))?;
        let setup = conn.setup();
        let screen = setup
            .roots
            .get(screen)
            .ok_or(CaptureError::Unavailable("no X screen"))?;
        let bpp = setup
            .pixmap_formats
            .iter()
            .find(|f| f.depth == screen.root_depth)
            .map(|f| f.bits_per_pixel);
        if bpp != Some(32) || setup.image_byte_order != ImageOrder::LSB_FIRST {
            return Err(CaptureError::Unavailable("an X screen we cannot read"));
        }
        let root = screen.root;
        Ok(Self { conn, root })
    }

    fn atom(&self, name: &[u8]) -> Option<u32> {
        let atom = self.conn.intern_atom(false, name).ok()?.reply().ok()?.atom;
        (atom != x11rb::NONE).then_some(atom)
    }

    fn text_property(&self, window: Window, property: u32, type_: u32) -> Option<String> {
        let reply = self
            .conn
            .get_property(false, window, property, type_, 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        let text = String::from_utf8_lossy(&reply.value).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    fn title(&self, window: Window) -> String {
        let utf8 = self.atom(b"UTF8_STRING");
        let net_name = self.atom(b"_NET_WM_NAME");
        if let (Some(name), Some(utf8)) = (net_name, utf8)
            && let Some(title) = self.text_property(window, name, utf8)
        {
            return title;
        }
        self.text_property(window, AtomEnum::WM_NAME.into(), AtomEnum::STRING.into())
            .unwrap_or_default()
    }

    fn monitors(&self) -> Vec<X11Source> {
        let Ok(Ok(reply)) = self
            .conn
            .randr_get_monitors(self.root, true)
            .map(|c| c.reply())
        else {
            return Vec::new();
        };
        reply
            .monitors
            .iter()
            .enumerate()
            .filter(|(_, m)| m.width > 0 && m.height > 0)
            .map(|(i, m)| {
                let name = self
                    .conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| format!("Screen {}", i + 1));
                X11Source {
                    id: monitor_id(m.x, m.y, m.width, m.height),
                    kind: X11SourceKind::Monitor,
                    title: name,
                    width: u32::from(m.width),
                    height: u32::from(m.height),
                    thumbnail: None,
                }
            })
            .collect()
    }

    fn windows(&self) -> Vec<X11Source> {
        let Some(list) = self.atom(b"_NET_CLIENT_LIST") else {
            return Vec::new();
        };
        let Ok(Ok(reply)) = self
            .conn
            .get_property(false, self.root, list, AtomEnum::WINDOW, 0, 4096)
            .map(|c| c.reply())
        else {
            return Vec::new();
        };
        let Some(windows) = reply.value32() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for window in windows {
            let viewable = self
                .conn
                .get_window_attributes(window)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|a| a.map_state == MapState::VIEWABLE);
            let Some(geometry) = self
                .conn
                .get_geometry(window)
                .ok()
                .and_then(|c| c.reply().ok())
            else {
                continue;
            };
            if !viewable || geometry.width < 2 || geometry.height < 2 {
                continue;
            }
            out.push(X11Source {
                id: window_id(window),
                kind: X11SourceKind::Window,
                title: self.title(window),
                width: u32::from(geometry.width),
                height: u32::from(geometry.height),
                thumbnail: None,
            });
        }
        out
    }

    /// Where `target` is now: the drawable and the area to read.
    fn area(&self, target: Target) -> Option<(Window, i16, i16, u16, u16)> {
        match target {
            Target::Area { x, y, w, h } => Some((self.root, x, y, w, h)),
            Target::Window(window) => {
                let g = self.conn.get_geometry(window).ok()?.reply().ok()?;
                Some((window, 0, 0, g.width, g.height))
            }
        }
    }

    /// One image with plain `GetImage` (thumbnails, and capture without
    /// MIT-SHM).
    fn image(&self, (drawable, x, y, w, h): (Window, i16, i16, u16, u16)) -> Option<CpuFrame> {
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, !0)
            .ok()?
            .reply()
            .ok()?;
        bgra_frame(
            reply.data.into(),
            u32::from(w),
            u32::from(h),
            Duration::ZERO,
        )
    }
}

/// `data` as a tightly packed BGRA frame of `w` × `h`.
fn bgra_frame(data: Arc<[u8]>, w: u32, h: u32, timestamp: Duration) -> Option<CpuFrame> {
    CpuFrame::new(
        PixelFormat::Bgra,
        w,
        h,
        vec![Plane {
            data,
            stride: w as usize * 4,
        }],
        SENT,
        timestamp,
    )
    .ok()
}

/// A BGRA frame scaled down (nearest pixel) to fit in `max`, opaque.
fn thumbnail(frame: &CpuFrame, max: (u32, u32)) -> Option<CpuFrame> {
    let (w, h) = super::fit_size((frame.width(), frame.height()), max);
    let (w, h) = (w.min(frame.width()), h.min(frame.height()));
    let plane = &frame.planes()[0];
    let mut out = vec![0_u8; w as usize * h as usize * 4];
    for ty in 0..h as usize {
        let sy = ty * frame.height() as usize / h as usize;
        for tx in 0..w as usize {
            let sx = tx * frame.width() as usize / w as usize;
            let src = &plane.data[sy * plane.stride + sx * 4..][..4];
            let dst = &mut out[(ty * w as usize + tx) * 4..][..4];
            dst.copy_from_slice(src);
            dst[3] = 255;
        }
    }
    bgra_frame(out.into(), w, h, Duration::ZERO)
}

/// Whether an X server answers here.
pub fn available() -> bool {
    Display::open().is_ok()
}

/// The monitors, then the windows, with thumbnails no larger than
/// `thumbnail_max` (taken for monitors and for windows that can be read).
///
/// # Errors
/// [`CaptureError::Unavailable`] without an X server we can read.
pub fn list_sources(thumbnail_max: (u32, u32)) -> Result<Vec<X11Source>, CaptureError> {
    let display = Display::open()?;
    let mut sources = display.monitors();
    sources.extend(display.windows());
    for source in &mut sources {
        let Some(target) = parse_id(&source.id) else {
            continue;
        };
        source.thumbnail = display
            .area(target)
            .and_then(|area| display.image(area))
            .and_then(|full| thumbnail(&full, thumbnail_max));
    }
    Ok(sources)
}

/// A running X11 capture; dropping it stops it and joins its thread.
pub struct X11Capture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    size: (u32, u32),
}

impl std::fmt::Debug for X11Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11Capture").finish_non_exhaustive()
    }
}

impl Drop for X11Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl X11Capture {
    /// Starts capturing the source `id` (from [`list_sources`]) at up to
    /// `max_fps`; frames go to `sink`, NV12 at the source's own size.
    ///
    /// # Errors
    /// [`CaptureError::NoSuchSource`] for an identifier that names nothing
    /// (any more), [`CaptureError::Unavailable`] without an X server.
    pub fn start(id: &str, max_fps: u32, sink: Box<dyn FrameSink>) -> Result<Self, CaptureError> {
        let target = parse_id(id).ok_or(CaptureError::NoSuchSource)?;
        let display = Display::open()?;
        let Some((_, _, _, w, h)) = display.area(target) else {
            return Err(CaptureError::NoSuchSource);
        };
        let stop = Arc::new(AtomicBool::new(false));
        let running = stop.clone();
        let interval = Duration::from_secs(1) / max_fps.max(1);
        let thread = std::thread::Builder::new()
            .name("twicecat-x11-capture".into())
            .spawn(move || capture(&display, target, interval, &running, sink))
            .map_err(|_| CaptureError::Failed("no thread for the capture"))?;
        Ok(Self {
            stop,
            thread: Some(thread),
            size: (u32::from(w), u32::from(h)),
        })
    }

    /// The source's size when the capture started, in pixels (a window
    /// may change it later; frames follow).
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// An MIT-SHM segment: a `/dev/shm` file the X server maps.
struct Segment {
    seg: shm::Seg,
    file: File,
    size: usize,
}

impl Segment {
    fn new(display: &Display, size: usize) -> Option<Self> {
        display
            .conn
            .extension_information(shm::X11_EXTENSION_NAME)
            .ok()??;
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = format!(
            "/dev/shm/twicecat-x11-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let file = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        // The server has its own reference once attached; nobody else
        // needs the name.
        let _ = std::fs::remove_file(&path);
        file.set_len(size as u64).ok()?;
        let seg = display.conn.generate_id().ok()?;
        let fd = file.try_clone().ok()?;
        display
            .conn
            .shm_attach_fd(seg, std::os::fd::OwnedFd::from(fd), false)
            .ok()?
            .check()
            .ok()?;
        Some(Self { seg, file, size })
    }

    /// Reads `(drawable, x, y, w, h)` into the segment and copies it out.
    fn read(&self, display: &Display, area: (Window, i16, i16, u16, u16)) -> Option<Arc<[u8]>> {
        let (drawable, x, y, w, h) = area;
        let len = usize::from(w) * usize::from(h) * 4;
        if len > self.size {
            return None;
        }
        display
            .conn
            .shm_get_image(
                drawable,
                x,
                y,
                w,
                h,
                !0,
                ImageFormat::Z_PIXMAP.into(),
                self.seg,
                0,
            )
            .ok()?
            .reply()
            .ok()?;
        let mut out = vec![0_u8; len];
        self.file.read_exact_at(&mut out, 0).ok()?;
        Some(out.into())
    }

    fn detach(self, display: &Display) {
        let _ = display.conn.shm_detach(self.seg);
        let _ = display.conn.flush();
    }
}

fn capture(
    display: &Display,
    target: Target,
    interval: Duration,
    stop: &AtomicBool,
    mut sink: Box<dyn FrameSink>,
) {
    let started = Instant::now();
    let mut segment: Option<Segment> = None;
    let mut shm_ok = true;
    let mut next = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let Some(area) = display.area(target) else {
            // The window is gone (closed).
            sink.ended(CaptureEnd::Stopped);
            break;
        };
        let (_, _, _, w, h) = area;
        let len = usize::from(w) * usize::from(h) * 4;
        if shm_ok && segment.as_ref().is_none_or(|s| s.size < len) {
            if let Some(old) = segment.take() {
                old.detach(display);
            }
            segment = Segment::new(display, len);
            shm_ok = segment.is_some();
        }
        let timestamp = started.elapsed();
        let frame = match &segment {
            Some(segment) => segment
                .read(display, area)
                .and_then(|data| bgra_frame(data, u32::from(w), u32::from(h), timestamp)),
            None => display.image(area),
        };
        // A window that cannot be read just now (being resized, unmapped)
        // gives no frame: try again next time.
        if let Some(Ok(nv12)) = frame.map(|f| convert::to_nv12(&f, SENT)) {
            sink.frame(nv12);
        }
        next += interval;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    if let Some(segment) = segment {
        segment.detach(display);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_round_trip_and_bad_ones_name_nothing() {
        assert_eq!(
            parse_id(&monitor_id(-1920, 0, 1920, 1080)),
            Some(Target::Area {
                x: -1920,
                y: 0,
                w: 1920,
                h: 1080
            })
        );
        assert_eq!(
            parse_id(&window_id(0x0420_0007)),
            Some(Target::Window(0x0420_0007))
        );
        for bad in [
            "",
            "x11-window:",
            "x11-window:abc",
            "x11-monitor:0,0,0x1080",
            "x11-monitor:0,0",
            "portal",
            "test-pattern",
        ] {
            assert_eq!(parse_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn thumbnails_keep_the_shape_and_are_opaque() {
        let mut data = vec![0_u8; 8 * 4 * 4];
        // The right half blue (B first), alpha left at 0.
        for y in 0..4 {
            for x in 4..8 {
                data[(y * 8 + x) * 4] = 255;
            }
        }
        let frame = bgra_frame(data.into(), 8, 4, Duration::ZERO).expect("frame");
        let small = thumbnail(&frame, (4, 4)).expect("thumbnail");
        assert_eq!((small.width(), small.height()), (4, 2));
        let px = &small.planes()[0].data;
        assert_eq!(&px[..4], &[0, 0, 0, 255]);
        assert_eq!(&px[3 * 4..4 * 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn debug_hides_titles() {
        let source = X11Source {
            id: window_id(1),
            kind: X11SourceKind::Window,
            title: "Secret plans".into(),
            width: 10,
            height: 10,
            thumbnail: None,
        };
        assert!(!format!("{source:?}").contains("Secret"));
    }

    /// Frames and the end to channels.
    struct ToChannel(
        std::sync::mpsc::Sender<CpuFrame>,
        std::sync::mpsc::Sender<CaptureEnd>,
    );

    impl FrameSink for ToChannel {
        fn frame(&mut self, frame: CpuFrame) {
            let _ = self.0.send(frame);
        }

        fn ended(&mut self, end: CaptureEnd) {
            let _ = self.1.send(end);
        }
    }

    #[test]
    #[ignore = "needs an X server (DISPLAY; Xwayland will do); shows a small red window for a second"]
    fn an_x11_window_is_captured_with_its_content_and_its_close_ends_the_capture() {
        use x11rb::protocol::xproto::{CreateWindowAux, WindowClass};
        use x11rb::wrapper::ConnectionExt as _;

        // Listing works (a rootless Xwayland cannot read monitors: no
        // thumbnails there, which a real X11 session has).
        list_sources((320, 180)).expect("an X server");

        // Our own window, filled red by the server (its background).
        let (conn, screen) = x11rb::connect(None).expect("an X server");
        let root = &conn.setup().roots[screen];
        let window = conn.generate_id().expect("id");
        conn.create_window(
            root.root_depth,
            window,
            root.root,
            0,
            0,
            320,
            240,
            0,
            WindowClass::INPUT_OUTPUT,
            root.root_visual,
            &CreateWindowAux::new().background_pixel(0x00FF_0000),
        )
        .expect("create")
        .check()
        .expect("created");
        conn.map_window(window).expect("map");
        conn.sync().expect("sync");
        std::thread::sleep(Duration::from_millis(500));

        let (frames_tx, frames) = std::sync::mpsc::channel();
        let (ended_tx, ended) = std::sync::mpsc::channel();
        let capture = X11Capture::start(
            &window_id(window),
            10,
            Box::new(ToChannel(frames_tx, ended_tx)),
        )
        .expect("capture");
        assert_eq!(capture.size(), (320, 240));
        let mut got = 0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while got < 5 && Instant::now() < deadline {
            if let Ok(frame) = frames.recv_timeout(Duration::from_millis(500)) {
                assert_eq!(frame.format(), PixelFormat::Nv12);
                assert_eq!((frame.width(), frame.height()), (320, 240));
                // Red in BT.709 limited range: Y 63, U 102, V 240.
                let (y, uv) = (&frame.planes()[0], &frame.planes()[1]);
                let centre = 120 * y.stride + 160;
                assert!(y.data[centre].abs_diff(63) <= 3, "Y {}", y.data[centre]);
                let c = 60 * uv.stride + 160;
                assert!(uv.data[c].abs_diff(102) <= 4, "U {}", uv.data[c]);
                assert!(uv.data[c + 1].abs_diff(240) <= 4, "V {}", uv.data[c + 1]);
                got += 1;
            }
        }
        assert!(got >= 5, "only {got} frames in 3 s");

        // Closing the window ends the capture, as the person stopping it.
        conn.destroy_window(window).expect("destroy");
        conn.sync().expect("sync");
        assert_eq!(
            ended.recv_timeout(Duration::from_secs(2)),
            Ok(CaptureEnd::Stopped)
        );
        drop(capture);

        // A window that does not exist cannot be captured.
        let (tx, _) = std::sync::mpsc::channel();
        let (end_tx, _) = std::sync::mpsc::channel();
        assert!(matches!(
            X11Capture::start(&window_id(window), 10, Box::new(ToChannel(tx, end_tx))),
            Err(CaptureError::NoSuchSource)
        ));
    }
}
