//! The options card for the spaces rail and the name dialog (sidebar plan,
//! Task 6): the way to arrange the rail without dragging.
//!
//! A right-click on a space or a folder opens a card with that item's moves:
//! Move up, Move down, Move to folder (a pick list: the folders, then "New
//! folder..."), Remove from folder, and for a folder Rename and Ungroup.
//! "New folder..." and Rename open a small dialog with one text field, which
//! takes the focus. Enter in the field saves and Esc cancels.
//!
//! Every entry maps to one [`SidebarOp`] ([`op_for`] and [`App::pick_folder`]);
//! the drag and drop of a later task calls the same operations. No keyboard
//! chord opens the card: iced 0.14 gives buttons and `mouse_area`s no keyboard
//! focus, so arranging the rail is a pointer feature (plan decision 5). Esc
//! closes the card or the dialog; it takes its place in the Esc order right
//! after the room switcher (see `App::sidebar_menu_escape`).
//!
//! `Debug` of every type here hides room IDs and the text of a name.

use std::fmt;

use iced::Task;
use twicecat_core::api::{RailEntry, RoomId, SidebarLabel, SidebarLabelError};

use crate::keys::Shortcut;
use crate::sidebar::{Dir, FolderTarget, Op as SidebarOp, SidebarState};
use crate::spaces::{RailItem, RailKey};
use crate::state::{App, Message, Screen};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder};

/// Widget ID of the name dialog's text field.
pub const NAME_ID: &str = "sidebar-menu-name";

/// Most characters the field keeps. A name longer than
/// [`SidebarLabel::MAX_CHARS`] is not saved: the dialog says so. The field
/// keeps more than that, so a name that is too long is seen and not cut
/// silently.
pub const MAX_NAME_INPUT: usize = 128;

/// What the dialog says about a name that is not valid.
pub const NAME_ERROR: &str =
    "Use 1 to 32 characters, with no control or hidden characters, or leave it blank.";

/// What the card is about.
#[derive(Clone, PartialEq, Eq)]
pub enum Target {
    /// A space of the rail.
    Space(RoomId),
    /// A folder of the rail, by its ID.
    Folder(u16),
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Space(_) => f.write_str("Space(<redacted>)"),
            Self::Folder(id) => f.debug_tuple("Folder").field(id).finish(),
        }
    }
}

/// What the name dialog is for.
#[derive(Clone, PartialEq, Eq)]
pub enum Purpose {
    /// A new folder, made around this space.
    NewFolder(RoomId),
    /// A new name for this folder.
    RenameFolder(u16),
}

impl fmt::Debug for Purpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewFolder(_) => f.write_str("NewFolder(<redacted>)"),
            Self::RenameFolder(id) => f.debug_tuple("RenameFolder").field(id).finish(),
        }
    }
}

/// The name dialog.
#[derive(Clone, PartialEq, Eq)]
pub struct NameDialog {
    /// What it is for.
    pub purpose: Purpose,
    /// What the field holds.
    pub text: String,
    /// The line under the field, after a name that was not saved.
    pub error: Option<&'static str>,
}

impl fmt::Debug for NameDialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NameDialog")
            .field("purpose", &self.purpose)
            .field("chars", &self.text.chars().count())
            .field("error", &self.error)
            .finish()
    }
}

/// The layer that is open over the window.
#[derive(Clone, PartialEq, Eq)]
pub enum SidebarMenu {
    /// The options card of an item.
    Card {
        /// The item.
        target: Target,
        /// Why the last action was refused ("The sidebar layout is full").
        error: Option<&'static str>,
    },
    /// The name dialog.
    Name(NameDialog),
}

impl fmt::Debug for SidebarMenu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Card { target, error } => f
                .debug_struct("Card")
                .field("target", target)
                .field("error", error)
                .finish(),
            Self::Name(dialog) => f.debug_tuple("Name").field(dialog).finish(),
        }
    }
}

/// A button of the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// Move the item one place up the rail.
    MoveUp,
    /// Move the item one place down the rail.
    MoveDown,
    /// Take a space out of its folder.
    RemoveFromFolder,
    /// Name or rename a folder (opens the dialog).
    Rename,
    /// Dissolve a folder.
    Ungroup,
}

impl Entry {
    /// The words on the button.
    pub fn label(self) -> &'static str {
        match self {
            Self::MoveUp => "Move up",
            Self::MoveDown => "Move down",
            Self::RemoveFromFolder => "Remove from folder",
            Self::Rename => "Rename\u{2026}",
            Self::Ungroup => "Ungroup",
        }
    }
}

