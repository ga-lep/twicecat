//! The app's model of the sidebar layout: the logged-in account's arrangement
//! of the spaces rail and of the sections in each space.
//!
//! The model owns no storage. It calls a [`SidebarStore`] that the app hands
//! to each method, and it saves the layout of its own account after every
//! successful [`Op`]. A file the app may not touch (`Newer`, `OtherAccount`)
//! turns customizing off: every operation is then refused and nothing is
//! saved. `Debug` of the model, of an [`Op`] and of the store's types shows
//! counts and kinds only: no room ID, account, folder name or path.

use std::collections::HashSet;
use std::fmt;

use iced::Task;
use twicecat_core::api::sidebar::limits::MAX_SECTION_KEYS;
use twicecat_core::api::{
    CustomSection, FolderLayout, RailEntry, RoomId, SectionKey, SidebarLabel, SidebarLayout,
    SpaceLayout, UserId,
};

use crate::sidebar_store::{SidebarLoad, SidebarStore};

/// Where the model stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarState {
    /// No layout: logged out, or the load has not answered yet (or could not
    /// run). Customizing is off.
    NotLoaded,
    /// A layout is loaded and can be changed.
    Loaded,
    /// The file was saved by a newer Twicecat. Customizing is off; the file is
    /// never changed.
    Newer,
    /// The file names another account. Customizing is off; the file is never
    /// changed.
    OtherAccount,
    /// The last change was refused because the layout would pass a limit
    /// ("The sidebar layout is full"). Other changes are still tried.
    Full,
}

/// Which way a move goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Towards the start of the rail (the top).
    Up,
    /// Towards the end of the rail (the bottom).
    Down,
}

/// The folder a space is moved into.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FolderTarget {
    /// A folder of the rail, by its ID.
    Existing(u16),
    /// A new folder, with the name the user gave it (if any), made at the
    /// place of the space. The lowest free ID is used.
    New(Option<SidebarLabel>),
}

impl fmt::Debug for FolderTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Existing(id) => f.debug_tuple("Existing").field(id).finish(),
            // The name is the user's own text: only whether there is one.
            Self::New(name) => f
                .debug_struct("New")
                .field("named", &name.is_some())
                .finish(),
        }
    }
}

/// A change of the layout.
///
/// `Debug` shows what kind of change it is and a folder's number, never a room
/// ID or a name.
#[derive(Clone, PartialEq, Eq)]
pub enum Op {
    /// Opens or closes the folder with this ID.
    ToggleFolder(u16),
    /// Collapses or expands a section of a space.
    ToggleSection {
        /// The space.
        space: RoomId,
        /// The section.
        key: SectionKey,
    },
    /// Moves a space one item up or down the rail as drawn. Inside a folder it
    /// moves within the folder and never leaves it. At the end it does
    /// nothing.
    MoveSpace {
        /// The space.
        id: RoomId,
        /// Which way.
        dir: Dir,
    },
    /// Takes a space out of where it is and puts it in a folder: at the end of
    /// an existing one, or alone in a new one made at its place. A folder it
    /// leaves with fewer than 2 spaces dissolves.
    MoveToFolder {
        /// The space.
        id: RoomId,
        /// The folder.
        folder: FolderTarget,
    },
    /// Takes a space out of its folder and puts it on the rail right after the
    /// folder. A folder left with fewer than 2 spaces dissolves.
    RemoveFromFolder(RoomId),
    /// Gives a folder a name, or clears it (`None`).
    RenameFolder {
        /// The folder.
        id: u16,
        /// The new name.
        name: Option<SidebarLabel>,
    },
    /// Dissolves a folder: its spaces go back on the rail, in order, at its
    /// place.
    UngroupFolder(u16),
    /// Moves a folder one item up or down the rail as drawn.
    MoveFolder {
        /// The folder.
        id: u16,
        /// Which way.
        dir: Dir,
    },
    /// Stars a room in a space: it goes to the end of Starred and leaves any
    /// section of the user's (a room has one place per space). Starred is
    /// local and per space; the Matrix `m.favourite` tag is not used.
    Star {
        /// The space.
        space: RoomId,
        /// The room.
        room: RoomId,
    },
    /// Takes a room out of Starred: it goes back to its default section.
    Unstar {
        /// The space.
        space: RoomId,
        /// The room.
        room: RoomId,
    },
    /// Makes a section of the user's own, last among theirs, under the lowest
    /// free ID, and puts `room` in it when there is one. The section takes its
    /// default place in the order.
    NewSection {
        /// The space.
        space: RoomId,
        /// The name.
        name: SidebarLabel,
        /// A room to put in it, taken out of its other place.
        room: Option<RoomId>,
    },
    /// Gives a section of the user's own a new name. Its rooms stay.
    RenameSection {
        /// The space.
        space: RoomId,
        /// The section.
        id: u16,
        /// The new name.
        name: SidebarLabel,
    },
    /// Deletes a section of the user's own. Its rooms are in no list any
    /// more, so each goes back to its default section; its keys leave `order`
    /// and `collapsed`.
    DeleteSection {
        /// The space.
        space: RoomId,
        /// The section.
        id: u16,
    },
    /// Swaps a section with its neighbour among the sections drawn now.
    MoveSection {
        /// The space.
        space: RoomId,
        /// The section.
        key: SectionKey,
        /// Which way.
        dir: Dir,
        /// The sections the list draws, in order (hidden ones are not in it,
        /// and are stepped over).
        drawn: Vec<SectionKey>,
    },
    /// Puts a room at the end of a section of the user's own, taking it out
    /// of Starred and of any other section of theirs.
    MoveRoomToSection {
        /// The space.
        space: RoomId,
        /// The room.
        room: RoomId,
        /// The section.
        section: u16,
    },
    /// Moves a room one place up or down inside the section of the user's own
    /// that holds it, over the rooms drawn now; other sections are not
    /// touched.
    MoveRoomInSection {
        /// The space.
        space: RoomId,
        /// The room.
        room: RoomId,
        /// Which way.
        dir: Dir,
        /// The rooms of that section as drawn, in order.
        drawn: Vec<RoomId>,
    },
    /// Takes a room out of Starred and of every section of the user's own, so
    /// it goes back to its default section.
    RoomToDefault {
        /// The space.
        space: RoomId,
        /// The room.
        room: RoomId,
    },
}

impl fmt::Debug for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToggleFolder(id) => f.debug_tuple("ToggleFolder").field(id).finish(),
            Self::ToggleSection { .. } => f.write_str("ToggleSection { .. }"),
            Self::MoveSpace { dir, .. } => f.debug_struct("MoveSpace").field("dir", dir).finish(),
            Self::MoveToFolder { folder, .. } => f
                .debug_struct("MoveToFolder")
                .field("folder", folder)
                .finish(),
            Self::RemoveFromFolder(_) => f.write_str("RemoveFromFolder(..)"),
            Self::RenameFolder { id, name } => f
                .debug_struct("RenameFolder")
                .field("id", id)
                .field("named", &name.is_some())
                .finish(),
            Self::UngroupFolder(id) => f.debug_tuple("UngroupFolder").field(id).finish(),
            Self::MoveFolder { id, dir } => f
                .debug_struct("MoveFolder")
                .field("id", id)
                .field("dir", dir)
                .finish(),
            Self::Star { .. } => f.write_str("Star { .. }"),
            Self::Unstar { .. } => f.write_str("Unstar { .. }"),
            Self::NewSection { room, .. } => f
                .debug_struct("NewSection")
                .field("with_room", &room.is_some())
                .finish(),
            Self::RenameSection { id, .. } => {
                f.debug_struct("RenameSection").field("id", id).finish()
            }
            Self::DeleteSection { id, .. } => {
                f.debug_struct("DeleteSection").field("id", id).finish()
            }
            Self::MoveSection { key, dir, .. } => f
                .debug_struct("MoveSection")
                .field("key", key)
                .field("dir", dir)
                .finish(),
            Self::MoveRoomToSection { section, .. } => f
                .debug_struct("MoveRoomToSection")
                .field("section", section)
                .finish(),
            Self::MoveRoomInSection { dir, .. } => f
                .debug_struct("MoveRoomInSection")
                .field("dir", dir)
                .finish(),
            Self::RoomToDefault { .. } => f.write_str("RoomToDefault { .. }"),
        }
    }
}

/// Why an [`Op`] (or a reset) was not done. Nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// No layout is loaded.
    NotLoaded,
    /// The stored file is `Newer` or names `OtherAccount`: it is not touched.
    Protected,
    /// The layout would pass a limit.
    Full,
    /// The folder, section or space the change names does not exist.
    Unknown,
}

impl Refused {
    /// The line to show for it.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotLoaded => "The sidebar layout is not loaded yet",
            Self::Protected => "The sidebar layout cannot be changed on this device",
            Self::Full => "The sidebar layout is full",
            Self::Unknown => "That item is not on the sidebar any more",
        }
    }
}

/// The logged-in account's sidebar layout.
#[derive(Clone, PartialEq, Eq)]
pub struct SidebarModel {
    account: Option<UserId>,
    layout: Option<SidebarLayout>,
    state: SidebarState,
}

impl Default for SidebarModel {
    fn default() -> Self {
        Self {
            account: None,
            layout: None,
            state: SidebarState::NotLoaded,
        }
    }
}

impl fmt::Debug for SidebarModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SidebarModel")
            .field("state", &self.state)
            .field("account", &self.account.as_ref().map(|_| "<redacted>"))
            // The layout's own `Debug` shows counts only.
            .field("layout", &self.layout)
            .finish()
    }
}

impl SidebarModel {
    /// Where the model stands.
    pub fn state(&self) -> SidebarState {
        self.state
    }

    /// The loaded layout, if any.
    pub fn layout(&self) -> Option<&SidebarLayout> {
        self.layout.as_ref()
    }

    /// Starts loading `account`'s layout. Until the answer comes back
    /// ([`Self::loaded`]) the model has no layout and refuses every change.
    pub fn load_for(&mut self, account: UserId, store: &dyn SidebarStore) -> Task<SidebarLoad> {
        let task = store.load(&account);
        self.account = Some(account);
        self.layout = None;
        self.state = SidebarState::NotLoaded;
        task
    }

