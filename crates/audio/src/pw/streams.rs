//! Our capture and playback streams. One PipeWire thread runs the main loop;
//! the streams' `process` callbacks run in PipeWire's real-time data loop
//! and only move samples between the graph's buffers and the lock-free
//! rings. The format is 48 kHz mono `f32`: PipeWire converts, so nothing
//! resamples on our side.

use std::io::Cursor;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;

use pipewire as pw;
use pw::properties::PropertiesBox;
use pw::spa::param::ParamType;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{Object, Pod, Value};
use pw::spa::utils::{Direction, SpaTypes};
use pw::stream::{StreamFlags, StreamListener, StreamRc, StreamState};

use super::graph::{self, StreamKind};
use super::registry::{self, Session};
use super::thread::ThreadUtils;
use crate::AudioError;
use crate::devices::{DeviceInfo, DeviceList};
use crate::frame::SAMPLE_RATE;
use crate::io::{IoConfig, IoEvent, IoInfo, StreamInfo};
use crate::pipeline::{CaptureSide, IoCounters, PlaybackSide, capture_ring_len, playback_ring_len};

const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();

/// The running streams; dropping them disconnects them.
pub(crate) struct Streams {
    stop: pw::channel::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Streams {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What [`open`] gives the processing thread.
pub(crate) struct Opened {
    pub streams: Streams,
    pub capture: Option<CaptureSide>,
    pub playback: Option<PlaybackSide>,
    /// What the output device plays, when asked for.
    pub reference: Option<CaptureSide>,
    pub info: IoInfo,
    /// The real-time module's thread utilities, if the context loaded it.
    pub thread_utils: Option<ThreadUtils>,
}

/// One direction to open.
struct Plan {
    kind: StreamKind,
    /// `node.name` of the chosen device; `None` follows the default.
    target: Option<String>,
}

/// The chosen device's node, or `None` (the default) with a
/// `FellBackToDefault` event when it is gone.
fn target(
    id: Option<&str>,
    list: &[DeviceInfo],
    input: bool,
    events: &mpsc::Sender<IoEvent>,
) -> Option<String> {
    let id = id?;
    if list.iter().any(|d| d.id == id)
        && let Some(name) = graph::node_name(id)
    {
        return Some(name.to_owned());
    }
    let _ = events.send(IoEvent::FellBackToDefault { input });
    None
}

/// What a stream is connected to, for [`IoInfo`]: the chosen device, else
/// the current default.
fn stream_info(target: Option<&str>, list: &[DeviceInfo]) -> StreamInfo {
    let device = match target {
        Some(name) => list.iter().find(|d| graph::node_name(&d.id) == Some(name)),
        None => list.iter().find(|d| d.is_default),
    };
    StreamInfo {
        device_id: device.map_or_else(|| "pipewire:default".into(), |d| d.id.clone()),
        device_name: device.map_or_else(|| "default".into(), |d| d.name.clone()),
        sample_rate: SAMPLE_RATE,
        channels: 1,
    }
}

/// Opens the configured streams on a new PipeWire thread.
pub(crate) fn open(
    config: &IoConfig,
    counters: &Arc<IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) -> Result<Opened, AudioError> {
    let devices: DeviceList = registry::list()?;
    let mut info = IoInfo::default();
    let mut capture = None;
    let mut playback = None;
    let mut capture_ring = None;
    let mut playback_ring = None;
    let mut reference = None;
    let mut reference_ring = None;
    let mut plans = Vec::new();
    if config.capture {
        let target = target(
            config.input_device.as_deref(),
            &devices.inputs,
            true,
            events,
        );
        info.input = Some(stream_info(target.as_deref(), &devices.inputs));
        let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(SAMPLE_RATE));
        capture = Some(CaptureSide::new(consumer, SAMPLE_RATE)?);
        capture_ring = Some(producer);
        plans.push(Plan {
            kind: StreamKind::Capture,
            target,
        });
    }
    if config.playback {
        let target = target(
            config.output_device.as_deref(),
            &devices.outputs,
            false,
            events,
        );
        info.output = Some(stream_info(target.as_deref(), &devices.outputs));
        let (producer, consumer) = rtrb::RingBuffer::new(playback_ring_len(SAMPLE_RATE));
        playback = Some(PlaybackSide::new(producer, SAMPLE_RATE)?);
        playback_ring = Some(consumer);
        plans.push(Plan {
            kind: StreamKind::Playback,
            target: target.clone(),
        });
        if config.echo_reference {
            // The same device the call plays on (or the default sink).
            info.echo_reference = Some(stream_info(target.as_deref(), &devices.outputs));
            let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(SAMPLE_RATE));
            reference = Some(CaptureSide::new(consumer, SAMPLE_RATE)?);
            reference_ring = Some(producer);
            plans.push(Plan {
                kind: StreamKind::SinkMonitor,
                target,
            });
        }
    }
    if plans.is_empty() {
        return Err(AudioError::NothingToOpen);
    }
    let (stop, stopped) = pw::channel::channel::<()>();
    let (ready_tx, ready) = mpsc::sync_channel(1);
    let counters = counters.clone();
    let events = events.clone();
    let thread = std::thread::Builder::new()
        .name("twicecat-pw-streams".into())
        .spawn(move || {
            let rings = Rings {
                capture: capture_ring,
                playback: playback_ring,
                reference: reference_ring,
            };
            run(plans, rings, counters, events, stopped, ready_tx);
        })
        .map_err(|_| AudioError::Internal)?;
    match ready.recv() {
        Ok(Ok(thread_utils)) => Ok(Opened {
            thread_utils,
            streams: Streams {
                stop,
                thread: Some(thread),
            },
            capture,
            playback,
            reference,
            info,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(AudioError::Internal)
        }
    }
}

struct Rings {
    capture: Option<rtrb::Producer<f32>>,
    playback: Option<rtrb::Consumer<f32>>,
    reference: Option<rtrb::Producer<f32>>,
}

/// The device side of a stream, owned by its callbacks.
enum Side {
    Capture(rtrb::Producer<f32>),
    Playback(rtrb::Consumer<f32>),
}

struct StreamData {
    side: Side,
    /// The microphone: its audio paces (wakes) the processing thread.
    paces: bool,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
    streaming: bool,
}

impl StreamData {
    fn input(&self) -> bool {
        matches!(self.side, Side::Capture(_))
    }
}

/// 48 kHz mono `f32`, the only format we accept; PipeWire's adapter
/// converts from and to the device.
fn format_param() -> Result<Vec<u8>, AudioError> {
    let mut audio = AudioInfoRaw::new();
    audio.set_format(AudioFormat::F32LE);
    audio.set_rate(SAMPLE_RATE);
    audio.set_channels(1);
    let mut position = [0; pw::spa::param::audio::MAX_CHANNELS];
    position[0] = pw::spa::sys::SPA_AUDIO_CHANNEL_MONO;
    audio.set_position(position);
    let object = Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio.into(),
    };
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|_| AudioError::Internal)
}

