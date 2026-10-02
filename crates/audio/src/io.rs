//! Device streams (cpal) and the processing thread.
//!
//! [`AudioIo::start`] opens the capture and playback devices on a dedicated
//! thread (cpal streams are not `Send` on every platform, so they live and
//! die there), then runs the [`Pipeline`] on that same thread. The device
//! callbacks only convert samples and move them through lock-free rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};

use crate::AudioError;
use crate::convert::{downmix, upmix};
use crate::frame::{Frame, SAMPLE_RATE, SILENCE};
use crate::pipeline::{
    CaptureSide, FrameProcessor, IoCounters, IoStats, Pipeline, PlaybackSide, capture_ring_len,
    playback_ring_len,
};

/// Which devices to open. A device ID comes from [`crate::DeviceInfo::id`];
/// `None` follows the system default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoConfig {
    /// Open a capture stream.
    pub capture: bool,
    /// Capture device.
    pub input_device: Option<String>,
    /// Open a playback stream.
    pub playback: bool,
    /// Playback device.
    pub output_device: Option<String>,
}

/// What was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    /// Device ID.
    pub device_id: String,
    /// Device name.
    pub device_name: String,
    /// The device's rate (the pipeline resamples to 48 kHz when it differs).
    pub sample_rate: u32,
    /// The device's channel count (mixed down or copied to mono).
    pub channels: u16,
}

/// Both directions, as opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoInfo {
    /// Capture stream, if opened.
    pub input: Option<StreamInfo>,
    /// Playback stream, if opened.
    pub output: Option<StreamInfo>,
}

/// Something the owner should know about; read with [`AudioIo::try_event`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IoEvent {
    /// The configured device was not found; the system default is used.
    FellBackToDefault {
        /// Capture (true) or playback.
        input: bool,
    },
    /// A stream failed, for example its device was unplugged.
    StreamError {
        /// Capture (true) or playback.
        input: bool,
        /// The backend's description (device names, no user data).
        message: String,
    },
    /// The processing thread stopped on an internal error.
    Stopped(AudioError),
}

/// A software stand-in for a microphone and speakers (tests, headless use):
/// called every 10 ms on the processing thread, in real time.
pub trait VirtualDevice: Send + 'static {
    /// Fills one microphone frame.
    fn capture(&mut self, frame: &mut Frame);
    /// Takes one frame that would be played.
    fn playback(&mut self, frame: &Frame);
}

/// Running audio I/O. Dropping it stops the streams and the thread.
pub struct AudioIo {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    counters: Arc<IoCounters>,
    events: mpsc::Receiver<IoEvent>,
    info: IoInfo,
}

