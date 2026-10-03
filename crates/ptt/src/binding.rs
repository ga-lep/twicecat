//! Push-to-talk bindings: a keyboard shortcut and a gamepad button. Both are
//! kept as strings in the audio settings (`ctrl+shift+KeyT`, `left_thumb`),
//! so the core stores them without knowing this crate.

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
}

/// A keyboard shortcut: optional modifiers and one key, written as
/// global-hotkey writes it (`shift+control+KeyT`, `F13`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyBinding(HotKey);

impl KeyBinding {
    pub(crate) fn hotkey(self) -> HotKey {
        self.0
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
