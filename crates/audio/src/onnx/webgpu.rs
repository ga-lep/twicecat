//! ONNX Runtime's WebGPU plugin provider (Microsoft's `onnxruntime-ep-webgpu`,
//! fetched by `scripts/fetch-webgpu-ep.sh`): where it is, registering it
//! with the environment, and keeping its GPU device to one thread at a time.
//!
//! The plugin is registered only while a WebGPU session lives (a [`Lease`]
//! each), and for a moment when the runtime loads, to see whether it finds
//! a GPU: ONNX Runtime keeps the plugin's GPU device until the plugin is
//! unregistered, and a device still alive at exit crashed the process inside
//! NVIDIA's Vulkan driver (its shader compiler thread, during the driver's
//! own teardown; 5 of 6 runs of the GPU tests, the sixth hung). ort releases
//! its environment only after the libraries' destructors have run, too late
//! for that.
//!
//! WebGPU sessions share one device, and using it from two threads at once
//! crashed the process (opening a session while another ran on the audio
//! thread; measured with the plugin 0.4.0). A thread opening, warming up or
//! closing a session holds [`Exclusive`] (re-entrant: its own runs go
//! through); runs elsewhere, on the audio thread, go through [`run`], which
//! never waits and skips the run instead.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use ort::ep::ExecutionProviderLibrary;
use ort::session::builder::SessionBuilder;

/// The WebGPU plugin provider release `scripts/fetch-webgpu-ep.sh` pins
/// (PyPI `onnxruntime-ep-webgpu`, MIT; needs ONNX Runtime 1.24.4 or later).
pub const WEBGPU_EP_VERSION: &str = "0.4.0";

/// The provider's name, as the plugin registers it.
const EP_NAME: &str = "WebGpuExecutionProvider";

fn library_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "onnxruntime_providers_webgpu.dll",
        "macos" => "libonnxruntime_providers_webgpu.dylib",
        _ => "libonnxruntime_providers_webgpu.so",
    }
}

/// Where `scripts/fetch-webgpu-ep.sh` put the WebGPU plugin provider, or
/// `TWICECAT_WEBGPU_EP_DIR`; `None` when neither exists.
pub fn default_webgpu_dir() -> Option<PathBuf> {
    let relative = format!("onnxruntime/webgpu-ep-{WEBGPU_EP_VERSION}");
    let dir = crate::paths::from_env_or_cache("TWICECAT_WEBGPU_EP_DIR", &relative)?;
    dir.is_dir().then_some(dir)
}

/// The plugin's directory, found when the runtime loaded.
static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The plugin, while registered (see the module docs).
static PLUGIN: Mutex<Option<ExecutionProviderLibrary>> = Mutex::new(None);

/// Notes where the plugin is (once, when the runtime loads).
pub(super) fn set_dir(dir: Option<PathBuf>) {
    let _ = DIR.set(dir);
}

/// Registers the plugin, unless it is; its device is listed if it found a
/// GPU it can use. Returns whether it is registered.
pub(super) fn register() -> bool {
    let mut plugin = PLUGIN.lock().unwrap_or_else(|e| e.into_inner());
    if plugin.is_some() {
        return true;
    }
    let Some(library) = DIR
        .get()
        .and_then(Option::as_ref)
        .map(|dir| dir.join(library_name()))
        .filter(|library| library.exists())
    else {
        return false;
    };
    match ort::environment::Environment::current()
        .and_then(|env| env.register_ep_library("webgpu", &library))
    {
        Ok(registered) => {
            *plugin = Some(registered);
            true
        }
        Err(e) => {
            tracing::info!("WebGPU provider not registered: {e}");
            false
        }
    }
}

/// Unregisters the plugin, which releases its GPU device. No WebGPU session
/// may be alive.
pub(super) fn unregister() {
    let plugin = PLUGIN.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(Err(e)) = plugin.map(ExecutionProviderLibrary::unregister) {
        tracing::warn!("WebGPU provider not unregistered: {e}");
    }
}

/// Whether the environment lists a WebGPU device (the plugin registered and
/// found an adapter).
pub(super) fn device_listed() -> bool {
    ort::environment::Environment::current()
        .is_ok_and(|env| env.devices().any(|d| d.ep().is_ok_and(|n| n == EP_NAME)))
}