    /// Takes the answer of a load. An answer for any other account than the
    /// one the model is loading for (a late one) is ignored, and so is a
    /// second answer for an account that is already loaded.
    pub fn loaded(&mut self, account: &UserId, load: SidebarLoad) {
        if self.account.as_ref() != Some(account) || self.state != SidebarState::NotLoaded {
            return;
        }
        match load {
            SidebarLoad::Layout(layout) if layout.account == *account => {
                self.layout = Some(layout);
                self.state = SidebarState::Loaded;
            }
            // A layout of another account must not get here; treat it as no
            // answer rather than showing it.
            SidebarLoad::Layout(_) | SidebarLoad::Unavailable => {}
            SidebarLoad::Default => {
                self.layout = Some(SidebarLayout::new(account.clone()));
                self.state = SidebarState::Loaded;
            }
            SidebarLoad::Newer => self.state = SidebarState::Newer,
            SidebarLoad::OtherAccount => self.state = SidebarState::OtherAccount,
        }
    }

    /// Forgets the layout and the account. Calls no store: nothing on disk
    /// changes at logout.
    pub fn unload(&mut self) {
        *self = Self::default();
    }

    /// Whether changes are allowed, and if not, why.
    fn check_open(&self) -> Result<(), Refused> {
        match self.state {
            SidebarState::Loaded | SidebarState::Full => Ok(()),
            SidebarState::NotLoaded => Err(Refused::NotLoaded),
            SidebarState::Newer | SidebarState::OtherAccount => Err(Refused::Protected),
        }
    }

    /// Does `op` and saves the new layout for the model's own account. The
    /// task yields whether the save worked. A change that changes nothing
    /// (a move past the end, a name set to itself) succeeds without a save.
    ///
    /// Rail moves need to know which spaces the rail draws: use
    /// [`Self::apply_known`].
    pub fn apply(&mut self, op: Op, store: &dyn SidebarStore) -> Result<Task<bool>, Refused> {
        self.apply_known(op, &[], store)
    }

    /// [`Self::apply`], where `known` are the top-level spaces the core
    /// reports, in its order. The rail draws those, in the layout's order,
    /// with the ones the layout does not name after them, so a move is
    /// relative to the neighbours that are drawn: stored spaces the core does
    /// not report are stepped over and kept.
    pub fn apply_known(
        &mut self,
        op: Op,
        known: &[RoomId],
        store: &dyn SidebarStore,
    ) -> Result<Task<bool>, Refused> {
        self.apply_all_known(vec![op], known, store)
    }

    /// [`Self::apply_known`] for several operations that make one change: the
    /// steps of a drop. They are done in order on a copy; if any is refused
    /// nothing changes, and the result is saved once, or not at all when no
    /// step changed anything.
    pub fn apply_all_known(
        &mut self,
        ops: Vec<Op>,
        known: &[RoomId],
        store: &dyn SidebarStore,
    ) -> Result<Task<bool>, Refused> {
        self.check_open()?;
        let (Some(account), Some(current)) = (self.account.clone(), self.layout.as_ref()) else {
            return Err(Refused::NotLoaded);
        };
        let mut next = current.clone();
        let mut changed = false;
        for op in ops {
            changed |= change(&mut next, op, known)? == Change::Done;
        }
        if !changed {
            return Ok(Task::none());
        }
        if !next.fits() {
            self.state = SidebarState::Full;
            return Err(Refused::Full);
        }
        let task = store.save(&account, next.clone());
        self.layout = Some(next);
        self.state = SidebarState::Loaded;
        Ok(task)
    }

    /// Removes the account's file and goes back to the default layout (the
    /// Reset button, Task 13). Refused for a protected file.
    pub fn reset(
        &mut self,
        store: &dyn SidebarStore,
    ) -> Result<Task<crate::sidebar_store::ResetOutcome>, Refused> {
        self.check_open()?;
        let Some(account) = self.account.clone() else {
            return Err(Refused::NotLoaded);
        };
        let task = store.reset(&account);
        self.layout = Some(SidebarLayout::new(account));
        self.state = SidebarState::Loaded;
        Ok(task)
    }
}

/// Whether an operation changed the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Done,
    Nothing,
}

/// Does `op` on `layout`. The caller checks the limits afterwards.
fn change(layout: &mut SidebarLayout, op: Op, known: &[RoomId]) -> Result<Change, Refused> {
    match op {
        Op::ToggleFolder(id) => {
            let folder = folder_mut(layout, id).ok_or(Refused::Unknown)?;
            folder.open = !folder.open;
        }
        Op::ToggleSection { space, key } => {
            let at = match layout.spaces.iter().position(|s| s.space == space) {
                Some(at) => at,
                None => {
                    layout.spaces.push(SpaceLayout::new(space));
                    layout.spaces.len() - 1
                }
            };
            let collapsed = &mut layout.spaces[at].collapsed;
            match collapsed.iter().position(|k| *k == key) {
                Some(n) => {
                    collapsed.remove(n);
                }
                None => collapsed.push(key),
            }
        }
        Op::MoveSpace { id, dir } => {
            materialize(layout, known);
            return move_space(layout, &id, dir, known);
        }
        Op::MoveFolder { id, dir } => {
            materialize(layout, known);
            let at = folder_index(layout, id).ok_or(Refused::Unknown)?;
            let Some(to) = step(&layout.rail, at, dir, |e| drawn(e, known)) else {
                return Ok(Change::Nothing);
            };
            let entry = layout.rail.remove(at);
            layout.rail.insert(to, entry);
        }
        Op::MoveToFolder { id, folder } => {
            materialize(layout, known);
            return move_to_folder(layout, id, folder);
        }
        Op::RemoveFromFolder(id) => {
            if !in_a_folder(layout, &id) {
                return Err(Refused::Unknown);
            }
            let at = take_out(layout, &id).ok_or(Refused::Unknown)?;
            layout.rail.insert(at, RailEntry::Space(id));
        }
        Op::RenameFolder { id, name } => {
            let folder = folder_mut(layout, id).ok_or(Refused::Unknown)?;
            if folder.name == name {
                return Ok(Change::Nothing);
            }
            folder.name = name;
        }
        Op::Star { space, room } => {
            let record = record_mut(layout, space);
            if record.starred.contains(&room) {
                return Ok(Change::Nothing);
            }
            take_room_out(record, &room);
            record.starred.push(room);
        }
        Op::Unstar { space, room } => {
            let Some(record) = existing_record_mut(layout, &space) else {
                return Ok(Change::Nothing);
            };
            let Some(at) = record.starred.iter().position(|r| *r == room) else {
                return Ok(Change::Nothing);
            };
            record.starred.remove(at);
        }
        Op::NewSection { space, name, room } => {
            let record = record_mut(layout, space);
            let id = (0..=u16::MAX)
                .find(|n| record.custom.iter().all(|c| c.id != *n))
                .ok_or(Refused::Full)?;
            let rooms = match room {
                Some(room) => {
                    take_room_out(record, &room);
                    vec![room]
                }
                None => Vec::new(),
            };
            record.custom.push(CustomSection { id, name, rooms });
        }
        Op::RenameSection { space, id, name } => {
            let section = section_mut(layout, &space, id).ok_or(Refused::Unknown)?;
            if section.name == name {
                return Ok(Change::Nothing);
            }
            section.name = name;
        }
        Op::DeleteSection { space, id } => {
            let record = existing_record_mut(layout, &space).ok_or(Refused::Unknown)?;
            let at = record
                .custom
                .iter()
                .position(|c| c.id == id)
                .ok_or(Refused::Unknown)?;
            record.custom.remove(at);
            let key = SectionKey::Custom(id);
            record.order.retain(|k| *k != key);
            record.collapsed.retain(|k| *k != key);
        }
        Op::MoveSection {
            space,
            key,
            dir,
            drawn,
        } => {
            let at = drawn
                .iter()
                .position(|k| *k == key)
                .ok_or(Refused::Unknown)?;
            let Some(to) = step(&drawn, at, dir, |_| true) else {
                return Ok(Change::Nothing);
            };
            let record = record_mut(layout, space);
            let mut swapped = drawn;
            swapped.swap(at, to);
            let order = merge_order(swapped, &record.order);
            // `order` holds every drawn key. More than it can hold is a
            // key-count limit, not a placement limit: refuse the move here,
            // before the layout is checked, so the state does not become
            // `Full` and other changes stay possible.
            if order.len() > MAX_SECTION_KEYS {
                return Err(Refused::Full);
            }
            record.order = order;
        }
        Op::MoveRoomToSection {
            space,
            room,
            section,
        } => {
            let record = existing_record_mut(layout, &space).ok_or(Refused::Unknown)?;
            let target = record
                .custom
                .iter()
                .find(|c| c.id == section)
                .ok_or(Refused::Unknown)?;
            if target.rooms.contains(&room) {
                return Ok(Change::Nothing);
            }
            take_room_out(record, &room);
            if let Some(target) = record.custom.iter_mut().find(|c| c.id == section) {
                target.rooms.push(room);
            }
        }
        Op::MoveRoomInSection {
            space,
            room,
            dir,
            drawn,
        } => {
            let record = existing_record_mut(layout, &space).ok_or(Refused::Unknown)?;
            let section = record
                .custom
                .iter_mut()
                .find(|c| c.rooms.contains(&room))
                .ok_or(Refused::Unknown)?;
            let at = section
                .rooms
                .iter()
                .position(|r| *r == room)
                .ok_or(Refused::Unknown)?;
            let Some(to) = step(&section.rooms, at, dir, |r| drawn.contains(r)) else {
                return Ok(Change::Nothing);
            };
            let moved = section.rooms.remove(at);
            section.rooms.insert(to, moved);
        }
        Op::RoomToDefault { space, room } => {
            let Some(record) = existing_record_mut(layout, &space) else {
                return Ok(Change::Nothing);
            };
            if !take_room_out(record, &room) {
                return Ok(Change::Nothing);
            }
        }
        Op::UngroupFolder(id) => {
            let at = folder_index(layout, id).ok_or(Refused::Unknown)?;
            let RailEntry::Folder(folder) = layout.rail.remove(at) else {
                return Err(Refused::Unknown);
            };
            for (n, space) in folder.spaces.into_iter().enumerate() {
                layout.rail.insert(at + n, RailEntry::Space(space));
            }
        }
    }
    Ok(Change::Done)
}