/// The capture callback: the buffer's samples into the ring.
fn capture(stream: &pw::stream::Stream, data: &mut StreamData) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let Side::Capture(ring) = &mut data.side else {
        return;
    };
    let Some(chunk) = buffer.datas_mut().first_mut() else {
        return;
    };
    let offset = chunk.chunk().offset() as usize;
    let size = chunk.chunk().size() as usize;
    let Some(bytes) = chunk.data() else {
        return;
    };
    let end = offset.saturating_add(size).min(bytes.len());
    let start = offset.min(end);
    for sample in bytes[start..end].chunks_exact(SAMPLE_BYTES) {
        let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
        if ring.push(value).is_err() {
            data.counters
                .capture_overruns
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    if data.paces && end > start {
        data.counters
            .captured(ring.buffer().capacity() - ring.slots());
    }
}

/// The playback callback: as many samples as the graph asks for, from the
/// ring, silence when it runs dry.
fn playback(stream: &pw::stream::Stream, data: &mut StreamData) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let Side::Playback(ring) = &mut data.side else {
        return;
    };
    let requested = usize::try_from(buffer.requested()).unwrap_or(usize::MAX);
    let Some(chunk) = buffer.datas_mut().first_mut() else {
        return;
    };
    let written = match chunk.data() {
        Some(bytes) => {
            let room = bytes.len() / SAMPLE_BYTES;
            // `requested` is 0 when the graph does not say; one quantum then.
            let frames = if requested == 0 {
                room.min(crate::frame::FRAME_LEN)
            } else {
                room.min(requested)
            };
            for out in bytes[..frames * SAMPLE_BYTES].chunks_exact_mut(SAMPLE_BYTES) {
                let value = ring.pop().unwrap_or_else(|_| {
                    data.counters
                        .playback_underruns
                        .fetch_add(1, Ordering::Relaxed);
                    0.0
                });
                out.copy_from_slice(&value.to_le_bytes());
            }
            frames
        }
        None => 0,
    };
    let chunk = chunk.chunk_mut();
    *chunk.offset_mut() = 0;
    *chunk.stride_mut() = SAMPLE_BYTES as i32;
    *chunk.size_mut() = u32::try_from(written * SAMPLE_BYTES).unwrap_or(0);
}

