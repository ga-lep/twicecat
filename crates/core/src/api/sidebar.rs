//! The user's arrangement of the sidebar: the order of the spaces rail, its
//! folders, and the sections inside each space.
//!
//! It is kept on this device only, in a file of its own per account (see
//! `DataPaths::read_sidebar`). It holds room IDs, the account ID and the names
//! the user gave to folders and sections; never a room name. `Debug` shows
//! counts only, so none of it reaches a log.

use std::collections::HashSet;
use std::fmt;

use super::{RoomId, UserId};

/// Limits on a stored layout. A layout that stays inside them always fits in
/// [`MAX_SIDEBAR_BYTES`](limits::MAX_SIDEBAR_BYTES) once written.
pub mod limits {
    /// Entries in the rail: spaces and folders.
    pub const MAX_RAIL_ENTRIES: usize = 200;
    /// Folders in the rail.
    pub const MAX_FOLDERS: usize = 50;
    /// Spaces in all folders together.
    pub const MAX_FOLDER_SPACES: usize = 200;
    /// Per-space records.
    pub const MAX_SPACE_RECORDS: usize = 200;
    /// Custom sections in one space.
    pub const MAX_CUSTOM_SECTIONS: usize = 20;
    /// Rooms in Starred and the custom sections of one space together.
    pub const MAX_PLACED_ROOMS: usize = 500;
    /// Keys in one space's `order` list, and in its `collapsed` list.
    pub const MAX_SECTION_KEYS: usize = 256;
    /// Room IDs (and sub-space keys) in the whole file.
    pub const MAX_ROOM_REFS: usize = 4_000;
    /// Bytes of all the room IDs in the whole file.
    pub const MAX_ID_BYTES_TOTAL: usize = 192 * 1024;
    /// The largest sidebar file read (256 KiB); anything bigger reads as no
    /// layout.
    pub const MAX_SIDEBAR_BYTES: u64 = 256 * 1024;
}

/// The longest name, in characters.
const LABEL_MAX_CHARS: usize = 32;
/// The longest name, in bytes.
const LABEL_MAX_BYTES: usize = 96;

/// A folder or section name given by the user.
///
/// 1 to 32 characters (at most 96 bytes) once trimmed, with no control,
/// invisible, blank-filler or bidi characters. It is `Copy`. `Debug` shows
/// only the length.
///
/// ```
/// use twicecat_core::api::sidebar::SidebarLabel;
/// let label = SidebarLabel::try_from("  Work  ").unwrap();
/// assert_eq!(label.as_str(), "Work");
/// assert_eq!(format!("{label:?}"), "<redacted, 4 bytes>");
/// assert!(SidebarLabel::try_from("").is_err());
/// assert!(SidebarLabel::try_from("   ").is_err());
/// assert!(SidebarLabel::try_from("a".repeat(33).as_str()).is_err());
/// assert!(SidebarLabel::try_from("evil\u{202E}").is_err());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct SidebarLabel {
    bytes: [u8; LABEL_MAX_BYTES],
    len: u8,
}

/// A name that fails the rules of [`SidebarLabel`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("not a valid folder or section name")]
pub struct SidebarLabelError;

impl SidebarLabel {
    /// The longest name, in characters.
    pub const MAX_CHARS: usize = LABEL_MAX_CHARS;

    /// The name as text.
    pub fn as_str(&self) -> &str {
        // The bytes are a whole `&str` copied in `try_from`.
        std::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or_default()
    }
}

/// Characters a name may not hold: controls, invisible format characters,
/// blank fillers, bidi controls and line separators.
fn is_forbidden_in_label(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{2800}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFC}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E01EF}'
        )
}

