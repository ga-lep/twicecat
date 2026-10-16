//! A camera made in this process, for tests (plan Task 9's virtual camera:
//! no `v4l2loopback` here). A PipeWire `Video/Source` node with the
//! `Camera` role, as the session manager makes for a real one, so it is
//! listed and opened as one: [`TestCamera::start`] sends the test pattern
//! with its stamps (to follow its frames through a call); the crate's own
//! tests also send one flat colour in YUYV or NV12, or a JPEG frame.
//!
//! Compiled for this crate's tests and with the `test-camera` feature
//! (the core's tests).

use std::cell::Cell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::context::ContextRc;
use pw::main_loop::MainLoopRc;
use pw::properties::PropertiesBox;
use pw::spa::buffer::DataType;
use pw::spa::param::ParamType;
use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pw::spa::param::format_utils::parse_format;
use pw::spa::param::video::{VideoFormat, VideoInfoRaw};
use pw::spa::pod::{ChoiceValue, Object, Pod, Property, Value};
use pw::spa::utils::{
    Choice, ChoiceEnum, ChoiceFlags, Direction, Fraction, Id, Rectangle, SpaTypes,
};
use pw::stream::{StreamFlags, StreamRc, StreamState};

use super::CaptureError;
use super::camera::CameraFormat;
use super::pw_video::serialize;
use crate::test_pattern::TestPattern;

/// What a test camera shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Picture {
    /// The test pattern with its stamps, NV12, said to be BT.709 limited.
    Pattern,
    /// Red in BT.601 limited (Y 81, U 90, V 240), colours unsaid, in the
    /// raw formats; `jpeg` in MJPEG.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only this crate's tests show a flat colour")
    )]
    Flat { jpeg: &'static [u8] },
}

/// A camera in this process, until dropped.
pub struct TestCamera {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for TestCamera {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestCamera").finish_non_exhaustive()
    }
}

impl Drop for TestCamera {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl TestCamera {
    /// A camera named `name` (its identifier; described as "Twicecat Test
    /// Camera") sending the test pattern at `width` × `height` and `fps`,
    /// in NV12. Returns once its node exists.
    ///
    /// # Errors
    /// [`CaptureError::Unavailable`] without a PipeWire daemon.
    pub fn start(name: &str, width: u32, height: u32, fps: u32) -> Result<Self, CaptureError> {
        Self::start_with(
            name,
            (width, height, fps),
            &[CameraFormat::Nv12],
            Picture::Pattern,
        )
    }

    /// A camera offering `formats` at one size and rate, showing `picture`.
    pub(crate) fn start_with(
        name: &str,
        mode: (u32, u32, u32),
        formats: &[CameraFormat],
        picture: Picture,
    ) -> Result<Self, CaptureError> {
        let (stop, stopped) = pw::channel::channel::<()>();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let name = name.to_owned();
        let formats = formats.to_vec();
        let thread = std::thread::Builder::new()
            .name("twicecat-test-camera".into())
            .spawn(move || run(&name, mode, &formats, picture, stopped, &ready_tx))
            .map_err(|_| CaptureError::Failed("no thread for the test camera"))?;
        match ready.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                drop(stop);
                Err(CaptureError::Failed("the test camera did not start"))
            }
        }
    }
}