impl AudioIo {
    /// Opens the devices and starts processing; returns once the streams run
    /// or failed to open.
    pub fn start(config: IoConfig, processor: impl FrameProcessor) -> Result<Self, AudioError> {
        if !config.capture && !config.playback {
            return Err(AudioError::NothingToOpen);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(IoCounters::default());
        let (events_tx, events) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let stop = stop.clone();
            let counters = counters.clone();
            std::thread::Builder::new()
                .name("tessera-audio-io".into())
                .spawn(move || run(config, processor, stop, counters, events_tx, ready_tx))
                .map_err(|_| AudioError::Internal)?
        };
        let info = match ready_rx.recv() {
            Ok(Ok(info)) => info,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(e);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(AudioError::Internal);
            }
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            counters,
            events,
            info,
        })
    }

    /// Runs `processor` against a [`VirtualDevice`] instead of real
    /// hardware, paced by a 10 ms timer.
    pub fn start_virtual(
        mut device: impl VirtualDevice,
        mut processor: impl FrameProcessor,
    ) -> Result<Self, AudioError> {
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(IoCounters::default());
        let (_events_tx, events) = mpsc::channel();
        let thread = {
            let stop = stop.clone();
            let counters = counters.clone();
            std::thread::Builder::new()
                .name("tessera-audio-virtual".into())
                .spawn(move || {
                    crate::denormals::flush_denormals_on_this_thread();
                    let tick = Duration::from_millis(10);
                    let mut next = Instant::now();
                    let mut capture = SILENCE;
                    while !stop.load(Ordering::Acquire) {
                        device.capture(&mut capture);
                        let mut playback = SILENCE;
                        processor.process(&capture, &mut playback);
                        device.playback(&playback);
                        counters.frames.fetch_add(1, Ordering::Relaxed);
                        next += tick;
                        let now = Instant::now();
                        if next > now {
                            std::thread::sleep(next - now);
                        } else {
                            // Fell behind (debugger, overloaded CI): resync.
                            next = now;
                        }
                    }
                })
                .map_err(|_| AudioError::Internal)?
        };
        let virtual_info = StreamInfo {
            device_id: "virtual".into(),
            device_name: "virtual".into(),
            sample_rate: SAMPLE_RATE,
            channels: 1,
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            counters,
            events,
            info: IoInfo {
                input: Some(virtual_info.clone()),
                output: Some(virtual_info),
            },
        })
    }

    /// The devices and formats in use.
    pub fn info(&self) -> &IoInfo {
        &self.info
    }

    /// Frame and xrun counters.
    pub fn stats(&self) -> IoStats {
        self.counters.snapshot()
    }

    /// The next pending event, if any.
    pub fn try_event(&self) -> Option<IoEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    config: IoConfig,
    processor: impl FrameProcessor,
    stop: Arc<AtomicBool>,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
    ready: mpsc::SyncSender<Result<IoInfo, AudioError>>,
) {
    // Everything that runs on this thread (the processor included) treats
    // denormals as zero; see `denormals`.
    crate::denormals::flush_denormals_on_this_thread();
    let host = cpal::default_host();
    let mut info = IoInfo::default();
    let mut streams = Vec::new();
    let mut capture = None;
    let mut playback = None;
    let opened = (|| {
        if config.capture {
            let (stream, side, stream_info) =
                open_input(&host, config.input_device.as_deref(), &counters, &events)?;
            streams.push(stream);
            capture = Some(side);
            info.input = Some(stream_info);
        }
        if config.playback {
            let (stream, side, stream_info) =
                open_output(&host, config.output_device.as_deref(), &counters, &events)?;
            streams.push(stream);
            playback = Some(side);
            info.output = Some(stream_info);
        }
        for stream in &streams {
            stream.play().map_err(|e| map_error(&e))?;
        }
        Ok(())
    })();
    if let Err(e) = opened {
        let _ = ready.send(Err(e));
        return;
    }
    let _ = ready.send(Ok(info));

    let mut pipeline = Pipeline::new(capture, playback, processor, counters);
    let tick = Duration::from_millis(10);
    let mut next_tick = Instant::now() + tick;
    while !stop.load(Ordering::Acquire) {
        let ran = if pipeline.has_capture() {
            pipeline.pump()
        } else if Instant::now() >= next_tick {
            next_tick += tick;
            pipeline.tick().map(|()| 1)
        } else {
            Ok(0)
        };
        match ran {
            Ok(0) => std::thread::sleep(Duration::from_millis(2)),
            Ok(_) => {}
            Err(e) => {
                let _ = events.send(IoEvent::Stopped(e));
                break;
            }
        }
    }
    // The streams stop when dropped, on the thread that created them.
    drop(streams);
}

fn map_error(e: &cpal::Error) -> AudioError {
    match e.kind() {
        cpal::ErrorKind::PermissionDenied => AudioError::PermissionDenied,
        cpal::ErrorKind::DeviceNotAvailable => AudioError::DeviceNotFound,
        cpal::ErrorKind::DeviceBusy => AudioError::DeviceBusy,
        cpal::ErrorKind::UnsupportedConfig => AudioError::UnsupportedFormat,
        _ => AudioError::Backend,
    }
}

