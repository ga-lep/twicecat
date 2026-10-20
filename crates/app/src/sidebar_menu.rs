//! The options card for the spaces rail, the sections of a space and their
//! rooms, and the name dialog (sidebar plan, Tasks 6 and 9): the way to
//! arrange the sidebar without dragging.
//!
//! A right-click on a space or a folder opens a card with that item's moves:
//! Move up, Move down, Move to folder (a pick list: the folders, then "New
//! folder..."), Remove from folder, and for a folder Rename and Ungroup.
//! "New folder..." and Rename open a small dialog with one text field, which
//! takes the focus. Enter in the field saves and Esc cancels.
//!
//! In a space's sectioned list a right-click, or the "⋯" button, opens
//! the card of a section header (Rename and Delete for the user's own
//! sections, Move up, Move down, Collapse or Expand, New section) or of a room
//! row (Star or Unstar, Move up and Move down inside a section of the user's
//! own, Back to its default section, and the "Move to section" list: the
//! user's sections, then "New section..."). A room has one place per space:
//! Starred, one section of the user's own, or its default section.
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
use twicecat_core::api::{
    CustomSection, RailEntry, RoomId, SectionKey, SidebarLabel, SidebarLabelError,
};

use crate::keys::Shortcut;
use crate::sections::{Spot, drawn_sections, spot_of};
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
    /// The header of a section of a space's list.
    Section {
        /// The space.
        space: RoomId,
        /// The section.
        key: SectionKey,
    },
    /// A room row of a space's sectioned list.
    Room {
        /// The space.
        space: RoomId,
        /// The room.
        room: RoomId,
    },
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Space(_) => f.write_str("Space(<redacted>)"),
            Self::Folder(id) => f.debug_tuple("Folder").field(id).finish(),
            Self::Section { key, .. } => f.debug_struct("Section").field("key", key).finish(),
            Self::Room { .. } => f.write_str("Room(<redacted>)"),
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
    /// A new section of the user's own in a space, holding this room when
    /// there is one.
    NewSection {
        /// The space.
        space: RoomId,
        /// The room to put in it.
        room: Option<RoomId>,
    },
    /// A new name for this section of the user's own.
    RenameSection {
        /// The space.
        space: RoomId,
        /// The section.
        id: u16,
    },
}

impl Purpose {
    /// Whether the name is for a section (which must have one) and not a
    /// folder (whose name is optional).
    pub fn is_section(&self) -> bool {
        matches!(self, Self::NewSection { .. } | Self::RenameSection { .. })
    }
}

impl fmt::Debug for Purpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewFolder(_) => f.write_str("NewFolder(<redacted>)"),
            Self::RenameFolder(id) => f.debug_tuple("RenameFolder").field(id).finish(),
            Self::NewSection { room, .. } => f
                .debug_struct("NewSection")
                .field("with_room", &room.is_some())
                .finish(),
            Self::RenameSection { id, .. } => {
                f.debug_struct("RenameSection").field("id", id).finish()
            }
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
    /// Star a room.
    Star,
    /// Take a room out of Starred.
    Unstar,
    /// Put a room back in its default section.
    BackToDefault,
    /// Collapse a section.
    Collapse,
    /// Expand a section.
    Expand,
    /// Delete a section of the user's own (opens nothing: its rooms go back
    /// to their default sections).
    Delete,
    /// Make a new section (opens the dialog).
    NewSection,
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
            Self::Star => "Star",
            Self::Unstar => "Unstar",
            Self::BackToDefault => "Back to its default section",
            Self::Collapse => "Collapse",
            Self::Expand => "Expand",
            Self::Delete => "Delete section",
            Self::NewSection => "New section\u{2026}",
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

/// A choice of the "Move to section" pick list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionPick {
    /// A section of the user's own.
    Existing(u16),
    /// A new section (opens the dialog).
    New,
}

