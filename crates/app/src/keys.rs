//! The keyboard shortcuts (plan Q18, Task 16): one table drives the key
//! handling and the list in Settings, so they cannot drift apart.
//!
//! [`shortcut_for`] is pure: a key, the modifiers and the platform give a
//! [`Shortcut`] or nothing. [`accepts`] then decides, from where the key was
//! pressed, whether the app acts on it: the composer keeps Enter, the arrows
//! and Tab in its suggestion list for itself.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};

use iced::Task;
use twicecat_core::api::{RoomId, RoomListChoice, RoomSummary};

use crate::calls::CallMessage;
use crate::settings::{SettingsMessage, Zoom};
use crate::state::{App, Message, Screen};
use crate::switcher::SwitcherMessage;
use crate::threads::ThreadsMessage;

/// Which modifier is "Command" for the shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Cmd is the command key.
    Mac,
    /// Ctrl is the command key.
    Other,
}

impl Platform {
    /// The platform this build runs on.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Other
        }
    }
}

/// What a shortcut asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// Open Settings.
    Settings,
    /// Attach a file to the open room.
    Attach,
    /// Open the room switcher.
    Switcher,
    /// Collapse the room list to a rail, or expand it again.
    ToggleRoomList,
    /// Open the room below the selected one.
    NextRoom,
    /// Open the room above the selected one.
    PreviousRoom,
    /// Open the next room with unread messages.
    NextUnread,
    /// Open the previous room with unread messages.
    PreviousUnread,
    /// Move the keyboard focus to the next field.
    FocusNext,
    /// Move the keyboard focus to the previous field.
    FocusPrevious,
    /// Close the topmost overlay or panel.
    Escape,
    /// Highlight the next room in the switcher.
    SwitcherNext,
    /// Highlight the previous room in the switcher.
    SwitcherPrevious,
    /// Mute or unmute the microphone in a call.
    ToggleMute,
    /// Deafen or undeafen in a call.
    ToggleDeafen,
    /// Make the text one step larger.
    ZoomIn,
    /// Make the text one step smaller.
    ZoomOut,
    /// Back to the normal text size.
    ZoomReset,
}

/// The key of a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySpec {
    Char(char),
    Named(Named),
}

/// The keys that produce a shortcut. `command` is Ctrl (Cmd on macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Trigger {
    key: KeySpec,
    command: bool,
    shift: bool,
    alt: bool,
}

/// A row of the table: how it reads in Settings and, when the app (and not a
/// widget) owns the keys, what they do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// The keys, as shown (Ctrl reads Cmd on macOS).
    pub keys: &'static str,
    /// What they do.
    pub action: &'static str,
    bound: Option<(Trigger, Shortcut)>,
}

const fn bound(
    keys: &'static str,
    action: &'static str,
    key: KeySpec,
    command: bool,
    shift: bool,
    alt: bool,
    shortcut: Shortcut,
) -> Binding {
    Binding {
        keys,
        action,
        bound: Some((
            Trigger {
                key,
                command,
                shift,
                alt,
            },
            shortcut,
        )),
    }
}

/// A row for keys a widget owns (the composer's Enter, say): shown, never
/// matched.
const fn widget(keys: &'static str, action: &'static str) -> Binding {
    Binding {
        keys,
        action,
        bound: None,
    }
}