/// Adds the plugin's device to a session, with graph capture or not. The
/// plugin takes one device and picks the adapter itself.
pub(super) fn add_to_session(
    builder: SessionBuilder,
    graph_capture: bool,
) -> Result<SessionBuilder, String> {
    let env = ort::environment::Environment::current().map_err(|e| e.to_string())?;
    let device = env
        .devices()
        .find(|d| d.ep().is_ok_and(|n| n == EP_NAME))
        .ok_or("no WebGPU device")?;
    let capture = if graph_capture { "1" } else { "0" };
    let mut options = vec![(format!("{EP_NAME}.enableGraphCapture"), capture.to_owned())];
    // More provider options, for measuring on other GPUs:
    // `TWICECAT_WEBGPU_EP_OPTIONS="maxNumPendingDispatches=512;..."`.
    for (key, value) in std::env::var("TWICECAT_WEBGPU_EP_OPTIONS")
        .unwrap_or_default()
        .split(';')
        .filter_map(|kv| kv.split_once('='))
    {
        options.push((format!("{EP_NAME}.{key}"), value.to_owned()));
    }
    builder
        .with_devices([device], Some(&options))
        .map_err(|e| e.to_string())
}

static OPENING: AtomicUsize = AtomicUsize::new(0);
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// WebGPU sessions alive (each holds a [`Lease`]).
static LIVE: AtomicUsize = AtomicUsize::new(0);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

thread_local! {
    /// How many `Exclusive` this thread holds.
    static HELD: Cell<usize> = const { Cell::new(0) };
}

/// Held while sessions are opened, warmed up or dropped (see the module
/// docs).
pub(crate) struct Exclusive {
    /// The outermost holder's.
    one: Option<MutexGuard<'static, ()>>,
}

impl Exclusive {
    /// Waits for other holders and for a run in progress elsewhere; runs
    /// that start meanwhile on other threads are skipped.
    pub(crate) fn take() -> Self {
        if HELD.get() > 0 {
            HELD.set(HELD.get() + 1);
            return Self { one: None };
        }
        let one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        OPENING.fetch_add(1, SeqCst);
        while RUNNING.load(SeqCst) > 0 {
            std::thread::sleep(Duration::from_micros(100));
        }
        HELD.set(1);
        Self { one: Some(one) }
    }
}

impl Drop for Exclusive {
    fn drop(&mut self) {
        HELD.set(HELD.get() - 1);
        if self.one.is_some() {
            OPENING.fetch_sub(1, SeqCst);
        }
    }
}

/// A WebGPU session's hold on the plugin: the first registers it, the last
/// one dropped unregisters it, which releases the GPU device. Taken and
/// dropped holding [`Exclusive`].
pub(crate) struct Lease(());

impl Lease {
    /// `None` when the plugin does not register.
    pub(crate) fn take(_held: &Exclusive) -> Option<Self> {
        if LIVE.fetch_add(1, SeqCst) == 0 && !register() {
            LIVE.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Self(()))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if LIVE.fetch_sub(1, SeqCst) == 1 {
            unregister();
        }
    }
}

/// Runs `work`, unless another thread holds [`Exclusive`] (`None`).
pub(crate) fn run<T>(work: impl FnOnce() -> T) -> Option<T> {
    if HELD.get() > 0 {
        return Some(work());
    }
    // Announce, then check (and the holder the other way round): one of the
    // two always sees the other.
    RUNNING.fetch_add(1, SeqCst);
    let result = (OPENING.load(SeqCst) == 0).then(work);
    RUNNING.fetch_sub(1, SeqCst);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_are_skipped_while_another_thread_holds_the_device() {
        // Re-entrant on the holder's thread.
        let held = Exclusive::take();
        assert_eq!(run(|| 1), Some(1));
        let nested = Exclusive::take();
        assert_eq!(run(|| 2), Some(2));
        drop(nested);
        // Skipped on another thread meanwhile, never waiting.
        let elsewhere = std::thread::spawn(|| run(|| 3)).join().unwrap();
        assert_eq!(elsewhere, None);
        drop(held);
        let after = std::thread::spawn(|| run(|| 4)).join().unwrap();
        assert_eq!(after, Some(4));
    }
}