/// Does `op` on `layout` and says whether it changed anything: how a drop is
/// worked out step by step on a copy (see `rail_drop`).
pub(crate) fn simulate(
    layout: &mut SidebarLayout,
    op: Op,
    known: &[RoomId],
) -> Result<bool, Refused> {
    Ok(change(layout, op, known)? == Change::Done)
}

/// A copy of `layout` with the rail written down as it is drawn (see
/// `materialize`).
pub(crate) fn materialized(layout: &SidebarLayout, known: &[RoomId]) -> SidebarLayout {
    let mut copy = layout.clone();
    materialize(&mut copy, known);
    copy
}

/// Whether the rail draws `entry`: a space the core reports, or a folder with
/// at least one.
pub(crate) fn is_drawn(entry: &RailEntry, known: &[RoomId]) -> bool {
    drawn(entry, known)
}

/// The record of `space`, made empty and added when the layout has none.
fn record_mut(layout: &mut SidebarLayout, space: RoomId) -> &mut SpaceLayout {
    let at = match layout.spaces.iter().position(|s| s.space == space) {
        Some(at) => at,
        None => {
            layout.spaces.push(SpaceLayout::new(space));
            layout.spaces.len() - 1
        }
    };
    &mut layout.spaces[at]
}

fn existing_record_mut<'a>(
    layout: &'a mut SidebarLayout,
    space: &RoomId,
) -> Option<&'a mut SpaceLayout> {
    layout.spaces.iter_mut().find(|s| s.space == *space)
}

fn section_mut<'a>(
    layout: &'a mut SidebarLayout,
    space: &RoomId,
    id: u16,
) -> Option<&'a mut CustomSection> {
    existing_record_mut(layout, space)?
        .custom
        .iter_mut()
        .find(|c| c.id == id)
}

/// Takes `room` out of Starred and out of every section of the user's own, so
/// a room has one place per space. Says whether it was in any.
fn take_room_out(record: &mut SpaceLayout, room: &RoomId) -> bool {
    let before = record.starred.len() + record.custom.iter().map(|c| c.rooms.len()).sum::<usize>();
    record.starred.retain(|r| r != room);
    for section in &mut record.custom {
        section.rooms.retain(|r| r != room);
    }
    before != record.starred.len() + record.custom.iter().map(|c| c.rooms.len()).sum::<usize>()
}

/// The stored order after the drawn sections were rearranged to `drawn`: the
/// drawn ones in that order, and each stored key that is not drawn (hidden,
/// or unknown to this device) right after the key that preceded it, so it is
/// kept where it was.
fn merge_order(drawn: Vec<SectionKey>, stored: &[SectionKey]) -> Vec<SectionKey> {
    let mut out = drawn;
    for (n, key) in stored.iter().enumerate() {
        if out.contains(key) {
            continue;
        }
        let after = stored[..n]
            .iter()
            .rev()
            .find_map(|earlier| out.iter().position(|k| k == earlier));
        out.insert(after.map_or(0, |p| p + 1), key.clone());
    }
    out
}

fn folder_mut(layout: &mut SidebarLayout, id: u16) -> Option<&mut FolderLayout> {
    layout.rail.iter_mut().find_map(|entry| match entry {
        RailEntry::Folder(folder) if folder.id == id => Some(folder),
        _ => None,
    })
}

fn folder_index(layout: &SidebarLayout, id: u16) -> Option<usize> {
    layout
        .rail
        .iter()
        .position(|entry| matches!(entry, RailEntry::Folder(f) if f.id == id))
}

fn in_a_folder(layout: &SidebarLayout, id: &RoomId) -> bool {
    layout
        .rail
        .iter()
        .any(|entry| matches!(entry, RailEntry::Folder(f) if f.spaces.contains(id)))
}

/// Adds the spaces the core reports and the layout does not name to the end of
/// the rail, in the core's order: the rail as it is drawn, written down.
fn materialize(layout: &mut SidebarLayout, known: &[RoomId]) {
    let mut placed: HashSet<RoomId> = HashSet::new();
    for entry in &layout.rail {
        match entry {
            RailEntry::Space(id) => {
                placed.insert(id.clone());
            }
            RailEntry::Folder(folder) => placed.extend(folder.spaces.iter().cloned()),
        }
    }
    for id in known {
        if placed.insert(id.clone()) {
            layout.rail.push(RailEntry::Space(id.clone()));
        }
    }
}

/// Whether the rail draws `entry`: a space the core reports, or a folder with
/// at least one.
fn drawn(entry: &RailEntry, known: &[RoomId]) -> bool {
    match entry {
        RailEntry::Space(id) => known.contains(id),
        RailEntry::Folder(folder) => folder.spaces.iter().any(|id| known.contains(id)),
    }
}

/// The index of the nearest item to `from` in direction `dir` that
/// `visible` accepts.
fn step<T>(items: &[T], from: usize, dir: Dir, visible: impl Fn(&T) -> bool) -> Option<usize> {
    match dir {
        Dir::Up => (0..from).rev().find(|&n| visible(&items[n])),
        Dir::Down => (from + 1..items.len()).find(|&n| visible(&items[n])),
    }
}

fn move_space(
    layout: &mut SidebarLayout,
    id: &RoomId,
    dir: Dir,
    known: &[RoomId],
) -> Result<Change, Refused> {
    if let Some(at) = layout
        .rail
        .iter()
        .position(|entry| matches!(entry, RailEntry::Space(s) if s == id))
    {
        let Some(to) = step(&layout.rail, at, dir, |e| drawn(e, known)) else {
            return Ok(Change::Nothing);
        };
        let entry = layout.rail.remove(at);
        layout.rail.insert(to, entry);
        return Ok(Change::Done);
    }
    for entry in &mut layout.rail {
        let RailEntry::Folder(folder) = entry else {
            continue;
        };
        let Some(at) = folder.spaces.iter().position(|s| s == id) else {
            continue;
        };
        let Some(to) = step(&folder.spaces, at, dir, |s| known.contains(s)) else {
            return Ok(Change::Nothing);
        };
        let space = folder.spaces.remove(at);
        folder.spaces.insert(to, space);
        return Ok(Change::Done);
    }
    Err(Refused::Unknown)
}

/// Takes `id` out of the rail, from the top level or from a folder, and says
/// where on the rail something that replaces it goes. A folder that is left
/// with fewer than 2 spaces dissolves into the rail.
fn take_out(layout: &mut SidebarLayout, id: &RoomId) -> Option<usize> {
    if let Some(at) = layout
        .rail
        .iter()
        .position(|entry| matches!(entry, RailEntry::Space(s) if s == id))
    {
        layout.rail.remove(at);
        return Some(at);
    }
    let at = layout
        .rail
        .iter()
        .position(|entry| matches!(entry, RailEntry::Folder(f) if f.spaces.contains(id)))?;
    let RailEntry::Folder(folder) = &mut layout.rail[at] else {
        return None;
    };
    folder.spaces.retain(|s| s != id);
    if folder.spaces.len() >= 2 {
        return Some(at + 1);
    }
    // The folder dissolves: what is left of it takes its place.
    let RailEntry::Folder(folder) = layout.rail.remove(at) else {
        return None;
    };
    let left = folder.spaces.len();
    for (n, space) in folder.spaces.into_iter().enumerate() {
        layout.rail.insert(at + n, RailEntry::Space(space));
    }
    Some(at + left)
}

