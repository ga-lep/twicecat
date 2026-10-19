//! Push-to-talk bindings: a keyboard shortcut, a mouse button and a gamepad
//! button. All are kept as strings in the audio settings
//! (`ctrl+shift+KeyT`, `back`, `left_thumb`), so the core stores them
//! without knowing this crate.

use std::fmt;
use std::str::FromStr;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};

/// Why a binding string was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingError {
    /// Not a key combination (modifiers first, then one key).
    #[error("not a key combination: {0}")]
    Key(String),
    /// Not a gamepad button name.
    #[error("not a gamepad button: {0}")]
    Button(String),
    /// Not a mouse button this system has, or the left or right one.
    #[error("not a mouse button: {0}")]
    Mouse(String),
}

/// A keyboard shortcut: optional modifiers and one key, written as
/// global-hotkey writes it (`shift+control+KeyT`, `F13`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBinding(HotKey);

/// The modifier keys held with a key pressed in Twicecat's window. `logo`
/// is the Windows key, or Command on macOS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

impl Held {
    fn modifiers(self) -> Modifiers {
        let mut mods = Modifiers::empty();
        for (held, modifier) in [
            (self.ctrl, Modifiers::CONTROL),
            (self.alt, Modifiers::ALT),
            (self.shift, Modifiers::SHIFT),
            (self.logo, Modifiers::SUPER),
        ] {
            if held {
                mods |= modifier;
            }
        }
        mods
    }
}

impl KeyBinding {
    pub(crate) fn hotkey(self) -> HotKey {
        self.0
    }

    /// The binding for the key `code` pressed with `held`, from a window's
    /// key event. `code` is the key's W3C code name (`KeyT`, `Digit5`,
    /// `F13`), as winit and Iced name physical keys. A modifier key alone,
    /// or a key global-hotkey has no name for, is refused.
    pub fn from_key(code: &str, held: Held) -> Result<Self, BindingError> {
        let key = Self::key_code(code).ok_or_else(|| BindingError::Key(code.to_owned()))?;
        Ok(Self(HotKey::new(Some(held.modifiers()), key)))
    }

    /// Whether `code` (a W3C code name) is this binding's key, whatever the
    /// modifiers: a release counts even when a modifier was let go first.
    pub fn is_key(self, code: &str) -> bool {
        Self::key_code(code) == Some(self.0.key)
    }

    /// Whether `held` are exactly this binding's modifiers: a press counts
    /// only then, as for the global shortcut.
    pub fn has_modifiers(self, held: Held) -> bool {
        self.0.mods == held.modifiers()
    }

    /// The key a W3C code name stands for, if global-hotkey can bind it.
    fn key_code(code: &str) -> Option<Code> {
        // One token: no `+`, so it is read as a key, never as a modifier.
        if code.is_empty() || code.contains('+') {
            return None;
        }
        HotKey::from_str(code).ok().map(|hotkey| hotkey.key)
    }

    /// The trigger suggested to the GlobalShortcuts portal, in the XDG
    /// shortcuts format (`CTRL+SHIFT+t`). `None` for a key without a known
    /// keysym name: the user then picks a key in the portal's dialog. The
    /// compositor decides in any case; this is only a suggestion.
    pub fn portal_trigger(self) -> Option<String> {
        let key = keysym_name(self.0.key)?;
        let mut out = String::new();
        for (modifier, name) in [
            (Modifiers::CONTROL, "CTRL"),
            (Modifiers::ALT, "ALT"),
            (Modifiers::SHIFT, "SHIFT"),
            (Modifiers::SUPER, "LOGO"),
        ] {
            if self.0.mods.contains(modifier) {
                out.push_str(name);
                out.push('+');
            }
        }
        out.push_str(&key);
        Some(out)
    }
}

impl FromStr for KeyBinding {
    type Err = BindingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        HotKey::from_str(s.trim())
            .map(Self)
            .map_err(|_| BindingError::Key(s.to_owned()))
    }
}

impl fmt::Display for KeyBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.into_string())
    }
}

