//! A PipeWire video stream we read frames from (plan Task 8): the stream of
//! a ScreenCast portal session, or any video node (tests).
//!
//! Negotiation: we offer DMA-BUFs in the layouts our Vulkan device imports
//! (one format pod per pixel format, with its modifiers, which the
//! producer may leave for us to fixate), then shared memory (BGRx, BGRA).
//! A DMA-BUF is waited on (its implicit fence), imported and converted to
//! NV12 on the GPU, scaled to fit the capture's size; a buffer that fails to
//! import makes us renegotiate shared memory once. Shared memory is copied
//! and converted on the CPU at its own size.
//!
//! The stream's callbacks run on our own thread's PipeWire main loop (not
//! the real-time data loop: `RT_PROCESS` is not set), so converting there
//! only delays the next frame, never the graph.
//!
//! `unsafe`: one FFI call, `poll(2)` on a DMA-BUF descriptor PipeWire
//! keeps open while we hold its buffer, with a stack array that outlives
//! the call.

use std::io::Cursor;
use std::os::fd::{BorrowedFd, OwnedFd, RawFd};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::context::ContextRc;
use pw::main_loop::MainLoopRc;
use pw::properties::PropertiesBox;
use pw::spa::buffer::{ChunkFlags, DataType};
use pw::spa::param::ParamType;
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::format_utils::parse_format;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::deserialize::PodDeserializer;
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, PropertyFlags, Value};
use pw::spa::utils::{
    Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Id, Rectangle, SpaTypes,
};
use pw::stream::{StreamFlags, StreamRc, StreamState};

use super::{CaptureEnd, CaptureError, FrameSink, fit_size};
use crate::gpu::{self, Converter, GpuDevice};
use crate::{
    ColorSpace, CpuFrame, DmaBufFrame, DmaBufPlane, Frame, Matrix, PixelFormat, Plane, Range,
    convert,
};

/// What we send: BT.709, limited range.
const SENT: ColorSpace = ColorSpace {
    matrix: Matrix::Bt709,
    range: Range::Limited,
};

/// How long a DMA-BUF's producer may take to finish writing it.
const FENCE_TIMEOUT_MS: i32 = 50;

/// The shared-memory layouts we take (what screen casts offer, B first in
/// memory so the CPU converter takes them as they are).
const SHM_FORMATS: [VideoFormat; 2] = [VideoFormat::BGRx, VideoFormat::BGRA];

/// The DMA-BUF layouts we import, with their DRM fourcc.
fn dma_formats() -> [(VideoFormat, u32); 4] {
    [
        (VideoFormat::BGRx, gpu::XRGB8888),
        (VideoFormat::BGRA, gpu::ARGB8888),
        (VideoFormat::RGBx, gpu::XBGR8888),
        (VideoFormat::RGBA, gpu::ABGR8888),
    ]
}

fn fourcc_of(format: VideoFormat) -> Option<u32> {
    dma_formats()
        .into_iter()
        .find(|(f, _)| *f == format)
        .map(|(_, fourcc)| fourcc)
}

/// How to open a stream.
pub(crate) struct StreamOptions {
    /// The PipeWire remote to connect to (a portal session's); the
    /// session's own daemon when `None`.
    pub remote: Option<OwnedFd>,
    /// The node to read; whatever the session manager links when `None`.
    pub node: Option<u32>,
    /// Frames larger than this are scaled down to fit (GPU path only).
    pub max_size: (u32, u32),
    /// Frames beyond this rate are dropped.
    pub max_fps: u32,
    /// Offer DMA-BUFs (imported on our Vulkan device) before shared memory.
    pub dma_buf: bool,
}

