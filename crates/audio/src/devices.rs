//! Device enumeration and hot-plug watching.
//!
//! On Linux with a PipeWire daemon the list comes from its registry and
//! changes arrive as events (`crate::pw`). On Windows the list is cpal's and
//! the MMDevice API's notifications say when to read it again
//! (`crate::wasapi`); the default devices are the *communications* ones.
//! Elsewhere cpal has no device-change notification, so [`DeviceWatcher`]
//! polls the list. Streams that follow the default are moved by the audio
//! I/O itself when it changes (`crate::io`).

use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};

use crate::AudioError;

/// One input or output device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier (cpal `DeviceId` text, e.g. a PipeWire node name),
    /// suitable for saving in settings.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The system default device (on Windows the default communications
    /// device, the one calls use).
    pub is_default: bool,
}

/// All devices, inputs and outputs separately.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// Capture devices.
    pub inputs: Vec<DeviceInfo>,
    /// Playback devices.
    pub outputs: Vec<DeviceInfo>,
}

fn info(device: &cpal::Device, default_id: Option<&str>) -> Option<DeviceInfo> {
    let id = device.id().ok()?.to_string();
    let name = device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| id.clone());
    Some(DeviceInfo {
        is_default: default_id == Some(id.as_str()),
        id,
        name,
    })
}

/// The default device's ID for capture (`input`) or playback: the
/// communications one on Windows when there is one, else cpal's.
pub(crate) fn default_id(host: &cpal::Host, input: bool) -> Option<String> {
    #[cfg(target_os = "windows")]
    if let Some(id) = crate::wasapi::communications_default(input) {
        return Some(id);
    }
    let device = if input {
        host.default_input_device()
    } else {
        host.default_output_device()
    };
    device.and_then(|d| d.id().ok()).map(|id| id.to_string())
}

/// Lists the devices: from PipeWire's registry on Linux when its daemon
/// runs, else from the platform's default cpal host (ALSA, CoreAudio,
/// WASAPI). Blocking.
pub fn list_devices() -> Result<DeviceList, AudioError> {
    #[cfg(target_os = "linux")]
    if crate::pw::available() {
        return crate::pw::list();
    }
    let host = cpal::default_host();
    let default_in = default_id(&host, true);
    let default_out = default_id(&host, false);
    let inputs: Vec<DeviceInfo> = host
        .input_devices()
        .map_err(|_| AudioError::Enumerate)?
        .filter_map(|d| info(&d, default_in.as_deref()))
        .collect();
    let outputs: Vec<DeviceInfo> = host
        .output_devices()
        .map_err(|_| AudioError::Enumerate)?
        .filter_map(|d| info(&d, default_out.as_deref()))
        .collect();
    Ok(DeviceList { inputs, outputs })
}

/// Reports each change of the device list: PipeWire registry events on
/// Linux when its daemon runs, else a poll on its own thread.
pub struct DeviceWatcher {
    #[cfg(target_os = "linux")]
    _events: Option<crate::pw::Watcher>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DeviceWatcher {
    /// Calls `on_change` with the new list whenever it differs from the
    /// last one (not for the first): at once with PipeWire; on Windows when
    /// the MMDevice API reports a change (polling every 30 s as well, or
    /// every `interval` if its notifications cannot be had); elsewhere
    /// polling [`list_devices`] every `interval`.
    pub fn spawn(
        interval: Duration,
        on_change: impl FnMut(DeviceList) + Send + 'static,
    ) -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        if crate::pw::available() {
            let shared = Arc::new(Mutex::new(on_change));
            let events = shared.clone();
            match crate::pw::Watcher::spawn(move |list| {
                if let Ok(mut f) = events.lock() {
                    f(list);
                }
            }) {
                Ok(watcher) => {
                    return Ok(Self {
                        _events: Some(watcher),
                        stop: Arc::new(AtomicBool::new(true)),
                        thread: None,
                    });
                }
                Err(_) => {
                    return Self::spawn_with(interval, list_devices, move |list| {
                        if let Ok(mut f) = shared.lock() {
                            f(list);
                        }
                    });
                }
            }
        }
        Self::spawn_polling(
            interval,
            list_devices,
            on_change,
            cfg!(target_os = "windows"),
        )
    }