/// An option of the "Move to section" list, with the words it shows. `Debug`
/// hides the words (a section's name is the user's own text).
#[derive(Clone)]
pub struct SectionChoice {
    /// What picking it does.
    pub pick: SectionPick,
    /// What the list shows.
    pub label: String,
}

impl PartialEq for SectionChoice {
    fn eq(&self, other: &Self) -> bool {
        self.pick == other.pick
    }
}

impl fmt::Display for SectionChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

impl fmt::Debug for SectionChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SectionChoice").field(&self.pick).finish()
    }
}

/// The message of the card and the dialog.
#[derive(Clone, PartialEq, Eq)]
pub enum SidebarMenuMessage {
    /// A right-click on an item of the rail.
    Open(RailKey),
    /// A right-click, or the "⋯" button, on a section header.
    OpenSection(SectionKey),
    /// A right-click, or the "⋯" button, on a room row of the
    /// sectioned list.
    OpenRoom(RoomId),
    /// The close button, Cancel.
    Close,
    /// The Esc key, with the card or the dialog the topmost layer.
    Escape,
    /// A button of the card.
    Entry(Entry),
    /// A choice of the "Move to folder" pick list.
    Folder(FolderPick),
    /// A choice of the "Move to section" pick list.
    Section(SectionPick),
    /// The text field of the dialog changed.
    NameInput(String),
    /// Enter in the field, or the dialog's main button.
    NameSubmit,
}

impl fmt::Debug for SidebarMenuMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(key) => f.debug_tuple("Open").field(key).finish(),
            Self::OpenSection(key) => f.debug_tuple("OpenSection").field(key).finish(),
            Self::OpenRoom(_) => f.write_str("OpenRoom(<redacted>)"),
            Self::Close => f.write_str("Close"),
            Self::Escape => f.write_str("Escape"),
            Self::Entry(entry) => f.debug_tuple("Entry").field(entry).finish(),
            Self::Folder(pick) => f.debug_tuple("Folder").field(pick).finish(),
            Self::Section(pick) => f.debug_tuple("Section").field(pick).finish(),
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
        // The cards of the sectioned list have their own (`section_entries`
        // and `room_entries`).
        Target::Section { .. } | Target::Room { .. } => Vec::new(),
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
        // Everything else is not a rail button: it opens the dialog, or it
        // belongs to the sections (see `section_op` and `room_op`).
        _ => None,
    }
}

/// The buttons of the card of a section header, top to bottom. The user's own
/// sections can be renamed and deleted; every section can be moved, collapsed
/// or expanded, and starts a new section.
pub fn section_entries(key: &SectionKey, collapsed: bool) -> Vec<Entry> {
    let toggle = if collapsed {
        Entry::Expand
    } else {
        Entry::Collapse
    };
    let mut out = Vec::new();
    if matches!(key, SectionKey::Custom(_)) {
        out.push(Entry::Rename);
    }
    out.extend([Entry::MoveUp, Entry::MoveDown, toggle, Entry::NewSection]);
    if matches!(key, SectionKey::Custom(_)) {
        out.push(Entry::Delete);
    }
    out
}

/// The buttons of the card of a room row, top to bottom, for the section
/// that draws it. ("Move to section" is the pick list under them.) Up and
/// down are only for the user's own sections, where the order is theirs.
pub fn room_entries(section: &SectionKey) -> Vec<Entry> {
    match section {
        SectionKey::Starred => vec![Entry::Unstar],
        SectionKey::Custom(_) => vec![
            Entry::Star,
            Entry::MoveUp,
            Entry::MoveDown,
            Entry::BackToDefault,
        ],
        _ => vec![Entry::Star],
    }
}

