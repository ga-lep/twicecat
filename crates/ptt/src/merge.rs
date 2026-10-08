//! Several push-to-talk sources at once: the microphone transmits while any
//! of them is held. The release tail is the audio gate's (`tessera-audio`),
//! so a release is reported at once.

/// Where a press comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// A global hotkey (X11, Windows, macOS).
    Hotkey,
    /// The GlobalShortcuts portal (Wayland).
    Portal,
    /// A gamepad button.
    Gamepad,
    /// A key pressed in Tessera's own window, forwarded by the app.
    Window,
}

impl Source {
    fn bit(self) -> u8 {
        match self {
            Self::Hotkey => 1,
            Self::Portal => 2,
            Self::Gamepad => 4,
            Self::Window => 8,
        }
    }
}

/// Which sources are held, and whether that means transmitting.
#[derive(Debug, Default)]
pub(crate) struct Merger {
    held: u8,
    transmitting: bool,
}

impl Merger {
    /// Records a press or release; returns the new transmit state when it
    /// changes.
    pub(crate) fn set(&mut self, source: Source, pressed: bool) -> Option<bool> {
        if pressed {
            self.held |= source.bit();
        } else {
            self.held &= !source.bit();
        }
        let now = self.held != 0;
        (now != self.transmitting).then(|| {
            self.transmitting = now;
            now
        })
    }

    pub(crate) fn transmitting(&self) -> bool {
        self.transmitting
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmits_while_any_source_is_held() {
        let mut m = Merger::default();
        assert_eq!(m.set(Source::Hotkey, true), Some(true));
        assert_eq!(m.set(Source::Gamepad, true), None, "already on");
        assert_eq!(m.set(Source::Hotkey, false), None, "the pad still holds");
        assert!(m.transmitting());
        assert_eq!(m.set(Source::Gamepad, false), Some(false));
        assert!(!m.transmitting());
    }

    #[test]
    fn repeats_and_stray_releases_change_nothing() {
        let mut m = Merger::default();
        assert_eq!(m.set(Source::Window, false), None, "release without press");
        assert_eq!(m.set(Source::Window, true), Some(true));
        assert_eq!(m.set(Source::Window, true), None, "key repeat");
        assert_eq!(m.set(Source::Portal, false), None, "other source");
        assert_eq!(m.set(Source::Window, false), Some(false));
        assert_eq!(m.set(Source::Window, false), None);
    }
}
