//! Device streams and the processing thread.
//!
//! [`AudioIo::start`] opens the capture and playback streams, then runs the
//! [`Pipeline`] on a dedicated thread. On Linux with a PipeWire daemon the
//! streams are our own PipeWire client's (`crate::pw`), on their own thread;
//! elsewhere they are cpal's, opened on the processing thread (cpal streams
//! are not `Send` on every platform, so they live and die there). The device
//! callbacks only convert samples and move them through lock-free rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod cpal;

use crate::AudioError;
use crate::frame::{Frame, SAMPLE_RATE, SILENCE};
use crate::pipeline::{FrameProcessor, IoCounters, IoStats, Pipeline};

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
    /// Also capture what the output device plays (every application's
    /// sound, a game's included) and hand it to the processor as the echo
    /// reference ([`FrameProcessor::echo_reference`]). PipeWire only;
    /// ignored elsewhere.
    pub echo_reference: bool,
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
    /// The echo reference stream (what the output device plays), if opened.
    pub echo_reference: Option<StreamInfo>,
    /// The processing thread's scheduling.
    pub realtime: Realtime,
}

/// Whether the processing thread runs with real-time priority, which keeps
/// it on time when the CPU is busy (a game). Asked for through PipeWire on
/// Linux, through the system's audio scheduling on Windows (MMCSS) and macOS
/// (the time-constraint policy); `TESSERA_AUDIO_REALTIME=0` turns the
/// request off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Realtime {
    /// Not asked for: Linux without PipeWire, a virtual device, or turned
    /// off.
    #[default]
    NotRequested,
    /// Granted, at this priority.
    Granted {
        /// The thread's real-time priority (`SCHED_FIFO`/`SCHED_RR`).
        priority: u32,
    },
    /// Granted by a scheduling class with no single priority number, named
    /// here (Windows' MMCSS, macOS's time-constraint policy).
    GrantedBy(&'static str),
    /// Asked for and refused; why. Processing runs at normal priority.
    Refused(String),
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
    thread: Option<Worker>,
    /// Our PipeWire streams (their own thread), stopped after processing.
    #[cfg(target_os = "linux")]
    streams: Option<crate::pw::Streams>,
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
        #[cfg(target_os = "linux")]
        if crate::pw::available() {
            return Self::start_pipewire(&config, processor);
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
                .spawn(move || cpal::run(config, processor, stop, counters, events_tx, ready_tx))
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
            thread: Some(Worker::Std(thread)),
            #[cfg(target_os = "linux")]
            streams: None,
            counters,
            events,
            info,
        })
    }

    /// PipeWire: the streams first (their context loads PipeWire's
    /// real-time module), then the processing thread, made through PipeWire
    /// so that module can raise its priority.
    #[cfg(target_os = "linux")]
    fn start_pipewire(
        config: &IoConfig,
        processor: impl FrameProcessor,
    ) -> Result<Self, AudioError> {
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(IoCounters::default());
        let (events_tx, events) = mpsc::channel();
        let opened = crate::pw::open(config, &counters, &events_tx)?;
        let mut info = opened.info;
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let stop = stop.clone();
            let counters = counters.clone();
            let (capture, playback, reference) =
                (opened.capture, opened.playback, opened.reference);
            let utils = opened.thread_utils;
            crate::pw::spawn_thread(utils, "tessera-voice", move || {
                crate::denormals::flush_denormals_on_this_thread();
                counters.set_processing_thread();
                let realtime = if realtime_wanted() {
                    crate::pw::make_current_realtime(utils)
                } else {
                    Realtime::NotRequested
                };
                let _ = ready_tx.send(realtime);
                let mut pipeline = Pipeline::new(capture, playback, processor, counters);
                pipeline.set_echo_reference(reference);
                process(pipeline, &stop, &events_tx);
            })?
        };
        info.realtime = ready_rx.recv().map_err(|_| AudioError::Internal)?;
        Ok(Self {
            stop,
            thread: Some(Worker::PipeWire(thread)),
            streams: Some(opened.streams),
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
            thread: Some(Worker::Std(thread)),
            #[cfg(target_os = "linux")]
            streams: None,
            counters,
            events,
            info: IoInfo {
                input: Some(virtual_info.clone()),
                output: Some(virtual_info),
                echo_reference: None,
                realtime: Realtime::NotRequested,
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

    /// The longest time from captured audio to processing it since the
    /// previous call, microseconds (0: no wake since).
    pub fn take_recent_max_wake_us(&self) -> u64 {
        self.counters.take_recent_max_wake_us()
    }

    /// The next pending event, if any.
    pub fn try_event(&self) -> Option<IoEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.counters.wake();
        if let Some(thread) = self.thread.take() {
            thread.join();
        }
        // The streams stop after the thread that reads them (and, on
        // PipeWire, after the processing thread the real-time module knows).
        #[cfg(target_os = "linux")]
        drop(self.streams.take());
    }
}

/// The processing thread.
enum Worker {
    Std(JoinHandle<()>),
    #[cfg(target_os = "linux")]
    PipeWire(crate::pw::PwThread),
}

impl Worker {
    fn join(self) {
        match self {
            Self::Std(thread) => {
                let _ = thread.join();
            }
            #[cfg(target_os = "linux")]
            Self::PipeWire(thread) => thread.join(),
        }
    }
}

/// Whether to ask for real-time priority (`TESSERA_AUDIO_REALTIME=0` says
/// no).
pub(super) fn realtime_wanted() -> bool {
    std::env::var_os("TESSERA_AUDIO_REALTIME").is_none_or(|v| v != "0")
}

/// Runs the pipeline until `stop`: paced by the capture stream (its
/// callback wakes this thread when audio comes), or by a 10 ms timer
/// without one.
fn process<P: FrameProcessor>(
    mut pipeline: Pipeline<P>,
    stop: &AtomicBool,
    events: &mpsc::Sender<IoEvent>,
) {
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
            // Asleep until the capture callback wakes us; the timeout only
            // bounds a missed wake (a stream that stopped delivering).
            Ok(0) if pipeline.has_capture() => std::thread::park_timeout(CAPTURE_WAIT),
            Ok(0) => std::thread::sleep(next_tick.saturating_duration_since(Instant::now())),
            Ok(_) => {}
            Err(e) => {
                let _ = events.send(IoEvent::Stopped(e));
                break;
            }
        }
    }
}

/// The longest the processing thread sleeps without being woken.
const CAPTURE_WAIT: Duration = Duration::from_millis(50);

#[cfg(test)]
mod tests {
    use super::*;

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
}