/// The operation a button of a section header's card stands for, or `None`
/// for the ones that open the dialog (Rename, New section). `drawn` are the
/// sections the list draws now.
pub fn section_op(
    space: &RoomId,
    key: &SectionKey,
    entry: Entry,
    drawn: &[SectionKey],
) -> Option<SidebarOp> {
    let space = space.clone();
    match (entry, key) {
        (Entry::MoveUp | Entry::MoveDown, key) => Some(SidebarOp::MoveSection {
            space,
            key: key.clone(),
            dir: if entry == Entry::MoveUp {
                Dir::Up
            } else {
                Dir::Down
            },
            drawn: drawn.to_vec(),
        }),
        (Entry::Collapse | Entry::Expand, key) => Some(SidebarOp::ToggleSection {
            space,
            key: key.clone(),
        }),
        (Entry::Delete, SectionKey::Custom(id)) => {
            Some(SidebarOp::DeleteSection { space, id: *id })
        }
        _ => None,
    }
}

/// The operation a button of a room's card stands for. `spot` is where the
/// list draws the room now.
pub fn room_op(space: &RoomId, room: &RoomId, entry: Entry, spot: &Spot) -> Option<SidebarOp> {
    let (space, room) = (space.clone(), room.clone());
    match entry {
        Entry::Star => Some(SidebarOp::Star { space, room }),
        Entry::Unstar => Some(SidebarOp::Unstar { space, room }),
        Entry::BackToDefault => Some(SidebarOp::RoomToDefault { space, room }),
        Entry::MoveUp | Entry::MoveDown => Some(SidebarOp::MoveRoomInSection {
            space,
            room,
            dir: if entry == Entry::MoveUp {
                Dir::Up
            } else {
                Dir::Down
            },
            drawn: spot.rooms.clone(),
        }),
        _ => None,
    }
}

/// The operation a choice of the "Move to section" list stands for, or `None`
/// for "New section..." (which opens the dialog).
pub fn op_for_section_pick(space: &RoomId, room: &RoomId, pick: SectionPick) -> Option<SidebarOp> {
    match pick {
        SectionPick::Existing(section) => Some(SidebarOp::MoveRoomToSection {
            space: space.clone(),
            room: room.clone(),
            section,
        }),
        SectionPick::New => None,
    }
}

/// The options of the "Move to section" list for a room: the user's sections
/// except the one it is in, then "New section...".
pub fn section_choices(custom: &[CustomSection], current: Option<u16>) -> Vec<SectionChoice> {
    let mut out: Vec<SectionChoice> = custom
        .iter()
        .filter(|section| Some(section.id) != current)
        .map(|section| SectionChoice {
            pick: SectionPick::Existing(section.id),
            label: name_or_placeholder(section.name.as_str(), MAX_NAME_CHARS),
        })
        .collect();
    out.push(SectionChoice {
        pick: SectionPick::New,
        label: "New section\u{2026}".to_owned(),
    });
    out
}

/// What the dialog says about a section name that is not valid. A section
/// must have a name.
pub const SECTION_NAME_ERROR: &str =
    "Use 1 to 32 characters, with no control or hidden characters.";