    /// Polls `enumerate` every `interval` (tests, and systems without
    /// PipeWire).
    pub fn spawn_with(
        interval: Duration,
        enumerate: impl FnMut() -> Result<DeviceList, AudioError> + Send + 'static,
        on_change: impl FnMut(DeviceList) + Send + 'static,
    ) -> std::io::Result<Self> {
        Self::spawn_polling(interval, enumerate, on_change, false)
    }

    /// Polls `enumerate`; with `endpoint_events` (Windows) the MMDevice API's
    /// notifications wake the thread too, and the poll is only a fallback.
    fn spawn_polling(
        interval: Duration,
        mut enumerate: impl FnMut() -> Result<DeviceList, AudioError> + Send + 'static,
        mut on_change: impl FnMut(DeviceList) + Send + 'static,
        endpoint_events: bool,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tessera-audio-devices".into())
            .spawn(move || {
                // Registered on this thread, which owns its COM objects;
                // the callback only wakes it.
                #[cfg(target_os = "windows")]
                let (_watch, interval) = {
                    let watch = endpoint_events
                        .then(|| {
                            let me = std::thread::current();
                            crate::wasapi::EndpointWatch::register(move |_| me.unpark())
                        })
                        .flatten();
                    let interval = if watch.is_some() {
                        interval.max(WINDOWS_FALLBACK_POLL)
                    } else {
                        interval
                    };
                    (watch, interval)
                };
                #[cfg(not(target_os = "windows"))]
                let _ = endpoint_events;
                let mut last = enumerate().ok();
                while !flag.load(Ordering::Acquire) {
                    std::thread::park_timeout(interval);
                    if flag.load(Ordering::Acquire) {
                        break;
                    }
                    // A failed poll keeps the last list (transient errors).
                    if let Ok(now) = enumerate()
                        && last.as_ref() != Some(&now)
                    {
                        last = Some(now.clone());
                        on_change(now);
                    }
                }
            })?;
        Ok(Self {
            #[cfg(target_os = "linux")]
            _events: None,
            stop,
            thread: Some(thread),
        })
    }
}

/// On Windows, how often the device list is read when no notification came
/// (a missed one, or a change the API does not report).
#[cfg(target_os = "windows")]
const WINDOWS_FALLBACK_POLL: Duration = Duration::from_secs(30);

impl Drop for DeviceWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;

    fn list(inputs: &[&str]) -> DeviceList {
        DeviceList {
            inputs: inputs
                .iter()
                .map(|id| DeviceInfo {
                    id: (*id).into(),
                    name: (*id).into(),
                    is_default: false,
                })
                .collect(),
            outputs: Vec::new(),
        }
    }

    #[test]
    fn watcher_reports_changes_only() {
        let script = Arc::new(Mutex::new(vec![
            Ok(list(&["mic"])),
            Ok(list(&["mic"])),
            Err(AudioError::Enumerate),
            Ok(list(&["mic", "headset"])),
            Ok(list(&["mic", "headset"])),
            Ok(list(&["headset"])),
        ]));
        let feed = script.clone();
        let enumerate = move || {
            let mut s = feed.lock().unwrap();
            if s.is_empty() {
                Ok(list(&["headset"]))
            } else {
                s.remove(0)
            }
        };
        let (tx, rx) = mpsc::channel();
        let watcher = DeviceWatcher::spawn_with(Duration::from_millis(1), enumerate, move |l| {
            let _ = tx.send(l);
        })
        .unwrap();
        let first = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first, list(&["mic", "headset"]));
        assert_eq!(second, list(&["headset"]));
        drop(watcher);
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "no more changes"
        );
    }

    #[test]
    fn dropping_the_watcher_stops_it_promptly() {
        let watcher = DeviceWatcher::spawn_with(
            Duration::from_secs(3600),
            || Ok(DeviceList::default()),
            |_| {},
        )
        .unwrap();
        let start = std::time::Instant::now();
        drop(watcher);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn listing_devices_does_not_panic_without_hardware() {
        // CI machines may have no audio device or daemon at all.
        let _ = list_devices();
    }
}