impl TryFrom<&str> for SidebarLabel {
    type Error = SidebarLabelError;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        let name = name.trim();
        let chars = name.chars().count();
        if !(1..=LABEL_MAX_CHARS).contains(&chars)
            || name.len() > LABEL_MAX_BYTES
            || name.chars().any(is_forbidden_in_label)
        {
            return Err(SidebarLabelError);
        }
        let mut bytes = [0; LABEL_MAX_BYTES];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        let len = u8::try_from(name.len()).map_err(|_| SidebarLabelError)?;
        Ok(Self { bytes, len })
    }
}

impl fmt::Debug for SidebarLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted, {} bytes>", self.len)
    }
}

/// Whether `id` is a room ID the sidebar file accepts: starts with `!`, 2 to
/// 255 bytes, ASCII graphic only. There is no `:server` rule, because room
/// version 12 IDs have none.
pub(crate) fn is_valid_room_id(id: &str) -> bool {
    id.starts_with('!') && is_graphic_id(id)
}

/// Whether `id` is a user ID the sidebar file accepts: starts with `@`, 2 to
/// 255 bytes, ASCII graphic only.
pub(crate) fn is_valid_user_id(id: &str) -> bool {
    id.starts_with('@') && is_graphic_id(id)
}

fn is_graphic_id(id: &str) -> bool {
    (2..=255).contains(&id.len()) && id.bytes().all(|b| (0x21..=0x7E).contains(&b))
}

/// A section of a space's room list.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum SectionKey {
    /// The starred rooms.
    Starred,
    /// The space's own rooms.
    Channels,
    /// Direct messages.
    Direct,
    /// The section of one sub-space.
    SubSpace(RoomId),
    /// A section the user made.
    Custom(u16),
}

impl SectionKey {
    /// The key as written in the file: `starred`, `channels`, `dms`,
    /// `sub:<room id>` or `custom:<id>`.
    pub(crate) fn to_key(&self) -> String {
        match self {
            Self::Starred => "starred".to_owned(),
            Self::Channels => "channels".to_owned(),
            Self::Direct => "dms".to_owned(),
            Self::SubSpace(id) => format!("sub:{}", id.as_str()),
            Self::Custom(n) => format!("custom:{n}"),
        }
    }

    /// Reads a key written by [`Self::to_key`]; `None` for anything else.
    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "starred" => Some(Self::Starred),
            "channels" => Some(Self::Channels),
            "dms" => Some(Self::Direct),
            _ => {
                if let Some(id) = key.strip_prefix("sub:") {
                    is_valid_room_id(id).then(|| Self::SubSpace(RoomId::from(id)))
                } else {
                    let n = key.strip_prefix("custom:")?;
                    let canonical = !n.is_empty()
                        && n.bytes().all(|b| b.is_ascii_digit())
                        && (n == "0" || !n.starts_with('0'));
                    if canonical {
                        n.parse().ok().map(Self::Custom)
                    } else {
                        None
                    }
                }
            }
        }
    }
}

impl fmt::Debug for SectionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Starred => f.write_str("Starred"),
            Self::Channels => f.write_str("Channels"),
            Self::Direct => f.write_str("Direct"),
            Self::SubSpace(_) => f.write_str("SubSpace(<redacted>)"),
            Self::Custom(n) => write!(f, "Custom({n})"),
        }
    }
}

/// A folder in the rail.
#[derive(Clone, PartialEq, Eq)]
pub struct FolderLayout {
    /// Unique among the folders.
    pub id: u16,
    /// Its name, if the user gave it one.
    pub name: Option<SidebarLabel>,
    /// Whether it is shown open.
    pub open: bool,
    /// The spaces in it, in order.
    pub spaces: Vec<RoomId>,
}

impl fmt::Debug for FolderLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FolderLayout")
            .field("id", &self.id)
            .field("named", &self.name.is_some())
            .field("open", &self.open)
            .field("spaces", &self.spaces.len())
            .finish()
    }
}

/// One entry of the rail.
#[derive(Clone, PartialEq, Eq)]
pub enum RailEntry {
    /// A space.
    Space(RoomId),
    /// A folder of spaces.
    Folder(FolderLayout),
}

