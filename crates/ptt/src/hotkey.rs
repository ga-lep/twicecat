//! The global-hotkey source (X11, Windows, macOS). The key is grabbed:
//! while it is bound, other applications do not receive it.
//!
//! global-hotkey's own threading rules apply to whoever starts the service:
//! on macOS the manager must be created on the main thread, and on Windows
//! on a thread that runs a message loop (the app's event loop thread).
//! Release is detected by polling every 50 ms on Windows and X11.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::binding::KeyBinding;
use crate::{PttEvent, Shared, Source};

/// How often the forwarding thread checks for a stop request.
const POLL: Duration = Duration::from_millis(200);

/// A registered hotkey and the thread forwarding its events.
pub(crate) struct Hotkeys {
    manager: GlobalHotKeyManager,
    hotkey: HotKey,
    stop: Arc<AtomicBool>,
    forward: Option<JoinHandle<()>>,
}

impl Hotkeys {
    /// Registers `key`; reports `Ready` or `Unavailable`.
    pub(crate) fn start(key: KeyBinding, shared: Arc<Shared>) -> Option<Self> {
        let hotkey = key.hotkey();
        let registered = GlobalHotKeyManager::new().and_then(|manager| {
            manager.register(hotkey)?;
            Ok(manager)
        });
        let manager = match registered {
            Ok(manager) => manager,
            Err(e) => {
                shared.notify(PttEvent::Unavailable {
                    source: Source::Hotkey,
                    reason: e.to_string(),
                });
                return None;
            }
        };
        // The event channel is global: drop what an earlier service left.
        while GlobalHotKeyEvent::receiver().try_recv().is_ok() {}
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let id = hotkey.id();
        let events = shared.clone();
        let forward = std::thread::Builder::new()
            .name("ptt-hotkey".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    if let Ok(event) = GlobalHotKeyEvent::receiver().recv_timeout(POLL)
                        && event.id == id
                    {
                        events.report(Source::Hotkey, event.state == HotKeyState::Pressed);
                    }
                }
            })
            .ok();
        shared.notify(PttEvent::Ready {
            source: Source::Hotkey,
            trigger: key.to_string(),
        });
        Some(Self {
            manager,
            hotkey,
            stop,
            forward,
        })
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        let _ = self.manager.unregister(self.hotkey);
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.forward.take() {
            let _ = thread.join();
        }
    }
}