/// Every keyboard shortcut. The Settings list shows these rows, and
/// [`shortcut_for`] matches the same rows.
pub const SHORTCUTS: &[Binding] = &[
    bound(
        "Ctrl+K",
        "Switch room",
        KeySpec::Char('k'),
        true,
        false,
        false,
        Shortcut::Switcher,
    ),
    bound(
        "Ctrl+B",
        "Collapse or expand the room list",
        KeySpec::Char('b'),
        true,
        false,
        false,
        Shortcut::ToggleRoomList,
    ),
    bound(
        "Alt+Down",
        "Open the next room",
        KeySpec::Named(Named::ArrowDown),
        false,
        false,
        true,
        Shortcut::NextRoom,
    ),
    bound(
        "Alt+Up",
        "Open the previous room",
        KeySpec::Named(Named::ArrowUp),
        false,
        false,
        true,
        Shortcut::PreviousRoom,
    ),
    bound(
        "Ctrl+Shift+Down",
        "Open the next room with unread messages",
        KeySpec::Named(Named::ArrowDown),
        true,
        true,
        false,
        Shortcut::NextUnread,
    ),
    bound(
        "Ctrl+Shift+Up",
        "Open the previous room with unread messages",
        KeySpec::Named(Named::ArrowUp),
        true,
        true,
        false,
        Shortcut::PreviousUnread,
    ),
    bound(
        "Ctrl+U",
        "Attach a file to the open room",
        KeySpec::Char('u'),
        true,
        false,
        false,
        Shortcut::Attach,
    ),
    bound(
        "Ctrl+,",
        "Open Settings",
        KeySpec::Char(','),
        true,
        false,
        false,
        Shortcut::Settings,
    ),
    bound(
        "Ctrl+Shift+M",
        "In a call: mute or unmute the microphone",
        KeySpec::Char('m'),
        true,
        true,
        false,
        Shortcut::ToggleMute,
    ),
    bound(
        "Ctrl+Shift+D",
        "In a call: deafen or undeafen",
        KeySpec::Char('d'),
        true,
        true,
        false,
        Shortcut::ToggleDeafen,
    ),
    bound(
        "Ctrl+=",
        "Make the text larger",
        KeySpec::Char('='),
        true,
        false,
        false,
        Shortcut::ZoomIn,
    ),
    bound(
        "Ctrl+-",
        "Make the text smaller",
        KeySpec::Char('-'),
        true,
        false,
        false,
        Shortcut::ZoomOut,
    ),
    bound(
        "Ctrl+0",
        "Reset the text size",
        KeySpec::Char('0'),
        true,
        false,
        false,
        Shortcut::ZoomReset,
    ),
    bound(
        "Tab",
        "Move to the next field",
        KeySpec::Named(Named::Tab),
        false,
        false,
        false,
        Shortcut::FocusNext,
    ),
    bound(
        "Shift+Tab",
        "Move to the previous field",
        KeySpec::Named(Named::Tab),
        false,
        true,
        false,
        Shortcut::FocusPrevious,
    ),
    bound(
        "Esc",
        "Close the open dialog, panel, Settings or the switcher",
        KeySpec::Named(Named::Escape),
        false,
        false,
        false,
        Shortcut::Escape,
    ),
    bound(
        "Down",
        "In the switcher: highlight the next room",
        KeySpec::Named(Named::ArrowDown),
        false,
        false,
        false,
        Shortcut::SwitcherNext,
    ),
    bound(
        "Up",
        "In the switcher: highlight the previous room",
        KeySpec::Named(Named::ArrowUp),
        false,
        false,
        false,
        Shortcut::SwitcherPrevious,
    ),
    widget("Enter", "Send the message (in the switcher: open the room)"),
    widget("Shift+Enter", "New line in the message"),
];

fn matches(trigger: &Trigger, key: &Key, modifiers: Modifiers, platform: Platform) -> bool {
    let command = match platform {
        Platform::Mac => modifiers.logo() && !modifiers.control(),
        Platform::Other => modifiers.control() && !modifiers.logo(),
    };
    let command_ok = if trigger.command {
        command
    } else {
        !modifiers.control() && !modifiers.logo()
    };
    if !command_ok || trigger.shift != modifiers.shift() || trigger.alt != modifiers.alt() {
        return false;
    }
    match (trigger.key, key.as_ref()) {
        (KeySpec::Named(want), Key::Named(got)) => want == got,
        (KeySpec::Char(want), Key::Character(got)) => {
            let mut chars = got.chars().flat_map(char::to_lowercase);
            chars.next() == Some(want) && chars.next().is_none()
        }
        _ => false,
    }
}

/// The shortcut the key press stands for, if any. `key` is the key with the
/// modifiers applied (Shift does not turn `,` into another shortcut).
pub fn shortcut_for(key: &Key, modifiers: Modifiers, platform: Platform) -> Option<Shortcut> {
    SHORTCUTS.iter().find_map(|row| {
        let (trigger, shortcut) = row.bound.as_ref()?;
        matches(trigger, key, modifiers, platform).then_some(*shortcut)
    })
}

/// Where a key was pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Context {
    /// A widget used the key already (the composer's own bindings, a text
    /// field's cursor keys).
    pub captured: bool,
    /// The room switcher is open.
    pub switcher_open: bool,
    /// The composer's suggestion list is open (Tab picks there).
    pub suggestions_open: bool,
}