impl fmt::Debug for RailEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Space(_) => f.write_str("Space(<redacted>)"),
            Self::Folder(folder) => f.debug_tuple("Folder").field(folder).finish(),
        }
    }
}

/// A section the user made in a space.
#[derive(Clone, PartialEq, Eq)]
pub struct CustomSection {
    /// Unique within its space.
    pub id: u16,
    /// Its name.
    pub name: SidebarLabel,
    /// The rooms put in it, in order.
    pub rooms: Vec<RoomId>,
}

impl fmt::Debug for CustomSection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CustomSection")
            .field("id", &self.id)
            .field("rooms", &self.rooms.len())
            .finish()
    }
}

/// The arrangement inside one space.
#[derive(Clone, PartialEq, Eq)]
pub struct SpaceLayout {
    /// The space.
    pub space: RoomId,
    /// The sections in the order the user chose; sections not listed take
    /// their default place.
    pub order: Vec<SectionKey>,
    /// The sections shown collapsed.
    pub collapsed: Vec<SectionKey>,
    /// The starred rooms, in order.
    pub starred: Vec<RoomId>,
    /// The sections the user made, in creation order.
    pub custom: Vec<CustomSection>,
}

impl SpaceLayout {
    /// An empty arrangement for `space`.
    pub fn new(space: RoomId) -> Self {
        Self {
            space,
            order: Vec::new(),
            collapsed: Vec::new(),
            starred: Vec::new(),
            custom: Vec::new(),
        }
    }
}

impl fmt::Debug for SpaceLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpaceLayout")
            .field("order", &self.order.len())
            .field("collapsed", &self.collapsed.len())
            .field("starred", &self.starred.len())
            .field("custom", &self.custom.len())
            .finish()
    }
}

/// The whole arrangement for one account.
///
/// ```
/// use twicecat_core::api::{RoomId, UserId};
/// use twicecat_core::api::sidebar::{RailEntry, SidebarLayout};
/// let mut layout = SidebarLayout::new(UserId::from("@me:x"));
/// layout.rail.push(RailEntry::Space(RoomId::from("!room:x")));
/// assert!(layout.fits());
/// let dbg = format!("{layout:?}");
/// assert!(!dbg.contains("room") && !dbg.contains("@me"));
/// ```
#[derive(Clone, PartialEq, Eq)]
pub struct SidebarLayout {
    /// The account this layout belongs to.
    pub account: UserId,
    /// The spaces rail, in order.
    pub rail: Vec<RailEntry>,
    /// The arrangement inside each space that has one.
    pub spaces: Vec<SpaceLayout>,
}

impl SidebarLayout {
    /// The default (empty) layout for `account`.
    pub fn new(account: UserId) -> Self {
        Self {
            account,
            rail: Vec::new(),
            spaces: Vec::new(),
        }
    }

    /// Whether the layout is valid and inside every limit, so that writing it
    /// changes nothing and the file fits in
    /// [`limits::MAX_SIDEBAR_BYTES`]. An edit for which this is false must be
    /// refused, not trimmed.
    pub fn fits(&self) -> bool {
        is_valid_user_id(self.account.as_str()) && self.clone().normalized() == *self
    }

    /// The layout with every invalid entry dropped, duplicates removed (the
    /// first one wins) and every limit applied, keeping the order of what
    /// stays.
    pub fn normalized(self) -> Self {
        let mut budget = Budget::new(&self.account);
        let rail = normalize_rail(self.rail, &mut budget);
        let mut seen = HashSet::new();
        let mut spaces = Vec::new();
        for record in self.spaces {
            if spaces.len() >= limits::MAX_SPACE_RECORDS {
                break;
            }
            if !is_valid_room_id(record.space.as_str()) || seen.contains(record.space.as_str()) {
                continue;
            }
            if !budget.take_fixed(SPACE_RECORD_COST) || !budget.take_id(&record.space) {
                break;
            }
            seen.insert(record.space.as_str().to_owned());
            spaces.push(normalize_space(record, &mut budget));
        }
        Self {
            account: self.account,
            rail,
            spaces,
        }
    }
}

