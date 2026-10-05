//! cpal's streams (Windows, macOS, and Linux without a PipeWire daemon),
//! opened on the processing thread: cpal streams are not `Send` on every
//! platform, so they live and die there. The callbacks only convert samples
//! and move them through lock-free rings.
//!
//! A stream whose device goes away, or that follows the default when the
//! default changes, is reopened there between frames ([`Devices`],
//! `super::reroute`): WASAPI never moves a stream itself. On Windows the
//! default devices are the *communications* ones (`crate::wasapi`), and the
//! echo reference ([`IoConfig::echo_reference`]) is WASAPI's loopback of the
//! output device: everything it plays, mixed by the system.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};

use super::reroute::{Placement, Reroute};
use super::{IoConfig, IoEvent, IoInfo, StreamInfo, process_with};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use super::{Realtime, realtime_wanted};
use crate::AudioError;
use crate::convert::{downmix, upmix};
use crate::frame::SAMPLE_RATE;
use crate::pipeline::{
    CaptureSide, FrameProcessor, IoCounters, Pipeline, PlaybackSide, capture_ring_len,
    playback_ring_len,
};

/// Calls `$build::<T>(...)` for the sample type of `$format`.
macro_rules! by_format {
    ($format:expr, $build:ident($($arg:expr),*)) => {
        match $format {
            SampleFormat::F32 => $build::<f32>($($arg),*),
            SampleFormat::I16 => $build::<i16>($($arg),*),
            SampleFormat::I32 => $build::<i32>($($arg),*),
            SampleFormat::U16 => $build::<u16>($($arg),*),
            SampleFormat::I8 => $build::<i8>($($arg),*),
            SampleFormat::U8 => $build::<u8>($($arg),*),
            _ => Err(AudioError::UnsupportedFormat),
        }
    };
}