/// Whether the app acts on `shortcut` in `context`.
///
/// Esc always does: text fields take it to drop their focus, which must not
/// keep an overlay open. The switcher's arrows count only while it is open,
/// so the composer keeps Up and Down. Tab moves the focus only when no
/// widget used it. The chords (Ctrl+K, Alt+Up, ...) are taken once: by the
/// composer through its key binding, else here.
pub fn accepts(shortcut: Shortcut, context: Context) -> bool {
    match shortcut {
        Shortcut::Escape => true,
        Shortcut::SwitcherNext | Shortcut::SwitcherPrevious => context.switcher_open,
        Shortcut::FocusNext | Shortcut::FocusPrevious => {
            !context.captured && !context.suggestions_open
        }
        _ => !context.captured,
    }
}

/// Whether the composer takes `shortcut` through its key binding, so the
/// editor neither inserts a character nor moves its cursor for it.
pub const fn composer_takes(shortcut: Shortcut) -> bool {
    matches!(
        shortcut,
        Shortcut::Settings
            | Shortcut::Attach
            | Shortcut::Switcher
            | Shortcut::ToggleRoomList
            | Shortcut::NextRoom
            | Shortcut::PreviousRoom
            | Shortcut::NextUnread
            | Shortcut::PreviousUnread
            | Shortcut::ToggleMute
            | Shortcut::ToggleDeafen
            | Shortcut::ZoomIn
            | Shortcut::ZoomOut
            | Shortcut::ZoomReset
    )
}

/// The message for a window event, if it is a shortcut. `captured` says a
/// widget used the key already.
pub fn event_message(event: &iced::Event, captured: bool) -> Option<Message> {
    let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        modified_key,
        modifiers,
        ..
    }) = event
    else {
        return None;
    };
    shortcut_for(modified_key, *modifiers, Platform::current())
        .map(|shortcut| Message::Shortcut { shortcut, captured })
}

/// Whether a room has anything unread, for the next-unread shortcut.
fn is_unread(room: &RoomSummary) -> bool {
    room.unread_messages > 0
        || room.unread_notifications > 0
        || room.unread_mentions > 0
        || room.highlights > 0
        || room.marked_unread
}

/// The room `step` places after (or before, when negative) the selected one
/// in `rooms`, wrapping at the ends; `only_unread` skips rooms with nothing
/// unread. Spaces and invitations have no timeline to open and are skipped.
/// With nothing selected, forward starts at the first room and backward at
/// the last. The selected room itself is never the answer, except when it is
/// the only candidate.
pub fn room_after(
    rooms: &[RoomSummary],
    selected: Option<&RoomId>,
    forward: bool,
    only_unread: bool,
) -> Option<RoomId> {
    let usable = |room: &RoomSummary| !room.is_space && !room.is_invite;
    let len = rooms.len();
    let start = selected.and_then(|id| rooms.iter().position(|room| room.id == *id));
    for offset in 1..=len {
        let at = match (start, forward) {
            (Some(start), true) => (start + offset) % len,
            (Some(start), false) => (start + len - offset % len) % len,
            (None, true) => offset - 1,
            (None, false) => len - offset,
        };
        let room = &rooms[at];
        if usable(room) && (!only_unread || is_unread(room)) {
            return Some(room.id.clone());
        }
    }
    None
}

impl App {
    /// Whether a dialog, card, panel or screen covers the main view, so the
    /// room shortcuts wait.
    pub fn any_overlay_open(&self) -> bool {
        self.switcher.open
            || self.calls.card().is_some()
            || self.audio_ui.open
            || self.uploads.dialog.is_some()
            || self.media.viewer.is_some()
            || self.settings_ui.open
            || self.rooms_ui.overlay_open()
    }