/// The xkb keysym name of a key, for the portal.
fn keysym_name(code: Code) -> Option<String> {
    let name = code.to_string();
    if let Some(letter) = name.strip_prefix("Key") {
        return Some(letter.to_ascii_lowercase());
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return Some(digit.to_owned());
    }
    if let Some(digit) = name.strip_prefix("Numpad")
        && digit.len() == 1
    {
        return Some(format!("KP_{digit}"));
    }
    if name.len() <= 3 && name.starts_with('F') && name[1..].parse::<u8>().is_ok() {
        return Some(name);
    }
    let named = match code {
        Code::Space => "space",
        Code::Tab => "Tab",
        Code::Enter => "Return",
        Code::Escape => "Escape",
        Code::Backspace => "BackSpace",
        Code::Backquote => "grave",
        Code::Minus => "minus",
        Code::Equal => "equal",
        Code::BracketLeft => "bracketleft",
        Code::BracketRight => "bracketright",
        Code::Backslash => "backslash",
        Code::Semicolon => "semicolon",
        Code::Quote => "apostrophe",
        Code::Comma => "comma",
        Code::Period => "period",
        Code::Slash => "slash",
        Code::Insert => "Insert",
        Code::Delete => "Delete",
        Code::Home => "Home",
        Code::End => "End",
        Code::PageUp => "Page_Up",
        Code::PageDown => "Page_Down",
        Code::ArrowUp => "Up",
        Code::ArrowDown => "Down",
        Code::ArrowLeft => "Left",
        Code::ArrowRight => "Right",
        Code::Pause => "Pause",
        Code::ScrollLock => "Scroll_Lock",
        Code::PrintScreen => "Print",
        Code::NumpadAdd => "KP_Add",
        Code::NumpadSubtract => "KP_Subtract",
        Code::NumpadMultiply => "KP_Multiply",
        Code::NumpadDivide => "KP_Divide",
        Code::NumpadDecimal => "KP_Decimal",
        Code::NumpadEnter => "KP_Enter",
        _ => return None,
    };
    Some(named.to_owned())
}

/// A mouse button: the middle one (the wheel's click), the two side buttons
/// (`back` and `forward`, "mouse 4" and "mouse 5"), or another one by the
/// system's own number (`button12`), which only X11, Wayland and macOS
/// report. Left and right click, so they are never bindings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseButton(Mouse);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Mouse {
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// The lowest number of another button, as winit numbers them: on X11 1 to
/// 9 are the left, middle and right buttons, the wheel's four directions,
/// back and forward (Wayland numbers others by their evdev code, from 0x110);
/// on macOS 0 to 4 are left, right, middle, back and forward. Windows has no
/// other buttons.
const FIRST_OTHER: Option<u16> = if cfg!(target_os = "macos") {
    Some(5)
} else if cfg!(target_os = "windows") {
    None
} else {
    Some(10)
};

impl MouseButton {
    /// The middle button (the wheel's click).
    pub const MIDDLE: Self = Self(Mouse::Middle);
    /// The back side button ("mouse 4").
    pub const BACK: Self = Self(Mouse::Back);
    /// The forward side button ("mouse 5").
    pub const FORWARD: Self = Self(Mouse::Forward);

    /// Another button by the number winit gives it (`Other(n)`), when this
    /// system has such a button.
    pub fn other(number: u16) -> Option<Self> {
        FIRST_OTHER
            .is_some_and(|first| number >= first)
            .then_some(Self(Mouse::Other(number)))
    }

    /// Another button by the number the system gives it, unchecked: the
    /// global sources decode their own system's numbers.
    #[cfg(any(test, target_os = "linux", target_os = "macos"))]
    pub(crate) const fn numbered(number: u16) -> Self {
        Self(Mouse::Other(number))
    }

    /// The number of another button; `None` for the middle and side ones.
    pub fn number(self) -> Option<u16> {
        match self.0 {
            Mouse::Other(number) => Some(number),
            Mouse::Middle | Mouse::Back | Mouse::Forward => None,
        }
    }
}