/// The configured device, else the default (reporting the fallback).
fn pick(
    host: &cpal::Host,
    id: Option<&str>,
    input: bool,
    events: &mpsc::Sender<IoEvent>,
) -> Result<cpal::Device, AudioError> {
    if let Some(id) = id {
        let found = id
            .parse::<cpal::DeviceId>()
            .ok()
            .and_then(|id| host.device_by_id(&id))
            .filter(|d| {
                if input {
                    d.supports_input()
                } else {
                    d.supports_output()
                }
            });
        if let Some(device) = found {
            return Ok(device);
        }
        let _ = events.send(IoEvent::FellBackToDefault { input });
    }
    let default = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    default.ok_or(AudioError::NoDevice)
}

/// 10 ms device buffers (one frame) when the device reports its range;
/// PipeWire turns this into the stream's `node.latency`. The host's default
/// otherwise, which can be far larger.
fn ten_ms_buffer(range: &cpal::SupportedBufferSize, rate: u32) -> cpal::BufferSize {
    match *range {
        cpal::SupportedBufferSize::Range { min, max } if min <= max => {
            cpal::BufferSize::Fixed((rate / 100).clamp(min, max))
        }
        _ => cpal::BufferSize::Default,
    }
}

/// The device's default format, moved to 48 kHz when the device supports
/// that rate with the same format and channels (no resampling then).
fn prefer_48k(
    default: SupportedStreamConfig,
    supported: impl Iterator<Item = cpal::SupportedStreamConfigRange>,
) -> SupportedStreamConfig {
    if default.sample_rate() == SAMPLE_RATE {
        return default;
    }
    supported
        .filter(|r| {
            r.sample_format() == default.sample_format() && r.channels() == default.channels()
        })
        .find(|r| r.min_sample_rate() <= SAMPLE_RATE && SAMPLE_RATE <= r.max_sample_rate())
        .map(|r| r.with_sample_rate(SAMPLE_RATE))
        .unwrap_or(default)
}

fn stream_info(device: &cpal::Device, config: &StreamConfig) -> StreamInfo {
    let device_id = device.id().map(|id| id.to_string()).unwrap_or_default();
    let device_name = device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| device_id.clone());
    StreamInfo {
        device_id,
        device_name,
        sample_rate: config.sample_rate,
        channels: config.channels,
    }
}

type Opened<S> = (cpal::Stream, S, StreamInfo);

fn open_input(
    host: &cpal::Host,
    id: Option<&str>,
    counters: &Arc<IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) -> Result<Opened<CaptureSide>, AudioError> {
    let device = pick(host, id, true, events)?;
    let default = device.default_input_config().map_err(|e| map_error(&e))?;
    let supported = device
        .supported_input_configs()
        .map_err(|e| map_error(&e))?;
    let chosen = prefer_48k(default, supported);
    let mut config = chosen.config();
    config.buffer_size = ten_ms_buffer(chosen.buffer_size(), config.sample_rate);
    let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(config.sample_rate));
    macro_rules! build {
        ($t:ty) => {
            build_input::<$t>(&device, config, producer, counters.clone(), events.clone())
        };
    }
    let stream = match chosen.sample_format() {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I8 => build!(i8),
        SampleFormat::U8 => build!(u8),
        _ => Err(AudioError::UnsupportedFormat),
    }?;
    let side = CaptureSide::new(consumer, config.sample_rate)?;
    Ok((stream, side, stream_info(&device, &config)))
}

fn open_output(
    host: &cpal::Host,
    id: Option<&str>,
    counters: &Arc<IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) -> Result<Opened<PlaybackSide>, AudioError> {
    let device = pick(host, id, false, events)?;
    let default = device.default_output_config().map_err(|e| map_error(&e))?;
    let supported = device
        .supported_output_configs()
        .map_err(|e| map_error(&e))?;
    let chosen = prefer_48k(default, supported);
    let mut config = chosen.config();
    config.buffer_size = ten_ms_buffer(chosen.buffer_size(), config.sample_rate);
    let (producer, consumer) = rtrb::RingBuffer::new(playback_ring_len(config.sample_rate));
    macro_rules! build {
        ($t:ty) => {
            build_output::<$t>(&device, config, consumer, counters.clone(), events.clone())
        };
    }
    let stream = match chosen.sample_format() {
        SampleFormat::F32 => build!(f32),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I32 => build!(i32),
        SampleFormat::U16 => build!(u16),
        SampleFormat::I8 => build!(i8),
        SampleFormat::U8 => build!(u8),
        _ => Err(AudioError::UnsupportedFormat),
    }?;
    let side = PlaybackSide::new(producer, config.sample_rate)?;
    Ok((stream, side, stream_info(&device, &config)))
}

