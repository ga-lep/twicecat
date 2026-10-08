//! cpal's streams (Windows, macOS, and Linux without a PipeWire daemon),
//! opened on the processing thread: cpal streams are not `Send` on every
//! platform, so they live and die there. The callbacks only convert samples
//! and move them through lock-free rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, SupportedStreamConfig};

use super::{IoConfig, IoEvent, IoInfo, StreamInfo, process};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use super::{Realtime, realtime_wanted};
use crate::AudioError;
use crate::convert::{downmix, upmix};
use crate::frame::SAMPLE_RATE;
use crate::pipeline::{
    CaptureSide, FrameProcessor, IoCounters, Pipeline, PlaybackSide, capture_ring_len,
    playback_ring_len,
};

/// The processing thread's body: opens the streams, reports what opened on
/// `ready`, then processes until `stop`.
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
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let raised = if realtime_wanted() {
        let (realtime, handle) = make_current_realtime();
        info.realtime = realtime;
        handle
    } else {
        None
    };
    let _ = ready.send(Ok(info));
    let pipeline = Pipeline::new(capture, playback, processor, counters);
    process(pipeline, &stop, &events);
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    if let Some(handle) = raised {
        let _ = audio_thread_priority::demote_current_thread_from_real_time(handle);
    }
    // The streams stop when dropped, on the thread that created them.
    drop(streams);
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

/// The device for one direction, its format (the default, at 48 kHz when
/// it can) and 10 ms buffers.
fn configure(
    host: &cpal::Host,
    id: Option<&str>,
    input: bool,
    events: &mpsc::Sender<IoEvent>,
) -> Result<(cpal::Device, SampleFormat, StreamConfig), AudioError> {
    let device = pick(host, id, input, events)?;
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
    Ok((device, chosen.sample_format(), config))
}

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

fn open_input(
    host: &cpal::Host,
    id: Option<&str>,
    counters: &Arc<IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) -> Result<Opened<CaptureSide>, AudioError> {
    let (device, format, config) = configure(host, id, true, events)?;
    let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(config.sample_rate));
    let stream = by_format!(
        format,
        build_input(&device, config, producer, counters.clone(), events.clone())
    )?;
    let side = CaptureSide::new(consumer, config.sample_rate)?;
    Ok((stream, side, stream_info(&device, &config)))
}

fn open_output(
    host: &cpal::Host,
    id: Option<&str>,
    counters: &Arc<IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) -> Result<Opened<PlaybackSide>, AudioError> {
    let (device, format, config) = configure(host, id, false, events)?;
    let (producer, consumer) = rtrb::RingBuffer::new(playback_ring_len(config.sample_rate));
    let stream = by_format!(
        format,
        build_output(&device, config, consumer, counters.clone(), events.clone())
    )?;
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
                counters.captured(ring.buffer().capacity() - ring.slots());
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
}
