//! Tessera push-to-talk: turns a held key or button into "transmit on/off".
//!
//! Sources, merged so that any one held means transmit:
//! - a global keyboard shortcut: global-hotkey on X11, Windows and macOS,
//!   the GlobalShortcuts portal on Wayland;
//! - a gamepad button (gilrs);
//! - a key pressed in Tessera's own window, forwarded by the app (the only
//!   keyboard source in gamescope's game mode, which has no global
//!   shortcuts).
//!
//! The service reports [`PttEvent`]s through a callback; the app forwards
//! `Transmit` to the core as `Command::PushToTalk`. The release tail is the
//! audio gate's. This crate never depends on matrix-sdk.

use std::sync::{Arc, Mutex};

mod binding;
mod gamepad;
mod hotkey;
mod merge;
#[cfg(target_os = "linux")]
mod portal;

pub use binding::{BindingError, GamepadButton, KeyBinding};
pub use merge::Source;

use merge::Merger;

/// The app ID the portal knows us by (as the keychain service).
const APP_ID: &str = "io.github.ga_lep.Tessera";

/// What the service reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PttEvent {
    /// Start (`true`) or stop transmitting.
    Transmit(bool),
    /// A source is listening. `trigger` says what to press: the binding,
    /// or for the portal the key the user confirmed in its dialog.
    Ready { source: Source, trigger: String },
    /// A source could not start; the others still work.
    Unavailable { source: Source, reason: String },
}

/// Receives the service's events. Called from the service's threads, in
/// order, with an internal lock held: keep it short (send on a channel).
pub type Sink = Arc<dyn Fn(PttEvent) + Send + Sync>;

/// Which global keyboard shortcut mechanism a session has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardBackend {
    /// The GlobalShortcuts portal (Wayland).
    Portal,
    /// global-hotkey (X11, Windows, macOS).
    Hotkey,
    /// None: only the window key and gamepads (gamescope, no display).
    WindowOnly,
}

impl KeyboardBackend {
    /// The mechanism for this session, from its environment.
    pub fn detect() -> Self {
        Self::for_session(|name| std::env::var(name).ok())
    }

    /// The mechanism for a session with these environment variables.
    pub fn for_session(env: impl Fn(&str) -> Option<String>) -> Self {
        if !cfg!(target_os = "linux") {
            return Self::Hotkey;
        }
        let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
        let desktop = env("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if desktop
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("gamescope"))
        {
            // SteamOS game mode: no global shortcuts; X11 grabs on
            // gamescope's Xwayland only see focused windows.
            Self::WindowOnly
        } else if set("WAYLAND_DISPLAY")
            || env("XDG_SESSION_TYPE").is_some_and(|t| t.eq_ignore_ascii_case("wayland"))
        {
            Self::Portal
        } else if set("DISPLAY") {
            Self::Hotkey
        } else {
            Self::WindowOnly
        }
    }
}

/// What to listen to.
#[derive(Clone, Debug)]
pub struct PttConfig {
    /// The global keyboard shortcut, if any.
    pub key: Option<KeyBinding>,
    /// The gamepad button, if any.
    pub gamepad: Option<GamepadButton>,
    /// The keyboard mechanism ([`KeyboardBackend::detect`] by default).
    pub keyboard: KeyboardBackend,
}

impl Default for PttConfig {
    fn default() -> Self {
        Self {
            key: None,
            gamepad: None,
            keyboard: KeyboardBackend::detect(),
        }
    }
}

/// State shared with the sources.
pub(crate) struct Shared {
    merger: Mutex<Merger>,
    sink: Sink,
}

impl Shared {
    /// A source was pressed or released.
    pub(crate) fn report(&self, source: Source, pressed: bool) {
        let mut merger = self.merger.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(on) = merger.set(source, pressed) {
            // Under the lock, so transitions reach the sink in order.
            (self.sink)(PttEvent::Transmit(on));
        }
    }

    pub(crate) fn notify(&self, event: PttEvent) {
        let _merger = self.merger.lock().unwrap_or_else(|e| e.into_inner());
        (self.sink)(event);
    }
}

/// The running push-to-talk sources. Dropping it stops them and, if it was
/// transmitting, reports `Transmit(false)`.
///
/// Start and drop it on the app's main thread: global-hotkey needs that on
/// macOS (main thread) and Windows (a thread with a message loop).
pub struct PttService {
    shared: Arc<Shared>,
    // Dropped in this order before the final release.
    hotkeys: Option<hotkey::Hotkeys>,
    #[cfg(target_os = "linux")]
    portal: Option<portal::Portal>,
    gamepads: Option<gamepad::Gamepads>,
}