/// A choice of the "Move to folder" pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPick {
    /// A folder of the rail.
    Existing(u16),
    /// A new folder (opens the dialog).
    New,
}

/// An option of the pick list, with the words it shows. `Debug` hides the
/// words (a folder's name is the user's own text).
#[derive(Clone)]
pub struct FolderChoice {
    /// What picking it does.
    pub pick: FolderPick,
    /// What the list shows.
    pub label: String,
}

impl PartialEq for FolderChoice {
    fn eq(&self, other: &Self) -> bool {
        self.pick == other.pick
    }
}

impl fmt::Display for FolderChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

impl fmt::Debug for FolderChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FolderChoice").field(&self.pick).finish()
    }
}

/// The message of the card and the dialog.
#[derive(Clone, PartialEq, Eq)]
pub enum SidebarMenuMessage {
    /// A right-click on an item of the rail.
    Open(RailKey),
    /// The close button, Cancel.
    Close,
    /// The Esc key, with the card or the dialog the topmost layer.
    Escape,
    /// A button of the card.
    Entry(Entry),
    /// A choice of the "Move to folder" pick list.
    Folder(FolderPick),
    /// The text field of the dialog changed.
    NameInput(String),
    /// Enter in the field, or the dialog's main button.
    NameSubmit,
}

impl fmt::Debug for SidebarMenuMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(key) => f.debug_tuple("Open").field(key).finish(),
            Self::Close => f.write_str("Close"),
            Self::Escape => f.write_str("Escape"),
            Self::Entry(entry) => f.debug_tuple("Entry").field(entry).finish(),
            Self::Folder(pick) => f.debug_tuple("Folder").field(pick).finish(),
            // The text is a folder name: only how long it is.
            Self::NameInput(text) => write!(f, "NameInput(<{} chars>)", text.chars().count()),
            Self::NameSubmit => f.write_str("NameSubmit"),
        }
    }
}

/// The buttons of the card of `target`, top to bottom. A space in a folder
/// can leave it. ("Move to folder" is the pick list under them.)
pub fn entries(target: &Target, in_folder: bool) -> Vec<Entry> {
    match target {
        Target::Space(_) if in_folder => {
            vec![Entry::MoveUp, Entry::MoveDown, Entry::RemoveFromFolder]
        }
        Target::Space(_) => vec![Entry::MoveUp, Entry::MoveDown],
        Target::Folder(_) => vec![
            Entry::Rename,
            Entry::MoveUp,
            Entry::MoveDown,
            Entry::Ungroup,
        ],
    }
}

/// The operation a button stands for, or `None` for the ones that open the
/// dialog (Rename) or do not belong on that card.
pub fn op_for(target: &Target, entry: Entry) -> Option<SidebarOp> {
    match (target, entry) {
        (Target::Space(id), Entry::MoveUp) => Some(SidebarOp::MoveSpace {
            id: id.clone(),
            dir: Dir::Up,
        }),
        (Target::Space(id), Entry::MoveDown) => Some(SidebarOp::MoveSpace {
            id: id.clone(),
            dir: Dir::Down,
        }),
        (Target::Space(id), Entry::RemoveFromFolder) => {
            Some(SidebarOp::RemoveFromFolder(id.clone()))
        }
        (Target::Folder(id), Entry::MoveUp) => Some(SidebarOp::MoveFolder {
            id: *id,
            dir: Dir::Up,
        }),
        (Target::Folder(id), Entry::MoveDown) => Some(SidebarOp::MoveFolder {
            id: *id,
            dir: Dir::Down,
        }),
        (Target::Folder(id), Entry::Ungroup) => Some(SidebarOp::UngroupFolder(*id)),
        (Target::Folder(_), Entry::Rename)
        | (Target::Space(_), Entry::Rename | Entry::Ungroup)
        | (Target::Folder(_), Entry::RemoveFromFolder) => None,
    }
}

/// The operation a choice of the pick list stands for, or `None` for "New
/// folder..." (which opens the dialog).
pub fn op_for_pick(space: &RoomId, pick: FolderPick) -> Option<SidebarOp> {
    match pick {
        FolderPick::Existing(folder) => Some(SidebarOp::MoveToFolder {
            id: space.clone(),
            folder: FolderTarget::Existing(folder),
        }),
        FolderPick::New => None,
    }
}

/// The operation that saves the dialog with `name` (`None`: no name).
pub fn op_for_name(purpose: &Purpose, name: Option<SidebarLabel>) -> SidebarOp {
    match purpose {
        Purpose::NewFolder(space) => SidebarOp::MoveToFolder {
            id: space.clone(),
            folder: FolderTarget::New(name),
        },
        Purpose::RenameFolder(id) => SidebarOp::RenameFolder { id: *id, name },
    }
}