/// A running stream; dropping it disconnects it and joins its thread.
pub(crate) struct VideoStream {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Opens a stream on its own thread; frames go to `sink`. Returns once the
/// stream is connected (frames follow once the producer agrees a format).
pub(crate) fn open(
    options: StreamOptions,
    sink: Box<dyn FrameSink>,
) -> Result<VideoStream, CaptureError> {
    let (stop, stopped) = pw::channel::channel::<()>();
    let (ready_tx, ready) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("twicecat-screen-capture".into())
        .spawn(move || run(options, sink, stopped, &ready_tx))
        .map_err(|_| CaptureError::Failed("no thread for the capture"))?;
    match ready.recv() {
        Ok(Ok(())) => Ok(VideoStream {
            stop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(CaptureError::Failed("the capture thread stopped"))
        }
    }
}

/// The GPU side, when DMA-BUFs are offered.
struct Gpu {
    converter: Converter,
    /// The layouts it imports, with their modifiers.
    offers: Vec<(VideoFormat, Vec<u64>)>,
}

impl Gpu {
    fn open() -> Option<Self> {
        let device = GpuDevice::open_for_capture().ok()?;
        if !device.can_import_dma_buf() {
            return None;
        }
        let offers: Vec<_> = dma_formats()
            .into_iter()
            .map(|(format, fourcc)| (format, device.dma_buf_modifiers(fourcc)))
            .filter(|(_, modifiers)| !modifiers.is_empty())
            .collect();
        if offers.is_empty() {
            return None;
        }
        let converter = Converter::new(&device).ok()?;
        Some(Self { converter, offers })
    }
}

/// The format both sides agreed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Negotiated {
    format: VideoFormat,
    width: u32,
    height: u32,
    /// Set for DMA-BUFs.
    modifier: Option<u64>,
}

struct State {
    sink: Box<dyn FrameSink>,
    gpu: Option<Gpu>,
    /// A DMA-BUF failed to import: shared memory only from now on.
    shm_only: bool,
    negotiated: Option<Negotiated>,
    max_size: (u32, u32),
    max_fps: u32,
    pace: Pace,
    started: Instant,
    streaming: bool,
    ended: bool,
}

impl State {
    fn end(&mut self, end: CaptureEnd) {
        if !self.ended {
            self.ended = true;
            self.sink.ended(end);
        }
    }

    /// The formats to offer now, best first.
    fn offers(&self) -> Vec<Vec<u8>> {
        let mut pods = Vec::new();
        if !self.shm_only
            && let Some(gpu) = &self.gpu
        {
            for (format, modifiers) in &gpu.offers {
                pods.extend(serialize(&format_object(
                    ParamType::EnumFormat,
                    &[*format],
                    Some(modifiers),
                    self.max_fps,
                )));
            }
        }
        pods.extend(serialize(&format_object(
            ParamType::EnumFormat,
            &SHM_FORMATS,
            None,
            self.max_fps,
        )));
        pods
    }
}

fn run(
    options: StreamOptions,
    sink: Box<dyn FrameSink>,
    stopped: pw::channel::Receiver<()>,
    ready: &mpsc::SyncSender<Result<(), CaptureError>>,
) {
    pw::init();
    let failed = |what| {
        let _ = ready.send(Err(CaptureError::Failed(what)));
    };
    let Ok(mainloop) = MainLoopRc::new(None) else {
        return failed("no PipeWire main loop");
    };
    let Ok(context) = ContextRc::new(&mainloop, None) else {
        return failed("no PipeWire context");
    };
    let core = match options.remote {
        Some(fd) => context.connect_fd_rc(fd, None),
        None => context.connect_rc(None),
    };
    let Ok(core) = core else {
        let _ = ready.send(Err(CaptureError::Unavailable("no PipeWire daemon")));
        return;
    };
    let mut props = PropertiesBox::new();
    props.insert("media.type", "Video");
    props.insert("media.category", "Capture");
    props.insert("media.role", "Screen");
    props.insert("node.name", "twicecat-screen-capture");
    let Ok(stream) = StreamRc::new(core.clone(), "twicecat-screen-capture", props) else {
        return failed("could not make the PipeWire stream");
    };
    let fps = options.max_fps.max(1);
    let state = State {
        sink,
        gpu: if options.dma_buf { Gpu::open() } else { None },
        shm_only: false,
        negotiated: None,
        max_size: options.max_size,
        max_fps: fps,
        pace: Pace::new(fps),
        started: Instant::now(),
        streaming: false,
        ended: false,
    };
    let offers = state.offers();
    let listener = stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, state, _, new| state_changed(state, new))
        .param_changed(param_changed)
        .process(process)
        .register();
    let Ok(_listener) = listener else {
        return failed("could not listen to the PipeWire stream");
    };
    let mut pods: Vec<&Pod> = offers.iter().filter_map(|b| Pod::from_bytes(b)).collect();
    if stream
        .connect(
            Direction::Input,
            options.node,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut pods,
        )
        .is_err()
    {
        return failed("could not connect the PipeWire stream");
    }
    let _ = ready.send(Ok(()));
    let quit = mainloop.clone();
    let _stop = stopped.attach(mainloop.loop_(), move |()| quit.quit());
    mainloop.run();
    let _ = stream.disconnect();
}

