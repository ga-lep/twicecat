//! cpal's streams (Windows, macOS, and Linux without a PipeWire daemon),
//! opened on the processing thread: cpal streams are not `Send` on every
//! platform, so they live and die there. The callbacks only convert samples
//! and move them through lock-free rings.
//!
//! On Windows the echo reference ([`IoConfig::echo_reference`]) is WASAPI's
//! loopback of the output device: everything it plays, mixed by the system.

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
    #[cfg(target_os = "windows")]
    let mut reference = None;
    let opened = (|| {
        if config.capture {
            let (stream, side, stream_info) =
                open_input(&host, config.input_device.as_deref(), &counters, &events)?;
            streams.push(stream);
            capture = Some(side);
            info.input = Some(stream_info);
        }
        if config.playback {
            let (device, format, stream_config) =
                configure(&host, config.output_device.as_deref(), false, &events)?;
            let (stream, side, stream_info) =
                open_output(&device, format, stream_config, &counters, &events)?;
            streams.push(stream);
            playback = Some(side);
            info.output = Some(stream_info);
            // The same device the call plays on.
            #[cfg(target_os = "windows")]
            if config.echo_reference {
                let (stream, side, stream_info) = open_loopback(&device, &events)?;
                streams.push(stream);
                reference = Some(side);
                info.echo_reference = Some(stream_info);
            }
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
    #[cfg_attr(not(target_os = "windows"), allow(unused_mut))]
    let mut pipeline = Pipeline::new(capture, playback, processor, counters);
    #[cfg(target_os = "windows")]
    pipeline.set_echo_reference(reference);
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
    device: &cpal::Device,
    format: SampleFormat,
    config: StreamConfig,
    counters: &Arc<IoCounters>,
    events: &mpsc::Sender<IoEvent>,
) -> Result<Opened<PlaybackSide>, AudioError> {
    let (producer, consumer) = rtrb::RingBuffer::new(playback_ring_len(config.sample_rate));
    let stream = by_format!(
        format,
        build_output(device, config, consumer, counters.clone(), events.clone())
    )?;
    let side = PlaybackSide::new(producer, config.sample_rate)?;
    Ok((stream, side, stream_info(device, &config)))
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
) -> Result<Opened<CaptureSide>, AudioError> {
    let mix = device.default_output_config().map_err(|e| map_error(&e))?;
    let format = mix.sample_format();
    let mut config = mix.config();
    config.buffer_size = ten_ms_buffer(mix.buffer_size(), config.sample_rate);
    let (producer, consumer) = rtrb::RingBuffer::new(capture_ring_len(config.sample_rate));
    let stream = by_format!(
        format,
        build_loopback(device, config, producer, events.clone())
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
            move |e| {
                // A failing reference costs the cancellation of other
                // applications' sound, not the call: reported as playback.
                let _ = events.send(IoEvent::StreamError {
                    input: false,
                    message: format!("echo reference: {e}"),
                });
            },
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
}
