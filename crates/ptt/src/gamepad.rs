//! The gamepad source: one button on any connected pad (gilrs; on SteamOS
//! and the Steam Frame, Steam Input's virtual pads).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use gilrs::{EventType, Gilrs};

use crate::binding::GamepadButton;
use crate::{PttEvent, Shared, Source};

/// How often the thread checks for a stop request.
const POLL: Duration = Duration::from_millis(100);

/// What the source needs from a gilrs event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PadEvent {
    Pressed(gilrs::Button),
    Released(gilrs::Button),
    /// The pad disconnected.
    Gone,
}

impl PadEvent {
    fn from_gilrs(event: &EventType) -> Option<Self> {
        match event {
            EventType::ButtonPressed(b, _) => Some(Self::Pressed(*b)),
            EventType::ButtonReleased(b, _) => Some(Self::Released(*b)),
            EventType::Disconnected | EventType::Dropped => Some(Self::Gone),
            _ => None,
        }
    }
}

/// Which pads (by gilrs ID) hold the button. Held while any pad holds it; a
/// pad that disconnects lets go.
#[derive(Debug, Default)]
pub(crate) struct PadState {
    held: HashSet<usize>,
}

impl PadState {
    /// Applies one event; returns whether the button is held afterwards.
    pub(crate) fn apply(&mut self, pad: usize, event: PadEvent, button: GamepadButton) -> bool {
        match event {
            PadEvent::Pressed(b) if b == button.button() => {
                self.held.insert(pad);
            }
            PadEvent::Released(b) if b == button.button() => {
                self.held.remove(&pad);
            }
            PadEvent::Gone => {
                self.held.remove(&pad);
            }
            _ => {}
        }
        !self.held.is_empty()
    }
}

/// The running gamepad thread.
pub(crate) struct Gamepads {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Gamepads {
    pub(crate) fn start(button: GamepadButton, shared: Arc<Shared>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ptt-gamepad".into())
            .spawn(move || run(button, &shared, &flag))
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Gamepads {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(button: GamepadButton, shared: &Shared, stop: &AtomicBool) {
    let mut gilrs = match Gilrs::new() {
        Ok(gilrs) => gilrs,
        Err(e) => {
            shared.notify(PttEvent::Unavailable {
                source: Source::Gamepad,
                reason: e.to_string(),
            });
            return;
        }
    };
    shared.notify(PttEvent::Ready {
        source: Source::Gamepad,
        trigger: button.to_string(),
    });
    let mut state = PadState::default();
    let mut held = false;
    while !stop.load(Ordering::Relaxed) {
        let Some(event) = gilrs.next_event_blocking(Some(POLL)) else {
            continue;
        };
        let Some(pad_event) = PadEvent::from_gilrs(&event.event) else {
            continue;
        };
        let now = state.apply(usize::from(event.id), pad_event, button);
        if now != held {
            held = now;
            shared.report(Source::Gamepad, now);
        }
    }
    if held {
        shared.report(Source::Gamepad, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_pad_holding_the_button_holds_and_a_lost_pad_lets_go() {
        let button: GamepadButton = "left_thumb".parse().unwrap();
        let thumb = gilrs::Button::LeftThumb;
        let mut s = PadState::default();
        assert!(
            !s.apply(0, PadEvent::Pressed(gilrs::Button::South), button),
            "other button"
        );
        assert!(s.apply(0, PadEvent::Pressed(thumb), button));
        assert!(s.apply(1, PadEvent::Pressed(thumb), button));
        assert!(
            s.apply(0, PadEvent::Released(thumb), button),
            "pad 1 still holds"
        );
        assert!(!s.apply(1, PadEvent::Gone, button), "pad 1 disconnected");
        assert!(!s.apply(1, PadEvent::Released(thumb), button));
    }
}