/// A section's name from the dialog's text: unlike a folder's, it cannot be
/// blank.
pub fn parse_section_name(text: &str) -> Result<SidebarLabel, SidebarLabelError> {
    SidebarLabel::try_from(text)
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

/// The operation that saves the dialog with `name` (`None`: no name). A
/// section cannot go without a name, so for one `None` gives no operation.
pub fn op_for_name(purpose: &Purpose, name: Option<SidebarLabel>) -> Option<SidebarOp> {
    match purpose {
        Purpose::NewFolder(space) => Some(SidebarOp::MoveToFolder {
            id: space.clone(),
            folder: FolderTarget::New(name),
        }),
        Purpose::RenameFolder(id) => Some(SidebarOp::RenameFolder { id: *id, name }),
        Purpose::NewSection { space, room } => Some(SidebarOp::NewSection {
            space: space.clone(),
            name: name?,
            room: room.clone(),
        }),
        Purpose::RenameSection { space, id } => Some(SidebarOp::RenameSection {
            space: space.clone(),
            id: *id,
            name: name?,
        }),
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
            SidebarMenuMessage::OpenSection(key) => self.open_section_menu(key),
            SidebarMenuMessage::OpenRoom(room) => self.open_room_menu(room),
            SidebarMenuMessage::Close | SidebarMenuMessage::Escape => {
                self.sidebar_menu = None;
                Task::none()
            }
            SidebarMenuMessage::Entry(entry) => self.menu_entry(entry),
            SidebarMenuMessage::Folder(pick) => self.pick_folder(pick),
            SidebarMenuMessage::Section(pick) => self.pick_section(pick),
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
        self.rail_drag.cancel();
        self.sidebar_menu = Some(SidebarMenu::Card {
            target,
            error: None,
        });
        Task::none()
    }

    /// Whether a card for the sectioned list can open now: on the main
    /// screen, with the list showing sections, no other layer over it, and a
    /// layout that can be changed.
    fn section_menu_can_open(&self) -> bool {
        let customizable = matches!(
            self.sidebar.state(),
            SidebarState::Loaded | SidebarState::Full
        );
        self.screen == Screen::Main
            && customizable
            && !self.any_overlay_open()
            && self.sections_shown()
    }

    /// A right-click, or the "⋯" button, on a section header.
    fn open_section_menu(&mut self, key: SectionKey) -> Task<Message> {
        if !self.section_menu_can_open() {
            return Task::none();
        }
        let (Some(space), Some(lines)) = (self.query.space.clone(), self.section_lines()) else {
            return Task::none();
        };
        if !drawn_sections(&lines).contains(&key) {
            return Task::none();
        }
        self.sidebar_menu = Some(SidebarMenu::Card {
            target: Target::Section { space, key },
            error: None,
        });
        Task::none()
    }

    /// A right-click, or the "⋯" button, on a room row of the sectioned
    /// list.
    fn open_room_menu(&mut self, room: RoomId) -> Task<Message> {
        if !self.section_menu_can_open() {
            return Task::none();
        }
        let (Some(space), Some(lines)) = (self.query.space.clone(), self.section_lines()) else {
            return Task::none();
        };
        if spot_of(&lines, &room).is_none() {
            return Task::none();
        }
        self.sidebar_menu = Some(SidebarMenu::Card {
            target: Target::Room { space, room },
            error: None,
        });
        Task::none()
    }

    /// The operation and the dialog behind a button of a section or room
    /// card, worked out from the list as it is drawn now. `None` when what
    /// the card is about is not drawn any more.
    fn section_entry(&mut self, target: &Target, entry: Entry) -> Option<Task<Message>> {
        let lines = self.section_lines()?;
        match target {
            Target::Section { space, key } => {
                let drawn = drawn_sections(&lines);
                if !drawn.contains(key) {
                    return None;
                }
                if let (Entry::Rename, SectionKey::Custom(id)) = (entry, key) {
                    let current = self.custom_name(space, *id);
                    return Some(self.open_name_dialog(
                        Purpose::RenameSection {
                            space: space.clone(),
                            id: *id,
                        },
                        current,
                    ));
                }
                if entry == Entry::NewSection {
                    return Some(self.open_name_dialog(
                        Purpose::NewSection {
                            space: space.clone(),
                            room: None,
                        },
                        String::new(),
                    ));
                }
                let op = section_op(space, key, entry, &drawn)?;
                drop(lines);
                Some(self.run_section_op(op))
            }
            Target::Room { space, room } => {
                let spot = spot_of(&lines, room)?;
                let op = room_op(space, room, entry, &spot)?;
                drop(lines);
                Some(self.run_section_op(op))
            }
            Target::Space(_) | Target::Folder(_) => None,
        }
    }

    /// The name of the user's section `id` in `space`, as text for the
    /// dialog's field.
    fn custom_name(&self, space: &RoomId, id: u16) -> String {
        self.sidebar
            .layout()
            .and_then(|layout| layout.spaces.iter().find(|s| s.space == *space))
            .and_then(|record| record.custom.iter().find(|c| c.id == id))
            .map(|section| section.name.as_str().to_owned())
            .unwrap_or_default()
    }

    /// [`Self::run_menu_op`] for an operation that changes what the list
    /// draws: the rooms in view may need their avatars and pages.
    fn run_section_op(&mut self, op: SidebarOp) -> Task<Message> {
        let done = self.run_menu_op(op);
        Task::batch([done, self.after_list_change()])
    }

    /// A choice of the "Move to section" list.
    pub(crate) fn pick_section(&mut self, pick: SectionPick) -> Task<Message> {
        let Some(SidebarMenu::Card {
            target: Target::Room { space, room },
            ..
        }) = &self.sidebar_menu
        else {
            return Task::none();
        };
        let (space, room) = (space.clone(), room.clone());
        match op_for_section_pick(&space, &room, pick) {
            Some(op) => self.run_section_op(op),
            None => self.open_name_dialog(
                Purpose::NewSection {
                    space,
                    room: Some(room),
                },
                String::new(),
            ),
        }
    }

    /// A button of the card.
    fn menu_entry(&mut self, entry: Entry) -> Task<Message> {
        let Some(SidebarMenu::Card { target, .. }) = &self.sidebar_menu else {
            return Task::none();
        };
        let target = target.clone();
        if matches!(target, Target::Section { .. } | Target::Room { .. }) {
            return match self.section_entry(&target, entry) {
                Some(task) => task,
                None => {
                    // What the card is about is gone.
                    self.sidebar_menu = None;
                    Task::none()
                }
            };
        }
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
        if purpose.is_section() {
            return match parse_section_name(&dialog.text) {
                Ok(name) => match op_for_name(&purpose, Some(name)) {
                    Some(op) => self.run_section_op(op),
                    None => Task::none(),
                },
                Err(_) => {
                    self.set_menu_error(SECTION_NAME_ERROR);
                    Task::none()
                }
            };
        }
        match parse_name(&dialog.text) {
            Ok(name) => match op_for_name(&purpose, name) {
                Some(op) => self.run_menu_op(op),
                None => Task::none(),
            },
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
        let gone = match menu {
            SidebarMenu::Card { target, .. } => self.target_gone(target),
            SidebarMenu::Name(dialog) => match &dialog.purpose {
                Purpose::NewFolder(space) => self.target_gone(&Target::Space(space.clone())),
                Purpose::RenameFolder(id) => self.target_gone(&Target::Folder(*id)),
                Purpose::NewSection {
                    space,
                    room: Some(room),
                } => self.target_gone(&Target::Room {
                    space: space.clone(),
                    room: room.clone(),
                }),
                Purpose::NewSection { space, room: None } => self.lines_of_space(space).is_none(),
                Purpose::RenameSection { space, id } => self.target_gone(&Target::Section {
                    space: space.clone(),
                    key: SectionKey::Custom(*id),
                }),
            },
        };
        if gone {
            self.sidebar_menu = None;
        }
    }

    /// The lines of the sectioned list while it is showing `space`.
    fn lines_of_space(&self, space: &RoomId) -> Option<Vec<crate::sections::Line<'_>>> {
        if self.query.space.as_ref() == Some(space) {
            self.section_lines()
        } else {
            None
        }
    }

    /// Whether what a card is about is no longer drawn.
    fn target_gone(&self, target: &Target) -> bool {
        match target {
            Target::Space(_) | Target::Folder(_) => {
                let items = self.spaces.rail_items(self.sidebar.layout());
                place_of(&items, target).is_none()
            }
            Target::Section { space, key } => !self
                .lines_of_space(space)
                .is_some_and(|lines| drawn_sections(&lines).contains(key)),
            Target::Room { space, room } => !self
                .lines_of_space(space)
                .is_some_and(|lines| spot_of(&lines, room).is_some()),
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
        ops.push(op_for_name(&Purpose::NewFolder(rid("!a:x")), None).expect("an op"));
        ops.push(op_for_name(&Purpose::RenameFolder(4), None).expect("an op"));
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

    // ---- the cards of the sectioned list (Task sbc.9) ----

    fn kind(op: &SidebarOp) -> String {
        format!("{op:?}")
            .split([' ', '(', '{'])
            .next()
            .unwrap_or("")
            .to_owned()
    }

    /// Where a room is drawn: section `key`, the second of three rooms.
    fn spot(key: SectionKey) -> Spot {
        Spot {
            section: key,
            rooms: vec![rid("!a:x"), rid("!b:x"), rid("!c:x")],
            index: 1,
        }
    }

    #[test]
    fn every_button_of_every_section_and_room_card_maps_to_an_operation_or_the_dialog() {
        let space = rid("!sp:x");
        let drawn = [
            SectionKey::Starred,
            SectionKey::Channels,
            SectionKey::Custom(4),
            SectionKey::Direct,
        ];
        let mut kinds = std::collections::BTreeSet::new();
        // The cards of the section headers: built-in and the user's own,
        // expanded and collapsed.
        for key in [
            SectionKey::Starred,
            SectionKey::Channels,
            SectionKey::Direct,
            SectionKey::SubSpace(rid("!sub:x")),
            SectionKey::Custom(4),
        ] {
            for collapsed in [false, true] {
                let entries = section_entries(&key, collapsed);
                let custom = matches!(key, SectionKey::Custom(_));
                assert_eq!(entries.contains(&Entry::Rename), custom, "{key:?}");
                assert_eq!(entries.contains(&Entry::Delete), custom, "{key:?}");
                assert_eq!(entries.contains(&Entry::Collapse), !collapsed);
                assert_eq!(entries.contains(&Entry::Expand), collapsed);
                for entry in entries {
                    let op = section_op(&space, &key, entry, &drawn);
                    match entry {
                        Entry::Rename | Entry::NewSection => assert_eq!(op, None, "dialog"),
                        _ => {
                            let op = op
                                .unwrap_or_else(|| panic!("{key:?} {entry:?} stands for nothing"));
                            kinds.insert(kind(&op));
                        }
                    }
                }
            }
        }
        // The cards of the room rows, by the section that draws the room.
        for key in [
            SectionKey::Starred,
            SectionKey::Channels,
            SectionKey::Direct,
            SectionKey::SubSpace(rid("!sub:x")),
            SectionKey::Custom(4),
        ] {
            let spot = spot(key.clone());
            for entry in room_entries(&key) {
                let op = room_op(&space, &rid("!b:x"), entry, &spot)
                    .unwrap_or_else(|| panic!("{key:?} {entry:?} stands for nothing"));
                kinds.insert(kind(&op));
            }
        }
        // The pick list and the dialog.
        let pick = op_for_section_pick(&space, &rid("!b:x"), SectionPick::Existing(4));
        kinds.insert(kind(&pick.expect("an op")));
        assert_eq!(
            op_for_section_pick(&space, &rid("!b:x"), SectionPick::New),
            None
        );
        let label = SidebarLabel::try_from("Ops").expect("a name");
        for purpose in [
            Purpose::NewSection {
                space: space.clone(),
                room: None,
            },
            Purpose::NewSection {
                space: space.clone(),
                room: Some(rid("!b:x")),
            },
            Purpose::RenameSection {
                space: space.clone(),
                id: 4,
            },
        ] {
            kinds.insert(kind(&op_for_name(&purpose, Some(label)).expect("an op")));
            assert_eq!(op_for_name(&purpose, None), None, "a section needs a name");
        }
        // Every section and room operation has a way in from a card.
        for want in [
            "Star",
            "Unstar",
            "NewSection",
            "RenameSection",
            "DeleteSection",
            "MoveSection",
            "MoveRoomToSection",
            "MoveRoomInSection",
            "RoomToDefault",
            "ToggleSection",
        ] {
            assert!(kinds.contains(want), "no card reaches {want}: {kinds:?}");
        }
    }

    #[test]
    fn a_room_card_offers_up_and_down_only_in_the_users_own_sections() {
        assert_eq!(room_entries(&SectionKey::Channels), [Entry::Star]);
        assert_eq!(room_entries(&SectionKey::Direct), [Entry::Star]);
        assert_eq!(
            room_entries(&SectionKey::SubSpace(rid("!s:x"))),
            [Entry::Star]
        );
        assert_eq!(room_entries(&SectionKey::Starred), [Entry::Unstar]);
        assert_eq!(
            room_entries(&SectionKey::Custom(1)),
            [
                Entry::Star,
                Entry::MoveUp,
                Entry::MoveDown,
                Entry::BackToDefault
            ]
        );
    }

    #[test]
    fn the_move_to_section_list_has_the_other_sections_then_new_section() {
        let label = |n: &str| SidebarLabel::try_from(n).expect("a name");
        let custom = [
            CustomSection {
                id: 1,
                name: label("Work"),
                rooms: Vec::new(),
            },
            CustomSection {
                id: 2,
                name: label("Fun"),
                rooms: Vec::new(),
            },
        ];
        let picks = |current| -> Vec<SectionPick> {
            section_choices(&custom, current)
                .into_iter()
                .map(|c| c.pick)
                .collect()
        };
        assert_eq!(
            picks(None),
            [
                SectionPick::Existing(1),
                SectionPick::Existing(2),
                SectionPick::New
            ]
        );
        assert_eq!(picks(Some(1)), [SectionPick::Existing(2), SectionPick::New]);
        assert_eq!(picks(Some(1)).len(), 2);
        let shown: Vec<String> = section_choices(&custom, None)
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(shown, ["Work", "Fun", "New section\u{2026}"]);
        assert_eq!(section_choices(&[], None).len(), 1, "only New section");
    }

    #[test]
    fn a_section_name_cannot_be_blank_and_follows_the_label_rules() {
        assert!(parse_section_name("").is_err());
        assert!(parse_section_name("   ").is_err());
        assert_eq!(
            parse_section_name("  Ops ").expect("a name").as_str(),
            "Ops"
        );
        assert!(parse_section_name(&"s".repeat(33)).is_err());
        assert!(parse_section_name(&"s".repeat(32)).is_ok());
        assert!(parse_section_name("\u{202E}x").is_err());
        assert!(parse_section_name("a\nb").is_err());
    }

    #[test]
    fn debug_of_the_section_cards_and_messages_hides_ids_and_names() {
        let texts = [
            format!(
                "{:?}",
                Target::Section {
                    space: rid("!secretroom:x"),
                    key: SectionKey::SubSpace(rid("!secretroom:x"))
                }
            ),
            format!(
                "{:?}",
                Target::Room {
                    space: rid("!secretroom:x"),
                    room: rid("!secretroom:x")
                }
            ),
            format!(
                "{:?}",
                Purpose::NewSection {
                    space: rid("!secretroom:x"),
                    room: Some(rid("!secretroom:x"))
                }
            ),
            format!(
                "{:?}",
                Purpose::RenameSection {
                    space: rid("!secretroom:x"),
                    id: 3
                }
            ),
            format!("{:?}", SidebarMenuMessage::OpenRoom(rid("!secretroom:x"))),
            format!(
                "{:?}",
                SidebarMenuMessage::OpenSection(SectionKey::SubSpace(rid("!secretroom:x")))
            ),
            format!(
                "{:?}",
                SectionChoice {
                    pick: SectionPick::Existing(1),
                    label: "Payroll".to_owned()
                }
            ),
            format!("{:?}", spot(SectionKey::SubSpace(rid("!secretroom:x")))),
        ];
        for text in texts {
            for secret in ["!secretroom", "@a:x", "Payroll"] {
                assert!(!text.contains(secret), "{secret} is in {text}");
            }
        }
    }
}