    /// Acts on a shortcut, unless where it was pressed says otherwise.
    pub(crate) fn on_shortcut(&mut self, shortcut: Shortcut, captured: bool) -> Task<Message> {
        let context = Context {
            captured,
            switcher_open: self.switcher.open,
            suggestions_open: self.composer.suggestions.is_open(),
        };
        let spent = std::mem::take(&mut self.escape_spent);
        // The Voice & audio card is waiting for the push-to-talk key.
        if self.audio_ui.capturing {
            return Task::none();
        }
        if !accepts(shortcut, context) {
            return Task::none();
        }
        if spent && shortcut == Shortcut::Escape && captured {
            // The composer's binding used this Esc already.
            return Task::none();
        }
        match shortcut {
            Shortcut::Settings => self.update(Message::Settings(SettingsMessage::Open)),
            Shortcut::Attach => self.update(Message::AttachRequested),
            Shortcut::Switcher => self.open_switcher(),
            Shortcut::ToggleRoomList if self.screen != Screen::Main => Task::none(),
            Shortcut::ToggleRoomList => {
                let next = match self.ui_settings.room_list {
                    RoomListChoice::Expanded => RoomListChoice::Rail,
                    RoomListChoice::Rail => RoomListChoice::Expanded,
                };
                self.update(Message::Settings(SettingsMessage::RoomList(next)))
            }
            Shortcut::NextRoom => self.step_room(true, false),
            Shortcut::PreviousRoom => self.step_room(false, false),
            Shortcut::NextUnread => self.step_room(true, true),
            Shortcut::PreviousUnread => self.step_room(false, true),
            Shortcut::FocusNext => iced::widget::operation::focus_next(),
            Shortcut::FocusPrevious => iced::widget::operation::focus_previous(),
            Shortcut::Escape => self.on_escape(captured),
            Shortcut::SwitcherNext => self.on_switcher(SwitcherMessage::Next),
            Shortcut::SwitcherPrevious => self.on_switcher(SwitcherMessage::Previous),
            // Only in a call; they work over dialogs too, as a call goes on
            // behind them.
            Shortcut::ToggleMute | Shortcut::ToggleDeafen
                if self.screen != Screen::Main || self.calls.call.is_none() =>
            {
                Task::none()
            }
            Shortcut::ZoomIn => self.update(Message::Settings(SettingsMessage::Zoom(Zoom::In))),
            Shortcut::ZoomOut => self.update(Message::Settings(SettingsMessage::Zoom(Zoom::Out))),
            Shortcut::ZoomReset => {
                self.update(Message::Settings(SettingsMessage::Zoom(Zoom::Reset)))
            }
            Shortcut::ToggleMute => self.update(Message::Call(CallMessage::ToggleMute)),
            Shortcut::ToggleDeafen => self.update(Message::Call(CallMessage::ToggleDeafen)),
        }
    }

    fn step_room(&mut self, forward: bool, only_unread: bool) -> Task<Message> {
        if self.screen != Screen::Main || self.any_overlay_open() {
            return Task::none();
        }
        match room_after(&self.rooms, self.selected.as_ref(), forward, only_unread) {
            Some(id) if self.selected.as_ref() != Some(&id) => self.update(Message::SelectRoom(id)),
            _ => Task::none(),
        }
    }

    /// Esc closes the topmost thing, one per press: the incoming-call card
    /// (hidden, still ringing), the switcher, the upload
    /// confirmation, the viewer, the Voice & audio card, Settings, a room
    /// dialog or panel, the
    /// composer's suggestions, reply or edit, then the thread panel, then the
    /// participants panel (the one on screen first when the window is too
    /// narrow for both).
    ///
    /// `from_field` is set when a text field used the key (it dropped its own
    /// focus): the composer, which leaves Esc alone, was not the target then,
    /// so its reply and edit stay.
    pub(crate) fn on_escape(&mut self, from_field: bool) -> Task<Message> {
        // The incoming-call card is the topmost layer.
        if self.screen == Screen::Main && self.calls.card().is_some() {
            return self.update(Message::Call(CallMessage::HideRing));
        }
        if self.switcher.open {
            return self.close_switcher();
        }
        if self.uploads.dialog.is_some() {
            return self.update(Message::UploadCancelled);
        }
        if self.media.viewer.is_some() {
            return self.update(Message::ViewerClosed);
        }
        // In the order of the layers: the card is drawn under the switcher,
        // the upload confirmation and the viewer, over Settings.
        if self.audio_ui.open {
            return self.update(Message::Audio(crate::audio_settings::AudioMessage::Close));
        }
        if self.settings_ui.open {
            return self.update(Message::Settings(SettingsMessage::Close));
        }
        if self.rooms_ui.overlay_open() {
            return self.update(Message::Rooms(crate::rooms::RoomsMessage::Escape));
        }
        if !from_field && self.composer_has_escape_work() {
            return self.update(Message::ComposerKey(crate::composer::ComposerKey::Escape));
        }
        // The panel on screen first. In a window too narrow for both, the
        // one that waits is not on screen and keeps its turn.
        let layout = self.pane_layout();
        if layout.thread {
            return self.update(Message::Threads(ThreadsMessage::Close));
        }
        if layout.call {
            return self.update(Message::Call(CallMessage::TogglePanel));
        }
        Task::none()
    }