/// Bytes a JSON string takes between its quotes: `"` and `\` are escaped.
pub(crate) fn json_len(text: &str) -> usize {
    text.len() + text.bytes().filter(|b| matches!(b, b'"' | b'\\')).count()
}

// Upper bounds of the JSON each kind of entry takes besides its strings, so
// that what the budget accepts always fits in `MAX_SIDEBAR_BYTES`.
const HEADER_COST: usize = 128;
const ID_COST: usize = 12;
const KEY_COST: usize = 20;
const RAIL_SPACE_COST: usize = 24;
const FOLDER_COST: usize = 96;
const SPACE_RECORD_COST: usize = 160;
const SECTION_COST: usize = 64;

/// What is left of the limits that count across the whole file.
struct Budget {
    refs: usize,
    id_bytes: usize,
    json_bytes: usize,
}

impl Budget {
    fn new(account: &UserId) -> Self {
        Self {
            refs: 0,
            id_bytes: 0,
            json_bytes: HEADER_COST + json_len(account.as_str()),
        }
    }

    /// Takes `bytes` of JSON that are not an ID; false (and nothing taken)
    /// if they do not fit.
    fn take_fixed(&mut self, bytes: usize) -> bool {
        let fits = self.json_bytes + bytes <= limits::MAX_SIDEBAR_BYTES as usize;
        if fits {
            self.json_bytes += bytes;
        }
        fits
    }

    /// Takes one reference to the room `id`, with `extra` bytes around it
    /// in the file; false (and nothing taken) if a limit would be passed.
    fn take_ref(&mut self, id: &RoomId, extra: usize) -> bool {
        let len = id.as_str().len();
        let json = json_len(id.as_str()) + extra;
        let fits = self.refs < limits::MAX_ROOM_REFS
            && self.id_bytes + len <= limits::MAX_ID_BYTES_TOTAL
            && self.json_bytes + json <= limits::MAX_SIDEBAR_BYTES as usize;
        if fits {
            self.refs += 1;
            self.id_bytes += len;
            self.json_bytes += json;
        }
        fits
    }

    fn take_id(&mut self, id: &RoomId) -> bool {
        self.take_ref(id, ID_COST)
    }
}

fn normalize_rail(rail: Vec<RailEntry>, budget: &mut Budget) -> Vec<RailEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut folder_ids: HashSet<u16> = HashSet::new();
    let mut folders = 0;
    let mut folder_spaces = 0;
    let mut out = Vec::new();
    for entry in rail {
        if out.len() >= limits::MAX_RAIL_ENTRIES {
            break;
        }
        match entry {
            RailEntry::Space(id) => {
                if is_valid_room_id(id.as_str())
                    && !seen.contains(id.as_str())
                    && budget.take_ref(&id, RAIL_SPACE_COST)
                {
                    seen.insert(id.as_str().to_owned());
                    out.push(RailEntry::Space(id));
                }
            }
            RailEntry::Folder(folder) => {
                if folders >= limits::MAX_FOLDERS || folder_ids.contains(&folder.id) {
                    continue;
                }
                let candidates = folder
                    .spaces
                    .iter()
                    .any(|s| is_valid_room_id(s.as_str()) && !seen.contains(s.as_str()));
                let name_cost = folder.name.map_or(0, |n| json_len(n.as_str()) + KEY_COST);
                if !candidates || !budget.take_fixed(FOLDER_COST + name_cost) {
                    continue;
                }
                let mut spaces = Vec::new();
                for id in folder.spaces {
                    if folder_spaces + spaces.len() >= limits::MAX_FOLDER_SPACES {
                        break;
                    }
                    if is_valid_room_id(id.as_str())
                        && !seen.contains(id.as_str())
                        && budget.take_id(&id)
                    {
                        seen.insert(id.as_str().to_owned());
                        spaces.push(id);
                    }
                }
                if spaces.is_empty() {
                    continue;
                }
                folders += 1;
                folder_spaces += spaces.len();
                folder_ids.insert(folder.id);
                out.push(RailEntry::Folder(FolderLayout { spaces, ..folder }));
            }
        }
    }
    out
}