fn state_changed(state: &mut State, new: StreamState) {
    match new {
        StreamState::Streaming => state.streaming = true,
        // The producer went away (the person stopped sharing, the window
        // closed) or the stream broke.
        StreamState::Error(_) if !state.streaming => state.end(CaptureEnd::Lost),
        StreamState::Error(_) | StreamState::Unconnected if state.streaming => {
            state.end(CaptureEnd::Stopped);
        }
        _ => {}
    }
}

/// A format pod: `formats` (one with `modifiers`, as DMA-BUF), any size up
/// to 8K, up to `fps`.
fn format_object(
    id: ParamType,
    formats: &[VideoFormat],
    modifiers: Option<&[u64]>,
    fps: u32,
) -> Object {
    let format = match formats {
        [one] => Value::Id(Id(one.as_raw())),
        many => Value::Choice(ChoiceValue::Id(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum {
                default: Id(many[0].as_raw()),
                alternatives: many.iter().map(|f| Id(f.as_raw())).collect(),
            },
        ))),
    };
    let mut properties = vec![
        Property::new(
            FormatProperties::MediaType.as_raw(),
            Value::Id(Id(MediaType::Video.as_raw())),
        ),
        Property::new(
            FormatProperties::MediaSubtype.as_raw(),
            Value::Id(Id(MediaSubtype::Raw.as_raw())),
        ),
        Property::new(FormatProperties::VideoFormat.as_raw(), format),
    ];
    if let Some(modifiers) = modifiers {
        let value = match modifiers {
            // One modifier: fixed, no choice left.
            [one] => Value::Long(*one as i64),
            many => Value::Choice(ChoiceValue::Long(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Enum {
                    default: many[0] as i64,
                    alternatives: many.iter().map(|&m| m as i64).collect(),
                },
            ))),
        };
        let mut flags = PropertyFlags::MANDATORY;
        if modifiers.len() > 1 {
            flags |= PropertyFlags::DONT_FIXATE;
        }
        properties.push(Property {
            key: FormatProperties::VideoModifier.as_raw(),
            flags,
            value,
        });
    }
    properties.push(Property::new(
        FormatProperties::VideoSize.as_raw(),
        Value::Choice(ChoiceValue::Rectangle(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: Rectangle {
                    width: 1920,
                    height: 1080,
                },
                min: Rectangle {
                    width: 1,
                    height: 1,
                },
                max: Rectangle {
                    width: 8192,
                    height: 8192,
                },
            },
        ))),
    ));
    properties.push(Property::new(
        FormatProperties::VideoFramerate.as_raw(),
        Value::Choice(ChoiceValue::Fraction(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Range {
                default: Fraction { num: fps, denom: 1 },
                min: Fraction { num: 0, denom: 1 },
                max: Fraction {
                    num: 1000,
                    denom: 1,
                },
            },
        ))),
    ));
    Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: id.as_raw(),
        properties,
    }
}

/// The buffers we take: DMA-BUFs, or shared memory (a descriptor or a
/// pointer).
fn buffers_object(dma_buf: bool) -> Object {
    let mask = if dma_buf {
        1 << DataType::DmaBuf.as_raw()
    } else {
        (1 << DataType::MemFd.as_raw()) | (1 << DataType::MemPtr.as_raw())
    };
    Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![Property::new(
            pw::spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Choice(ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Flags {
                    default: mask,
                    flags: Vec::new(),
                },
            ))),
        )],
    }
}

fn serialize(object: &Object) -> Option<Vec<u8>> {
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object.clone()))
        .ok()
        .map(|(cursor, _)| cursor.into_inner())
}

/// What a negotiated format says about the modifier.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ModifierProp {
    /// No modifier: shared memory.
    Absent,
    /// One modifier: a DMA-BUF in that layout.
    Fixed(u64),
    /// Several left for us to pick (the producer did not fixate).
    Choice(Vec<u64>),
}

fn modifier_of(param: &Pod) -> ModifierProp {
    let Ok((_, Value::Object(object))) = PodDeserializer::deserialize_any_from(param.as_bytes())
    else {
        return ModifierProp::Absent;
    };
    modifier_in(&object)
}

