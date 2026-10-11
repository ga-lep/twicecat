//! The tray icon (plan Q12): two icons drawn in code, the tooltip, and the
//! model that creates the tray once and updates it only when something
//! changed. The desktop part sits behind [`TrayFactory`] and [`TrayHandle`],
//! so tests use fakes.

use std::fmt;

use iced::Subscription;
use iced::futures::stream;
use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::state::Message;

/// Side of the icon, in pixels.
pub const ICON_SIZE: u32 = 64;

/// Colour of the unread dot.
pub const DOT_RED: [u8; 4] = [220, 38, 38, 255];

const BACKGROUND: [u8; 4] = [45, 91, 227, 255];
const FOREGROUND: [u8; 4] = [255, 255, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// The tooltip for `unread` unread notifications.
pub fn tray_tooltip(unread: u64) -> String {
    match unread {
        0 => "Tessera".to_owned(),
        1..=99 => format!("Tessera \u{2013} {unread} unread"),
        _ => "Tessera \u{2013} 99+ unread".to_owned(),
    }
}

/// Where the red dot is centred, for an icon of `size` pixels.
pub fn dot_center(size: u32) -> (u32, u32) {
    (size * 3 / 4, size / 4)
}

/// The icon as `size * size` RGBA pixels: a rounded square with a "T", and
/// with `badge` a red dot at [`dot_center`].
pub fn icon_rgba(size: u32, badge: bool) -> Vec<u8> {
    let n = size as usize;
    let mut pixels = vec![0_u8; n * n * 4];
    let corner = size / 5;
    let dot = dot_center(size);
    let ring = size / 4;
    let dot_radius = size / 5;
    for y in 0..size {
        for x in 0..size {
            let mut colour = CLEAR;
            if inside_rounded_square(x, y, size, corner) {
                colour = BACKGROUND;
                if in_t(x, y, size) {
                    colour = FOREGROUND;
                }
            }
            if badge {
                let d = distance_squared((x, y), dot);
                if d <= ring * ring {
                    colour = FOREGROUND;
                }
                if d <= dot_radius * dot_radius {
                    colour = DOT_RED;
                }
            }
            let at = (y as usize * n + x as usize) * 4;
            pixels[at..at + 4].copy_from_slice(&colour);
        }
    }
    pixels
}

fn distance_squared(a: (u32, u32), b: (u32, u32)) -> u32 {
    let dx = a.0.abs_diff(b.0);
    let dy = a.1.abs_diff(b.1);
    dx * dx + dy * dy
}

fn inside_rounded_square(x: u32, y: u32, size: u32, corner: u32) -> bool {
    let cx = x.clamp(corner, size - 1 - corner);
    let cy = y.clamp(corner, size - 1 - corner);
    distance_squared((x, y), (cx, cy)) <= corner * corner
}

fn in_t(x: u32, y: u32, size: u32) -> bool {
    let bar = x >= size / 4 && x < size * 3 / 4 && y >= size * 7 / 25 && y < size * 2 / 5;
    let stem = x >= size * 9 / 20 && x < size * 11 / 20 && y >= size * 7 / 25 && y < size * 4 / 5;
    bar || stem
}

/// What the tray looks like.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    /// The icon has the red dot.
    pub badge: bool,
    /// The tooltip text.
    pub tooltip: String,
}

impl Look {
    /// The look for `unread` notifications; with the badge setting off the
    /// tray stays plain.
    pub fn for_unread(unread: u64, badge_enabled: bool) -> Self {
        let shown = if badge_enabled { unread } else { 0 };
        Self {
            badge: shown > 0,
            tooltip: tray_tooltip(shown),
        }
    }
}

/// A tray that exists.
pub trait TrayHandle {
    /// Switches between the plain icon and the one with the dot.
    fn set_badge(&mut self, badge: bool);
    /// Replaces the tooltip.
    fn set_tooltip(&mut self, tooltip: &str);
}

/// Creates the tray. Called on the main thread with the event loop running
/// (a macOS rule).
pub trait TrayFactory {
    /// Creates the tray with `look`, or fails (no tray host, no D-Bus).
    fn create(&self, look: &Look) -> Result<Box<dyn TrayHandle>, TrayError>;
}

/// The tray could not be created. The reason is not kept: the app runs
/// without a tray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayError;