/// Keeps the valid, new keys of `keys`, at most [`limits::MAX_SECTION_KEYS`].
fn normalize_keys(keys: Vec<SectionKey>, budget: &mut Budget) -> Vec<SectionKey> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for key in keys {
        if out.len() >= limits::MAX_SECTION_KEYS || seen.contains(&key) {
            continue;
        }
        let taken = match &key {
            SectionKey::SubSpace(id) => {
                is_valid_room_id(id.as_str()) && budget.take_ref(id, KEY_COST)
            }
            _ => budget.take_fixed(KEY_COST),
        };
        if taken {
            seen.insert(key.clone());
            out.push(key);
        }
    }
    out
}

fn normalize_space(record: SpaceLayout, budget: &mut Budget) -> SpaceLayout {
    let order = normalize_keys(record.order, budget);
    let collapsed = normalize_keys(record.collapsed, budget);
    let mut placed: HashSet<String> = HashSet::new();
    let mut place = |id: &RoomId, budget: &mut Budget, count: usize| {
        let ok = count < limits::MAX_PLACED_ROOMS
            && is_valid_room_id(id.as_str())
            && !placed.contains(id.as_str())
            && budget.take_id(id);
        if ok {
            placed.insert(id.as_str().to_owned());
        }
        ok
    };
    let mut starred = Vec::new();
    for id in record.starred {
        if place(&id, budget, starred.len()) {
            starred.push(id);
        }
    }
    let mut placed_count = starred.len();
    let mut section_ids = HashSet::new();
    let mut custom = Vec::new();
    for section in record.custom {
        if custom.len() >= limits::MAX_CUSTOM_SECTIONS || section_ids.contains(&section.id) {
            continue;
        }
        if !budget.take_fixed(SECTION_COST + json_len(section.name.as_str())) {
            break;
        }
        section_ids.insert(section.id);
        let mut rooms = Vec::new();
        for id in section.rooms {
            if place(&id, budget, placed_count) {
                placed_count += 1;
                rooms.push(id);
            }
        }
        custom.push(CustomSection { rooms, ..section });
    }
    SpaceLayout {
        space: record.space,
        order,
        collapsed,
        starred,
        custom,
    }
}

impl fmt::Debug for SidebarLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SidebarLayout")
            .field("rail", &self.rail.len())
            .field("spaces", &self.spaces.len())
            .finish()
    }
}

/// What reading an account's sidebar file found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarRead {
    /// The stored layout, validated.
    Layout(SidebarLayout),
    /// No file (or one that is too big, or not a regular file).
    Missing,
    /// Not a usable file: it was set aside as `.bad`.
    Corrupt,
    /// Saved by a newer Twicecat (the version is given). The file is left as
    /// it is and must not be overwritten.
    Newer(u64),
    /// The file names another account. It is left as it is and must not be
    /// overwritten.
    OtherAccount,
}

#[cfg(test)]
mod tests {
    use super::limits::*;
    use super::*;

    fn rid(n: usize) -> RoomId {
        RoomId::from(format!("!r{n}:x"))
    }

    fn label(text: &str) -> SidebarLabel {
        SidebarLabel::try_from(text).expect("a valid label")
    }

    fn layout() -> SidebarLayout {
        SidebarLayout::new(UserId::from("@me:x"))
    }