/// Its format pod for one format.
fn offer(format: CameraFormat, (w, h, fps): (u32, u32, u32), picture: Picture) -> Object {
    let id = |v: u32| Value::Id(Id(v));
    let mut properties = vec![
        Property::new(
            FormatProperties::MediaType.as_raw(),
            id(MediaType::Video.as_raw()),
        ),
        Property::new(
            FormatProperties::MediaSubtype.as_raw(),
            id(match format {
                CameraFormat::Mjpeg => MediaSubtype::Mjpg.as_raw(),
                _ => MediaSubtype::Raw.as_raw(),
            }),
        ),
    ];
    match format {
        CameraFormat::Nv12 => properties.push(Property::new(
            FormatProperties::VideoFormat.as_raw(),
            id(VideoFormat::NV12.as_raw()),
        )),
        CameraFormat::Yuyv => properties.push(Property::new(
            FormatProperties::VideoFormat.as_raw(),
            id(VideoFormat::YUY2.as_raw()),
        )),
        CameraFormat::Mjpeg => {}
    }
    properties.push(Property::new(
        FormatProperties::VideoSize.as_raw(),
        Value::Rectangle(Rectangle {
            width: w,
            height: h,
        }),
    ));
    properties.push(Property::new(
        FormatProperties::VideoFramerate.as_raw(),
        Value::Choice(ChoiceValue::Fraction(Choice(
            ChoiceFlags::empty(),
            ChoiceEnum::Enum {
                default: Fraction { num: fps, denom: 1 },
                alternatives: vec![Fraction { num: fps, denom: 1 }],
            },
        ))),
    ));
    if picture == Picture::Pattern {
        properties.push(Property::new(
            pw::spa::sys::SPA_FORMAT_VIDEO_colorMatrix,
            id(pw::spa::sys::SPA_VIDEO_COLOR_MATRIX_BT709),
        ));
        properties.push(Property::new(
            pw::spa::sys::SPA_FORMAT_VIDEO_colorRange,
            id(pw::spa::sys::SPA_VIDEO_COLOR_RANGE_16_235),
        ));
    }
    Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties,
    }
}