fn build_input<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut ring: rtrb::Producer<f32>,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    device
        .build_input_stream::<T, _, _>(
            config,
            // Real-time: no allocation, no locks.
            move |data: &[T], _| {
                downmix(data, channels, |s| {
                    if ring.push(s).is_err() {
                        counters.capture_overruns.fetch_add(1, Ordering::Relaxed);
                    }
                });
            },
            move |e| {
                let _ = events.send(IoEvent::StreamError {
                    input: true,
                    message: e.to_string(),
                });
            },
            None,
        )
        .map_err(|e| map_error(&e))
}

fn build_output<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut ring: rtrb::Consumer<f32>,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    device
        .build_output_stream::<T, _, _>(
            config,
            // Real-time: no allocation, no locks.
            move |out: &mut [T], _| {
                upmix(out, channels, || {
                    ring.pop().unwrap_or_else(|_| {
                        counters.playback_underruns.fetch_add(1, Ordering::Relaxed);
                        0.0
                    })
                });
            },
            move |e| {
                let _ = events.send(IoEvent::StreamError {
                    input: false,
                    message: e.to_string(),
                });
            },
            None,
        )
        .map_err(|e| map_error(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_buffers_are_ten_ms_within_the_range() {
        use cpal::{BufferSize, SupportedBufferSize as R};
        let range = |min, max| R::Range { min, max };
        assert_eq!(
            ten_ms_buffer(&range(16, 8192), 48_000),
            BufferSize::Fixed(480)
        );
        assert_eq!(
            ten_ms_buffer(&range(16, 8192), 44_100),
            BufferSize::Fixed(441)
        );
        assert_eq!(
            ten_ms_buffer(&range(1024, 8192), 48_000),
            BufferSize::Fixed(1024)
        );
        assert_eq!(
            ten_ms_buffer(&range(64, 256), 48_000),
            BufferSize::Fixed(256)
        );
        assert_eq!(ten_ms_buffer(&R::Unknown, 48_000), BufferSize::Default);
        assert_eq!(ten_ms_buffer(&range(512, 256), 48_000), BufferSize::Default);
    }

    #[test]
    fn nothing_to_open_is_an_error() {
        let err = AudioIo::start(IoConfig::default(), |_: &_, _: &mut _| {});
        assert!(matches!(err, Err(AudioError::NothingToOpen)));
    }

    struct Loop {
        sent: std::sync::Arc<std::sync::Mutex<Vec<f32>>>,
    }

    impl VirtualDevice for Loop {
        fn capture(&mut self, frame: &mut Frame) {
            frame.fill(0.5);
        }
        fn playback(&mut self, frame: &Frame) {
            self.sent.lock().unwrap().extend_from_slice(frame);
        }
    }

    #[test]
    fn virtual_device_runs_in_real_time() {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let io = AudioIo::start_virtual(
            Loop { sent: sent.clone() },
            |capture: &Frame, playback: &mut Frame| playback.copy_from_slice(capture),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        drop(io);
        let sent = sent.lock().unwrap();
        let frames = sent.len() / crate::FRAME_LEN;
        assert!((10..=30).contains(&frames), "{frames} frames in 200 ms");
        assert!(sent.iter().all(|s| *s == 0.5));
    }

    #[test]
    #[ignore = "needs an audio output device; plays 300 ms of silence"]
    fn opens_the_default_output_and_processes_frames() {
        let io = AudioIo::start(
            IoConfig {
                playback: true,
                ..IoConfig::default()
            },
            |_: &_, _: &mut _| {},
        )
        .expect("default output device");
        std::thread::sleep(Duration::from_millis(300));
        let stats = io.stats();
        assert!(stats.frames >= 20, "{stats:?}");
        assert!(io.info().output.is_some());
        assert!(io.info().input.is_none());
    }
}