    #[test]
    fn labels_are_trimmed_and_checked() {
        assert_eq!(label("  Work  ").as_str(), "Work");
        assert_eq!(label(&"a".repeat(32)).as_str().len(), 32);
        // 32 characters of 3 bytes each is 96 bytes: the most allowed.
        assert_eq!(label(&"\u{4E2D}".repeat(32)).as_str().chars().count(), 32);
        for bad in [
            "",
            "   ",
            "\t",
            &"a".repeat(33),
            &"\u{4E2D}".repeat(33),
            "a\u{202E}b",
            "a\nb",
            "a\u{200B}b",
            "a\u{2028}b",
            "a\u{3164}b",
            "a\u{FEFF}",
            "a\u{0}",
        ] {
            assert!(SidebarLabel::try_from(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_label_debug_hides_the_text() {
        let dbg = format!("{:?}", label("Payroll"));
        assert!(!dbg.contains("Payroll"), "{dbg}");
        assert_eq!(dbg, "<redacted, 7 bytes>");
    }

    #[test]
    fn ids_follow_the_byte_rules() {
        for ok in [
            "!a",
            "!abc:example.org",
            "!v12roomidwithoutserver",
            &format!("!{}", "a".repeat(254)),
        ] {
            assert!(is_valid_room_id(ok), "{ok}");
        }
        for bad in [
            "",
            "!",
            "a:x",
            "#alias:x",
            "!a b",
            "!a\u{e9}",
            "!a\n",
            "!\u{7f}",
            &format!("!{}", "a".repeat(255)),
        ] {
            assert!(!is_valid_room_id(bad), "{bad:?}");
        }
        assert!(is_valid_user_id("@me:x"));
        assert!(!is_valid_user_id("!me:x"));
        assert!(!is_valid_user_id("@"));
        assert!(!is_valid_user_id("@m e:x"));
        assert!(is_valid_user_id(&format!("@{}", "a".repeat(254))));
        assert!(!is_valid_user_id(&format!("@{}", "a".repeat(255))));
    }

    #[test]
    fn section_keys_round_trip_through_their_text() {
        for key in [
            SectionKey::Starred,
            SectionKey::Channels,
            SectionKey::Direct,
            SectionKey::SubSpace(RoomId::from("!t:x")),
            SectionKey::Custom(0),
            SectionKey::Custom(65535),
        ] {
            assert_eq!(SectionKey::from_key(&key.to_key()), Some(key));
        }
        for bad in [
            "",
            "Starred",
            "sub:",
            "sub:a",
            "sub:!a b",
            "custom:",
            "custom:-1",
            "custom:65536",
            "custom:01",
            "custom:+1",
            "custom: 1",
            "dms ",
            "other",
        ] {
            assert_eq!(SectionKey::from_key(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_valid_layout_is_unchanged_by_normalizing() {
        let mut l = layout();
        l.rail.push(RailEntry::Space(rid(1)));
        l.rail.push(RailEntry::Folder(FolderLayout {
            id: 1,
            name: Some(label("Work")),
            open: true,
            spaces: vec![rid(2), rid(3)],
        }));
        let mut sp = SpaceLayout::new(rid(1));
        sp.order = vec![
            SectionKey::Starred,
            SectionKey::SubSpace(rid(9)),
            SectionKey::Custom(2),
        ];
        sp.collapsed = vec![SectionKey::Direct];
        sp.starred = vec![rid(10)];
        sp.custom = vec![CustomSection {
            id: 2,
            name: label("Ops"),
            rooms: vec![rid(11), rid(12)],
        }];
        l.spaces.push(sp);
        assert!(l.fits());
        assert_eq!(l.clone().normalized(), l);
    }

    #[test]
    fn invalid_and_duplicate_entries_are_dropped_and_the_rest_kept() {
        let mut l = layout();
        l.rail = vec![
            RailEntry::Space(RoomId::from("nobang")),
            RailEntry::Space(rid(1)),
            RailEntry::Space(rid(1)),
            RailEntry::Folder(FolderLayout {
                id: 5,
                name: None,
                open: false,
                spaces: vec![rid(1), rid(2), RoomId::from("!bad id")],
            }),
            RailEntry::Folder(FolderLayout {
                id: 5,
                name: None,
                open: false,
                spaces: vec![rid(3)],
            }),
            RailEntry::Folder(FolderLayout {
                id: 6,
                name: None,
                open: false,
                spaces: vec![],
            }),
            RailEntry::Space(rid(2)),
        ];
        let n = l.normalized();
        assert_eq!(
            n.rail,
            vec![
                RailEntry::Space(rid(1)),
                RailEntry::Folder(FolderLayout {
                    id: 5,
                    name: None,
                    open: false,
                    spaces: vec![rid(2)]
                }),
            ]
        );
    }

    #[test]
    fn a_room_is_placed_once_per_space() {
        let mut l = layout();
        let mut sp = SpaceLayout::new(rid(1));
        sp.starred = vec![rid(10), rid(10), rid(11)];
        sp.custom = vec![
            CustomSection {
                id: 1,
                name: label("A"),
                rooms: vec![rid(11), rid(12)],
            },
            CustomSection {
                id: 1,
                name: label("Dup id"),
                rooms: vec![rid(13)],
            },
            CustomSection {
                id: 2,
                name: label("B"),
                rooms: vec![rid(12), rid(14)],
            },
        ];
        sp.order = vec![
            SectionKey::Starred,
            SectionKey::Starred,
            SectionKey::Custom(1),
        ];
        l.spaces = vec![sp, SpaceLayout::new(rid(1))];
        let n = l.normalized();
        assert_eq!(n.spaces.len(), 1);
        let sp = &n.spaces[0];
        assert_eq!(sp.starred, vec![rid(10), rid(11)]);
        assert_eq!(sp.custom.len(), 2);
        assert_eq!(sp.custom[0].rooms, vec![rid(12)]);
        assert_eq!(sp.custom[1].rooms, vec![rid(14)]);
        assert_eq!(sp.order, vec![SectionKey::Starred, SectionKey::Custom(1)]);
    }

    #[test]
    fn no_limit_is_ever_exceeded() {
        let mut l = layout();
        l.rail = (0..300).map(|n| RailEntry::Space(rid(n))).collect();
        assert_eq!(l.clone().normalized().rail.len(), MAX_RAIL_ENTRIES);
        assert!(!l.fits());

        let mut l = layout();
        l.rail = (0..60)
            .map(|n| {
                RailEntry::Folder(FolderLayout {
                    id: u16::try_from(n).unwrap(),
                    name: None,
                    open: false,
                    spaces: vec![rid(n * 2), rid(n * 2 + 1)],
                })
            })
            .collect();
        assert_eq!(l.normalized().rail.len(), MAX_FOLDERS);

        let mut l = layout();
        l.rail = (0..10)
            .map(|n| {
                RailEntry::Folder(FolderLayout {
                    id: u16::try_from(n).unwrap(),
                    name: None,
                    open: false,
                    spaces: (0..30).map(|m| rid(n * 100 + m)).collect(),
                })
            })
            .collect();
        let kept: usize = l
            .normalized()
            .rail
            .iter()
            .map(|e| match e {
                RailEntry::Folder(f) => f.spaces.len(),
                RailEntry::Space(_) => 0,
            })
            .sum();
        assert_eq!(kept, MAX_FOLDER_SPACES);

        let mut l = layout();
        l.spaces = (0..250).map(|n| SpaceLayout::new(rid(n))).collect();
        assert_eq!(l.normalized().spaces.len(), MAX_SPACE_RECORDS);

        let mut l = layout();
        let mut sp = SpaceLayout::new(rid(1));
        sp.custom = (0..30)
            .map(|n| CustomSection {
                id: n,
                name: label("S"),
                rooms: vec![],
            })
            .collect();
        sp.starred = (0..700_usize).map(|n| rid(1000 + n)).collect();
        l.spaces = vec![sp];
        let n = l.normalized();
        assert_eq!(n.spaces[0].custom.len(), MAX_CUSTOM_SECTIONS);
        assert_eq!(n.spaces[0].starred.len(), MAX_PLACED_ROOMS);
    }

    #[test]
    fn the_reference_count_and_id_bytes_are_bounded_across_the_file() {
        // 10 spaces of 500 starred rooms is 5,000 references.
        let mut l = layout();
        l.spaces = (0..10)
            .map(|s| {
                let mut sp = SpaceLayout::new(rid(s));
                sp.starred = (0..500).map(|n| rid(10_000 + s * 1000 + n)).collect();
                sp
            })
            .collect();
        assert!(!l.fits());
        let n = l.normalized();
        let refs: usize = n.spaces.iter().map(|s| 1 + s.starred.len()).sum();
        assert!(refs <= MAX_ROOM_REFS, "{refs}");
        assert!(refs > MAX_ROOM_REFS - 600, "{refs}: it keeps what fits");
        assert!(n.fits());

        // 255-byte IDs: the byte budget binds first.
        let long = |n: usize| RoomId::from(format!("!{n:0>8}{}", "a".repeat(246)));
        let mut l = layout();
        l.spaces = (0..8)
            .map(|s| {
                let mut sp = SpaceLayout::new(long(s));
                sp.starred = (0..500).map(|n| long(100 + s * 1000 + n)).collect();
                sp
            })
            .collect();
        assert!(!l.fits());
        let n = l.normalized();
        let bytes: usize = n
            .spaces
            .iter()
            .map(|s| {
                s.space.as_str().len() + s.starred.iter().map(|r| r.as_str().len()).sum::<usize>()
            })
            .sum();
        assert!(bytes <= MAX_ID_BYTES_TOTAL, "{bytes}");
        assert!(
            bytes > MAX_ID_BYTES_TOTAL - 300,
            "{bytes}: it keeps what fits"
        );
    }

    #[test]
    fn fits_needs_a_valid_account() {
        assert!(layout().fits());
        assert!(!SidebarLayout::new(UserId::from("me")).fits());
        assert!(!SidebarLayout::new(UserId::from("")).fits());
    }

    #[test]
    fn debug_shows_counts_and_no_id_or_name() {
        let secret_room = RoomId::from("!secretroom:x");
        let mut l = SidebarLayout::new(UserId::from("@me:x"));
        l.rail.push(RailEntry::Space(secret_room.clone()));
        l.rail.push(RailEntry::Folder(FolderLayout {
            id: 1,
            name: Some(label("Payroll")),
            open: false,
            spaces: vec![secret_room.clone()],
        }));
        let mut sp = SpaceLayout::new(secret_room.clone());
        sp.order = vec![SectionKey::SubSpace(secret_room.clone())];
        sp.collapsed = vec![SectionKey::SubSpace(secret_room.clone())];
        sp.starred = vec![secret_room.clone()];
        sp.custom = vec![CustomSection {
            id: 2,
            name: label("Payroll"),
            rooms: vec![secret_room.clone()],
        }];
        l.spaces.push(sp.clone());
        let all = [
            format!("{l:?}"),
            format!("{:?}", l.rail),
            format!("{:?}", l.rail[0]),
            format!("{:?}", l.rail[1]),
            format!("{sp:?}"),
            format!("{:?}", sp.custom[0]),
            format!("{:?}", sp.order[0]),
            format!("{:?}", SidebarRead::Layout(l.clone())),
            format!("{:?}", SidebarRead::Newer(2)),
            format!("{:?}", label("Payroll")),
        ];
        for dbg in all {
            for secret in ["secretroom", "@me", "Payroll", "me:x"] {
                assert!(!dbg.contains(secret), "{secret} leaked in {dbg}");
            }
        }
        assert!(format!("{l:?}").contains('2'));
    }
}