enum State {
    /// Not tried yet.
    Waiting,
    /// Tried and failed; never tried again.
    Failed,
    /// Running.
    Active {
        handle: Box<dyn TrayHandle>,
        look: Look,
    },
}

/// The tray and the unread count it shows.
pub struct TrayModel {
    factory: Box<dyn TrayFactory>,
    state: State,
    unread: u64,
}

impl fmt::Debug for TrayModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = match self.state {
            State::Waiting => "waiting",
            State::Failed => "failed",
            State::Active { .. } => "active",
        };
        f.debug_struct("TrayModel")
            .field("state", &state)
            .field("unread", &self.unread)
            .finish()
    }
}

impl TrayModel {
    /// A model that creates its tray with `factory` on [`TrayModel::ensure`].
    pub fn new(factory: Box<dyn TrayFactory>) -> Self {
        Self {
            factory,
            state: State::Waiting,
            unread: 0,
        }
    }

    /// Creates the tray the first time; later calls do nothing, also after a
    /// failure. Returns whether this call failed.
    pub fn ensure(&mut self, badge_enabled: bool) -> bool {
        if !matches!(self.state, State::Waiting) {
            return false;
        }
        let look = Look::for_unread(self.unread, badge_enabled);
        match self.factory.create(&look) {
            Ok(handle) => {
                self.state = State::Active { handle, look };
                false
            }
            Err(TrayError) => {
                self.state = State::Failed;
                true
            }
        }
    }

    /// Records the unread count and updates the tray: the icon only when the
    /// variant changed, the tooltip only when the text changed.
    pub fn set_unread(&mut self, unread: u64, badge_enabled: bool) {
        self.unread = unread;
        let wanted = Look::for_unread(unread, badge_enabled);
        if let State::Active { handle, look } = &mut self.state {
            if look.badge != wanted.badge {
                handle.set_badge(wanted.badge);
            }
            if look.tooltip != wanted.tooltip {
                handle.set_tooltip(&wanted.tooltip);
            }
            *look = wanted;
        }
    }

    /// Redraws the tray for the last unread count after the badge setting
    /// changed; like [`TrayModel::set_unread`], only what differs is touched.
    pub fn refresh(&mut self, badge_enabled: bool) {
        self.set_unread(self.unread, badge_enabled);
    }

    /// Whether a tray is running.
    pub fn is_active(&self) -> bool {
        matches!(self.state, State::Active { .. })
    }
}

/// What a click in the tray asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    /// **Show Tessera**, or a click on the icon.
    Show,
    /// **Quit**.
    Quit,
}

const SHOW_ID: &str = "tessera-show";
const QUIT_ID: &str = "tessera-quit";

/// A factory that never makes a tray: the default until the real one is
/// installed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTrayFactory;

impl TrayFactory for NoTrayFactory {
    fn create(&self, _look: &Look) -> Result<Box<dyn TrayHandle>, TrayError> {
        Err(TrayError)
    }
}

/// The real tray through `tray-icon` (the KSNI backend on Linux).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTrayFactory;

struct SystemTray {
    icon: TrayIcon,
}

fn icon(badge: bool) -> Result<Icon, TrayError> {
    Icon::from_rgba(icon_rgba(ICON_SIZE, badge), ICON_SIZE, ICON_SIZE).map_err(|_| TrayError)
}

impl TrayFactory for SystemTrayFactory {
    fn create(&self, look: &Look) -> Result<Box<dyn TrayHandle>, TrayError> {
        let menu = Menu::new();
        let show = MenuItem::with_id(SHOW_ID, "Show Tessera", true, None);
        let quit = MenuItem::with_id(QUIT_ID, "Quit", true, None);
        menu.append_items(&[&show, &quit]).map_err(|_| TrayError)?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon(look.badge)?)
            .with_tooltip(&look.tooltip)
            .with_title("Tessera")
            .build()
            .map_err(|_| TrayError)?;
        Ok(Box::new(SystemTray { icon }))
    }
}

impl TrayHandle for SystemTray {
    fn set_badge(&mut self, badge: bool) {
        if let Ok(icon) = icon(badge) {
            // A failed update leaves the old icon; nothing to tell the user.
            let _ = self.icon.set_icon(Some(icon));
        }
    }