fn modifier_in(object: &Object) -> ModifierProp {
    let Some(property) = object
        .properties
        .iter()
        .find(|p| p.key == FormatProperties::VideoModifier.as_raw())
    else {
        return ModifierProp::Absent;
    };
    let mut values: Vec<u64> = match &property.value {
        Value::Long(m) => vec![*m as u64],
        Value::Choice(ChoiceValue::Long(Choice(_, choice))) => match choice {
            ChoiceEnum::None(m) => vec![*m as u64],
            ChoiceEnum::Enum {
                default,
                alternatives,
            } => std::iter::once(default)
                .chain(alternatives)
                .map(|&m| m as u64)
                .collect(),
            _ => return ModifierProp::Absent,
        },
        _ => return ModifierProp::Absent,
    };
    let mut seen = Vec::with_capacity(values.len());
    values.retain(|m| {
        let new = !seen.contains(m);
        seen.push(*m);
        new
    });
    match values.as_slice() {
        [] => ModifierProp::Absent,
        [one] => ModifierProp::Fixed(*one),
        _ => ModifierProp::Choice(values),
    }
}

fn param_changed(stream: &pw::stream::Stream, state: &mut State, id: u32, param: Option<&Pod>) {
    if id != ParamType::Format.as_raw() {
        return;
    }
    let Some(param) = param else {
        state.negotiated = None;
        return;
    };
    let Ok((MediaType::Video, MediaSubtype::Raw)) = parse_format(param) else {
        return;
    };
    let mut info = VideoInfoRaw::new();
    if info.parse(param).is_err() {
        return;
    }
    let size = info.size();
    let modifier = match modifier_of(param) {
        ModifierProp::Absent => None,
        ModifierProp::Fixed(m) => Some(m),
        ModifierProp::Choice(list) => {
            // Fixate: the first, the producer's own default, which we
            // offered (each one we offer imports). The producer then sends
            // the format again with it.
            let fixed = format_object(
                ParamType::EnumFormat,
                &[info.format()],
                Some(&list[..1]),
                state.max_fps,
            );
            let mut bytes: Vec<Vec<u8>> = serialize(&fixed).into_iter().collect();
            bytes.extend(serialize(&format_object(
                ParamType::EnumFormat,
                &SHM_FORMATS,
                None,
                state.max_fps,
            )));
            let mut pods: Vec<&Pod> = bytes.iter().filter_map(|b| Pod::from_bytes(b)).collect();
            if stream.update_params(&mut pods).is_err() {
                tracing::warn!("could not fixate the screen cast's buffer layout");
            }
            return;
        }
    };
    if modifier.is_some() && fourcc_of(info.format()).is_none() {
        return;
    }
    state.negotiated = Some(Negotiated {
        format: info.format(),
        width: size.width,
        height: size.height,
        modifier,
    });
    let Some(buffers) = serialize(&buffers_object(modifier.is_some())) else {
        return;
    };
    let Some(pod) = Pod::from_bytes(&buffers) else {
        return;
    };
    if stream.update_params(&mut [pod]).is_err() {
        tracing::warn!("could not set the screen cast's buffers");
    }
}

fn process(stream: &pw::stream::Stream, state: &mut State) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let Some(negotiated) = state.negotiated else {
        return;
    };
    let now = Instant::now();
    if !state.pace.due(now) {
        return;
    }
    let Some(data) = buffer.datas_mut().first_mut() else {
        return;
    };
    let chunk = data.chunk();
    if chunk.flags().contains(ChunkFlags::CORRUPTED) {
        return;
    }
    let (offset, size, stride) = (chunk.offset(), chunk.size(), chunk.stride());
    let timestamp = now.duration_since(state.started);
    let frame = match data.type_() {
        DataType::DmaBuf => {
            let (Some(modifier), Some(fourcc)) =
                (negotiated.modifier, fourcc_of(negotiated.format))
            else {
                return;
            };
            let Ok(stride) = u32::try_from(stride) else {
                return;
            };
            let raw: RawFd = data.fd();
            // Duplicated: the frame owns its copy of the descriptor.
            let Some(fd) = borrow_fd(raw).and_then(|fd| fd.try_clone_to_owned().ok()) else {
                return;
            };
            wait_written(&fd);
            let frame = Frame::DmaBuf(DmaBufFrame {
                fourcc,
                modifier,
                width: negotiated.width,
                height: negotiated.height,
                planes: vec![DmaBufPlane { fd, offset, stride }],
                color: SENT,
                timestamp,
            });
            match gpu_convert(state, &frame) {
                Some(frame) => frame,
                None => {
                    drop(buffer);
                    fall_back_to_shm(stream, state);
                    return;
                }
            }
        }
        DataType::MemFd | DataType::MemPtr => {
            if size == 0 {
                // No new content (some producers send empty buffers).
                return;
            }
            let Some(bytes) = data.data() else {
                return;
            };
            match shm_frame(bytes, offset, stride, negotiated, timestamp) {
                Some(frame) => frame,
                None => return,
            }
        }
        _ => return,
    };
    state.pace.taken(now);
    state.sink.frame(frame);
}