/// The buffers it fills: one block of shared memory of `size` bytes.
fn buffers(size: u32, stride: u32) -> Object {
    let int = |key, v: u32| Property::new(key, Value::Int(v as i32));
    Object {
        type_: SpaTypes::ObjectParamBuffers.as_raw(),
        id: ParamType::Buffers.as_raw(),
        properties: vec![
            int(pw::spa::sys::SPA_PARAM_BUFFERS_buffers, 4),
            int(pw::spa::sys::SPA_PARAM_BUFFERS_blocks, 1),
            int(pw::spa::sys::SPA_PARAM_BUFFERS_size, size),
            int(pw::spa::sys::SPA_PARAM_BUFFERS_stride, stride),
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
    }
}

/// Bytes and row length of one frame in `format`.
fn layout(format: CameraFormat, (w, h): (u32, u32), picture: Picture) -> (u32, u32) {
    match (format, picture) {
        (CameraFormat::Nv12, _) => (w * h * 3 / 2, w),
        (CameraFormat::Yuyv, _) => (w * h * 2, w * 2),
        (CameraFormat::Mjpeg, Picture::Flat { jpeg }) => (jpeg.len() as u32, 0),
        (CameraFormat::Mjpeg, Picture::Pattern) => (0, 0),
    }
}

/// Fills `bytes` with frame `counter`; its size in bytes.
fn fill(
    bytes: &mut [u8],
    format: CameraFormat,
    (w, h): (u32, u32),
    picture: Picture,
    pattern: Option<&TestPattern>,
    counter: u32,
    sent: Duration,
) -> u32 {
    let (w, h) = (w as usize, h as usize);
    match (format, picture) {
        (CameraFormat::Nv12, Picture::Pattern) => {
            let Some(pattern) = pattern else {
                return 0;
            };
            let frame = pattern.frame(counter, sent);
            let mut at = 0;
            for (plane, rows) in frame.planes().iter().zip([h, h / 2]) {
                for row in 0..rows {
                    bytes[at..at + w].copy_from_slice(&plane.data[row * plane.stride..][..w]);
                    at += w;
                }
            }
        }
        (CameraFormat::Nv12, Picture::Flat { .. }) => {
            let (y, uv) = bytes.split_at_mut(w * h);
            y.fill(81);
            for pair in uv[..w * h / 2].chunks_exact_mut(2) {
                pair.copy_from_slice(&[90, 240]);
            }
        }
        (CameraFormat::Yuyv, _) => {
            for px in bytes[..w * h * 2].chunks_exact_mut(4) {
                px.copy_from_slice(&[81, 90, 81, 240]);
            }
        }
        (CameraFormat::Mjpeg, Picture::Flat { jpeg }) => {
            bytes[..jpeg.len()].copy_from_slice(jpeg);
        }
        (CameraFormat::Mjpeg, Picture::Pattern) => return 0,
    }
    layout(format, (w as u32, h as u32), picture).0
}

fn run(
    name: &str,
    (w, h, fps): (u32, u32, u32),
    formats: &[CameraFormat],
    picture: Picture,
    stop: pw::channel::Receiver<()>,
    ready: &mpsc::SyncSender<Result<(), CaptureError>>,
) {
    pw::init();
    let failed = |e| {
        let _ = ready.send(Err(e));
    };
    let Ok(mainloop) = MainLoopRc::new(None) else {
        return failed(CaptureError::Failed("no PipeWire main loop"));
    };
    let Ok(context) = ContextRc::new(&mainloop, None) else {
        return failed(CaptureError::Failed("no PipeWire context"));
    };
    let Ok(core) = context.connect_rc(None) else {
        return failed(CaptureError::Unavailable("no PipeWire daemon"));
    };
    let mut props = PropertiesBox::new();
    props.insert("media.class", "Video/Source");
    props.insert("media.role", "Camera");
    props.insert("node.name", name);
    props.insert("node.description", "Twicecat Test Camera");
    let Ok(stream) = StreamRc::new(core, name, props) else {
        return failed(CaptureError::Failed("could not make the PipeWire stream"));
    };
    let pods: Vec<Vec<u8>> = formats
        .iter()
        .filter_map(|&f| serialize(&offer(f, (w, h, fps), picture)))
        .collect();
    let agreed: Rc<Cell<Option<CameraFormat>>> = Rc::default();
    let pattern = (picture == Picture::Pattern).then(|| TestPattern::new(w, h));
    let started = Instant::now();
    let counter = Cell::new(0_u32);
    let ready_once = Cell::new(Some(ready.clone()));
    let listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, _, new| {
            if matches!(new, StreamState::Paused | StreamState::Streaming)
                && let Some(ready) = ready_once.take()
            {
                let _ = ready.send(Ok(()));
            }
        })
        .param_changed({
            let agreed = agreed.clone();
            move |stream, _, id, param| {
                let Some(param) = param else {
                    return;
                };
                if id != ParamType::Format.as_raw() {
                    return;
                }
                let format = match parse_format(param) {
                    Ok((_, MediaSubtype::Mjpg)) => CameraFormat::Mjpeg,
                    _ => {
                        let mut info = VideoInfoRaw::new();
                        if info.parse(param).is_err() {
                            return;
                        }
                        if info.format() == VideoFormat::NV12 {
                            CameraFormat::Nv12
                        } else {
                            CameraFormat::Yuyv
                        }
                    }
                };
                agreed.set(Some(format));
                let (size, stride) = layout(format, (w, h), picture);
                let Some(bytes) = serialize(&buffers(size, stride)) else {
                    return;
                };
                if let Some(pod) = Pod::from_bytes(&bytes) {
                    let _ = stream.update_params(&mut [pod]);
                }
            }
        })
        .process(move |stream, _| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(format) = agreed.get() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let Some(bytes) = data.data() else {
                return;
            };
            let n = counter.get();
            counter.set(n.wrapping_add(1));
            let size = fill(
                bytes,
                format,
                (w, h),
                picture,
                pattern.as_ref(),
                n,
                started.elapsed(),
            );
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.size_mut() = size;
            *chunk.stride_mut() = layout(format, (w, h), picture).1 as i32;
        })
        .register();
    let Ok(_listener) = listener else {
        return failed(CaptureError::Failed("could not listen to the stream"));
    };
    let mut pods: Vec<&Pod> = pods.iter().filter_map(|b| Pod::from_bytes(b)).collect();
    if stream
        .connect(
            Direction::Output,
            None,
            StreamFlags::DRIVER | StreamFlags::MAP_BUFFERS,
            &mut pods,
        )
        .is_err()
    {
        return failed(CaptureError::Failed("could not connect the test camera"));
    }
    let tick = stream.clone();
    let timer = mainloop.loop_().add_timer(move |_| {
        let _ = tick.trigger_process();
    });
    let _ = timer.update_timer(
        Some(Duration::from_millis(1)),
        Some(Duration::from_secs(1) / fps.max(1)),
    );
    let quit = mainloop.clone();
    let _stop = stop.attach(mainloop.loop_(), move |()| quit.quit());
    mainloop.run();
    let _ = stream.disconnect();
}
