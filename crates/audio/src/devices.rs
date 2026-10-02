//! Device enumeration and hot-plug watching.
//!
//! cpal has no device-change notification, so [`DeviceWatcher`] polls the
//! device list. Default devices need no watching: cpal streams opened on the
//! default device follow the system default by themselves.

use std::sync::Arc;
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
    /// The system default device.
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

/// Drops the inputs that are also outputs. cpal's PipeWire backend offers
/// every sink as an input too (capturing what plays on it): not a
/// microphone. Only for PipeWire, where a sink and its capture share one ID;
/// an ALSA device can really be both.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn without_sinks(inputs: Vec<DeviceInfo>, outputs: &[DeviceInfo]) -> Vec<DeviceInfo> {
    inputs
        .into_iter()
        .filter(|i| !outputs.iter().any(|o| o.id == i.id))
        .collect()
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

/// Lists the devices of the platform's default host (PipeWire on Linux
/// when its daemon runs, else ALSA; CoreAudio; WASAPI). Blocking. On
/// PipeWire, inputs leave out sinks.
pub fn list_devices() -> Result<DeviceList, AudioError> {
    let host = cpal::default_host();
    let default_in = host
        .default_input_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
    let default_out = host
        .default_output_device()
        .and_then(|d| d.id().ok())
        .map(|id| id.to_string());
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
    #[cfg(target_os = "linux")]
    let inputs = if host.id() == cpal::HostId::PipeWire {
        without_sinks(inputs, &outputs)
    } else {
        inputs
    };
    Ok(DeviceList { inputs, outputs })
}

/// Polls the device list on its own thread and reports each change.
pub struct DeviceWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl DeviceWatcher {
    /// Polls [`list_devices`] every `interval` and calls `on_change` with the
    /// new list whenever it differs from the last one (not for the first).
    pub fn spawn(
        interval: Duration,
        on_change: impl FnMut(DeviceList) + Send + 'static,
    ) -> std::io::Result<Self> {
        Self::spawn_with(interval, list_devices, on_change)
    }

    /// Like [`spawn`](Self::spawn) with a custom enumerator (tests).
    pub fn spawn_with(
        interval: Duration,
        mut enumerate: impl FnMut() -> Result<DeviceList, AudioError> + Send + 'static,
        mut on_change: impl FnMut(DeviceList) + Send + 'static,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("tessera-audio-devices".into())
            .spawn(move || {
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
            stop,
            thread: Some(thread),
        })
    }
}

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

    #[test]
    fn sinks_are_not_microphones() {
        let dev = |id: &str| DeviceInfo {
            id: id.into(),
            name: id.into(),
            is_default: false,
        };
        let inputs = vec![dev("pipewire:mic"), dev("pipewire:speakers")];
        let outputs = vec![dev("pipewire:speakers")];
        assert_eq!(without_sinks(inputs, &outputs), vec![dev("pipewire:mic")]);
    }

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