    fn set_tooltip(&mut self, tooltip: &str) {
        let _ = self.icon.set_tooltip(Some(tooltip));
    }
}

/// The action a menu item or a tray event stands for, if any.
fn menu_action(event: &MenuEvent) -> Option<TrayAction> {
    if event.id == SHOW_ID {
        Some(TrayAction::Show)
    } else if event.id == QUIT_ID {
        Some(TrayAction::Quit)
    } else {
        None
    }
}

fn click_action(event: &TrayIconEvent) -> Option<TrayAction> {
    match event {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } => Some(TrayAction::Show),
        _ => None,
    }
}

/// Tray and menu events as a stream. Two small threads move the events from
/// the `tray-icon` receivers into the stream; they end with the process.
fn events() -> impl iced::futures::Stream<Item = TrayAction> {
    let (tx, rx) = mpsc::unbounded_channel();
    let menu_tx = tx.clone();
    let _ = std::thread::Builder::new()
        .name("tray-menu-events".into())
        .spawn(move || {
            while let Ok(event) = MenuEvent::receiver().recv() {
                if let Some(action) = menu_action(&event)
                    && menu_tx.send(action).is_err()
                {
                    break;
                }
            }
        });
    let _ = std::thread::Builder::new()
        .name("tray-click-events".into())
        .spawn(move || {
            while let Ok(event) = TrayIconEvent::receiver().recv() {
                if let Some(action) = click_action(&event)
                    && tx.send(action).is_err()
                {
                    break;
                }
            }
        });
    stream::unfold(rx, |mut rx| async move {
        let action = rx.recv().await?;
        Some((action, rx))
    })
}

/// The tray's menu and click events as messages.
pub fn subscription() -> Subscription<Message> {
    Subscription::run(events).map(Message::Tray)
}

#[cfg(test)]
pub use fake::{FakeTray, FakeTrayFactory};

#[cfg(test)]
mod fake {
    use std::sync::{Arc, Mutex};

    use super::{Look, TrayError, TrayFactory, TrayHandle};

    /// What a fake tray was told.
    #[derive(Debug, Clone, Default)]
    pub struct FakeTray {
        /// Looks given at creation.
        pub created: Arc<Mutex<Vec<Look>>>,
        /// `set_badge` calls.
        pub badges: Arc<Mutex<Vec<bool>>>,
        /// `set_tooltip` calls.
        pub tooltips: Arc<Mutex<Vec<String>>>,
    }

    /// Creates [`FakeTray`] handles, or fails.
    #[derive(Debug, Clone, Default)]
    pub struct FakeTrayFactory {
        /// What the handles record.
        pub tray: FakeTray,
        /// Creation fails.
        pub fail: bool,
    }

    struct Handle(FakeTray);

    impl TrayFactory for FakeTrayFactory {
        fn create(&self, look: &Look) -> Result<Box<dyn TrayHandle>, TrayError> {
            if self.fail {
                return Err(TrayError);
            }
            self.tray
                .created
                .lock()
                .expect("test mutex")
                .push(look.clone());
            Ok(Box::new(Handle(self.tray.clone())))
        }
    }

    impl TrayHandle for Handle {
        fn set_badge(&mut self, badge: bool) {
            self.0.badges.lock().expect("test mutex").push(badge);
        }