/// What the dialog's text makes: no name (blank), a name, or `Err` when it
/// is not a valid one.
pub fn parse_name(text: &str) -> Result<Option<SidebarLabel>, SidebarLabelError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    SidebarLabel::try_from(text).map(Some)
}

/// Where the item sits on the rail as drawn: whether it can go up, whether it
/// can go down, and whether it is a space inside a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// There is something drawn above it (in its folder, for a space in one).
    pub can_up: bool,
    /// There is something drawn below it.
    pub can_down: bool,
    /// It is a space in a folder.
    pub in_folder: bool,
}

/// The place of `target` among `items`, or `None` when it is not drawn.
pub fn place_of(items: &[RailItem<'_>], target: &Target) -> Option<Place> {
    for (at, item) in items.iter().enumerate() {
        match (item, target) {
            (RailItem::Space(space), Target::Space(id)) if space.id == *id => {
                return Some(Place {
                    can_up: at > 0,
                    can_down: at + 1 < items.len(),
                    in_folder: false,
                });
            }
            (RailItem::Folder(folder), Target::Folder(id)) if folder.id == *id => {
                return Some(Place {
                    can_up: at > 0,
                    can_down: at + 1 < items.len(),
                    in_folder: false,
                });
            }
            (RailItem::Folder(folder), Target::Space(id)) => {
                if let Some(n) = folder.spaces.iter().position(|s| s.id == *id) {
                    return Some(Place {
                        can_up: n > 0,
                        can_down: n + 1 < folder.spaces.len(),
                        in_folder: true,
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// The options of the "Move to folder" list for a space: the folders of the
/// rail except the one it is in, then "New folder...".
pub fn folder_choices(items: &[RailItem<'_>], space: &RoomId) -> Vec<FolderChoice> {
    let mut out = Vec::new();
    let mut number = 0;
    for item in items {
        let RailItem::Folder(folder) = item else {
            continue;
        };
        number += 1;
        if folder.spaces.iter().any(|s| s.id == *space) {
            continue;
        }
        let label = match folder.name {
            Some(name) => name_or_placeholder(name, MAX_NAME_CHARS),
            None => format!("Folder {number}"),
        };
        out.push(FolderChoice {
            pick: FolderPick::Existing(folder.id),
            label,
        });
    }
    out.push(FolderChoice {
        pick: FolderPick::New,
        label: "New folder\u{2026}".to_owned(),
    });
    out
}

impl App {
    /// Whether the card or the dialog is open.
    pub fn sidebar_menu_open(&self) -> bool {
        self.sidebar_menu.is_some()
    }

    /// The message of the card and the dialog.
    pub(crate) fn on_sidebar_menu(&mut self, message: SidebarMenuMessage) -> Task<Message> {
        match message {
            SidebarMenuMessage::Open(key) => self.open_sidebar_menu(&key),
            SidebarMenuMessage::Close | SidebarMenuMessage::Escape => {
                self.sidebar_menu = None;
                Task::none()
            }
            SidebarMenuMessage::Entry(entry) => self.menu_entry(entry),
            SidebarMenuMessage::Folder(pick) => self.pick_folder(pick),
            SidebarMenuMessage::NameInput(text) => {
                if let Some(SidebarMenu::Name(dialog)) = &mut self.sidebar_menu {
                    dialog.text = text.chars().take(MAX_NAME_INPUT).collect();
                    dialog.error = None;
                }
                Task::none()
            }
            SidebarMenuMessage::NameSubmit => self.submit_name(),
        }
    }

    /// A right-click on an item of the rail. Only on the main screen, with no
    /// other layer over it, and only when the layout can be changed.
    fn open_sidebar_menu(&mut self, key: &RailKey) -> Task<Message> {
        let customizable = matches!(
            self.sidebar.state(),
            SidebarState::Loaded | SidebarState::Full
        );
        if self.screen != Screen::Main || !customizable || self.any_overlay_open() {
            return Task::none();
        }
        let items = self.spaces.rail_items(self.sidebar.layout());
        let target = match key {
            RailKey::Home => return Task::none(),
            RailKey::Space(id) => Target::Space(id.clone()),
            RailKey::Folder(id) => Target::Folder(*id),
        };
        if place_of(&items, &target).is_none() {
            return Task::none();
        }
        self.rail_hover = None;
        self.rail_pressed = None;
        self.sidebar_menu = Some(SidebarMenu::Card {
            target,
            error: None,
        });
        Task::none()
    }

    /// A button of the card.
    fn menu_entry(&mut self, entry: Entry) -> Task<Message> {
        let Some(SidebarMenu::Card { target, .. }) = &self.sidebar_menu else {
            return Task::none();
        };
        let target = target.clone();
        if let (Target::Folder(id), Entry::Rename) = (&target, entry) {
            let current = self
                .sidebar
                .layout()
                .and_then(|layout| {
                    layout.rail.iter().find_map(|e| match e {
                        RailEntry::Folder(f) if f.id == *id => f.name,
                        _ => None,
                    })
                })
                .map(|name| name.as_str().to_owned())
                .unwrap_or_default();
            return self.open_name_dialog(Purpose::RenameFolder(*id), current);
        }
        match op_for(&target, entry) {
            Some(op) => self.run_menu_op(op),
            None => Task::none(),
        }
    }

    /// A choice of the "Move to folder" list.
    pub(crate) fn pick_folder(&mut self, pick: FolderPick) -> Task<Message> {
        let Some(SidebarMenu::Card {
            target: Target::Space(space),
            ..
        }) = &self.sidebar_menu
        else {
            return Task::none();
        };
        let space = space.clone();
        match op_for_pick(&space, pick) {
            Some(op) => self.run_menu_op(op),
            None => self.open_name_dialog(Purpose::NewFolder(space), String::new()),
        }
    }

    /// Opens the dialog and gives its field the focus.
    fn open_name_dialog(&mut self, purpose: Purpose, text: String) -> Task<Message> {
        self.sidebar_menu = Some(SidebarMenu::Name(NameDialog {
            purpose,
            text,
            error: None,
        }));
        iced::widget::operation::focus(NAME_ID)
    }

    /// Enter in the field, or the dialog's main button.
    fn submit_name(&mut self) -> Task<Message> {
        let Some(SidebarMenu::Name(dialog)) = &self.sidebar_menu else {
            return Task::none();
        };
        let purpose = dialog.purpose.clone();
        match parse_name(&dialog.text) {
            Ok(name) => self.run_menu_op(op_for_name(&purpose, name)),
            Err(_) => {
                self.set_menu_error(NAME_ERROR);
                Task::none()
            }
        }
    }

    /// Does the operation: the layer closes when it worked (or had nothing to
    /// do), and stays open with the reason when it was refused.
    fn run_menu_op(&mut self, op: SidebarOp) -> Task<Message> {
        match self.apply_sidebar(op) {
            Ok(task) => {
                self.sidebar_menu = None;
                task
            }
            Err(refused) => {
                self.set_menu_error(refused.message());
                Task::none()
            }
        }
    }

    fn set_menu_error(&mut self, line: &'static str) {
        match &mut self.sidebar_menu {
            Some(SidebarMenu::Card { error, .. }) => *error = Some(line),
            Some(SidebarMenu::Name(dialog)) => dialog.error = Some(line),
            None => {}
        }
    }

    /// A shortcut pressed while the card or the dialog may be open. Esc
    /// closes it, when it is the topmost layer: the ring card, the
    /// verification card and the switcher are drawn over it, so they take the
    /// key first. The room-stepping chords wait under it, as they do under
    /// every other card. `None` when the key is not for the card.
    pub(crate) fn sidebar_menu_shortcut(&mut self, shortcut: Shortcut) -> Option<Task<Message>> {
        if self.sidebar_menu.is_none() || self.screen != Screen::Main {
            return None;
        }
        match shortcut {
            Shortcut::Escape => {
                if self.calls.card().is_some() || self.verification.is_some() || self.switcher.open
                {
                    return None;
                }
                // The key is used here; the composer's copy of it must not
                // close another layer.
                self.escape_spent = false;
                self.sidebar_menu = None;
                Some(Task::none())
            }
            Shortcut::NextRoom
            | Shortcut::PreviousRoom
            | Shortcut::NextUnread
            | Shortcut::PreviousUnread => Some(Task::none()),
            _ => None,
        }
    }

    /// Closes the card or the dialog when what it is about is no longer
    /// drawn.
    pub(crate) fn close_sidebar_menu_if_gone(&mut self) {
        let Some(menu) = &self.sidebar_menu else {
            return;
        };
        let target = match menu {
            SidebarMenu::Card { target, .. } => target.clone(),
            SidebarMenu::Name(dialog) => match &dialog.purpose {
                Purpose::NewFolder(space) => Target::Space(space.clone()),
                Purpose::RenameFolder(id) => Target::Folder(*id),
            },
        };
        let items = self.spaces.rail_items(self.sidebar.layout());
        if place_of(&items, &target).is_none() {
            self.sidebar_menu = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(id: &str) -> RoomId {
        RoomId::from(id)
    }

    #[test]
    fn debug_hides_room_ids_and_the_text_of_a_name() {
        let texts = [
            format!("{:?}", Target::Space(rid("!secretroom:x"))),
            format!("{:?}", Purpose::NewFolder(rid("!secretroom:x"))),
            format!(
                "{:?}",
                SidebarMenu::Card {
                    target: Target::Space(rid("!secretroom:x")),
                    error: None
                }
            ),
            format!(
                "{:?}",
                SidebarMenu::Name(NameDialog {
                    purpose: Purpose::NewFolder(rid("!secretroom:x")),
                    text: "Payroll".to_owned(),
                    error: None,
                })
            ),
            format!(
                "{:?}",
                SidebarMenuMessage::Open(RailKey::Space(rid("!secretroom:x")))
            ),
            format!("{:?}", SidebarMenuMessage::NameInput("Payroll".to_owned())),
            format!(
                "{:?}",
                FolderChoice {
                    pick: FolderPick::Existing(1),
                    label: "Payroll".to_owned()
                }
            ),
        ];
        for text in texts {
            for secret in ["!secretroom", "@a:x", "Payroll"] {
                assert!(!text.contains(secret), "{secret} is in {text}");
            }
        }
    }

    #[test]
    fn every_button_of_every_card_maps_to_an_operation_or_the_dialog() {
        let space = Target::Space(rid("!a:x"));
        let folder = Target::Folder(4);
        // (card, buttons, what each stands for)
        let mut seen = Vec::new();
        for (target, in_folder) in [(&space, false), (&space, true), (&folder, false)] {
            for entry in entries(target, in_folder) {
                let op = op_for(target, entry);
                match (target, entry) {
                    (Target::Folder(_), Entry::Rename) => assert_eq!(op, None),
                    _ => assert!(op.is_some(), "{target:?} {entry:?} stands for nothing"),
                }
                seen.push((entry, op));
            }
        }
        // The operations the buttons reach, with the pick list's.
        let mut ops: Vec<SidebarOp> = seen.into_iter().filter_map(|(_, op)| op).collect();
        ops.push(op_for_pick(&rid("!a:x"), FolderPick::Existing(2)).expect("an op"));
        ops.push(op_for_name(&Purpose::NewFolder(rid("!a:x")), None));
        ops.push(op_for_name(&Purpose::RenameFolder(4), None));
        let kinds: std::collections::BTreeSet<String> = ops
            .iter()
            .map(|op| {
                format!("{op:?}")
                    .split([' ', '(', '{'])
                    .next()
                    .unwrap_or("")
                    .to_owned()
            })
            .collect();
        // Every operation on the rail has a way in.
        for kind in [
            "MoveSpace",
            "MoveToFolder",
            "RemoveFromFolder",
            "RenameFolder",
            "UngroupFolder",
            "MoveFolder",
        ] {
            assert!(kinds.contains(kind), "no card reaches {kind}: {kinds:?}");
        }
        // New folder and Rename open the dialog, and the pick for them has no op.
        assert_eq!(op_for_pick(&rid("!a:x"), FolderPick::New), None);
    }

    #[test]
    fn a_space_card_has_the_moves_and_leaves_a_folder_only_from_one() {
        let space = Target::Space(rid("!a:x"));
        assert_eq!(
            entries(&space, false),
            [Entry::MoveUp, Entry::MoveDown],
            "the pick list below them is Move to folder"
        );
        assert_eq!(
            entries(&space, true),
            [Entry::MoveUp, Entry::MoveDown, Entry::RemoveFromFolder]
        );
        assert_eq!(
            entries(&Target::Folder(1), false),
            [
                Entry::Rename,
                Entry::MoveUp,
                Entry::MoveDown,
                Entry::Ungroup
            ]
        );
    }

    #[test]
    fn a_name_is_trimmed_blank_means_none_and_a_bad_one_is_an_error() {
        assert_eq!(parse_name("   "), Ok(None));
        assert_eq!(parse_name(""), Ok(None));
        let work = parse_name("  Work  ").expect("valid").expect("a name");
        assert_eq!(work.as_str(), "Work");
        assert!(parse_name(&"w".repeat(33)).is_err());
        assert!(parse_name(&"w".repeat(32)).is_ok());
        assert!(parse_name("\u{202E}x").is_err());
        assert!(parse_name("a\nb").is_err());
    }
}
