//! Our own WASAPI streams (Windows), in place of cpal's, so that each one
//! is marked as a call: `IAudioClient2::SetClientProperties` with
//! `AudioCategory_Communications`, which must come before `Initialize`;
//! cpal initializes its client itself, with no way in between. Windows then
//! treats the streams as
//! communications (its ducking of other applications, a Bluetooth headset's
//! headset profile, a driver's communications processing mode).
//!
//! Each stream runs on its own thread at MMCSS priority, woken by WASAPI's
//! event every device period, and only moves samples between the device
//! buffer and the pipeline's lock-free ring. Windows converts to and from
//! the device's format (`AUTOCONVERTPCM`): the rings carry 48 kHz mono
//! `f32`, as our PipeWire client's do, so nothing resamples on our side.
//! A device that goes away ends the thread and calls `on_lost`, and the
//! processing thread reopens the stream (`io::reroute`).

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, AUDCLNT_STREAMOPTIONS_NONE,
    AudioCategory_Communications, AudioClientProperties, IAudioCaptureClient, IAudioClient2,
    IAudioRenderClient, WAVEFORMATEX,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
use windows::core::HSTRING;

use crate::AudioError;
use crate::frame::SAMPLE_RATE;
use crate::pipeline::IoCounters;

/// `WAVE_FORMAT_IEEE_FLOAT` (mmreg.h).
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

/// The device buffer asked for: 10 ms (one frame), in 100 ns units.
const BUFFER_100NS: i64 = 100_000;

/// How long a stream thread waits for WASAPI's event before looking again:
/// a loopback gets none while nothing plays, and a stop must not wait long.
const WAIT_MS: u32 = 100;

/// What a stream moves, and where.
pub(crate) enum Side {
    /// The microphone, into the capture ring.
    Capture {
        ring: rtrb::Producer<f32>,
        counters: Arc<IoCounters>,
    },
    /// The speakers, from the playback ring.
    Render {
        ring: rtrb::Consumer<f32>,
        counters: Arc<IoCounters>,
    },
    /// What the render device plays (the echo reference), into its ring.
    Loopback { ring: rtrb::Producer<f32> },
}