fn move_to_folder(
    layout: &mut SidebarLayout,
    id: RoomId,
    target: FolderTarget,
) -> Result<Change, Refused> {
    match target {
        FolderTarget::Existing(folder) => {
            let into = folder_index(layout, folder).ok_or(Refused::Unknown)?;
            let RailEntry::Folder(f) = &layout.rail[into] else {
                return Err(Refused::Unknown);
            };
            if f.spaces.contains(&id) {
                return Ok(Change::Nothing);
            }
            take_out(layout, &id).ok_or(Refused::Unknown)?;
            // The folder may have moved if the space's own folder dissolved.
            let folder = folder_mut(layout, folder).ok_or(Refused::Unknown)?;
            folder.spaces.push(id);
        }
        FolderTarget::New(name) => {
            let new_id = (0..=u16::MAX)
                .find(|n| folder_index(layout, *n).is_none())
                .ok_or(Refused::Full)?;
            let at = take_out(layout, &id).ok_or(Refused::Unknown)?;
            layout.rail.insert(
                at,
                RailEntry::Folder(FolderLayout {
                    id: new_id,
                    name,
                    // Closed, so the rail shows its preview.
                    open: false,
                    spaces: vec![id],
                }),
            );
        }
    }
    Ok(Change::Done)
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{FolderLayout, SidebarLabel};

    use super::*;
    use crate::sidebar_store::{Call, MemorySidebar};

    fn user(name: &str) -> UserId {
        UserId::from(name)
    }

    fn rid(name: &str) -> RoomId {
        RoomId::from(name)
    }

    fn folder_layout(account: &UserId) -> SidebarLayout {
        let mut layout = SidebarLayout::new(account.clone());
        layout.rail = vec![
            RailEntry::Space(rid("!solo:x")),
            RailEntry::Folder(FolderLayout {
                id: 1,
                name: None,
                open: false,
                spaces: vec![rid("!in1:x"), rid("!in2:x")],
            }),
        ];
        layout
    }

    fn loaded_model(store: &MemorySidebar, layout: SidebarLayout) -> SidebarModel {
        let account = layout.account.clone();
        let mut model = SidebarModel::default();
        let _ = model.load_for(account.clone(), store);
        model.loaded(&account, SidebarLoad::Layout(layout));
        model
    }

    #[test]
    fn a_fresh_model_has_nothing_and_refuses_changes() {
        let store = MemorySidebar::default();
        let mut model = SidebarModel::default();
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
        assert_eq!(
            model.apply(Op::ToggleFolder(1), &store).err(),
            Some(Refused::NotLoaded)
        );
        assert!(store.calls().is_empty());
    }

    #[test]
    fn load_for_asks_the_store_once_and_waits_for_the_answer() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        assert_eq!(store.calls(), vec![Call::Load(a.clone())]);
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.apply(Op::ToggleFolder(1), &store).is_err());
        model.loaded(&a, SidebarLoad::Default);
        assert_eq!(model.state(), SidebarState::Loaded);
        assert_eq!(model.layout(), Some(&SidebarLayout::new(a)));
    }

    #[test]
    fn a_late_answer_for_another_account_is_ignored() {
        let store = MemorySidebar::default();
        let (a, b) = (user("@a:x"), user("@b:x"));
        let mut model = SidebarModel::default();
        let _ = model.load_for(b.clone(), &store);
        model.loaded(&a, SidebarLoad::Layout(folder_layout(&a)));
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
        // And one with no load under way.
        model.unload();
        model.loaded(&b, SidebarLoad::Default);
        assert_eq!(model.state(), SidebarState::NotLoaded);
    }

    #[test]
    fn a_layout_of_another_account_is_not_taken() {
        let store = MemorySidebar::default();
        let (a, b) = (user("@a:x"), user("@b:x"));
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::Layout(folder_layout(&b)));
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
    }

    #[test]
    fn newer_other_account_and_unavailable_turn_customizing_off() {
        for (load, state) in [
            (SidebarLoad::Newer, SidebarState::Newer),
            (SidebarLoad::OtherAccount, SidebarState::OtherAccount),
            (SidebarLoad::Unavailable, SidebarState::NotLoaded),
        ] {
            let store = MemorySidebar::default();
            let a = user("@a:x");
            let mut model = SidebarModel::default();
            let _ = model.load_for(a.clone(), &store);
            model.loaded(&a, load);
            assert_eq!(model.state(), state);
            assert!(model.layout().is_none());
            assert!(model.apply(Op::ToggleFolder(1), &store).is_err());
            assert!(model.reset(&store).is_err());
            assert_eq!(store.calls(), vec![Call::Load(a)], "no save, no reset");
        }
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::Newer);
        assert_eq!(
            model.apply(Op::ToggleFolder(1), &store).err(),
            Some(Refused::Protected)
        );
    }

    #[test]
    fn a_successful_change_saves_once_for_the_account_with_the_new_layout() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        assert!(model.apply(Op::ToggleFolder(1), &store).is_ok());
        let mut expected = folder_layout(&a);
        if let RailEntry::Folder(folder) = &mut expected.rail[1] {
            folder.open = true;
        }
        assert_eq!(
            store.calls(),
            vec![Call::Load(a.clone()), Call::Save(a, expected.clone())]
        );
        assert_eq!(model.layout(), Some(&expected));
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn toggling_a_section_collapses_and_expands_it() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        let op = Op::ToggleSection {
            space: rid("!sp:x"),
            key: SectionKey::Direct,
        };
        assert!(model.apply(op.clone(), &store).is_ok());
        let record = |m: &SidebarModel| {
            m.layout()
                .and_then(|l| l.spaces.iter().find(|s| s.space == rid("!sp:x")))
                .map(|s| s.collapsed.clone())
        };
        assert_eq!(record(&model), Some(vec![SectionKey::Direct]));
        assert!(model.apply(op, &store).is_ok());
        assert_eq!(record(&model), Some(Vec::new()));
        let saves = store
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Save(..)))
            .count();
        assert_eq!(saves, 2);
    }

    #[test]
    fn an_unknown_folder_is_refused_and_saves_nothing() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        assert_eq!(
            model.apply(Op::ToggleFolder(9), &store).err(),
            Some(Refused::Unknown)
        );
        assert_eq!(store.calls(), vec![Call::Load(a)]);
    }

    #[test]
    fn a_change_that_passes_a_limit_is_refused_as_full_and_saves_nothing() {
        use twicecat_core::api::sidebar::limits::MAX_SPACE_RECORDS;
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut layout = SidebarLayout::new(a.clone());
        layout.spaces = (0..MAX_SPACE_RECORDS)
            .map(|n| SpaceLayout::new(rid(&format!("!s{n}:x"))))
            .collect();
        assert!(layout.fits());
        let mut model = loaded_model(&store, layout.clone());
        let op = Op::ToggleSection {
            space: rid("!one-too-many:x"),
            key: SectionKey::Starred,
        };
        assert_eq!(model.apply(op, &store).err(), Some(Refused::Full));
        assert_eq!(model.state(), SidebarState::Full);
        assert_eq!(model.layout(), Some(&layout), "the layout is unchanged");
        assert_eq!(store.calls(), vec![Call::Load(a)], "nothing saved");
        // Other changes are still tried, and a good one leaves Full.
        let op = Op::ToggleSection {
            space: rid("!s0:x"),
            key: SectionKey::Starred,
        };
        assert!(model.apply(op, &store).is_ok());
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn unload_drops_everything_and_calls_no_store() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        let before = store.calls();
        model.unload();
        assert_eq!(model.state(), SidebarState::NotLoaded);
        assert!(model.layout().is_none());
        assert_eq!(
            model.apply(Op::ToggleFolder(1), &store).err(),
            Some(Refused::NotLoaded)
        );
        assert_eq!(store.calls(), before);
    }

    #[test]
    fn reset_removes_the_file_and_goes_back_to_the_default() {
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut model = loaded_model(&store, folder_layout(&a));
        assert!(model.reset(&store).is_ok());
        assert_eq!(store.calls().last(), Some(&Call::Reset(a.clone())));
        assert_eq!(model.layout(), Some(&SidebarLayout::new(a)));
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn a_full_layout_can_be_reset() {
        use twicecat_core::api::sidebar::limits::MAX_SPACE_RECORDS;
        let store = MemorySidebar::default();
        let a = user("@a:x");
        let mut layout = SidebarLayout::new(a.clone());
        layout.spaces = (0..MAX_SPACE_RECORDS)
            .map(|n| SpaceLayout::new(rid(&format!("!s{n}:x"))))
            .collect();
        let mut model = loaded_model(&store, layout);
        let op = Op::ToggleSection {
            space: rid("!one-too-many:x"),
            key: SectionKey::Starred,
        };
        assert_eq!(model.apply(op, &store).err(), Some(Refused::Full));
        assert_eq!(model.state(), SidebarState::Full);
        assert!(model.reset(&store).is_ok());
        assert_eq!(store.calls().last(), Some(&Call::Reset(a.clone())));
        assert_eq!(model.layout(), Some(&SidebarLayout::new(a)));
        assert_eq!(model.state(), SidebarState::Loaded);
    }

    #[test]
    fn reset_is_refused_for_a_protected_file_and_calls_no_store() {
        for load in [SidebarLoad::Newer, SidebarLoad::OtherAccount] {
            let store = MemorySidebar::default();
            let a = user("@a:x");
            let mut model = SidebarModel::default();
            let _ = model.load_for(a.clone(), &store);
            model.loaded(&a, load);
            assert_eq!(model.reset(&store).err(), Some(Refused::Protected));
            assert_eq!(store.calls(), vec![Call::Load(a)], "no reset");
        }
        let store = MemorySidebar::default();
        let mut model = SidebarModel::default();
        assert_eq!(model.reset(&store).err(), Some(Refused::NotLoaded));
        assert!(store.calls().is_empty());
    }

    #[test]
    fn debug_shows_no_room_id_and_no_account() {
        let store = MemorySidebar::default();
        let a = user("@secretme:x");
        let mut layout = folder_layout(&a);
        layout.spaces.push(SpaceLayout::new(rid("!secretspace:x")));
        let model = loaded_model(&store, layout);
        let text = format!("{model:?}");
        for secret in ["!solo", "!in1", "!secretspace", "@secretme"] {
            assert!(!text.contains(secret), "{text}");
        }
    }

    // ---- arranging the rail (Task sbc.6) ----

    fn label(name: &str) -> SidebarLabel {
        SidebarLabel::try_from(name).expect("a valid name")
    }

    fn sp(id: &str) -> RailEntry {
        RailEntry::Space(rid(id))
    }

    fn folder(id: u16, spaces: &[&str]) -> RailEntry {
        RailEntry::Folder(FolderLayout {
            id,
            name: None,
            open: false,
            spaces: spaces.iter().map(|s| rid(s)).collect(),
        })
    }

    /// The rail as text: `a`, or `[1: a b]` for a folder.
    fn shape(layout: &SidebarLayout) -> Vec<String> {
        layout
            .rail
            .iter()
            .map(|entry| match entry {
                RailEntry::Space(id) => id.as_str().to_owned(),
                RailEntry::Folder(f) => {
                    let ids: Vec<&str> = f.spaces.iter().map(|s| s.as_str()).collect();
                    format!("[{}: {}]", f.id, ids.join(" "))
                }
            })
            .collect()
    }

    fn known(ids: &[&str]) -> Vec<RoomId> {
        ids.iter().map(|s| rid(s)).collect()
    }

    /// A loaded model whose layout has `rail`, and the store it saves to.
    fn rail_model(rail: Vec<RailEntry>) -> (SidebarModel, MemorySidebar) {
        let store = MemorySidebar::default();
        let mut layout = SidebarLayout::new(user("@a:x"));
        layout.rail = rail;
        (loaded_model(&store, layout), store)
    }

    fn saves(store: &MemorySidebar) -> usize {
        store
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Save(..)))
            .count()
    }

    fn rail_of(model: &SidebarModel) -> Vec<String> {
        shape(model.layout().expect("a layout"))
    }

    fn move_space(id: &str, dir: Dir) -> Op {
        Op::MoveSpace { id: rid(id), dir }
    }

    #[test]
    fn move_up_on_the_first_item_and_down_on_the_last_change_nothing_and_save_nothing() {
        let all = known(&["!a:x", "!b:x", "!c:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), sp("!b:x"), sp("!c:x")]);
        for op in [move_space("!a:x", Dir::Up), move_space("!c:x", Dir::Down)] {
            assert!(model.apply_known(op, &all, &store).is_ok());
        }
        assert_eq!(rail_of(&model), ["!a:x", "!b:x", "!c:x"]);
        assert_eq!(saves(&store), 0, "no save for a move that goes nowhere");
        // The same at the ends of an unwritten layout (the core's order).
        let (mut model, store) = rail_model(Vec::new());
        assert!(
            model
                .apply_known(move_space("!a:x", Dir::Up), &all, &store)
                .is_ok()
        );
        assert_eq!(saves(&store), 0);
        assert!(model.layout().expect("a layout").rail.is_empty());
        // And moving the only folder of the rail.
        let (mut model, store) = rail_model(vec![folder(1, &["!a:x", "!b:x"])]);
        let both = known(&["!a:x", "!b:x"]);
        let up = Op::MoveFolder {
            id: 1,
            dir: Dir::Up,
        };
        assert!(model.apply_known(up, &both, &store).is_ok());
        assert_eq!(saves(&store), 0);
    }

    #[test]
    fn move_down_moves_a_space_one_item_and_saves_once() {
        let all = known(&["!a:x", "!b:x", "!c:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), sp("!b:x"), sp("!c:x")]);
        assert!(
            model
                .apply_known(move_space("!b:x", Dir::Down), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!a:x", "!c:x", "!b:x"]);
        assert_eq!(saves(&store), 1);
        assert!(
            model
                .apply_known(move_space("!b:x", Dir::Up), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!a:x", "!b:x", "!c:x"]);
        assert_eq!(saves(&store), 2);
    }

    #[test]
    fn a_move_starts_from_the_rail_as_drawn_including_spaces_the_layout_does_not_name() {
        // The layout names only `c`; the core's other spaces are drawn after it.
        let all = known(&["!a:x", "!b:x", "!c:x"]);
        let (mut model, store) = rail_model(vec![sp("!c:x")]);
        assert!(
            model
                .apply_known(move_space("!a:x", Dir::Up), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!a:x", "!c:x", "!b:x"]);
        assert_eq!(saves(&store), 1);
    }

    #[test]
    fn a_move_steps_over_entries_that_are_not_drawn() {
        // `!gone` is stored but the core does not report it.
        let all = known(&["!a:x", "!b:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), sp("!gone:x"), sp("!b:x")]);
        assert!(
            model
                .apply_known(move_space("!a:x", Dir::Down), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!gone:x", "!b:x", "!a:x"]);
        // The stored one stays stored.
        let (mut model, store) = rail_model(vec![sp("!gone:x"), sp("!a:x")]);
        assert!(
            model
                .apply_known(move_space("!a:x", Dir::Up), &all, &store)
                .is_ok()
        );
        assert_eq!(saves(&store), 0, "nothing drawn above it");
    }

    #[test]
    fn inside_an_open_folder_a_move_stays_in_the_folder() {
        let all = known(&["!a:x", "!b:x", "!c:x", "!d:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), folder(1, &["!b:x", "!c:x", "!d:x"])]);
        assert!(
            model
                .apply_known(move_space("!b:x", Dir::Down), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!a:x", "[1: !c:x !b:x !d:x]"]);
        assert_eq!(saves(&store), 1);
        // At the end of the folder it does not leave it, and saves nothing.
        assert!(
            model
                .apply_known(move_space("!d:x", Dir::Down), &all, &store)
                .is_ok()
        );
        assert!(
            model
                .apply_known(move_space("!c:x", Dir::Up), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!a:x", "[1: !c:x !b:x !d:x]"]);
        assert_eq!(saves(&store), 1);
    }

    #[test]
    fn move_down_on_a_folders_own_card_moves_the_folder() {
        let all = known(&["!a:x", "!b:x", "!c:x", "!d:x"]);
        let (mut model, store) =
            rail_model(vec![folder(1, &["!a:x", "!b:x"]), sp("!c:x"), sp("!d:x")]);
        let down = Op::MoveFolder {
            id: 1,
            dir: Dir::Down,
        };
        assert!(model.apply_known(down, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["!c:x", "[1: !a:x !b:x]", "!d:x"]);
        assert_eq!(saves(&store), 1);
        let up = Op::MoveFolder {
            id: 1,
            dir: Dir::Up,
        };
        assert!(model.apply_known(up, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["[1: !a:x !b:x]", "!c:x", "!d:x"]);
    }

    #[test]
    fn moving_a_space_into_a_new_folder_makes_the_folder_at_its_place() {
        let all = known(&["!a:x", "!b:x", "!c:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), sp("!b:x"), sp("!c:x")]);
        let op = Op::MoveToFolder {
            id: rid("!b:x"),
            folder: FolderTarget::New(Some(label("Work"))),
        };
        assert!(model.apply_known(op, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["!a:x", "[0: !b:x]", "!c:x"]);
        let RailEntry::Folder(f) = &model.layout().expect("a layout").rail[1] else {
            panic!("a folder")
        };
        assert_eq!(f.name, Some(label("Work")));
        assert!(!f.open, "the folder shows its preview");
        assert_eq!(saves(&store), 1);
    }

    #[test]
    fn folder_ids_are_the_lowest_free_one() {
        let all = known(&["!a:x", "!b:x", "!c:x", "!d:x", "!e:x"]);
        let (mut model, store) = rail_model(vec![
            folder(0, &["!a:x", "!b:x"]),
            folder(2, &["!c:x", "!d:x"]),
            sp("!e:x"),
        ]);
        let op = Op::MoveToFolder {
            id: rid("!e:x"),
            folder: FolderTarget::New(None),
        };
        assert!(model.apply_known(op, &all, &store).is_ok());
        assert_eq!(
            rail_of(&model),
            ["[0: !a:x !b:x]", "[2: !c:x !d:x]", "[1: !e:x]"]
        );
    }

    #[test]
    fn moving_a_space_into_an_existing_folder_appends_it() {
        let all = known(&["!a:x", "!b:x", "!c:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), folder(1, &["!b:x", "!c:x"])]);
        let into = |id: &str| Op::MoveToFolder {
            id: rid(id),
            folder: FolderTarget::Existing(1),
        };
        assert!(model.apply_known(into("!a:x"), &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["[1: !b:x !c:x !a:x]"]);
        assert_eq!(saves(&store), 1);
        // Already there: nothing to do, nothing saved.
        assert!(model.apply_known(into("!a:x"), &all, &store).is_ok());
        assert_eq!(saves(&store), 1);
        // A folder that does not exist is refused.
        let bad = Op::MoveToFolder {
            id: rid("!a:x"),
            folder: FolderTarget::Existing(9),
        };
        assert_eq!(
            model.apply_known(bad, &all, &store).err(),
            Some(Refused::Unknown)
        );
        assert_eq!(saves(&store), 1);
    }

    #[test]
    fn moving_the_second_to_last_space_out_of_a_folder_dissolves_it() {
        let all = known(&["!a:x", "!b:x", "!c:x"]);
        // Out to the top level.
        let (mut model, store) = rail_model(vec![folder(1, &["!a:x", "!b:x"]), sp("!c:x")]);
        let out = Op::RemoveFromFolder(rid("!b:x"));
        assert!(model.apply_known(out, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["!a:x", "!b:x", "!c:x"]);
        assert_eq!(saves(&store), 1);
        // Into another folder.
        let (mut model, store) = rail_model(vec![
            folder(1, &["!a:x", "!b:x"]),
            folder(2, &["!c:x", "!d:x"]),
        ]);
        let all = known(&["!a:x", "!b:x", "!c:x", "!d:x"]);
        let across = Op::MoveToFolder {
            id: rid("!b:x"),
            folder: FolderTarget::Existing(2),
        };
        assert!(model.apply_known(across, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["!a:x", "[2: !c:x !d:x !b:x]"]);
        // A folder with three keeps its shape and loses one.
        let (mut model, store) = rail_model(vec![folder(1, &["!a:x", "!b:x", "!c:x"])]);
        let out = Op::RemoveFromFolder(rid("!b:x"));
        assert!(model.apply_known(out, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["[1: !a:x !c:x]", "!b:x"]);
        // A space that is not in a folder is refused.
        let top = Op::RemoveFromFolder(rid("!b:x"));
        assert_eq!(
            model.apply_known(top, &all, &store).err(),
            Some(Refused::Unknown)
        );
    }

    #[test]
    fn moving_a_space_from_a_folder_into_a_new_folder_dissolves_the_old_one() {
        let all = known(&["!a:x", "!b:x"]);
        let (mut model, store) = rail_model(vec![folder(1, &["!a:x", "!b:x"])]);
        let op = Op::MoveToFolder {
            id: rid("!b:x"),
            folder: FolderTarget::New(None),
        };
        assert!(model.apply_known(op, &all, &store).is_ok());
        assert_eq!(rail_of(&model), ["!a:x", "[0: !b:x]"]);
    }

    #[test]
    fn ungroup_puts_the_spaces_back_in_their_order() {
        let all = known(&["!a:x", "!b:x", "!c:x", "!d:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x"), folder(1, &["!c:x", "!b:x", "!d:x"])]);
        assert!(
            model
                .apply_known(Op::UngroupFolder(1), &all, &store)
                .is_ok()
        );
        assert_eq!(rail_of(&model), ["!a:x", "!c:x", "!b:x", "!d:x"]);
        assert_eq!(saves(&store), 1);
        assert_eq!(
            model.apply_known(Op::UngroupFolder(1), &all, &store).err(),
            Some(Refused::Unknown)
        );
    }

    #[test]
    fn renaming_a_folder_sets_or_clears_its_name_and_saves_only_a_change() {
        let all = known(&["!a:x", "!b:x"]);
        let (mut model, store) = rail_model(vec![folder(1, &["!a:x", "!b:x"])]);
        let rename = |name| Op::RenameFolder { id: 1, name };
        let name_of = |m: &SidebarModel| match &m.layout().expect("a layout").rail[0] {
            RailEntry::Folder(f) => f.name,
            RailEntry::Space(_) => None,
        };
        assert!(model.apply_known(rename(None), &all, &store).is_ok());
        assert_eq!(saves(&store), 0, "no name to no name");
        assert!(
            model
                .apply_known(rename(Some(label("Work"))), &all, &store)
                .is_ok()
        );
        assert_eq!(name_of(&model), Some(label("Work")));
        assert_eq!(saves(&store), 1);
        assert!(
            model
                .apply_known(rename(Some(label("Work"))), &all, &store)
                .is_ok()
        );
        assert_eq!(saves(&store), 1);
        assert!(model.apply_known(rename(None), &all, &store).is_ok());
        assert_eq!(name_of(&model), None);
        assert_eq!(saves(&store), 2);
        let ghost = Op::RenameFolder { id: 9, name: None };
        assert_eq!(
            model.apply_known(ghost, &all, &store).err(),
            Some(Refused::Unknown)
        );
    }

    #[test]
    fn a_move_of_a_space_the_layout_does_not_have_is_refused() {
        let all = known(&["!a:x"]);
        let (mut model, store) = rail_model(vec![sp("!a:x")]);
        assert_eq!(
            model
                .apply_known(move_space("!nope:x", Dir::Up), &all, &store)
                .err(),
            Some(Refused::Unknown)
        );
        let down = Op::MoveFolder {
            id: 4,
            dir: Dir::Down,
        };
        assert_eq!(
            model.apply_known(down, &all, &store).err(),
            Some(Refused::Unknown)
        );
        assert_eq!(saves(&store), 0);
    }

    #[test]
    fn an_edit_that_would_pass_max_folders_is_refused_as_full_and_saves_nothing() {
        use twicecat_core::api::sidebar::limits::MAX_FOLDERS;
        let ids: Vec<String> = (0..=MAX_FOLDERS).map(|n| format!("!s{n}:x")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let all = known(&refs);
        // MAX_FOLDERS folders of one space each, and one space left over.
        let mut rail: Vec<RailEntry> = refs[..MAX_FOLDERS]
            .iter()
            .enumerate()
            .map(|(n, id)| folder(u16::try_from(n).expect("small"), &[id]))
            .collect();
        rail.push(sp(refs[MAX_FOLDERS]));
        let (mut model, store) = rail_model(rail.clone());
        assert!(model.layout().expect("a layout").fits());
        let op = Op::MoveToFolder {
            id: rid(refs[MAX_FOLDERS]),
            folder: FolderTarget::New(None),
        };
        assert_eq!(
            model.apply_known(op, &all, &store).err(),
            Some(Refused::Full)
        );
        assert_eq!(model.state(), SidebarState::Full);
        assert_eq!(model.layout().expect("a layout").rail, rail, "unchanged");
        assert_eq!(saves(&store), 0);
        assert_eq!(Refused::Full.message(), "The sidebar layout is full");
    }

    #[test]
    fn in_the_newer_state_every_rail_operation_is_refused() {
        let all = known(&["!a:x", "!b:x"]);
        let ops = [
            move_space("!a:x", Dir::Down),
            Op::MoveToFolder {
                id: rid("!a:x"),
                folder: FolderTarget::New(None),
            },
            Op::RemoveFromFolder(rid("!a:x")),
            Op::RenameFolder { id: 1, name: None },
            Op::UngroupFolder(1),
            Op::MoveFolder {
                id: 1,
                dir: Dir::Up,
            },
        ];
        for load in [SidebarLoad::Newer, SidebarLoad::OtherAccount] {
            let store = MemorySidebar::default();
            let a = user("@a:x");
            let mut model = SidebarModel::default();
            let _ = model.load_for(a.clone(), &store);
            model.loaded(&a, load);
            for op in &ops {
                assert_eq!(
                    model.apply_known(op.clone(), &all, &store).err(),
                    Some(Refused::Protected),
                    "{op:?}"
                );
            }
            assert_eq!(saves(&store), 0);
        }
    }

    #[test]
    fn debug_of_an_op_shows_no_room_id_and_no_folder_name() {
        let ops = [
            Op::ToggleFolder(7),
            Op::ToggleSection {
                space: rid("!secretroom:x"),
                key: SectionKey::SubSpace(rid("!secretroom:x")),
            },
            Op::MoveSpace {
                id: rid("!secretroom:x"),
                dir: Dir::Up,
            },
            Op::MoveToFolder {
                id: rid("!secretroom:x"),
                folder: FolderTarget::New(Some(label("Payroll"))),
            },
            Op::MoveToFolder {
                id: rid("!secretroom:x"),
                folder: FolderTarget::Existing(2),
            },
            Op::RemoveFromFolder(rid("!secretroom:x")),
            Op::RenameFolder {
                id: 3,
                name: Some(label("Payroll")),
            },
            Op::UngroupFolder(3),
            Op::MoveFolder {
                id: 3,
                dir: Dir::Down,
            },
        ];
        for op in ops {
            let text = format!("{op:?}");
            for secret in ["!secretroom", "@a:x", "Payroll"] {
                assert!(!text.contains(secret), "{secret} is in {text}");
            }
        }
    }

    // ---- sections: Starred and the user's own (Task sbc.9) ----

    mod section_ops {
        use twicecat_core::api::sidebar::limits::{MAX_CUSTOM_SECTIONS, MAX_PLACED_ROOMS};
        use twicecat_core::api::{CustomSection, SpaceLayout};

        use super::*;

        fn sp_id() -> RoomId {
            rid("!sp:x")
        }

        fn r(n: &str) -> RoomId {
            rid(&format!("!{n}:x"))
        }

        fn model_with(space: Option<SpaceLayout>) -> (SidebarModel, MemorySidebar) {
            let store = MemorySidebar::default();
            let mut layout = SidebarLayout::new(user("@a:x"));
            layout.spaces.extend(space);
            (loaded_model(&store, layout), store)
        }

        fn record(model: &SidebarModel) -> SpaceLayout {
            model
                .layout()
                .expect("a layout")
                .spaces
                .iter()
                .find(|s| s.space == sp_id())
                .cloned()
                .expect("a record for the space")
        }

        fn custom(id: u16, name: &str, rooms: &[&str]) -> CustomSection {
            CustomSection {
                id,
                name: label(name),
                rooms: rooms.iter().map(|n| r(n)).collect(),
            }
        }

        fn names(section: &CustomSection) -> Vec<String> {
            section
                .rooms
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect()
        }

        fn star(room: &str) -> Op {
            Op::Star {
                space: sp_id(),
                room: r(room),
            }
        }

        fn unstar(room: &str) -> Op {
            Op::Unstar {
                space: sp_id(),
                room: r(room),
            }
        }

        fn new_section(name: &str, room: Option<&str>) -> Op {
            Op::NewSection {
                space: sp_id(),
                name: label(name),
                room: room.map(r),
            }
        }

        fn to_section(room: &str, section: u16) -> Op {
            Op::MoveRoomToSection {
                space: sp_id(),
                room: r(room),
                section,
            }
        }

        #[test]
        fn star_puts_the_room_in_starred_and_unstar_takes_it_out() {
            let (mut model, store) = model_with(None);
            assert!(model.apply(star("a"), &store).is_ok());
            assert!(model.apply(star("b"), &store).is_ok());
            assert_eq!(record(&model).starred, [r("a"), r("b")]);
            assert_eq!(saves(&store), 2);
            // Starring twice changes nothing and saves nothing.
            assert!(model.apply(star("a"), &store).is_ok());
            assert_eq!(saves(&store), 2);
            assert!(model.apply(unstar("a"), &store).is_ok());
            assert_eq!(record(&model).starred, [r("b")]);
            assert_eq!(saves(&store), 3);
            assert!(model.apply(unstar("a"), &store).is_ok());
            assert_eq!(saves(&store), 3, "a room that is not starred: no save");
        }

        #[test]
        fn a_starred_room_leaves_its_custom_section_so_it_has_one_place() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.custom = vec![custom(1, "Work", &["a", "b"])];
            let (mut model, store) = model_with(Some(rec));
            assert!(model.apply(star("a"), &store).is_ok());
            let rec = record(&model);
            assert_eq!(rec.starred, [r("a")]);
            assert_eq!(names(&rec.custom[0]), ["!b:x"]);
            // Unstar returns it to its default section: it is in no list.
            assert!(model.apply(unstar("a"), &store).is_ok());
            let rec = record(&model);
            assert!(rec.starred.is_empty());
            assert_eq!(names(&rec.custom[0]), ["!b:x"]);
        }

        #[test]
        fn a_new_section_is_saved_with_the_lowest_free_id_and_changes_no_order() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.custom = vec![custom(0, "Zero", &[]), custom(2, "Two", &[])];
            rec.order = vec![SectionKey::Direct, SectionKey::Channels];
            let (mut model, store) = model_with(Some(rec));
            assert!(model.apply(new_section("Ops", None), &store).is_ok());
            let rec = record(&model);
            assert_eq!(rec.custom.len(), 3);
            assert_eq!(rec.custom[2].id, 1, "the lowest free ID");
            assert_eq!(rec.custom[2].name.as_str(), "Ops");
            assert!(rec.custom[2].rooms.is_empty());
            assert_eq!(rec.order, [SectionKey::Direct, SectionKey::Channels]);
            assert_eq!(saves(&store), 1);
        }

        #[test]
        fn a_new_section_made_from_a_room_holds_that_room_and_only_there() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.starred = vec![r("a")];
            rec.custom = vec![custom(1, "Old", &["a2", "b"])];
            let (mut model, store) = model_with(Some(rec));
            assert!(model.apply(new_section("Ops", Some("b")), &store).is_ok());
            let rec = record(&model);
            assert_eq!(names(&rec.custom[0]), ["!a2:x"]);
            assert_eq!(rec.custom[1].id, 0);
            assert_eq!(names(&rec.custom[1]), ["!b:x"]);
            // And from a starred room: it leaves Starred.
            assert!(model.apply(new_section("Two", Some("a")), &store).is_ok());
            assert!(record(&model).starred.is_empty());
            assert_eq!(saves(&store), 2);
        }

        #[test]
        fn rename_keeps_the_rooms_and_an_unchanged_name_saves_nothing() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.custom = vec![custom(3, "Old", &["a", "b"])];
            let (mut model, store) = model_with(Some(rec));
            let rename = |name: &str, id: u16| Op::RenameSection {
                space: sp_id(),
                id,
                name: label(name),
            };
            assert!(model.apply(rename("New", 3), &store).is_ok());
            let rec = record(&model);
            assert_eq!(rec.custom[0].name.as_str(), "New");
            assert_eq!(names(&rec.custom[0]), ["!a:x", "!b:x"]);
            assert_eq!(saves(&store), 1);
            assert!(model.apply(rename("New", 3), &store).is_ok());
            assert_eq!(saves(&store), 1);
            assert_eq!(
                model.apply(rename("X", 9), &store).err(),
                Some(Refused::Unknown)
            );
            assert_eq!(saves(&store), 1);
        }

        #[test]
        fn delete_returns_its_rooms_to_their_default_sections_and_drops_its_keys() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.custom = vec![custom(1, "Work", &["a", "b"]), custom(2, "Keep", &["c"])];
            rec.order = vec![
                SectionKey::Channels,
                SectionKey::Custom(1),
                SectionKey::Custom(2),
            ];
            rec.collapsed = vec![SectionKey::Custom(1), SectionKey::Direct];
            let (mut model, store) = model_with(Some(rec));
            let delete = |id: u16| Op::DeleteSection { space: sp_id(), id };
            assert!(model.apply(delete(1), &store).is_ok());
            let rec = record(&model);
            assert_eq!(rec.custom.len(), 1);
            assert_eq!(rec.custom[0].id, 2);
            // Its rooms are in no list any more, so each is placed by default.
            let placed: Vec<RoomId> = rec
                .custom
                .iter()
                .flat_map(|c| c.rooms.clone())
                .chain(rec.starred.clone())
                .collect();
            assert!(!placed.contains(&r("a")) && !placed.contains(&r("b")));
            assert_eq!(rec.order, [SectionKey::Channels, SectionKey::Custom(2)]);
            assert_eq!(rec.collapsed, [SectionKey::Direct]);
            assert_eq!(saves(&store), 1);
            assert_eq!(
                model.apply(delete(1), &store).err(),
                Some(Refused::Unknown),
                "deleted twice"
            );
        }

        #[test]
        fn move_room_to_section_takes_it_out_of_every_other_place() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.starred = vec![r("s")];
            rec.custom = vec![custom(1, "A", &["x", "y"]), custom(2, "B", &["z"])];
            let (mut model, store) = model_with(Some(rec));
            assert!(model.apply(to_section("x", 2), &store).is_ok());
            let rec = record(&model);
            assert_eq!(names(&rec.custom[0]), ["!y:x"]);
            assert_eq!(names(&rec.custom[1]), ["!z:x", "!x:x"]);
            // From Starred too.
            assert!(model.apply(to_section("s", 1), &store).is_ok());
            let rec = record(&model);
            assert!(rec.starred.is_empty());
            assert_eq!(names(&rec.custom[0]), ["!y:x", "!s:x"]);
            // Into the section it is already in: nothing.
            assert!(model.apply(to_section("s", 1), &store).is_ok());
            assert_eq!(saves(&store), 2);
            assert_eq!(
                model.apply(to_section("s", 9), &store).err(),
                Some(Refused::Unknown)
            );
        }

        #[test]
        fn moving_inside_a_custom_section_changes_only_that_section() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.starred = vec![r("s1"), r("s2")];
            rec.order = vec![SectionKey::Custom(2), SectionKey::Custom(1)];
            rec.custom = vec![
                custom(1, "A", &["a", "gone", "b", "c"]),
                custom(2, "B", &["x", "y"]),
            ];
            let (mut model, store) = model_with(Some(rec.clone()));
            let step = |room: &str, dir: Dir, drawn: &[&str]| Op::MoveRoomInSection {
                space: sp_id(),
                room: r(room),
                dir,
                drawn: drawn.iter().map(|n| r(n)).collect(),
            };
            // "gone" is not drawn: b swaps with the drawn neighbour a, and the
            // room that is not drawn keeps its place after a.
            assert!(
                model
                    .apply(step("b", Dir::Up, &["a", "b", "c"]), &store)
                    .is_ok()
            );
            let now = record(&model);
            assert_eq!(names(&now.custom[0]), ["!b:x", "!a:x", "!gone:x", "!c:x"]);
            assert_eq!(names(&now.custom[1]), ["!x:x", "!y:x"], "the other section");
            assert_eq!(now.starred, rec.starred);
            assert_eq!(now.order, rec.order);
            assert_eq!(now.collapsed, rec.collapsed);
            assert_eq!(saves(&store), 1);
            // The first drawn room cannot go up, the last cannot go down.
            assert!(
                model
                    .apply(step("b", Dir::Up, &["b", "a", "c"]), &store)
                    .is_ok()
            );
            assert!(
                model
                    .apply(step("c", Dir::Down, &["b", "a", "c"]), &store)
                    .is_ok()
            );
            assert_eq!(saves(&store), 1);
            // A room in no custom section.
            assert_eq!(
                model
                    .apply(step("s1", Dir::Down, &["s1", "s2"]), &store)
                    .err(),
                Some(Refused::Unknown)
            );
        }

        fn key_text(keys: &[SectionKey]) -> Vec<String> {
            keys.iter().map(|k| format!("{k:?}")).collect()
        }

        fn move_section(key: SectionKey, dir: Dir, drawn: &[SectionKey]) -> Op {
            Op::MoveSection {
                space: sp_id(),
                key,
                dir,
                drawn: drawn.to_vec(),
            }
        }

        #[test]
        fn moving_a_section_swaps_it_with_its_drawn_neighbour_and_skips_hidden_ones() {
            use SectionKey::{Channels, Custom, Direct, Starred};
            let mut rec = SpaceLayout::new(sp_id());
            rec.custom = vec![custom(1, "A", &["a"])];
            let (mut model, store) = model_with(Some(rec));
            // Starred is hidden (empty): the drawn sections are these three.
            let drawn = [Channels, Custom(1), Direct];
            assert!(
                model
                    .apply(move_section(Custom(1), Dir::Down, &drawn), &store)
                    .is_ok()
            );
            assert_eq!(
                key_text(&record(&model).order),
                key_text(&[Channels, Direct, Custom(1)])
            );
            assert_eq!(saves(&store), 1);
            // Up from the first drawn section does nothing.
            let drawn = [Channels, Direct, Custom(1)];
            assert!(
                model
                    .apply(move_section(Channels, Dir::Up, &drawn), &store)
                    .is_ok()
            );
            assert!(
                model
                    .apply(move_section(Custom(1), Dir::Down, &drawn), &store)
                    .is_ok()
            );
            assert_eq!(saves(&store), 1);
            // A hidden section that the stored order names keeps its place
            // beside the one it followed.
            let mut rec = record(&model);
            rec.order = vec![Starred, Channels, Direct, Custom(1)];
            let (mut model, store) = model_with(Some(rec));
            let drawn = [Channels, Direct, Custom(1)];
            assert!(
                model
                    .apply(move_section(Direct, Dir::Up, &drawn), &store)
                    .is_ok()
            );
            assert_eq!(
                key_text(&record(&model).order),
                key_text(&[Starred, Direct, Channels, Custom(1)]),
                "Starred (hidden) stays first; Direct and Channels swap"
            );
            // A section that is not drawn cannot be moved.
            assert_eq!(
                model
                    .apply(move_section(Starred, Dir::Down, &drawn), &store)
                    .err(),
                Some(Refused::Unknown)
            );
        }

        #[test]
        fn moving_a_section_among_more_than_256_drawn_ones_is_refused_without_full() {
            use twicecat_core::api::sidebar::limits::MAX_SECTION_KEYS;
            // A huge space gives many sub-space sections. The order cannot
            // hold them all, which is a key-count limit and not a placement
            // limit: the move is refused and the state stays as it was.
            let drawn: Vec<SectionKey> = (0..MAX_SECTION_KEYS + 44)
                .map(|n| SectionKey::SubSpace(r(&format!("sub{n}"))))
                .collect();
            let (mut model, store) = model_with(None);
            assert_eq!(
                model
                    .apply(move_section(drawn[5].clone(), Dir::Down, &drawn), &store)
                    .err(),
                Some(Refused::Full)
            );
            assert_eq!(model.state(), SidebarState::Loaded, "not Full");
            assert_eq!(saves(&store), 0);
            // The model still takes other changes.
            assert!(model.apply(star("a"), &store).is_ok());
            assert_eq!(saves(&store), 1);
        }

        #[test]
        fn room_to_default_clears_starred_and_custom() {
            let mut rec = SpaceLayout::new(sp_id());
            rec.starred = vec![r("s")];
            rec.custom = vec![custom(1, "A", &["x"])];
            let (mut model, store) = model_with(Some(rec));
            let back = |room: &str| Op::RoomToDefault {
                space: sp_id(),
                room: r(room),
            };
            assert!(model.apply(back("x"), &store).is_ok());
            assert!(model.apply(back("s"), &store).is_ok());
            let rec = record(&model);
            assert!(rec.starred.is_empty() && rec.custom[0].rooms.is_empty());
            assert_eq!(saves(&store), 2);
            assert!(model.apply(back("nobody"), &store).is_ok());
            assert_eq!(saves(&store), 2);
        }

        #[test]
        fn passing_a_limit_is_refused_and_saves_nothing() {
            // Twenty sections: a 21st is refused.
            let mut rec = SpaceLayout::new(sp_id());
            rec.custom = (0..MAX_CUSTOM_SECTIONS as u16)
                .map(|n| custom(n, "S", &[]))
                .collect();
            let (mut model, store) = model_with(Some(rec));
            assert_eq!(
                model.apply(new_section("One more", None), &store).err(),
                Some(Refused::Full)
            );
            assert_eq!(model.state(), SidebarState::Full);
            assert_eq!(record(&model).custom.len(), MAX_CUSTOM_SECTIONS);
            assert_eq!(saves(&store), 0);

            // The room budget of one space: Starred and the sections together.
            let mut rec = SpaceLayout::new(sp_id());
            rec.starred = (0..MAX_PLACED_ROOMS - 1)
                .map(|n| r(&format!("s{n}")))
                .collect();
            rec.custom = vec![custom(1, "A", &[])];
            let (mut model, store) = model_with(Some(rec));
            assert!(model.apply(star("last"), &store).is_ok(), "the 500th");
            assert_eq!(saves(&store), 1);
            assert_eq!(
                model.apply(to_section("one-too-many", 1), &store).err(),
                Some(Refused::Full)
            );
            assert_eq!(
                model.apply(star("one-too-many"), &store).err(),
                Some(Refused::Full)
            );
            assert_eq!(
                model
                    .apply(new_section("B", Some("one-too-many")), &store)
                    .err(),
                Some(Refused::Full)
            );
            assert_eq!(saves(&store), 1);
            assert_eq!(record(&model).starred.len(), MAX_PLACED_ROOMS);
        }

        #[test]
        fn the_new_operations_are_refused_while_customizing_is_off() {
            let store = MemorySidebar::default();
            let mut model = SidebarModel::default();
            assert_eq!(
                model.apply(star("a"), &store).err(),
                Some(Refused::NotLoaded)
            );
            let mut model = SidebarModel::default();
            let a = user("@a:x");
            let _ = model.load_for(a.clone(), &store);
            model.loaded(&a, SidebarLoad::Newer);
            assert_eq!(
                model.apply(new_section("X", None), &store).err(),
                Some(Refused::Protected)
            );
            assert_eq!(saves(&store), 0);
        }

        #[test]
        fn debug_of_the_new_operations_hides_ids_and_names() {
            let ops = [
                star("secretroom"),
                unstar("secretroom"),
                new_section("Payroll", Some("secretroom")),
                Op::RenameSection {
                    space: r("secretroom"),
                    id: 1,
                    name: label("Payroll"),
                },
                Op::DeleteSection {
                    space: r("secretroom"),
                    id: 1,
                },
                move_section(
                    SectionKey::SubSpace(r("secretroom")),
                    Dir::Up,
                    &[SectionKey::SubSpace(r("secretroom"))],
                ),
                to_section("secretroom", 1),
                Op::MoveRoomInSection {
                    space: r("secretroom"),
                    room: r("secretroom"),
                    dir: Dir::Up,
                    drawn: vec![r("secretroom")],
                },
                Op::RoomToDefault {
                    space: r("secretroom"),
                    room: r("secretroom"),
                },
            ];
            for op in ops {
                let text = format!("{op:?}");
                for secret in ["secretroom", "@a:x", "Payroll"] {
                    assert!(!text.contains(secret), "{secret} is in {text}");
                }
            }
        }
    }

    /// A store over the real files that does each job at once, so a test can
    /// follow a layout through the model and the disk.
    struct DiskNow(crate::sidebar_store::DiskSidebar);

    impl SidebarStore for DiskNow {
        fn load(&self, account: &UserId) -> Task<SidebarLoad> {
            Task::done(iced::futures::executor::block_on(self.0.load_job(account)))
        }

        fn save(&self, account: &UserId, layout: SidebarLayout) -> Task<bool> {
            Task::done(iced::futures::executor::block_on(
                self.0.save_job(account, layout),
            ))
        }

        fn reset(&self, account: &UserId) -> Task<crate::sidebar_store::ResetOutcome> {
            Task::done(iced::futures::executor::block_on(self.0.reset_job(account)))
        }
    }

    #[test]
    fn changes_survive_logout_and_never_touch_another_accounts_file() {
        use twicecat_core::api::SidebarRead;
        use twicecat_core::paths::DataPaths;

        let root =
            std::env::temp_dir().join(format!("twicecat-sidebar-model-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        let paths = DataPaths::at(&root);
        let store = DiskNow(crate::sidebar_store::DiskSidebar::new(paths.clone()));
        let (a, b) = (user("@a:x"), user("@b:x"));
        let space_op = |key| Op::ToggleSection {
            space: rid("!sp:x"),
            key,
        };

        // @a changes the layout and logs out.
        let mut model = SidebarModel::default();
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::Default);
        assert!(model.apply(space_op(SectionKey::Direct), &store).is_ok());
        let a_layout = model.layout().cloned().expect("a layout");
        let file_a = paths.sidebar_file(&a);
        let bytes_a = std::fs::read(&file_a).expect("a's file");
        model.unload();
        assert_eq!(std::fs::read(&file_a).expect("kept"), bytes_a);

        // @a logs back in: the same layout loads.
        let _ = model.load_for(a.clone(), &store);
        model.loaded(&a, SidebarLoad::from(paths.read_sidebar(&a)));
        assert_eq!(model.layout(), Some(&a_layout));
        model.unload();

        // @b gets the default, makes two changes; @a's file is untouched.
        let _ = model.load_for(b.clone(), &store);
        model.loaded(&b, SidebarLoad::from(paths.read_sidebar(&b)));
        assert_eq!(model.layout(), Some(&SidebarLayout::new(b.clone())));
        assert!(model.apply(space_op(SectionKey::Starred), &store).is_ok());
        assert!(model.apply(space_op(SectionKey::Direct), &store).is_ok());
        assert_eq!(std::fs::read(&file_a).expect("a's file"), bytes_a);
        assert_eq!(
            paths.read_sidebar(&b),
            SidebarRead::Layout(model.layout().cloned().expect("a layout"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A temp data root with two accounts' files on disk, and a model over
    /// the real store.
    fn two_account_root(name: &str) -> (std::path::PathBuf, twicecat_core::paths::DataPaths) {
        let root = std::env::temp_dir().join(format!(
            "twicecat-sidebar-reset-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("dir");
        (root.clone(), twicecat_core::paths::DataPaths::at(&root))
    }

    fn login_from_disk(
        model: &mut SidebarModel,
        account: &UserId,
        store: &DiskNow,
        paths: &twicecat_core::paths::DataPaths,
    ) {
        let _ = model.load_for(account.clone(), store);
        model.loaded(account, SidebarLoad::from(paths.read_sidebar(account)));
    }

    #[test]
    fn reset_as_b_removes_only_bs_file_and_leaves_as_byte_for_byte() {
        use twicecat_core::api::SidebarRead;

        let (root, paths) = two_account_root("two");
        let store = DiskNow(crate::sidebar_store::DiskSidebar::new(paths.clone()));
        let (a, b) = (user("@a:x"), user("@b:x"));
        let toggle = |key| Op::ToggleSection {
            space: rid("!sp:x"),
            key,
        };
        let mut model = SidebarModel::default();
        for account in [&a, &b] {
            login_from_disk(&mut model, account, &store, &paths);
            assert!(model.apply(toggle(SectionKey::Direct), &store).is_ok());
            model.unload();
        }
        let (file_a, file_b) = (paths.sidebar_file(&a), paths.sidebar_file(&b));
        let bytes_a = std::fs::read(&file_a).expect("a's file");
        let modified_a = std::fs::metadata(&file_a)
            .and_then(|m| m.modified())
            .expect("mtime");
        assert!(file_b.exists());

        login_from_disk(&mut model, &b, &store, &paths);
        assert_eq!(model.state(), SidebarState::Loaded);
        let task = model.reset(&store);
        assert!(task.is_ok());
        assert!(!file_b.exists(), "b's file is gone");
        assert_eq!(model.layout(), Some(&SidebarLayout::new(b.clone())));
        // a's file: same bytes, same modification time, still a's layout.
        assert_eq!(std::fs::read(&file_a).expect("a's file"), bytes_a);
        assert_eq!(
            std::fs::metadata(&file_a)
                .and_then(|m| m.modified())
                .expect("mtime"),
            modified_a
        );
        model.unload();
        // b logs in again: the default, not the old arrangement. a's own
        // layout still loads for a.
        login_from_disk(&mut model, &b, &store, &paths);
        assert_eq!(model.layout(), Some(&SidebarLayout::new(b)));
        model.unload();
        login_from_disk(&mut model, &a, &store, &paths);
        assert!(matches!(paths.read_sidebar(&a), SidebarRead::Layout(_)));
        assert_ne!(model.layout(), Some(&SidebarLayout::new(a)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reset_leaves_a_protected_file_byte_for_byte_the_same() {
        let planted = [
            (
                r#"{"version":1,"account":"@other:x","rail":[],"spaces":[]}"#,
                SidebarState::OtherAccount,
            ),
            (
                r#"{"version":2,"account":"@a:x","rail":[],"spaces":[]}"#,
                SidebarState::Newer,
            ),
        ];
        for (n, (text, state)) in planted.into_iter().enumerate() {
            let (root, paths) = two_account_root(&format!("protected{n}"));
            let store = DiskNow(crate::sidebar_store::DiskSidebar::new(paths.clone()));
            let a = user("@a:x");
            let file = paths.sidebar_file(&a);
            std::fs::write(&file, text).expect("plant");
            let modified = std::fs::metadata(&file)
                .and_then(|m| m.modified())
                .expect("mtime");
            let mut model = SidebarModel::default();
            login_from_disk(&mut model, &a, &store, &paths);
            assert_eq!(model.state(), state);
            assert_eq!(model.reset(&store).err(), Some(Refused::Protected));
            assert_eq!(std::fs::read(&file).expect("kept"), text.as_bytes());
            assert_eq!(
                std::fs::metadata(&file)
                    .and_then(|m| m.modified())
                    .expect("mtime"),
                modified
            );
            // Even the store itself, asked directly, leaves it alone.
            assert_ne!(
                iced::futures::executor::block_on(
                    crate::sidebar_store::DiskSidebar::new(paths.clone()).reset_job(&a)
                ),
                crate::sidebar_store::ResetOutcome::Removed
            );
            assert_eq!(std::fs::read(&file).expect("kept"), text.as_bytes());
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