    /// Whether Esc would close the composer's suggestions or leave reply or
    /// edit mode.
    pub(crate) fn composer_has_escape_work(&self) -> bool {
        self.composer.suggestions.is_open()
            || !matches!(self.composer.mode, crate::composer::Mode::Normal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    fn named(n: Named) -> Key {
        Key::Named(n)
    }

    const CTRL: Modifiers = Modifiers::CTRL;
    const LOGO: Modifiers = Modifiers::LOGO;

    #[test]
    fn ctrl_chords_on_linux_and_cmd_chords_on_macos() {
        use Platform::{Mac, Other};
        let table = [
            ("k", Shortcut::Switcher),
            ("K", Shortcut::Switcher),
            ("u", Shortcut::Attach),
            (",", Shortcut::Settings),
            ("b", Shortcut::ToggleRoomList),
        ];
        for (key, shortcut) in table {
            assert_eq!(shortcut_for(&ch(key), CTRL, Other), Some(shortcut), "{key}");
            assert_eq!(shortcut_for(&ch(key), LOGO, Mac), Some(shortcut), "{key}");
            // The other platform's modifier is an ordinary key press.
            assert_eq!(shortcut_for(&ch(key), LOGO, Other), None, "{key}");
            assert_eq!(shortcut_for(&ch(key), CTRL, Mac), None, "{key}");
            assert_eq!(shortcut_for(&ch(key), Modifiers::empty(), Other), None);
            assert_eq!(shortcut_for(&ch(key), CTRL | Modifiers::ALT, Other), None);
            assert_eq!(shortcut_for(&ch(key), CTRL | LOGO, Other), None);
        }
        assert_eq!(shortcut_for(&ch("j"), CTRL, Other), None);
    }

    #[test]
    fn alt_arrows_switch_rooms_and_ctrl_shift_arrows_find_unread() {
        let up = named(Named::ArrowUp);
        let down = named(Named::ArrowDown);
        let other = Platform::Other;
        assert_eq!(
            shortcut_for(&up, Modifiers::ALT, other),
            Some(Shortcut::PreviousRoom)
        );
        assert_eq!(
            shortcut_for(&down, Modifiers::ALT, other),
            Some(Shortcut::NextRoom)
        );
        assert_eq!(
            shortcut_for(&up, CTRL | Modifiers::SHIFT, other),
            Some(Shortcut::PreviousUnread)
        );
        assert_eq!(
            shortcut_for(&down, CTRL | Modifiers::SHIFT, other),
            Some(Shortcut::NextUnread)
        );
        assert_eq!(
            shortcut_for(&down, LOGO | Modifiers::SHIFT, Platform::Mac),
            Some(Shortcut::NextUnread)
        );
        // Shift+Up selects text, Ctrl+Up is the editor's: neither is a shortcut.
        assert_eq!(shortcut_for(&up, Modifiers::SHIFT, other), None);
        assert_eq!(shortcut_for(&up, CTRL, other), None);
        assert_eq!(
            shortcut_for(&up, Modifiers::ALT | Modifiers::SHIFT, other),
            None
        );
    }

    #[test]
    fn escape_and_tab() {
        let other = Platform::Other;
        let none = Modifiers::empty();
        assert_eq!(
            shortcut_for(&named(Named::Escape), none, other),
            Some(Shortcut::Escape)
        );
        assert_eq!(shortcut_for(&named(Named::Escape), CTRL, other), None);
        assert_eq!(
            shortcut_for(&named(Named::Tab), none, other),
            Some(Shortcut::FocusNext)
        );
        assert_eq!(
            shortcut_for(&named(Named::Tab), Modifiers::SHIFT, other),
            Some(Shortcut::FocusPrevious)
        );
        assert_eq!(shortcut_for(&named(Named::Tab), CTRL, other), None);
    }

    fn key_event(key: Key, modifiers: Modifiers) -> iced::Event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        })
    }

    #[test]
    fn window_key_events_become_shortcut_messages_with_the_captured_flag() {
        let ctrl_comma = key_event(ch(","), Modifiers::COMMAND);
        assert!(matches!(
            event_message(&ctrl_comma, false),
            Some(Message::Shortcut {
                shortcut: Shortcut::Settings,
                captured: false
            })
        ));
        assert!(matches!(
            event_message(&ctrl_comma, true),
            Some(Message::Shortcut {
                shortcut: Shortcut::Settings,
                captured: true
            })
        ));
        assert!(event_message(&key_event(ch(","), Modifiers::empty()), false).is_none());
        assert!(event_message(&key_event(ch("."), Modifiers::COMMAND), false).is_none());
        let released = iced::Event::Window(iced::window::Event::Focused);
        assert!(event_message(&released, false).is_none());
    }

    #[test]
    fn settings_lists_every_bound_shortcut_and_the_widget_keys() {
        for want in [
            "Ctrl+K", "Ctrl+,", "Ctrl+U", "Ctrl+B", "Alt+Down", "Esc", "Enter",
        ] {
            assert!(SHORTCUTS.iter().any(|row| row.keys == want), "{want}");
        }
    }

    fn context(captured: bool, switcher_open: bool, suggestions_open: bool) -> Context {
        Context {
            captured,
            switcher_open,
            suggestions_open,
        }
    }

    #[test]
    fn the_composer_keeps_its_own_keys() {
        let none = Modifiers::empty();
        let other = Platform::Other;
        // Enter and Shift+Enter are never shortcuts.
        assert_eq!(shortcut_for(&named(Named::Enter), none, other), None);
        assert_eq!(
            shortcut_for(&named(Named::Enter), Modifiers::SHIFT, other),
            None
        );
        // Up (edit the last message, move the cursor) and Down belong to the
        // composer unless the switcher is open.
        for key in [Named::ArrowUp, Named::ArrowDown] {
            let shortcut = shortcut_for(&named(key), none, other).expect("switcher arrow");
            for captured in [false, true] {
                assert!(!accepts(shortcut, context(captured, false, false)));
                assert!(!accepts(shortcut, context(captured, false, true)));
            }
            assert!(accepts(shortcut, context(false, true, false)));
        }
        // Tab picks a suggestion: the editor takes it, and with the list
        // open the app does not move the focus either.
        let tab = Shortcut::FocusNext;
        assert!(!accepts(tab, context(true, false, true)));
        assert!(!accepts(tab, context(false, false, true)));
        assert!(!accepts(tab, context(true, false, false)));
        assert!(accepts(tab, context(false, false, false)));
        // Esc is the app's even when a field used it (it drops its focus).
        assert!(accepts(Shortcut::Escape, context(true, false, true)));
        // A chord is taken once: by the composer's binding or here.
        assert!(accepts(Shortcut::Switcher, context(false, false, true)));
        assert!(!accepts(Shortcut::Switcher, context(true, false, false)));
    }

    #[test]
    fn the_composer_takes_the_chords_but_not_escape_tab_or_arrows() {
        for row in SHORTCUTS {
            let Some((_, shortcut)) = row.bound else {
                continue;
            };
            let expected = !matches!(
                shortcut,
                Shortcut::Escape
                    | Shortcut::FocusNext
                    | Shortcut::FocusPrevious
                    | Shortcut::SwitcherNext
                    | Shortcut::SwitcherPrevious
            );
            assert_eq!(composer_takes(shortcut), expected, "{shortcut:?}");
        }
    }

    #[test]
    fn every_row_is_found_by_its_own_label() {
        // The label is what Settings shows: it must name the keys that match.
        for row in SHORTCUTS {
            let Some((trigger, shortcut)) = row.bound else {
                continue;
            };
            let mut modifiers = Modifiers::empty();
            let label = row.keys;
            if label.contains("Ctrl+") {
                assert!(trigger.command, "{label}");
                modifiers |= CTRL;
            } else {
                assert!(!trigger.command, "{label}");
            }
            assert_eq!(label.contains("Shift+"), trigger.shift, "{label}");
            if trigger.shift {
                modifiers |= Modifiers::SHIFT;
            }
            assert_eq!(label.contains("Alt+"), trigger.alt, "{label}");
            if trigger.alt {
                modifiers |= Modifiers::ALT;
            }
            let key = match trigger.key {
                KeySpec::Char(c) => ch(&c.to_string()),
                KeySpec::Named(n) => named(n),
            };
            assert_eq!(
                shortcut_for(&key, modifiers, Platform::Other),
                Some(shortcut),
                "{label}"
            );
        }
    }

    #[test]
    fn keys_are_listed_once() {
        for (i, a) in SHORTCUTS.iter().enumerate() {
            for b in &SHORTCUTS[i + 1..] {
                assert_ne!(a.keys, b.keys);
                if let (Some((ta, _)), Some((tb, _))) = (a.bound, b.bound) {
                    assert_ne!(ta, tb, "{} and {}", a.keys, b.keys);
                }
            }
        }
    }
}