impl FromStr for MouseButton {
    type Err = BindingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim().to_ascii_lowercase();
        match wanted.as_str() {
            "middle" => Some(Self::MIDDLE),
            "back" => Some(Self::BACK),
            "forward" => Some(Self::FORWARD),
            other => other
                .strip_prefix("button")
                .filter(|n| !n.starts_with('+'))
                .and_then(|n| n.parse().ok())
                .and_then(Self::other),
        }
        .ok_or_else(|| BindingError::Mouse(s.to_owned()))
    }
}

impl fmt::Display for MouseButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Mouse::Middle => f.write_str("middle"),
            Mouse::Back => f.write_str("back"),
            Mouse::Forward => f.write_str("forward"),
            Mouse::Other(number) => write!(f, "button{number}"),
        }
    }
}

/// A gamepad button, by its position (`south` is A on an Xbox pad, cross on
/// a PlayStation pad).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GamepadButton(gilrs::Button);

/// Button names in the settings, and the gilrs buttons they stand for.
const BUTTONS: [(&str, gilrs::Button); 19] = [
    ("south", gilrs::Button::South),
    ("east", gilrs::Button::East),
    ("north", gilrs::Button::North),
    ("west", gilrs::Button::West),
    ("c", gilrs::Button::C),
    ("z", gilrs::Button::Z),
    ("left_trigger", gilrs::Button::LeftTrigger),
    ("left_trigger2", gilrs::Button::LeftTrigger2),
    ("right_trigger", gilrs::Button::RightTrigger),
    ("right_trigger2", gilrs::Button::RightTrigger2),
    ("select", gilrs::Button::Select),
    ("start", gilrs::Button::Start),
    ("mode", gilrs::Button::Mode),
    ("left_thumb", gilrs::Button::LeftThumb),
    ("right_thumb", gilrs::Button::RightThumb),
    ("dpad_up", gilrs::Button::DPadUp),
    ("dpad_down", gilrs::Button::DPadDown),
    ("dpad_left", gilrs::Button::DPadLeft),
    ("dpad_right", gilrs::Button::DPadRight),
];

impl GamepadButton {
    pub(crate) fn button(self) -> gilrs::Button {
        self.0
    }

    /// Every button name the settings accept.
    pub fn names() -> impl Iterator<Item = &'static str> {
        BUTTONS.iter().map(|(name, _)| *name)
    }
}

impl FromStr for GamepadButton {
    type Err = BindingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let wanted = s.trim();
        BUTTONS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(_, button)| Self(*button))
            .ok_or_else(|| BindingError::Button(s.to_owned()))
    }
}

impl fmt::Display for GamepadButton {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = BUTTONS
            .iter()
            .find(|(_, button)| *button == self.0)
            .map_or("unknown", |(name, _)| name);
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> KeyBinding {
        s.parse().unwrap()
    }