        fn set_tooltip(&mut self, tooltip: &str) {
            self.0
                .tooltips
                .lock()
                .expect("test mutex")
                .push(tooltip.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(rgba: &[u8], size: u32, (x, y): (u32, u32)) -> [u8; 4] {
        let at = ((y * size + x) * 4) as usize;
        rgba[at..at + 4].try_into().expect("four bytes")
    }

    #[test]
    fn tooltips() {
        assert_eq!(tray_tooltip(0), "Tessera");
        assert_eq!(tray_tooltip(1), "Tessera \u{2013} 1 unread");
        assert_eq!(tray_tooltip(3), "Tessera \u{2013} 3 unread");
        assert_eq!(tray_tooltip(99), "Tessera \u{2013} 99 unread");
        assert_eq!(tray_tooltip(100), "Tessera \u{2013} 99+ unread");
        assert_eq!(tray_tooltip(u64::MAX), "Tessera \u{2013} 99+ unread");
    }

    #[test]
    fn icons_have_the_right_size_and_a_red_dot_only_in_the_badge_variant() {
        for size in [32, 64] {
            let plain = icon_rgba(size, false);
            let badge = icon_rgba(size, true);
            assert_eq!(plain.len(), (size * size * 4) as usize);
            assert_eq!(badge.len(), plain.len());
            let center = dot_center(size);
            assert_eq!(pixel(&badge, size, center), DOT_RED, "size {size}");
            assert_ne!(pixel(&plain, size, center), DOT_RED, "size {size}");
            assert!(
                !plain.chunks(4).any(|p| p == DOT_RED),
                "no red anywhere in the plain icon"
            );
            // The two differ only around the dot.
            assert_ne!(plain, badge);
            // Something is drawn, and the corners stay transparent.
            assert_eq!(pixel(&plain, size, (0, 0))[3], 0);
            assert!(plain.chunks(4).any(|p| p[3] == 255));
        }
    }

    #[test]
    fn the_badge_setting_off_keeps_the_tray_plain() {
        let look = Look::for_unread(5, false);
        assert!(!look.badge);
        assert_eq!(look.tooltip, "Tessera");
        let look = Look::for_unread(5, true);
        assert!(look.badge);
        assert_eq!(look.tooltip, "Tessera \u{2013} 5 unread");
    }

    fn model(fail: bool) -> (TrayModel, FakeTray) {
        let factory = FakeTrayFactory {
            fail,
            ..FakeTrayFactory::default()
        };
        let tray = factory.tray.clone();
        (TrayModel::new(Box::new(factory)), tray)
    }

    #[test]
    fn the_tray_is_created_once_with_the_current_look() {
        let (mut model, tray) = model(false);
        model.set_unread(4, true);
        assert!(!model.is_active());
        assert!(!model.ensure(true));
        assert!(!model.ensure(true));
        assert!(model.is_active());
        let created = tray.created.lock().expect("test mutex");
        assert_eq!(created.len(), 1);
        assert!(created[0].badge);
        assert_eq!(created[0].tooltip, tray_tooltip(4));
    }

    #[test]
    fn a_creation_failure_is_tolerated_and_not_retried() {
        let (mut model, tray) = model(true);
        assert!(model.ensure(true), "the first call reports the failure");
        assert!(!model.ensure(true), "no retry");
        model.set_unread(7, true);
        assert!(!model.is_active());
        assert!(tray.created.lock().expect("test mutex").is_empty());
        assert!(tray.badges.lock().expect("test mutex").is_empty());
    }

    #[test]
    fn updates_touch_the_icon_and_tooltip_only_when_they_change() {
        let (mut model, tray) = model(false);
        model.ensure(true);
        model.set_unread(0, true);
        assert!(tray.badges.lock().expect("test mutex").is_empty());
        assert!(tray.tooltips.lock().expect("test mutex").is_empty());
        model.set_unread(1, true);
        assert_eq!(*tray.badges.lock().expect("test mutex"), [true]);
        assert_eq!(tray.tooltips.lock().expect("test mutex").len(), 1);
        // Same variant, new text: tooltip only.
        model.set_unread(2, true);
        assert_eq!(*tray.badges.lock().expect("test mutex"), [true]);
        assert_eq!(tray.tooltips.lock().expect("test mutex").len(), 2);
        // Same look: nothing.
        model.set_unread(2, true);
        assert_eq!(tray.tooltips.lock().expect("test mutex").len(), 2);
        // Above 99 the text stops changing.
        model.set_unread(100, true);
        model.set_unread(250, true);
        assert_eq!(tray.tooltips.lock().expect("test mutex").len(), 3);
        // Back to zero: both change.
        model.set_unread(0, true);
        assert_eq!(*tray.badges.lock().expect("test mutex"), [true, false]);
        assert_eq!(
            tray.tooltips
                .lock()
                .expect("test mutex")
                .last()
                .map(String::as_str),
            Some("Tessera")
        );
    }

    #[test]
    fn menu_ids_map_to_actions() {
        assert_eq!(
            menu_action(&MenuEvent { id: SHOW_ID.into() }),
            Some(TrayAction::Show)
        );
        assert_eq!(
            menu_action(&MenuEvent { id: QUIT_ID.into() }),
            Some(TrayAction::Quit)
        );
        assert_eq!(menu_action(&MenuEvent { id: "x".into() }), None);
    }
}