/// Which frames to take, at most `fps` a second: each frame is due one
/// interval after the last one's slot, with a quarter interval of slack, so
/// a producer at that very rate whose frames come a little early is not
/// halved, and one twice as fast gives every other frame.
#[derive(Clone, Copy, Debug)]
struct Pace {
    interval: Duration,
    next: Option<Instant>,
}

impl Pace {
    fn new(fps: u32) -> Self {
        Self {
            interval: Duration::from_secs(1) / fps.max(1),
            next: None,
        }
    }

    fn due(&self, now: Instant) -> bool {
        self.next.is_none_or(|next| now + self.interval / 4 >= next)
    }

    /// A frame was taken at `now`.
    fn taken(&mut self, now: Instant) {
        let next = self
            .next
            .map_or(now + self.interval, |next| next + self.interval);
        // After a pause (no frames for a while), from now, not the past.
        self.next = Some(if next + self.interval < now {
            now + self.interval
        } else {
            next.max(now)
        });
    }
}

/// The descriptor PipeWire gave, borrowed; `None` for an invalid one.
fn borrow_fd(raw: RawFd) -> Option<BorrowedFd<'static>> {
    if raw < 0 {
        return None;
    }
    // SAFETY: PipeWire keeps `raw` open while the buffer is dequeued, and
    // the borrow only lives in `process` (it is duplicated at once); the
    // 'static here never escapes that call.
    Some(unsafe { BorrowedFd::borrow_raw(raw) })
}

/// A DMA-BUF imported and converted on the GPU, scaled to fit; `None` when
/// the GPU could not (the stream then falls back to shared memory).
fn gpu_convert(state: &mut State, frame: &Frame) -> Option<CpuFrame> {
    let gpu = state.gpu.as_mut()?;
    let target = fit_size((frame.width(), frame.height()), state.max_size);
    match gpu.converter.convert(frame, &[target], SENT) {
        Ok(mut frames) if !frames.is_empty() => Some(frames.swap_remove(0)),
        _ => None,
    }
}

/// After a DMA-BUF failed: offer shared memory only (once).
fn fall_back_to_shm(stream: &pw::stream::Stream, state: &mut State) {
    if state.shm_only {
        return;
    }
    tracing::info!("the screen cast's GPU buffers could not be imported; using shared memory");
    state.shm_only = true;
    state.negotiated = None;
    let offers = state.offers();
    let mut pods: Vec<&Pod> = offers.iter().filter_map(|b| Pod::from_bytes(b)).collect();
    if stream.update_params(&mut pods).is_err() {
        state.end(CaptureEnd::Lost);
    }
}

/// A shared-memory buffer as an NV12 frame (copied, converted on the CPU).
fn shm_frame(
    bytes: &[u8],
    offset: u32,
    stride: i32,
    negotiated: Negotiated,
    timestamp: Duration,
) -> Option<CpuFrame> {
    let (width, height) = (negotiated.width, negotiated.height);
    if width == 0 || height == 0 {
        return None;
    }
    let row = width as usize * 4;
    let stride = usize::try_from(stride).ok().filter(|&s| s >= row)?;
    let start = offset as usize;
    let len = stride.checked_mul(height as usize - 1)?.checked_add(row)?;
    let pixels = bytes.get(start..start.checked_add(len)?)?;
    // Copied row by row into a tight plane: the one copy out of PipeWire's
    // buffer, and yuvutils converts nothing from a padded plane whose last
    // row is not padded too (as a producer's buffer may end).
    let mut tight = Vec::with_capacity(row * height as usize);
    for line in pixels.chunks(stride) {
        tight.extend_from_slice(&line[..row]);
    }
    let bgra = CpuFrame::new(
        PixelFormat::Bgra,
        width,
        height,
        vec![Plane {
            data: tight.into(),
            stride: row,
        }],
        SENT,
        timestamp,
    )
    .ok()?;
    convert::to_nv12(&bgra, SENT).ok()
}