/// A running stream; dropping it stops it and waits for its thread.
pub(crate) struct Stream {
    /// Shared with the thread; closed when both are done with it.
    stop: Arc<OwnedEvent>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: a valid event handle owned by `self.stop`.
        let _ = unsafe { SetEvent(self.stop.0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An event handle, closed when dropped.
struct OwnedEvent(HANDLE);

// SAFETY: an event handle may be signalled and waited on from any thread.
unsafe impl Send for OwnedEvent {}
// SAFETY: as above; `SetEvent` and waits are thread-safe.
unsafe impl Sync for OwnedEvent {}

impl OwnedEvent {
    /// An auto-reset event, not signalled.
    fn new() -> Result<Self, AudioError> {
        // SAFETY: no security attributes, no name; the handle is owned and
        // closed by `Self`.
        unsafe { CreateEventW(None, false, false, None) }
            .map(Self)
            .map_err(|_| AudioError::Backend)
    }
}

impl Drop for OwnedEvent {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateEventW and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Opens the endpoint with WASAPI ID `endpoint` (cpal's ID without its
/// `wasapi:` prefix) for `side`, and starts it. Returns once it runs or
/// failed to; `on_lost` is called (from the stream's thread) if the device
/// later goes away or the stream fails.
pub(crate) fn open(
    endpoint: &str,
    side: Side,
    on_lost: impl Fn() + Send + 'static,
) -> Result<Stream, AudioError> {
    let stop = Arc::new(OwnedEvent::new()?);
    let (ready_tx, ready) = mpsc::sync_channel(1);
    let endpoint = endpoint.to_owned();
    let thread_stop = stop.clone();
    let name = match side {
        Side::Capture { .. } => "twicecat-wasapi-capture",
        Side::Render { .. } => "twicecat-wasapi-render",
        Side::Loopback { .. } => "twicecat-wasapi-loopback",
    };
    let thread = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let client = match Client::open(&endpoint, &side) {
                Ok(client) => client,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            if !client.run(side, &thread_stop) {
                on_lost();
            }
        })
        .map_err(|_| AudioError::Internal)?;
    match ready.recv() {
        Ok(Ok(())) => Ok(Stream {
            stop,
            thread: Some(thread),
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

/// One initialised and started WASAPI client, on its thread.
struct Client {
    audio: IAudioClient2,
    io: Io,
    event: OwnedEvent,
    /// The device buffer's length in frames (render).
    buffer_frames: u32,
}

enum Io {
    Capture(IAudioCaptureClient),
    Render(IAudioRenderClient),
}

impl Client {
    /// Activates, marks (call streams), initialises and starts the client.
    fn open(endpoint: &str, side: &Side) -> Result<Self, AudioError> {
        let loopback = matches!(side, Side::Loopback { .. });
        let enumerator = super::enumerator().ok_or(AudioError::Backend)?;
        let event = OwnedEvent::new()?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 1,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let mut flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if loopback {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        // SAFETY: COM calls on this thread, which created `enumerator` (COM
        // set up by `super::enumerator`); every pointer argument points to
        // a live local for the duration of its call; the event handle stays
        // open in `Self` for as long as the client may signal it.
        unsafe {
            let device = enumerator
                .GetDevice(&HSTRING::from(endpoint))
                .map_err(|_| AudioError::DeviceNotFound)?;
            let audio: IAudioClient2 = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|_| AudioError::Backend)?;
            if !loopback {
                let properties = AudioClientProperties {
                    cbSize: size_of::<AudioClientProperties>() as u32,
                    bIsOffload: false.into(),
                    eCategory: AudioCategory_Communications,
                    Options: AUDCLNT_STREAMOPTIONS_NONE,
                };
                audio
                    .SetClientProperties(&properties)
                    .map_err(|_| AudioError::Backend)?;
            }
            audio
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    BUFFER_100NS,
                    0,
                    &format,
                    None,
                )
                .map_err(|_| AudioError::UnsupportedFormat)?;
            audio
                .SetEventHandle(event.0)
                .map_err(|_| AudioError::Backend)?;
            let buffer_frames = audio.GetBufferSize().map_err(|_| AudioError::Backend)?;
            let io = match side {
                Side::Render { .. } => {
                    Io::Render(audio.GetService().map_err(|_| AudioError::Backend)?)
                }
                Side::Capture { .. } | Side::Loopback { .. } => {
                    Io::Capture(audio.GetService().map_err(|_| AudioError::Backend)?)
                }
            };
            audio.Start().map_err(|_| AudioError::Backend)?;
            Ok(Self {
                audio,
                io,
                event,
                buffer_frames,
            })
        }
    }

    /// Moves samples until `stop`; `false` if the stream failed (the device
    /// went away), `true` if it was stopped.
    fn run(self, mut side: Side, stop: &OwnedEvent) -> bool {
        let raised = audio_thread_priority::promote_current_thread_to_real_time(
            crate::FRAME_LEN as u32,
            SAMPLE_RATE,
        )
        .ok();
        let handles = [stop.0, self.event.0];
        let stopped = loop {
            // SAFETY: both handles stay open for the whole loop (owned by
            // `stop` and `self.event`).
            let woke = unsafe { WaitForMultipleObjects(&handles, false, WAIT_MS) };
            if woke == WAIT_OBJECT_0 {
                break true;
            }
            let moved = match (&self.io, &mut side) {
                (Io::Capture(io), Side::Capture { ring, counters }) => {
                    self.capture(io, ring, Some(counters))
                }
                (Io::Capture(io), Side::Loopback { ring }) => self.capture(io, ring, None),
                (Io::Render(io), Side::Render { ring, counters }) => {
                    self.render(io, ring, counters)
                }
                _ => Err(()),
            };
            if moved.is_err() {
                break false;
            }
        };
        // SAFETY: the client is started; stopping it is always allowed.
        let _ = unsafe { self.audio.Stop() };
        if let Some(handle) = raised {
            let _ = audio_thread_priority::demote_current_thread_from_real_time(handle);
        }
        stopped
    }

    /// Takes every captured packet into `ring` (silence flagged as such is
    /// written as zeros). Real-time: no allocation, no locks.
    fn capture(
        &self,
        io: &IAudioCaptureClient,
        ring: &mut rtrb::Producer<f32>,
        counters: Option<&Arc<IoCounters>>,
    ) -> Result<(), ()> {
        loop {
            // SAFETY: COM calls on the client's thread. GetBuffer hands a
            // buffer of `frames` mono f32 samples (the format we asked for)
            // that stays valid until ReleaseBuffer, which follows.
            unsafe {
                let packet = io.GetNextPacketSize().map_err(|_| ())?;
                if packet == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                io.GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|_| ())?;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null();
                for i in 0..frames as usize {
                    let s = if silent {
                        0.0
                    } else {
                        *data.cast::<f32>().add(i)
                    };
                    if ring.push(s).is_err()
                        && let Some(counters) = counters
                    {
                        counters
                            .capture_overruns
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                io.ReleaseBuffer(frames).map_err(|_| ())?;
            }
            if let Some(counters) = counters {
                counters.captured(ring.buffer().capacity() - ring.slots());
            }
        }
    }

    /// Fills the free part of the device buffer from `ring` (silence where
    /// it runs dry, counted). Real-time: no allocation, no locks.
    fn render(
        &self,
        io: &IAudioRenderClient,
        ring: &mut rtrb::Consumer<f32>,
        counters: &IoCounters,
    ) -> Result<(), ()> {
        // SAFETY: COM calls on the client's thread. GetBuffer hands
        // `free` frames of mono f32 (our format), writable until
        // ReleaseBuffer, which follows.
        unsafe {
            let padding = self.audio.GetCurrentPadding().map_err(|_| ())?;
            let free = self.buffer_frames.saturating_sub(padding);
            if free == 0 {
                return Ok(());
            }
            let data = io.GetBuffer(free).map_err(|_| ())?.cast::<f32>();
            for i in 0..free as usize {
                let s = ring.pop().unwrap_or_else(|_| {
                    counters
                        .playback_underruns
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    0.0
                });
                *data.add(i) = s;
            }
            io.ReleaseBuffer(free, 0).map_err(|_| ())?;
        }
        Ok(())
    }
}