    #[test]
    fn key_bindings_round_trip_in_global_hotkey_syntax() {
        assert_eq!(key("ctrl+shift+KeyT").to_string(), "shift+control+KeyT");
        assert_eq!(key("F13").to_string(), "F13");
        assert_eq!(key(" alt+Space ").to_string(), "alt+Space");
        let again = key(&key("super+alt+Digit5").to_string());
        assert_eq!(again, key("super+alt+Digit5"));
        for bad in ["", "ctrl+", "KeyT+ctrl", "ctrl+KeyT+KeyU", "Banana"] {
            assert!(bad.parse::<KeyBinding>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn window_keys_become_bindings() {
        let ctrl_shift = Held {
            ctrl: true,
            shift: true,
            ..Held::default()
        };
        let made = KeyBinding::from_key("KeyT", ctrl_shift).unwrap();
        assert_eq!(made, key("ctrl+shift+KeyT"));
        assert_eq!(made.to_string(), "shift+control+KeyT");
        for (code, held, written) in [
            ("F13", Held::default(), "F13"),
            ("Digit5", Held::default(), "Digit5"),
            ("Numpad0", Held::default(), "Numpad0"),
            ("Backquote", Held::default(), "Backquote"),
            (
                "Space",
                Held {
                    alt: true,
                    ..Held::default()
                },
                "alt+Space",
            ),
            (
                "KeyM",
                Held {
                    logo: true,
                    ..Held::default()
                },
                "super+KeyM",
            ),
        ] {
            assert_eq!(
                KeyBinding::from_key(code, held).unwrap().to_string(),
                written
            );
        }
        // Modifier keys alone, keys with no name, and malformed codes.
        for bad in [
            "ShiftLeft",
            "ControlRight",
            "SuperLeft",
            "AltLeft",
            "Fn",
            "",
            "ctrl+KeyT",
            "KeyT+",
        ] {
            assert!(
                KeyBinding::from_key(bad, Held::default()).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn window_keys_match_the_binding() {
        let binding = key("ctrl+shift+KeyT");
        let ctrl_shift = Held {
            ctrl: true,
            shift: true,
            ..Held::default()
        };
        assert!(binding.is_key("KeyT"));
        assert!(!binding.is_key("KeyY"));
        assert!(!binding.is_key("ControlLeft"));
        assert!(binding.has_modifiers(ctrl_shift));
        assert!(
            !binding.has_modifiers(Held::default()),
            "a press needs them"
        );
        assert!(
            !binding.has_modifiers(Held {
                alt: true,
                ..ctrl_shift
            }),
            "and no others"
        );
        let plain = key("F13");
        assert!(plain.is_key("F13") && plain.has_modifiers(Held::default()));
    }

    #[test]
    fn portal_triggers_use_xdg_shortcut_names() {
        assert_eq!(
            key("ctrl+shift+KeyT").portal_trigger().as_deref(),
            Some("CTRL+SHIFT+t")
        );
        assert_eq!(key("F13").portal_trigger().as_deref(), Some("F13"));
        assert_eq!(
            key("super+Space").portal_trigger().as_deref(),
            Some("LOGO+space")
        );
        assert_eq!(
            key("alt+Backquote").portal_trigger().as_deref(),
            Some("ALT+grave")
        );
        assert_eq!(key("Numpad0").portal_trigger().as_deref(), Some("KP_0"));
        assert_eq!(key("PageUp").portal_trigger().as_deref(), Some("Page_Up"));
        // No keysym name known: the portal's dialog picks the key.
        assert_eq!(key("AudioVolumeMute").portal_trigger(), None);
    }

    #[test]
    fn mouse_buttons_round_trip_by_name() {
        for (text, button) in [
            ("middle", MouseButton::MIDDLE),
            ("Back", MouseButton::BACK),
            (" forward ", MouseButton::FORWARD),
        ] {
            assert_eq!(text.parse::<MouseButton>(), Ok(button));
            assert_eq!(button.to_string().parse::<MouseButton>(), Ok(button));
            assert_eq!(button.number(), None);
        }
        assert_eq!(MouseButton::BACK.to_string(), "back");
        for bad in [
            "",
            "left",
            "right",
            "mouse4",
            "button",
            "button+12",
            "button-1",
            "button70000",
        ] {
            assert!(bad.parse::<MouseButton>().is_err(), "{bad:?}");
        }
        // Other buttons, by the system's own numbers.
        match FIRST_OTHER {
            Some(first) => {
                assert_eq!(
                    MouseButton::other(first - 1),
                    None,
                    "a named one or the wheel"
                );
                let button = MouseButton::other(first).unwrap();
                assert_eq!(button.to_string(), format!("button{first}"));
                assert_eq!(button.to_string().parse::<MouseButton>(), Ok(button));
                assert_eq!(button.number(), Some(first));
            }
            None => {
                assert_eq!(MouseButton::other(12), None);
                assert!("button12".parse::<MouseButton>().is_err());
            }
        }
    }

    #[test]
    fn gamepad_buttons_round_trip_by_name() {
        for name in GamepadButton::names() {
            let button: GamepadButton = name.parse().unwrap();
            assert_eq!(button.to_string(), name);
        }
        assert_eq!(
            "Left_Thumb".parse::<GamepadButton>().unwrap().button(),
            gilrs::Button::LeftThumb
        );
        assert!("unknown".parse::<GamepadButton>().is_err());
        assert!("".parse::<GamepadButton>().is_err());
    }
}