/// Waits (briefly) until the producer finished writing a DMA-BUF: polling
/// a DMA-BUF for reading waits for its implicit write fence.
fn wait_written(fd: &OwnedFd) {
    use std::os::fd::AsRawFd;
    use std::os::raw::{c_int, c_short, c_ulong};

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: c_short,
        revents: c_short,
    }
    const POLLIN: c_short = 0x1;
    unsafe extern "C" {
        fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
    }
    let mut fds = [PollFd {
        fd: fd.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    }];
    // SAFETY: `fds` is a valid array of one `pollfd` (same layout as libc's)
    // that lives until the call returns; the descriptor is open (we own it).
    // A timeout or error only means we convert without waiting.
    let _ = unsafe { poll(fds.as_mut_ptr(), 1, FENCE_TIMEOUT_MS) };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(object: &Object) -> Object {
        let bytes = serialize(object).expect("serializes");
        let Ok((_, Value::Object(back))) = PodDeserializer::deserialize_any_from(&bytes) else {
            panic!("not an object");
        };
        back
    }

    #[test]
    fn dma_buf_offers_carry_their_modifiers_for_us_to_fixate() {
        let offer = format_object(
            ParamType::EnumFormat,
            &[VideoFormat::BGRx],
            Some(&[0x0100_0000_0000_0001, 0]),
            60,
        );
        let back = roundtrip(&offer);
        let property = back
            .properties
            .iter()
            .find(|p| p.key == FormatProperties::VideoModifier.as_raw())
            .expect("a modifier");
        assert!(property.flags.contains(PropertyFlags::MANDATORY));
        assert!(property.flags.contains(PropertyFlags::DONT_FIXATE));
        assert_eq!(
            modifier_in(&back),
            ModifierProp::Choice(vec![0x0100_0000_0000_0001, 0])
        );
        // One modifier is fixed: nothing left to pick.
        let fixed = format_object(ParamType::EnumFormat, &[VideoFormat::BGRx], Some(&[7]), 60);
        assert_eq!(modifier_in(&roundtrip(&fixed)), ModifierProp::Fixed(7));
    }

    #[test]
    fn shared_memory_offers_have_no_modifier_and_both_layouts() {
        let offer = format_object(ParamType::EnumFormat, &SHM_FORMATS, None, 30);
        let back = roundtrip(&offer);
        assert_eq!(modifier_in(&back), ModifierProp::Absent);
        let format = back
            .properties
            .iter()
            .find(|p| p.key == FormatProperties::VideoFormat.as_raw())
            .expect("a format");
        let Value::Choice(ChoiceValue::Id(Choice(_, ChoiceEnum::Enum { alternatives, .. }))) =
            &format.value
        else {
            panic!("a choice of formats: {format:?}");
        };
        assert_eq!(
            alternatives,
            &vec![
                Id(VideoFormat::BGRx.as_raw()),
                Id(VideoFormat::BGRA.as_raw())
            ]
        );
    }

    #[test]
    fn a_choice_that_repeats_one_modifier_is_fixed() {
        let object = Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::Format.as_raw(),
            properties: vec![Property::new(
                FormatProperties::VideoModifier.as_raw(),
                Value::Choice(ChoiceValue::Long(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Enum {
                        default: 5,
                        alternatives: vec![5],
                    },
                ))),
            )],
        };
        assert_eq!(modifier_in(&object), ModifierProp::Fixed(5));
    }

    #[test]
    fn buffer_types_follow_the_layout() {
        let types = |dma| {
            let back = roundtrip(&buffers_object(dma));
            match &back.properties[0].value {
                Value::Choice(ChoiceValue::Int(Choice(_, ChoiceEnum::Flags { default, .. }))) => {
                    *default
                }
                other => panic!("flags: {other:?}"),
            }
        };
        assert_eq!(types(true), 1 << DataType::DmaBuf.as_raw());
        assert_eq!(
            types(false),
            (1 << DataType::MemFd.as_raw()) | (1 << DataType::MemPtr.as_raw())
        );
    }

    #[test]
    fn frames_are_paced_without_halving_an_early_producer() {
        let start = Instant::now();
        let ms = |n: u64| start + Duration::from_micros(n * 100);
        // A 60 fps producer, each frame 0.2 ms early, under a 60 fps cap:
        // every frame taken.
        let mut pace = Pace::new(60);
        let mut taken = 0;
        for i in 0..60 {
            let at = ms(i * 167 - i.min(1) * 2);
            if pace.due(at) {
                pace.taken(at);
                taken += 1;
            }
        }
        assert_eq!(taken, 60);
        // The same producer under a 30 fps cap: every other frame.
        let mut pace = Pace::new(30);
        let mut taken = 0;
        for i in 0..60 {
            let at = ms(i * 167);
            if pace.due(at) {
                pace.taken(at);
                taken += 1;
            }
        }
        assert_eq!(taken, 30);
    }

    #[test]
    fn shared_memory_frames_are_read_with_their_stride_and_offset() {
        // As wide as the converter's own tests (its SIMD paths want rows of
        // some length), with padded rows after an 8-byte offset.
        let (w, h) = (64_u32, 2_u32);
        let stride = w as usize * 4 + 16;
        let mut bytes = vec![0_u8; 8 + stride * 2];
        // A white first row, a black second one.
        bytes[8..8 + w as usize * 4].fill(255);
        let negotiated = Negotiated {
            format: VideoFormat::BGRx,
            width: w,
            height: h,
            modifier: None,
        };
        let frame = shm_frame(&bytes, 8, stride as i32, negotiated, Duration::ZERO).expect("frame");
        assert_eq!(frame.format(), PixelFormat::Nv12);
        assert_eq!((frame.width(), frame.height()), (64, 2));
        // Limited range: white near 235, black near 16.
        let y = &frame.planes()[0];
        assert!(y.data[0] > 230, "{}", y.data[0]);
        assert!(y.data[y.stride] < 20, "{}", y.data[y.stride]);
        // Too short for the size: no frame, no panic.
        assert!(
            shm_frame(
                &bytes[..stride],
                8,
                stride as i32,
                negotiated,
                Duration::ZERO
            )
            .is_none()
        );
        // A stride narrower than a row is refused.
        assert!(shm_frame(&bytes, 0, 8, negotiated, Duration::ZERO).is_none());
    }

    /// A producer stream in this process: red BGRx frames of 320 × 240 at
    /// 30 fps, from its own thread, until `stop`.
    fn produce(
        node: mpsc::Sender<u32>,
        stop: pw::channel::Receiver<()>,
    ) -> std::thread::JoinHandle<()> {
        const W: u32 = 320;
        const H: u32 = 240;
        std::thread::spawn(move || {
            pw::init();
            let mainloop = MainLoopRc::new(None).expect("loop");
            let context = ContextRc::new(&mainloop, None).expect("context");
            let core = context.connect_rc(None).expect("PipeWire daemon");
            let mut props = PropertiesBox::new();
            props.insert("media.class", "Video/Source");
            props.insert("node.name", "twicecat-test-producer");
            let stream = StreamRc::new(core, "twicecat-test-producer", props).expect("stream");
            let format = Object {
                type_: SpaTypes::ObjectParamFormat.as_raw(),
                id: ParamType::EnumFormat.as_raw(),
                properties: vec![
                    Property::new(
                        FormatProperties::MediaType.as_raw(),
                        Value::Id(Id(MediaType::Video.as_raw())),
                    ),
                    Property::new(
                        FormatProperties::MediaSubtype.as_raw(),
                        Value::Id(Id(MediaSubtype::Raw.as_raw())),
                    ),
                    Property::new(
                        FormatProperties::VideoFormat.as_raw(),
                        Value::Id(Id(VideoFormat::BGRx.as_raw())),
                    ),
                    Property::new(
                        FormatProperties::VideoSize.as_raw(),
                        Value::Rectangle(Rectangle {
                            width: W,
                            height: H,
                        }),
                    ),
                    Property::new(
                        FormatProperties::VideoFramerate.as_raw(),
                        Value::Fraction(Fraction { num: 30, denom: 1 }),
                    ),
                ],
            };
            let format = serialize(&format).expect("format");
            let sent_node = std::cell::Cell::new(false);
            let _listener = stream
                .add_local_listener_with_user_data(())
                .state_changed(move |stream, _, _, new| {
                    if matches!(new, StreamState::Paused | StreamState::Streaming)
                        && !sent_node.replace(true)
                    {
                        let _ = node.send(stream.node_id());
                    }
                })
                .param_changed(|stream, _, id, param| {
                    if id != ParamType::Format.as_raw() || param.is_none() {
                        return;
                    }
                    let int = |key, v: i32| Property::new(key, Value::Int(v));
                    let buffers = Object {
                        type_: SpaTypes::ObjectParamBuffers.as_raw(),
                        id: ParamType::Buffers.as_raw(),
                        properties: vec![
                            int(pw::spa::sys::SPA_PARAM_BUFFERS_buffers, 4),
                            int(pw::spa::sys::SPA_PARAM_BUFFERS_blocks, 1),
                            int(pw::spa::sys::SPA_PARAM_BUFFERS_size, (W * H * 4) as i32),
                            int(pw::spa::sys::SPA_PARAM_BUFFERS_stride, (W * 4) as i32),
                            Property::new(
                                pw::spa::sys::SPA_PARAM_BUFFERS_dataType,
                                Value::Choice(ChoiceValue::Int(Choice(
                                    ChoiceFlags::empty(),
                                    ChoiceEnum::Flags {
                                        default: 1 << DataType::MemFd.as_raw(),
                                        flags: Vec::new(),
                                    },
                                ))),
                            ),
                        ],
                    };
                    let bytes = serialize(&buffers).expect("buffers");
                    let pod = Pod::from_bytes(&bytes).expect("pod");
                    stream.update_params(&mut [pod]).expect("buffers set");
                })
                .process(|stream, _| {
                    let Some(mut buffer) = stream.dequeue_buffer() else {
                        return;
                    };
                    let Some(data) = buffer.datas_mut().first_mut() else {
                        return;
                    };
                    if let Some(bytes) = data.data() {
                        for px in bytes[..(W * H * 4) as usize].chunks_exact_mut(4) {
                            px.copy_from_slice(&[0, 0, 255, 255]);
                        }
                    }
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.size_mut() = W * H * 4;
                    *chunk.stride_mut() = (W * 4) as i32;
                })
                .register()
                .expect("listener");
            let pod = Pod::from_bytes(&format).expect("pod");
            stream
                .connect(
                    Direction::Output,
                    None,
                    StreamFlags::DRIVER | StreamFlags::MAP_BUFFERS,
                    &mut [pod],
                )
                .expect("connect");
            let tick = stream.clone();
            let timer = mainloop.loop_().add_timer(move |_| {
                let _ = tick.trigger_process();
            });
            let _ = timer.update_timer(
                Some(Duration::from_millis(1)),
                Some(Duration::from_millis(33)),
            );
            let quit = mainloop.clone();
            let _stop = stop.attach(mainloop.loop_(), move |()| quit.quit());
            mainloop.run();
            let _ = stream.disconnect();
        })
    }

    /// Hands frames and the end to channels.
    struct ToChannel {
        frames: mpsc::Sender<CpuFrame>,
        ended: mpsc::Sender<CaptureEnd>,
    }

    impl FrameSink for ToChannel {
        fn frame(&mut self, frame: CpuFrame) {
            let _ = self.frames.send(frame);
        }

        fn ended(&mut self, end: CaptureEnd) {
            let _ = self.ended.send(end);
        }
    }

    #[test]
    #[ignore = "needs a PipeWire daemon"]
    fn a_producer_s_frames_arrive_as_nv12_and_its_end_is_seen() {
        let (node_tx, node_rx) = mpsc::channel();
        let (stop_producer, stopped) = pw::channel::channel::<()>();
        let producer = produce(node_tx, stopped);
        let node = node_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the producer's node");

        let (frames_tx, frames) = mpsc::channel();
        let (ended_tx, ended) = mpsc::channel();
        let stream = open(
            StreamOptions {
                remote: None,
                node: Some(node),
                max_size: (1920, 1080),
                max_fps: 60,
                dma_buf: false,
            },
            Box::new(ToChannel {
                frames: frames_tx,
                ended: ended_tx,
            }),
        )
        .expect("stream");

        let mut got = 0;
        let deadline = Instant::now() + Duration::from_secs(5);
        while got < 10 && Instant::now() < deadline {
            let Ok(frame) = frames.recv_timeout(Duration::from_millis(500)) else {
                continue;
            };
            assert_eq!(frame.format(), PixelFormat::Nv12);
            assert_eq!((frame.width(), frame.height()), (320, 240));
            // Pure red in BT.709 limited range: Y 63, U 102, V 240.
            let (y, uv) = (&frame.planes()[0], &frame.planes()[1]);
            assert!(y.data[0].abs_diff(63) <= 2, "Y {}", y.data[0]);
            assert!(uv.data[0].abs_diff(102) <= 3, "U {}", uv.data[0]);
            assert!(uv.data[1].abs_diff(240) <= 3, "V {}", uv.data[1]);
            got += 1;
        }
        assert!(got >= 10, "only {got} frames in 5 s");

        // The producer goes away, as when the person stops sharing.
        let _ = stop_producer.send(());
        producer.join().expect("producer");
        let end = ended.recv_timeout(Duration::from_secs(3));
        drop(stream);
        assert_eq!(end, Ok(CaptureEnd::Stopped));
    }
}