fn state_changed(data: &mut StreamData, new: StreamState) {
    let message = match new {
        StreamState::Streaming => {
            data.streaming = true;
            return;
        }
        StreamState::Error(message) => message,
        // Lost after it ran: the device went away and nothing took over.
        StreamState::Unconnected if data.streaming => "the stream was disconnected".into(),
        _ => return,
    };
    data.streaming = false;
    let _ = data.events.send(IoEvent::StreamError {
        input: data.input(),
        message,
    });
}

fn properties(kind: StreamKind, target: Option<&str>) -> PropertiesBox {
    let mut props = PropertiesBox::new();
    for (key, value) in graph::stream_properties(kind, target, std::process::id()) {
        props.insert(key, value);
    }
    props
}

fn run(
    plans: Vec<Plan>,
    mut rings: Rings,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
    stopped: pw::channel::Receiver<()>,
    ready: mpsc::SyncSender<Result<Option<ThreadUtils>, AudioError>>,
) {
    let session = match Session::connect() {
        Ok(session) => session,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let format = match format_param() {
        Ok(format) => format,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut live: Vec<(StreamRc, StreamListener<StreamData>)> = Vec::new();
    for plan in plans {
        let (side, name, direction) = match plan.kind {
            StreamKind::Capture => {
                let Some(ring) = rings.capture.take() else {
                    continue;
                };
                (Side::Capture(ring), "twicecat-capture", Direction::Input)
            }
            StreamKind::SinkMonitor => {
                let Some(ring) = rings.reference.take() else {
                    continue;
                };
                (
                    Side::Capture(ring),
                    "twicecat-echo-reference",
                    Direction::Input,
                )
            }
            StreamKind::Playback => {
                let Some(ring) = rings.playback.take() else {
                    continue;
                };
                (Side::Playback(ring), "twicecat-playback", Direction::Output)
            }
        };
        let opened = (|| {
            let stream = StreamRc::new(
                session.core.clone(),
                name,
                properties(plan.kind, plan.target.as_deref()),
            )
            .map_err(|_| AudioError::Backend)?;
            let data = StreamData {
                side,
                paces: plan.kind == StreamKind::Capture,
                counters: counters.clone(),
                events: events.clone(),
                streaming: false,
            };
            let builder = stream
                .add_local_listener_with_user_data(data)
                .state_changed(|_, data, _, new| state_changed(data, new));
            let builder = match direction {
                Direction::Input => builder.process(capture),
                _ => builder.process(playback),
            };
            let listener = builder.register().map_err(|_| AudioError::Backend)?;
            let pod = Pod::from_bytes(&format).ok_or(AudioError::Internal)?;
            stream
                .connect(
                    direction,
                    None,
                    StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS | StreamFlags::RT_PROCESS,
                    &mut [pod],
                )
                .map_err(|_| AudioError::Backend)?;
            Ok::<_, AudioError>((stream, listener))
        })();
        match opened {
            Ok(stream) => live.push(stream),
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        }
    }
    let _ = ready.send(Ok(session.thread_utils()));
    let mainloop = session.mainloop.clone();
    let _stop = stopped.attach(session.mainloop.loop_(), move |()| mainloop.quit());
    session.mainloop.run();
    for (stream, listener) in live {
        let _ = stream.disconnect();
        drop(listener);
    }
}