/// The processing thread's body: opens the streams, reports what opened on
/// `ready`, then processes until `stop`, reopening streams as needed.
pub(super) fn run(
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
    counters.set_processing_thread();
    let mut devices = Devices::new(config, counters.clone(), events.clone());
    let mut info = IoInfo::default();
    let opened = (|| {
        let capture = match devices.config.capture {
            true => Some(devices.open_capture()?),
            false => None,
        };
        let playback = match devices.config.playback {
            true => Some(devices.open_playback()?),
            false => None,
        };
        Ok::<_, AudioError>((capture, playback))
    })();
    let (capture, playback) = match opened {
        Ok(opened) => opened,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let capture = capture.map(|(side, stream_info)| {
        info.input = Some(stream_info);
        side
    });
    let mut reference = None;
    let playback = playback.map(|(side, stream_info, loopback)| {
        info.output = Some(stream_info);
        if let Some((side, stream_info)) = loopback {
            info.echo_reference = Some(stream_info);
            reference = Some(side);
        }
        side
    });
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let raised = if realtime_wanted() {
        let (realtime, handle) = make_current_realtime();
        info.realtime = realtime;
        handle
    } else {
        None
    };
    let _ = ready.send(Ok(info));
    let mut pipeline = Pipeline::new(capture, playback, processor, counters);
    pipeline.set_echo_reference(reference);
    // Registered on this thread, which owns its COM objects; the callback
    // only sets flags and wakes the thread.
    #[cfg(target_os = "windows")]
    let _watch = {
        let reroute = devices.reroute.clone();
        crate::wasapi::EndpointWatch::register(move |event| match event {
            crate::wasapi::EndpointEvent::Devices => reroute.devices_changed(),
            crate::wasapi::EndpointEvent::CommunicationsDefault { input } => {
                reroute.default_changed(input);
            }
        })
    };
    process_with(pipeline, &stop, &events, |pipeline| {
        devices.maintain(pipeline);
    });
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if let Some(handle) = raised {
        let _ = audio_thread_priority::demote_current_thread_from_real_time(handle);
    }
    // The streams stop when dropped, on the thread that created them.
    drop(devices);
}

/// The least time between two reopenings of one direction, so a device
/// that fails as soon as it opens is not reopened in a loop.
const REOPEN_INTERVAL: Duration = Duration::from_millis(500);

/// How often a displaced stream (on the default instead of the chosen
/// device, or on none) looks for a way back without being told devices
/// changed: macOS and Linux without PipeWire have no device notifications
/// here, and on Windows a stream can fail where none follows (a device held
/// in exclusive mode, the microphone denied). Each look lists devices on
/// the processing thread, so not often.
const DISPLACED_RECHECK: Duration = Duration::from_secs(5);

/// A playback stream as opened: its side, what it is, and the echo
/// reference on the same device (Windows) if asked for.
type OpenedPlayback = (PlaybackSide, StreamInfo, Option<(CaptureSide, StreamInfo)>);

/// A running stream, kept until it is dropped: cpal's, or on Windows our
/// own WASAPI client's.
enum Held {
    Cpal(#[allow(dead_code, reason = "held to keep the stream running")] cpal::Stream),
    #[cfg(target_os = "windows")]
    Own(#[allow(dead_code, reason = "held to keep the stream running")] crate::wasapi::Stream),
}

/// Which kind of stream our WASAPI client opens.
#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Own {
    Capture,
    Render,
    Loopback,
}

/// The pipeline's end of a stream our WASAPI client opened (48 kHz: Windows
/// converts to and from the device's format).
#[cfg(target_os = "windows")]
enum OwnSide {
    Capture(rtrb::Consumer<f32>),
    Playback(rtrb::Producer<f32>),
}

#[cfg(target_os = "windows")]
impl OwnSide {
    fn capture(self) -> Result<CaptureSide, AudioError> {
        match self {
            Self::Capture(ring) => CaptureSide::new(ring, SAMPLE_RATE),
            Self::Playback(_) => Err(AudioError::Internal),
        }
    }

    fn playback(self) -> Result<PlaybackSide, AudioError> {
        match self {
            Self::Playback(ring) => PlaybackSide::new(ring, SAMPLE_RATE),
            Self::Capture(_) => Err(AudioError::Internal),
        }
    }
}

/// The open streams and what is needed to reopen them. Lives on the
/// processing thread.
struct Devices {
    host: cpal::Host,
    config: IoConfig,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
    reroute: Arc<Reroute>,
    capture: Option<Held>,
    playback: Option<Held>,
    /// Windows' loopback of the playback device (the echo reference).
    loopback: Option<Held>,
    /// The device each direction's stream is on (playback, capture).
    open_on: [Option<String>; 2],
    /// When each direction (playback, capture) was last reopened.
    last_reopen: [Option<Instant>; 2],
    /// When each direction was last checked for a way back while displaced.
    last_check: [Option<Instant>; 2],
    /// Whether the last reopening of each direction failed (reported once).
    failed: [bool; 2],
}

impl Devices {
    fn new(config: IoConfig, counters: Arc<IoCounters>, events: mpsc::Sender<IoEvent>) -> Self {
        Self {
            host: cpal::default_host(),
            config,
            counters,
            events,
            reroute: Arc::new(Reroute::new(std::thread::current())),
            capture: None,
            playback: None,
            loopback: None,
            open_on: [None, None],
            last_reopen: [None, None],
            last_check: [None, None],
            failed: [false, false],
        }
    }

    /// Opens and starts the capture stream.
    fn open_capture(&mut self) -> Result<(CaptureSide, StreamInfo), AudioError> {
        let (device, placement) = self.pick(true)?;
        #[cfg(target_os = "windows")]
        if let Some((stream, side, info)) = self.open_own(&device, Own::Capture) {
            let side = side.capture()?;
            self.capture = Some(Held::Own(stream));
            self.placed(true, placement, &info);
            return Ok((side, info));
        }
        let (format, config) = configure(&device, true)?;
        let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(config.sample_rate));
        let stream = by_format!(
            format,
            build_input(
                &device,
                config,
                producer,
                self.counters.clone(),
                self.events.clone(),
                self.reroute.clone()
            )
        )?;
        stream.play().map_err(|e| map_error(&e))?;
        self.capture = Some(Held::Cpal(stream));
        let side = CaptureSide::new(consumer, config.sample_rate)?;
        let info = stream_info(&device, &config);
        self.placed(true, placement, &info);
        Ok((side, info))
    }

    /// Opens and starts the playback stream, with the echo reference on the
    /// same device when asked for (Windows).
    fn open_playback(&mut self) -> Result<OpenedPlayback, AudioError> {
        let (device, placement) = self.pick(false)?;
        #[cfg(target_os = "windows")]
        if let Some((stream, side, info)) = self.open_own(&device, Own::Render) {
            let side = side.playback()?;
            self.playback = Some(Held::Own(stream));
            self.placed(false, placement, &info);
            let reference = self.open_reference(&device);
            return Ok((side, info, reference));
        }
        let (format, config) = configure(&device, false)?;
        let (producer, consumer) = rtrb::RingBuffer::new(playback_ring_len(config.sample_rate));
        let stream = by_format!(
            format,
            build_output(
                &device,
                config,
                consumer,
                self.counters.clone(),
                self.events.clone(),
                self.reroute.clone()
            )
        )?;
        stream.play().map_err(|e| map_error(&e))?;
        self.playback = Some(Held::Cpal(stream));
        let side = PlaybackSide::new(producer, config.sample_rate)?;
        let info = stream_info(&device, &config);
        self.placed(false, placement, &info);
        #[cfg(target_os = "windows")]
        let reference = self.open_reference(&device);
        #[cfg(not(target_os = "windows"))]
        let reference = None;
        Ok((side, info, reference))
    }

    /// The configured device for `input`, else the default, and where that
    /// puts the stream. Falling back is reported when it happens, not again
    /// on each reopening while the stream stays there.
    fn pick(&self, input: bool) -> Result<(cpal::Device, Placement), AudioError> {
        let id = if input {
            self.config.input_device.as_deref()
        } else {
            self.config.output_device.as_deref()
        };
        let (device, placement) = pick(&self.host, id, input)?;
        if placement == Placement::FellBack && self.reroute.placement(input) != Placement::FellBack
        {
            let _ = self.events.send(IoEvent::FellBackToDefault { input });
        }
        Ok((device, placement))
    }

    /// Records where a direction's stream now runs.
    fn placed(&mut self, input: bool, placement: Placement, info: &StreamInfo) {
        self.reroute.placed(input, placement);
        self.open_on[usize::from(input)] = Some(info.device_id.clone());
    }

    /// Windows: the echo reference when asked for, the loopback of the
    /// playback `device`: through our client, else cpal's. Best effort: a
    /// device that cannot be looped back still plays, without a reference.
    #[cfg(target_os = "windows")]
    fn open_reference(&mut self, device: &cpal::Device) -> Option<(CaptureSide, StreamInfo)> {
        if !self.config.echo_reference {
            return None;
        }
        match self.try_open_reference(device) {
            Ok(reference) => Some(reference),
            Err(e) => {
                self.loopback = None;
                tracing::warn!("the echo reference could not be opened ({e}); playing without it");
                None
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn try_open_reference(
        &mut self,
        device: &cpal::Device,
    ) -> Result<(CaptureSide, StreamInfo), AudioError> {
        if let Some((stream, side, info)) = self.open_own(device, Own::Loopback) {
            let side = side.capture()?;
            self.loopback = Some(Held::Own(stream));
            return Ok((side, info));
        }
        let (stream, reference, reference_info) =
            open_loopback(device, &self.events, &self.reroute)?;
        stream.play().map_err(|e| map_error(&e))?;
        self.loopback = Some(Held::Cpal(stream));
        Ok((reference, reference_info))
    }

    /// Windows: opens `device` through our own WASAPI client (streams marked
    /// as calls; `crate::wasapi`), unless `TESSERA_AUDIO_BACKEND=cpal`.
    /// `None` when it cannot, and cpal then opens the device instead.
    #[cfg(target_os = "windows")]
    fn open_own(
        &self,
        device: &cpal::Device,
        kind: Own,
    ) -> Option<(crate::wasapi::Stream, OwnSide, StreamInfo)> {
        if std::env::var_os("TESSERA_AUDIO_BACKEND").is_some_and(|v| v == "cpal") {
            return None;
        }
        let device_id = device.id().ok()?.to_string();
        let endpoint = device_id.strip_prefix(crate::wasapi::CPAL_PREFIX)?;
        let input = kind == Own::Capture;
        let (ours, side) = match kind {
            Own::Capture | Own::Loopback => {
                let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(SAMPLE_RATE));
                let side = match kind {
                    Own::Capture => crate::wasapi::Side::Capture {
                        ring: producer,
                        counters: self.counters.clone(),
                    },
                    _ => crate::wasapi::Side::Loopback { ring: producer },
                };
                (side, OwnSide::Capture(consumer))
            }
            Own::Render => {
                let (producer, consumer) = rtrb::RingBuffer::new(playback_ring_len(SAMPLE_RATE));
                let side = crate::wasapi::Side::Render {
                    ring: consumer,
                    counters: self.counters.clone(),
                };
                (side, OwnSide::Playback(producer))
            }
        };
        let reroute = self.reroute.clone();
        match crate::wasapi::open_stream(endpoint, ours, move || reroute.lost(input)) {
            Ok(stream) => {
                let device_name = device
                    .description()
                    .map(|d| d.name().to_owned())
                    .unwrap_or_else(|_| device_id.clone());
                let info = StreamInfo {
                    device_id,
                    device_name,
                    sample_rate: SAMPLE_RATE,
                    channels: 1,
                };
                Some((stream, side, info))
            }
            Err(e) => {
                tracing::warn!("WASAPI could not open the device ({e:?}); using cpal");
                None
            }
        }
    }

    /// Between frames: reopens what was asked for (at most every
    /// [`REOPEN_INTERVAL`] per direction), after checking whether a
    /// displaced stream can go back.
    fn maintain<P: FrameProcessor>(&mut self, pipeline: &mut Pipeline<P>) {
        for input in [false, true] {
            let wanted = if input {
                self.config.capture
            } else {
                self.config.playback
            };
            if !wanted {
                continue;
            }
            let i = usize::from(input);
            if self.reroute.take_default_moved(input) && !self.on_default(input) {
                self.reroute.reopen_later(input);
            }
            let displaced = matches!(
                self.reroute.placement(input),
                Placement::FellBack | Placement::Missing
            );
            let due =
                displaced && self.last_check[i].is_none_or(|t| t.elapsed() >= DISPLACED_RECHECK);
            if self.reroute.take_check(input) || due {
                self.last_check[i] = Some(Instant::now());
                if self.can_go_back(input) {
                    self.reroute.reopen_later(input);
                }
            }
            if !self.reroute.requested(input) {
                continue;
            }
            if self.last_reopen[i].is_some_and(|t| t.elapsed() < REOPEN_INTERVAL) {
                continue; // still asked for: retried on a later pass
            }
            self.reroute.take_request(input);
            self.last_reopen[i] = Some(Instant::now());
            if input {
                self.reopen_capture(pipeline);
            } else {
                self.reopen_playback(pipeline);
            }
        }
    }

    /// Whether the stream for `input` is on the current default device.
    fn on_default(&self, input: bool) -> bool {
        let default = default_device(&self.host, input)
            .and_then(|d| d.id().ok())
            .map(|id| id.to_string());
        default.is_some() && default == self.open_on[usize::from(input)]
    }

    /// Whether a displaced stream has somewhere better to be: the chosen
    /// device is back, or a device exists where there was none.
    fn can_go_back(&self, input: bool) -> bool {
        match self.reroute.placement(input) {
            Placement::FellBack => {
                let chosen = if input {
                    &self.config.input_device
                } else {
                    &self.config.output_device
                };
                chosen
                    .as_deref()
                    .is_some_and(|id| find(&self.host, id, input).is_some())
            }
            Placement::Missing => default_device(&self.host, input).is_some(),
            Placement::Chosen | Placement::Default => false,
        }
    }

    fn reopen_capture<P: FrameProcessor>(&mut self, pipeline: &mut Pipeline<P>) {
        // The old stream goes first, releasing its device.
        pipeline.replace_capture(None);
        self.capture = None;
        self.open_on[1] = None;
        match self.open_capture() {
            Ok((side, device)) => {
                pipeline.replace_capture(Some(side));
                self.switched(true, device);
            }
            Err(e) => {
                self.capture = None;
                self.lost_for_good(true, &e);
            }
        }
    }

    fn reopen_playback<P: FrameProcessor>(&mut self, pipeline: &mut Pipeline<P>) {
        pipeline.replace_playback(None);
        pipeline.set_echo_reference(None);
        self.loopback = None;
        self.playback = None;
        self.open_on[0] = None;
        match self.open_playback() {
            Ok((side, device, reference)) => {
                pipeline.replace_playback(Some(side));
                pipeline.set_echo_reference(reference.map(|(side, _)| side));
                self.switched(false, device);
            }
            Err(e) => {
                self.loopback = None;
                self.playback = None;
                self.lost_for_good(false, &e);
            }
        }
    }

    fn switched(&mut self, input: bool, device: StreamInfo) {
        self.failed[usize::from(input)] = false;
        let _ = self.events.send(IoEvent::Switched { input, device });
    }

    /// The stream could not be reopened: reported once; a device that comes
    /// later, or a later try that works, brings it back
    /// (`Placement::Missing`).
    fn lost_for_good(&mut self, input: bool, error: &AudioError) {
        self.reroute.placed(input, Placement::Missing);
        let i = usize::from(input);
        if !self.failed[i] {
            self.failed[i] = true;
            let _ = self.events.send(IoEvent::StreamError {
                input,
                message: match error {
                    AudioError::NoDevice | AudioError::DeviceNotFound => {
                        "the device is gone and no other could be opened".into()
                    }
                    other => format!("the stream could not be reopened: {other}"),
                },
            });
        }
    }
}

/// What a stream error means: reopen (the device is gone or the stream is
/// unusable); count it (an underrun or overrun, a glitch: WASAPI reports
/// them as errors); nothing (the backend moved the stream itself, or its
/// own thread got no real-time priority); or report it.
fn on_stream_error(
    e: &cpal::Error,
    input: bool,
    reroute: &Reroute,
    counters: Option<&IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) {
    match e.kind() {
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated => {
            reroute.lost(input);
        }
        cpal::ErrorKind::Xrun => {
            if let Some(counters) = counters {
                let xruns = if input {
                    &counters.capture_overruns
                } else {
                    &counters.playback_underruns
                };
                xruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied => {}
        _ => {
            let _ = events.send(IoEvent::StreamError {
                input,
                message: e.to_string(),
            });
        }
    }
}

/// Windows and macOS: raises the calling thread through the system's audio
/// scheduling, as Firefox does: MMCSS's "Audio" task on Windows; on macOS the
/// time-constraint policy for one frame per period (10 ms). The handle undoes
/// it. (Linux: PipeWire's real-time module, `crate::pw`.)
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn make_current_realtime() -> (Realtime, Option<audio_thread_priority::RtPriorityHandle>) {
    const CLASS: &str = if cfg!(target_os = "windows") {
        "MMCSS Audio task"
    } else {
        "time-constraint policy"
    };
    match audio_thread_priority::promote_current_thread_to_real_time(
        crate::FRAME_LEN as u32,
        SAMPLE_RATE,
    ) {
        Ok(handle) => (Realtime::GrantedBy(CLASS), Some(handle)),
        Err(e) => (Realtime::Refused(e.to_string()), None),
    }
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

/// The device with cpal ID `id`, if present for that direction.
fn find(host: &cpal::Host, id: &str, input: bool) -> Option<cpal::Device> {
    id.parse::<cpal::DeviceId>()
        .ok()
        .and_then(|id| host.device_by_id(&id))
        .filter(|d| {
            if input {
                d.supports_input()
            } else {
                d.supports_output()
            }
        })
}

/// The default device: on Windows the communications one when there is
/// one, else the host's.
fn default_device(host: &cpal::Host, input: bool) -> Option<cpal::Device> {
    #[cfg(target_os = "windows")]
    if let Some(device) =
        crate::wasapi::communications_default(input).and_then(|id| find(host, &id, input))
    {
        return Some(device);
    }
    if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    }
}

/// The configured device, else the default, and where that puts the
/// stream.
fn pick(
    host: &cpal::Host,
    id: Option<&str>,
    input: bool,
) -> Result<(cpal::Device, Placement), AudioError> {
    let placement = match id {
        Some(id) => match find(host, id, input) {
            Some(device) => return Ok((device, Placement::Chosen)),
            None => Placement::FellBack,
        },
        None => Placement::Default,
    };
    let device = default_device(host, input).ok_or(AudioError::NoDevice)?;
    Ok((device, placement))
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

/// The format for one direction of `device` (its default, at 48 kHz when
/// it can) and 10 ms buffers.
fn configure(
    device: &cpal::Device,
    input: bool,
) -> Result<(SampleFormat, StreamConfig), AudioError> {
    let (default, supported) = if input {
        let default = device.default_input_config();
        let supported = device.supported_input_configs().map(Iterator::collect);
        (default, supported)
    } else {
        let default = device.default_output_config();
        let supported = device.supported_output_configs().map(Iterator::collect);
        (default, supported)
    };
    let supported: Vec<_> = supported.map_err(|e| map_error(&e))?;
    let chosen = prefer_48k(default.map_err(|e| map_error(&e))?, supported.into_iter());
    let mut config = chosen.config();
    config.buffer_size = ten_ms_buffer(chosen.buffer_size(), config.sample_rate);
    Ok((chosen.sample_format(), config))
}

/// Windows: WASAPI's loopback of `device` (an input stream on an output
/// device is one), in the device's mix format, which shared-mode loopback
/// requires. Nothing arrives while the device plays nothing; the pipeline
/// then hands the processor silence. Its overruns are not counted: the
/// counters describe the microphone and the speakers.
#[cfg(target_os = "windows")]
fn open_loopback(
    device: &cpal::Device,
    events: &mpsc::Sender<IoEvent>,
    reroute: &Arc<Reroute>,
) -> Result<(cpal::Stream, CaptureSide, StreamInfo), AudioError> {
    let mix = device.default_output_config().map_err(|e| map_error(&e))?;
    let format = mix.sample_format();
    let mut config = mix.config();
    config.buffer_size = ten_ms_buffer(mix.buffer_size(), config.sample_rate);
    let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(config.sample_rate));
    let stream = by_format!(
        format,
        build_loopback(device, config, producer, events.clone(), reroute.clone())
    )?;
    let side = CaptureSide::new(consumer, config.sample_rate)?;
    Ok((stream, side, stream_info(device, &config)))
}

#[cfg(target_os = "windows")]
fn build_loopback<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut ring: rtrb::Producer<f32>,
    events: mpsc::Sender<IoEvent>,
    reroute: Arc<Reroute>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    device
        .build_input_stream::<T, _, _>(
            config,
            // Real-time: no allocation, no locks. A full ring drops the
            // newest samples; the pipeline trims a backlog on its side.
            move |data: &[T], _| {
                downmix(data, channels, |s| {
                    let _ = ring.push(s);
                });
            },
            // It goes with the playback stream: reopened with it. Its
            // glitches are not the speakers', so not counted.
            move |e| on_stream_error(&e, false, &reroute, None, &events),
            None,
        )
        .map_err(|e| map_error(&e))
}

fn build_input<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut ring: rtrb::Producer<f32>,
    counters: Arc<IoCounters>,
    events: mpsc::Sender<IoEvent>,
    reroute: Arc<Reroute>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let errors = counters.clone();
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
                counters.captured(ring.buffer().capacity() - ring.slots());
            },
            move |e| on_stream_error(&e, true, &reroute, Some(&errors), &events),
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
    reroute: Arc<Reroute>,
) -> Result<cpal::Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let errors = counters.clone();
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
            move |e| on_stream_error(&e, false, &reroute, Some(&errors), &events),
            None,
        )
        .map_err(|e| map_error(&e))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::AudioIo;

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

    /// Plays a quiet tone and keeps the echo reference it is handed.
    #[cfg(target_os = "windows")]
    struct ToneAndReference {
        tone: Vec<f32>,
        at: usize,
        heard: Arc<std::sync::Mutex<Vec<f32>>>,
    }

    #[cfg(target_os = "windows")]
    impl crate::FrameProcessor for ToneAndReference {
        fn process(&mut self, _: &crate::Frame, playback: &mut crate::Frame) {
            for s in playback.iter_mut() {
                *s = self.tone[self.at % self.tone.len()];
                self.at += 1;
            }
        }
        fn echo_reference(&mut self, played: &crate::Frame) {
            self.heard.lock().unwrap().extend_from_slice(played);
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "needs an audio output device; plays a faint 1 kHz tone (-46 dBFS) for 1.5 s"]
    fn wasapi_loopback_hands_the_processor_what_the_device_plays() {
        use crate::resample::tests::{sine, tone_amplitude};
        let heard = Arc::new(std::sync::Mutex::new(Vec::new()));
        // 1 kHz repeats every 48 samples: a whole number of periods loops.
        let io = AudioIo::start(
            IoConfig {
                playback: true,
                echo_reference: true,
                ..IoConfig::default()
            },
            ToneAndReference {
                tone: sine(SAMPLE_RATE, 1000.0, 0.005, 4800),
                at: 0,
                heard: heard.clone(),
            },
        )
        .expect("default output device");
        let reference = io.info().echo_reference.clone().expect("loopback opened");
        assert_eq!(
            reference.device_id,
            io.info().output.as_ref().unwrap().device_id
        );
        std::thread::sleep(Duration::from_millis(1500));
        drop(io);
        let heard = heard.lock().unwrap();
        assert!(
            heard.len() >= SAMPLE_RATE as usize,
            "{} samples",
            heard.len()
        );
        // The last second: the stream has settled. Other applications may
        // play too, so only the tone's own frequency is measured. What the
        // device does to the mix before loopback (a virtual device's fader:
        // Elgato Wave Link's is -6 dB here) scales it, which echo
        // cancellation adapts to; the tone must be there, not louder.
        let last = &heard[heard.len() - SAMPLE_RATE as usize..];
        let a = tone_amplitude(last, SAMPLE_RATE, 1000.0);
        assert!(
            (0.0005..0.0055).contains(&a),
            "tone amplitude {a} (played 0.005)"
        );
    }

    #[test]
    #[ignore = "needs an audio output device; plays silence"]
    fn a_lost_playback_stream_is_reopened_between_frames_at_most_twice_a_second() {
        let (tx, rx) = mpsc::channel();
        let counters = Arc::new(IoCounters::default());
        let mut devices = Devices::new(
            IoConfig {
                playback: true,
                echo_reference: cfg!(target_os = "windows"),
                ..IoConfig::default()
            },
            counters.clone(),
            tx,
        );
        let (side, first, reference) = devices.open_playback().expect("default output device");
        assert_eq!(reference.is_some(), cfg!(target_os = "windows"));
        let mut pipeline = Pipeline::new(None, Some(side), |_: &_, _: &mut _| {}, counters);
        pipeline.set_echo_reference(reference.map(|(side, _)| side));

        devices.reroute.lost(false);
        devices.maintain(&mut pipeline);
        let device = switched(&rx).expect("reopened");
        assert_eq!(device.device_id, first.device_id, "the same default device");
        assert!(devices.playback.is_some());
        assert_eq!(devices.loopback.is_some(), cfg!(target_os = "windows"));
        // On Windows both are our own client's (marked as calls), not cpal's
        // fallback.
        #[cfg(target_os = "windows")]
        if std::env::var_os("TESSERA_AUDIO_BACKEND").is_none() {
            assert!(matches!(devices.playback, Some(Held::Own(_))));
            assert!(matches!(devices.loopback, Some(Held::Own(_))));
        }
        pipeline.tick().expect("frames run on the new stream");

        // Again at once: deferred, not lost.
        devices.reroute.lost(false);
        devices.maintain(&mut pipeline);
        assert!(switched(&rx).is_none(), "within the interval");
        std::thread::sleep(REOPEN_INTERVAL);
        devices.maintain(&mut pipeline);
        assert!(switched(&rx).is_some(), "reopened after the interval");
    }

    /// The device of a playback switch among the events so far; others
    /// (the old stream's last errors) are skipped.
    fn switched(rx: &mpsc::Receiver<IoEvent>) -> Option<StreamInfo> {
        rx.try_iter().find_map(|e| match e {
            IoEvent::Switched {
                input: false,
                device,
            } => Some(device),
            _ => None,
        })
    }
}