impl PttService {
    /// Starts the configured sources. Each reports `Ready` or `Unavailable`
    /// through `sink`; the portal runs on `runtime`.
    pub fn start(config: PttConfig, runtime: &tokio::runtime::Handle, sink: Sink) -> Self {
        let shared = Arc::new(Shared {
            merger: Mutex::new(Merger::default()),
            sink,
        });
        let mut service = Self {
            shared: shared.clone(),
            hotkeys: None,
            #[cfg(target_os = "linux")]
            portal: None,
            gamepads: None,
        };
        if let Some(key) = config.key {
            match config.keyboard {
                KeyboardBackend::Hotkey => {
                    service.hotkeys = hotkey::Hotkeys::start(key, shared.clone());
                }
                #[cfg(target_os = "linux")]
                KeyboardBackend::Portal => {
                    service.portal = Some(portal::Portal::start(
                        key.portal_trigger(),
                        shared.clone(),
                        runtime,
                    ));
                }
                #[cfg(not(target_os = "linux"))]
                KeyboardBackend::Portal => {
                    shared.notify(PttEvent::Unavailable {
                        source: Source::Portal,
                        reason: "the shortcuts portal exists on Linux only".into(),
                    });
                }
                KeyboardBackend::WindowOnly => {}
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = runtime;
        if let Some(button) = config.gamepad {
            service.gamepads = Some(gamepad::Gamepads::start(button, shared));
        }
        service
    }

    /// The bound key was pressed or released in Tessera's own window.
    pub fn window_key(&self, pressed: bool) {
        self.shared.report(Source::Window, pressed);
    }

    /// Whether the microphone should transmit now.
    pub fn transmitting(&self) -> bool {
        self.shared
            .merger
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .transmitting()
    }
}

impl Drop for PttService {
    fn drop(&mut self) {
        self.hotkeys = None;
        #[cfg(target_os = "linux")]
        {
            self.portal = None;
        }
        self.gamepads = None;
        for source in [
            Source::Hotkey,
            Source::Portal,
            Source::Gamepad,
            Source::Window,
        ] {
            self.shared.report(source, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(vars: &[(&str, &str)]) -> KeyboardBackend {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        KeyboardBackend::for_session(|name| {
            vars.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
        })
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn each_linux_session_gets_its_keyboard_mechanism() {
        assert_eq!(
            session(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")]),
            KeyboardBackend::Portal,
            "Wayland, even with Xwayland"
        );
        assert_eq!(
            session(&[("XDG_SESSION_TYPE", "wayland")]),
            KeyboardBackend::Portal
        );
        assert_eq!(
            session(&[("XDG_SESSION_TYPE", "x11"), ("DISPLAY", ":0")]),
            KeyboardBackend::Hotkey
        );
        assert_eq!(
            session(&[("XDG_CURRENT_DESKTOP", "gamescope"), ("DISPLAY", ":0")]),
            KeyboardBackend::WindowOnly,
            "SteamOS game mode"
        );
        assert_eq!(
            session(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", "")]),
            KeyboardBackend::WindowOnly
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn other_systems_use_global_hotkeys() {
        assert_eq!(session(&[]), KeyboardBackend::Hotkey);
    }

    /// A service with only the window source (no hotkey, portal or pad is
    /// touched): presses become transitions, and dropping it lets go.
    #[test]
    fn the_window_key_transmits_and_dropping_the_service_lets_go() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let config = PttConfig {
            key: None,
            gamepad: None,
            keyboard: KeyboardBackend::WindowOnly,
        };
        let service = PttService::start(
            config,
            runtime.handle(),
            Arc::new(move |e| seen.lock().unwrap().push(e)),
        );
        service.window_key(true);
        service.window_key(true);
        assert!(service.transmitting());
        service.window_key(false);
        service.window_key(true);
        drop(service);
        assert_eq!(
            *events.lock().unwrap(),
            [
                PttEvent::Transmit(true),
                PttEvent::Transmit(false),
                PttEvent::Transmit(true),
                PttEvent::Transmit(false),
            ]
        );
    }

    /// A key binding with no global mechanism starts nothing: the window
    /// key is the only keyboard source.
    #[test]
    fn window_only_sessions_start_no_global_shortcut() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let config = PttConfig {
            key: Some("F13".parse().unwrap()),
            gamepad: None,
            keyboard: KeyboardBackend::WindowOnly,
        };
        let service = PttService::start(
            config,
            runtime.handle(),
            Arc::new(move |e| seen.lock().unwrap().push(e)),
        );
        assert!(service.hotkeys.is_none());
        drop(service);
        assert!(events.lock().unwrap().is_empty());
    }
}
