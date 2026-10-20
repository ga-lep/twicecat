//! The room list of a space as collapsible sections (sidebar plan, decision 4,
//! Task 8): where each room goes, the rolled-up badge of a collapsed section,
//! and the windowing of a list whose lines differ in height.
//!
//! Sections show only with a space selected, the All tab, a blank search and
//! the expanded list ([`App::sections_shown`]). Anything else keeps the flat
//! list of `room_list`.
//!
//! Room names and IDs belong to people: nothing here logs them, and no
//! `Debug` of a type here shows one.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use twicecat_core::api::{
    RoomId, RoomListChoice, RoomListTab, RoomNotificationModeView, RoomSummary, SectionKey,
    SpaceLayout, SpaceView,
};

use crate::room_list::{Badge, clamp_window, row_height};
use crate::state::App;
use crate::ui::tokens::Density;

#[cfg(test)]
thread_local! {
    /// How many times [`build`] ran on this thread (tests count rebuilds).
    static BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times [`build`] ran on this thread.
#[cfg(test)]
pub(crate) fn build_runs() -> usize {
    BUILDS.with(std::cell::Cell::get)
}

/// One line of the sectioned list. `Debug` shows no room: a room's ID and name
/// belong to people and must not reach a log.
#[derive(Clone, PartialEq)]
pub enum Line<'a> {
    /// The header of a section.
    Header(Header<'a>),
    /// A room.
    Room(&'a RoomSummary),
    /// The hint under an empty custom section.
    EmptyHint(SectionKey),
}

impl std::fmt::Debug for Line<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header(header) => f.debug_tuple("Header").field(header).finish(),
            Self::Room(_) => f.write_str("Room(..)"),
            Self::EmptyHint(key) => f.debug_tuple("EmptyHint").field(key).finish(),
        }
    }
}

/// The header of a section. `Debug` of the key hides a room ID; the name is
/// not shown by `Debug`.
#[derive(Clone, PartialEq)]
pub struct Header<'a> {
    /// Which section.
    pub key: SectionKey,
    /// The name as stored or reported: sub-space names come from other
    /// people and pass through `text_safety` where they are drawn.
    pub name: &'a str,
    /// Whether the section is collapsed.
    pub collapsed: bool,
    /// The rolled-up badge of its rooms: [`Badge::None`] unless collapsed.
    pub badge: Badge,
}

impl std::fmt::Debug for Header<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Header")
            .field("key", &self.key)
            .field("collapsed", &self.collapsed)
            .field("badge", &self.badge)
            .finish()
    }
}

/// Height of a section header, comfortable.
pub const HEADER_HEIGHT: f32 = 32.0;
/// Height of a section header, compact.
pub const COMPACT_HEADER_HEIGHT: f32 = 28.0;
/// Height of the hint under an empty custom section, comfortable.
pub const HINT_HEIGHT: f32 = 36.0;
/// Height of the hint under an empty custom section, compact.
pub const COMPACT_HINT_HEIGHT: f32 = 32.0;

/// The words under an empty custom section.
pub const EMPTY_HINT: &str = "Drop or move rooms here";

/// One section while the rooms are being placed.
struct Bucket<'a> {
    key: SectionKey,
    name: &'a str,
    /// Hidden while empty (every section but the user's own).
    hide_empty: bool,
    /// The rooms with their sort key: the order the user gave in Starred and
    /// custom sections, else the order of the list.
    rooms: Vec<(usize, &'a RoomSummary)>,
}

/// Places the rooms of `space` in sections and returns the lines to draw
/// (decision 4).
///
/// `spaces` are all the spaces the core reports: the level 1 ones whose ID is
/// among `space.rooms` are its sub-spaces. `rooms` are the rooms of the list
/// in its own (recent activity) order. Each room is placed once, in the first
/// of: Starred, a custom section, its sub-space's section, Direct messages
/// (when it is a DM), Channels. Spaces are never rows, and the selected
/// space's own ID, which a level 1 space lists among its rooms, is left out.
/// Every section but a custom one is hidden while empty; an empty custom one
/// is followed by [`Line::EmptyHint`]. A collapsed section keeps its header,
/// with the badge of its rooms, and drops its rooms.
pub fn build<'a>(
    space: &'a SpaceView,
    spaces: &'a [SpaceView],
    rooms: &'a [RoomSummary],
    layout: Option<&'a SpaceLayout>,
) -> Vec<Line<'a>> {
    #[cfg(test)]
    BUILDS.with(|n| n.set(n.get() + 1));
    let own = &space.id;
    // The sub-spaces: level 1 children of a level 0 space. A level 1 space
    // has none, whatever its list holds.
    let held: HashSet<&RoomId> = space.rooms.iter().collect();
    let mut seen_subs: HashSet<&RoomId> = HashSet::new();
    let subs: Vec<&SpaceView> = if space.level == 0 {
        spaces
            .iter()
            .filter(|s| {
                s.level == 1 && s.id != *own && held.contains(&s.id) && seen_subs.insert(&s.id)
            })
            .collect()
    } else {
        Vec::new()
    };
    // Which sub-space holds a room (the first one, in the core's order). A
    // sub-space's own ID and the selected space's are not rooms.
    let mut sub_of: HashMap<&RoomId, &RoomId> = HashMap::new();
    for sub in &subs {
        for id in &sub.rooms {
            if *id != sub.id && *id != *own {
                sub_of.entry(id).or_insert(&sub.id);
            }
        }
    }
    let mut not_rows: HashSet<&RoomId> = subs.iter().map(|s| &s.id).collect();
    not_rows.insert(own);

    let mut starred_at: HashMap<&RoomId, usize> = HashMap::new();
    let mut custom_at: HashMap<&RoomId, (u16, usize)> = HashMap::new();
    if let Some(layout) = layout {
        for (n, id) in layout.starred.iter().enumerate() {
            starred_at.entry(id).or_insert(n);
        }
        for custom in &layout.custom {
            for (n, id) in custom.rooms.iter().enumerate() {
                custom_at.entry(id).or_insert((custom.id, n));
            }
        }
    }

    // The sections in their default order.
    let mut buckets: Vec<Bucket<'a>> = Vec::new();
    let mut slot: HashMap<SectionKey, usize> = HashMap::new();
    let mut add = |key: SectionKey, name: &'a str, hide_empty: bool| {
        if !slot.contains_key(&key) {
            slot.insert(key.clone(), buckets.len());
            buckets.push(Bucket {
                key,
                name,
                hide_empty,
                rooms: Vec::new(),
            });
        }
    };
    add(SectionKey::Starred, "Starred", true);
    add(SectionKey::Channels, "Channels", true);
    for sub in &subs {
        add(SectionKey::SubSpace(sub.id.clone()), &sub.name, true);
    }
    for custom in layout.map_or(&[][..], |l| l.custom.as_slice()) {
        add(SectionKey::Custom(custom.id), custom.name.as_str(), false);
    }
    add(SectionKey::Direct, "Direct messages", true);

    let mut placed: HashSet<&RoomId> = HashSet::new();
    for (n, room) in rooms.iter().enumerate() {
        if room.is_space || not_rows.contains(&room.id) || !placed.insert(&room.id) {
            continue;
        }
        let (key, at) = if let Some(&at) = starred_at.get(&room.id) {
            (SectionKey::Starred, at)
        } else if let Some(&(id, at)) = custom_at.get(&room.id) {
            (SectionKey::Custom(id), at)
        } else if let Some(&sub) = sub_of.get(&room.id) {
            (SectionKey::SubSpace(sub.clone()), n)
        } else if room.is_dm {
            (SectionKey::Direct, n)
        } else {
            (SectionKey::Channels, n)
        };
        if let Some(&i) = slot.get(&key) {
            buckets[i].rooms.push((at, room));
        }
    }
    for bucket in &mut buckets {
        if matches!(bucket.key, SectionKey::Starred | SectionKey::Custom(_)) {
            bucket.rooms.sort_by_key(|(at, _)| *at);
        }
    }

    let collapsed: &[SectionKey] = layout.map_or(&[], |l| l.collapsed.as_slice());
    let mut lines = Vec::new();
    for key in section_order(&buckets, layout) {
        let bucket = &buckets[slot[&key]];
        let is_collapsed = collapsed.contains(&key);
        let badge = if is_collapsed {
            let all: Vec<&RoomSummary> = bucket.rooms.iter().map(|(_, r)| *r).collect();
            section_badge(&all)
        } else {
            Badge::None
        };
        lines.push(Line::Header(Header {
            key: key.clone(),
            name: bucket.name,
            collapsed: is_collapsed,
            badge,
        }));
        if is_collapsed {
            continue;
        }
        if bucket.rooms.is_empty() {
            lines.push(Line::EmptyHint(key));
        } else {
            lines.extend(bucket.rooms.iter().map(|(_, r)| Line::Room(r)));
        }
    }
    lines
}

/// The sections to draw, in order: the stored order for the keys it names
/// (unknown ones are skipped, and kept in the file), and each drawn section it
/// does not name right after the section that precedes it by default.
fn section_order(buckets: &[Bucket<'_>], layout: Option<&SpaceLayout>) -> Vec<SectionKey> {
    let defaults: Vec<&SectionKey> = buckets
        .iter()
        .filter(|b| !(b.hide_empty && b.rooms.is_empty()))
        .map(|b| &b.key)
        .collect();
    let mut order: Vec<SectionKey> = Vec::new();
    for key in layout.map_or(&[][..], |l| l.order.as_slice()) {
        if defaults.contains(&key) && !order.contains(key) {
            order.push(key.clone());
        }
    }
    for (i, key) in defaults.iter().enumerate() {
        if order.contains(key) {
            continue;
        }
        let after = defaults[..i]
            .iter()
            .rev()
            .find_map(|earlier| order.iter().position(|k| k == *earlier));
        order.insert(after.map_or(0, |p| p + 1), (*key).clone());
    }
    order
}

/// The badge of a collapsed section over its rooms: `badge_for`'s priority
/// (highlights, then notifications, then a dot) over the sums. Muted rooms
/// are left out, as the core leaves them out of a space's totals; the sums
/// saturate.
pub fn section_badge(rooms: &[&RoomSummary]) -> Badge {
    let (mut notifications, mut highlights, mut unread) = (0u64, 0u64, false);
    for room in rooms {
        if room.notification_mode == Some(RoomNotificationModeView::Mute) {
            continue;
        }
        notifications = notifications.saturating_add(room.unread_notifications);
        highlights = highlights.saturating_add(room.highlights);
        unread |= room.unread_messages > 0 || room.marked_unread;
    }
    if highlights > 0 {
        Badge::Highlight(highlights)
    } else if notifications > 0 {
        Badge::Count(notifications)
    } else if unread {
        Badge::Dot
    } else {
        Badge::None
    }
}

/// The sections the lines draw, in order: what a move up or down swaps among.
pub fn drawn_sections(lines: &[Line<'_>]) -> Vec<SectionKey> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Header(header) => Some(header.key.clone()),
            Line::Room(_) | Line::EmptyHint(_) => None,
        })
        .collect()
}

/// Where a room is drawn.
#[derive(Clone, PartialEq, Eq)]
pub struct Spot {
    /// The section that holds it.
    pub section: SectionKey,
    /// The rooms drawn in that section, in order.
    pub rooms: Vec<RoomId>,
    /// Its place among them.
    pub index: usize,
}

impl std::fmt::Debug for Spot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spot")
            .field("section", &self.section)
            .field("rooms", &self.rooms.len())
            .field("index", &self.index)
            .finish()
    }
}

/// Where `room` is drawn, or `None` when no line shows it (its section is
/// collapsed, or it is not in the list).
pub fn spot_of(lines: &[Line<'_>], room: &RoomId) -> Option<Spot> {
    let mut section: Option<&SectionKey> = None;
    let mut rooms: Vec<RoomId> = Vec::new();
    let mut found: Option<(SectionKey, usize)> = None;
    for line in lines {
        match line {
            Line::Header(header) => {
                if found.is_some() {
                    break;
                }
                section = Some(&header.key);
                rooms.clear();
            }
            Line::Room(r) => {
                if r.id == *room
                    && let Some(key) = section
                {
                    found = Some((key.clone(), rooms.len()));
                }
                rooms.push(r.id.clone());
            }
            Line::EmptyHint(_) => {}
        }
    }
    let (section, index) = found?;
    Some(Spot {
        section,
        rooms,
        index,
    })
}

/// The height of `line` in `density`.
pub fn line_height(line: &Line<'_>, density: Density) -> f32 {
    match (line, density) {
        (Line::Header(_), Density::Comfortable) => HEADER_HEIGHT,
        (Line::Header(_), Density::Compact) => COMPACT_HEADER_HEIGHT,
        (Line::Room(_), density) => row_height(density),
        (Line::EmptyHint(_), Density::Comfortable) => HINT_HEIGHT,
        (Line::EmptyHint(_), Density::Compact) => COMPACT_HINT_HEIGHT,
    }
}

/// The top of every line and, last, the total height: `lines.len() + 1`
/// prefix sums.
pub fn line_offsets(lines: &[Line<'_>], density: Density) -> Vec<f32> {
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    let mut top = 0.0_f32;
    offsets.push(top);
    for line in lines {
        top += line_height(line, density);
        offsets.push(top);
    }
    offsets
}

/// The lines to build for a scrollable showing `viewport_h` pixels at
/// `offset_y`: those that cross the viewport, plus `overscan` on each side.
/// `offsets` are the prefix sums of [`line_offsets`]; two binary searches
/// find the ends. NaN and negative inputs count as zero and a stale offset
/// is clamped to the last page, as [`crate::room_list::visible_range`] does
/// (both use the same clamping), so the range never leaves the lines.
pub fn visible_lines(
    offsets: &[f32],
    offset_y: f32,
    viewport_h: f32,
    overscan: usize,
) -> Range<usize> {
    let Some(len) = offsets.len().checked_sub(1).filter(|len| *len > 0) else {
        return 0..0;
    };
    let (offset, viewport) = clamp_window(offset_y, viewport_h, offsets[len]);
    // The first line whose bottom is below the top of the viewport, and the
    // first whose top is at or below its bottom.
    let first = offsets[1..].partition_point(|bottom| *bottom <= offset);
    let last = offsets[..len].partition_point(|top| *top < offset + viewport);
    let start = first.saturating_sub(overscan).min(len);
    let end = last.saturating_add(overscan).min(len).max(start);
    start..end
}

/// The heights of the spacers above and below the built `range` of lines, so
/// the scrollable's content stays as tall as all the lines.
pub fn line_spacers(offsets: &[f32], range: &Range<usize>) -> (f32, f32) {
    let total = offsets.last().copied().unwrap_or(0.0);
    let top = offsets.get(range.start).copied().unwrap_or(0.0);
    let end = offsets.get(range.end).copied().unwrap_or(total);
    (top, (total - end).max(0.0))
}

/// The load-all rule: in sectioned mode every room of the space is needed to
/// place it, so another page is asked for whenever the core says there are
/// more rooms than are loaded (the caller sends it once per loaded length).
pub fn must_load_all(len: usize, total: Option<u32>) -> bool {
    total.is_some_and(|total| usize::try_from(total).unwrap_or(usize::MAX) > len)
}

impl App {
    /// Whether the room list shows sections now: a space is selected, the tab
    /// is All, the search is blank and the list is expanded (not the rail).
    /// Anything else keeps the flat list.
    pub fn sections_shown(&self) -> bool {
        self.query.tab == RoomListTab::All
            && self.query.search.trim().is_empty()
            && self.pane_layout().list == RoomListChoice::Expanded
            && self
                .query
                .space
                .as_ref()
                .is_some_and(|id| self.spaces.contains(id))
    }

    /// The lines of the sectioned list; `None` while the list is flat.
    pub fn section_lines(&self) -> Option<Vec<Line<'_>>> {
        if !self.sections_shown() {
            return None;
        }
        let id = self.query.space.as_ref()?;
        let space = self.spaces.spaces().iter().find(|s| s.id == *id)?;
        let stored = self
            .sidebar
            .layout()
            .and_then(|layout| layout.spaces.iter().find(|s| s.space == *id));
        Some(build(space, self.spaces.spaces(), &self.rooms, stored))
    }

    /// The lines of the sectioned list with no layout: every room in its
    /// default section, and every section open. A drop uses it to tell which
    /// section is a room's own default one; `None` while the list is flat.
    pub(crate) fn default_section_lines(&self) -> Option<Vec<Line<'_>>> {
        if !self.sections_shown() {
            return None;
        }
        let id = self.query.space.as_ref()?;
        let space = self.spaces.spaces().iter().find(|s| s.id == *id)?;
        Some(build(space, self.spaces.spaces(), &self.rooms, None))
    }

    /// The rooms as the list draws them, in order: the list itself while it
    /// is flat, else the room lines of the sections, so a room in a
    /// collapsed section is not among them. The room-stepping keys walk this.
    pub(crate) fn rooms_as_drawn(&self) -> Cow<'_, [RoomSummary]> {
        match self.section_lines() {
            None => Cow::Borrowed(&self.rooms),
            Some(lines) => Cow::Owned(
                lines
                    .into_iter()
                    .filter_map(|line| match line {
                        Line::Room(room) => Some(room.clone()),
                        Line::Header(_) | Line::EmptyHint(_) => None,
                    })
                    .collect(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use tokio::sync::mpsc;
    use twicecat_core::api::{
        Command, CoreEvent, CustomSection, DeviceId, ListDiff, RoomId, RoomListChoice, RoomListTab,
        RoomNotificationModeView, RoomSummary, SectionKey, SidebarLabel, SidebarLayout,
        SpaceLayout, SpaceView, UserId,
    };

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::room_list::{Badge, OVERSCAN_ROWS, visible_range};
    use crate::sidebar::Op;
    use crate::sidebar_store::{MemorySidebar, SidebarAnswer, SidebarLoad};
    use crate::spaces::SpacesMessage;
    use crate::state::{Message, Startup};
    use crate::ui::tokens::Density;

    fn rid(n: &str) -> RoomId {
        RoomId::from(format!("!{n}:x"))
    }

    fn room(n: &str) -> RoomSummary {
        RoomSummary::new(rid(n), n.to_owned())
    }

    fn dm(n: &str) -> RoomSummary {
        let mut r = room(n);
        r.is_dm = true;
        r
    }

    fn space_row(n: &str) -> RoomSummary {
        let mut r = room(n);
        r.is_space = true;
        r
    }

    fn view(n: &str, level: u32, rooms: &[&str]) -> SpaceView {
        SpaceView {
            id: rid(n),
            name: n.to_owned(),
            avatar: None,
            level,
            children: rooms.len() as u64,
            rooms: rooms.iter().map(|r| rid(r)).collect(),
        }
    }

    /// Space `sp` (level 0) with the sub-spaces `tech` (which holds a
    /// level-2 room `rustroom`) and `voice`, and the SDK's own-ID-last
    /// convention for the level-1 spaces.
    fn spaces() -> Vec<SpaceView> {
        vec![
            view(
                "sp",
                0,
                &["chan1", "chan2", "dm1", "tech", "voice", "starred1"],
            ),
            view(
                "tech",
                1,
                &["t1", "t2", "rustroom", "rust", "techdm", "tech"],
            ),
            view("voice", 1, &["v1", "voice"]),
        ]
    }

    /// Rooms in recent-activity order, the sub-space rows among them.
    fn rooms() -> Vec<RoomSummary> {
        let mut v1 = room("v1");
        v1.is_call_room = true;
        vec![
            room("t1"),
            room("chan1"),
            space_row("tech"),
            v1,
            room("rustroom"),
            room("chan2"),
            dm("dm1"),
            space_row("voice"),
            space_row("rust"),
            room("t2"),
            dm("techdm"),
            room("starred1"),
        ]
    }

    fn lines_of<'a>(lines: &[Line<'a>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| match line {
                Line::Header(h) => format!("# {}", h.name),
                Line::Room(r) => r.name.clone(),
                Line::EmptyHint(_) => "~hint".to_owned(),
            })
            .collect()
    }

    fn built(layout: Option<&SpaceLayout>) -> Vec<String> {
        let sp = spaces();
        let rs = rooms();
        lines_of(&build(&sp[0], &sp, &rs, layout))
    }

    fn label(s: &str) -> SidebarLabel {
        SidebarLabel::try_from(s).expect("a valid name")
    }

    // ---- the options card's questions about the lines (Task sbc.9) ----

    #[test]
    fn debug_of_a_line_shows_no_room_id_and_no_room_name() {
        let mut secret = RoomSummary::new(RoomId::from("!secretroom:x"), "Payroll team".to_owned());
        secret.is_dm = true;
        let lines = [
            Line::Room(&secret),
            Line::Header(Header {
                key: SectionKey::SubSpace(RoomId::from("!secretroom:x")),
                name: "Payroll team",
                collapsed: true,
                badge: Badge::Dot,
            }),
            Line::EmptyHint(SectionKey::SubSpace(RoomId::from("!secretroom:x"))),
        ];
        for line in &lines {
            let text = format!("{line:?}");
            for hidden in ["secretroom", "Payroll"] {
                assert!(!text.contains(hidden), "{hidden} is in {text}");
            }
        }
        assert_eq!(format!("{:?}", lines[0]), "Room(..)");
        assert!(format!("{:?}", lines[1]).starts_with("Header"));
        assert!(format!("{:?}", lines[2]).starts_with("EmptyHint"));
        let all = format!("{lines:?}");
        assert!(
            !all.contains("secretroom") && !all.contains("Payroll"),
            "{all}"
        );
    }

    #[test]
    fn the_drawn_sections_and_a_rooms_spot_come_from_the_lines() {
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.custom = vec![
            CustomSection {
                id: 4,
                name: label("Mine"),
                rooms: vec![rid("chan2"), rid("t1")],
            },
            CustomSection {
                id: 5,
                name: label("Empty"),
                rooms: Vec::new(),
            },
        ];
        layout.collapsed = vec![SectionKey::Direct];
        let sp = spaces();
        let rs = rooms();
        let lines = build(&sp[0], &sp, &rs, Some(&layout));
        assert_eq!(
            drawn_sections(&lines),
            [
                SectionKey::Channels,
                SectionKey::SubSpace(rid("tech")),
                SectionKey::SubSpace(rid("voice")),
                SectionKey::Custom(4),
                SectionKey::Custom(5),
                SectionKey::Direct,
            ]
        );
        let spot = spot_of(&lines, &rid("t1")).expect("drawn");
        assert_eq!(spot.section, SectionKey::Custom(4));
        assert_eq!(spot.rooms, [rid("chan2"), rid("t1")]);
        assert_eq!(spot.index, 1);
        assert_eq!(spot_of(&lines, &rid("chan2")).map(|s| s.index), Some(0));
        // In a collapsed section, or not in the list: not drawn.
        assert_eq!(spot_of(&lines, &rid("dm1")), None);
        assert_eq!(spot_of(&lines, &rid("nobody")), None);
        // The first section's room, and the last section's.
        assert_eq!(
            spot_of(&lines, &rid("chan1")).map(|s| s.section),
            Some(SectionKey::Channels)
        );
        assert_eq!(
            spot_of(&lines, &rid("v1")).map(|s| s.section),
            Some(SectionKey::SubSpace(rid("voice")))
        );
    }

    // ---- placement ----

    #[test]
    fn every_room_of_the_space_appears_exactly_once() {
        let out = built(None);
        let mut seen = HashSet::new();
        for line in &out {
            if !line.starts_with('#') {
                assert!(seen.insert(line.clone()), "{line} twice in {out:?}");
            }
        }
        let want: HashSet<String> = [
            "t1", "t2", "chan1", "chan2", "v1", "rustroom", "dm1", "techdm", "starred1",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(seen, want);
    }

    #[test]
    fn the_default_order_and_the_default_places() {
        assert_eq!(
            built(None),
            [
                "# Channels",
                "chan1",
                "chan2",
                "starred1",
                "# tech",
                "t1",
                "rustroom",
                "t2",
                "techdm",
                "# voice",
                "v1",
                "# Direct messages",
                "dm1",
            ]
        );
    }

    #[test]
    fn starred_beats_custom_beats_sub_space_beats_dm_beats_channels() {
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.starred = vec![rid("t1"), rid("chan1")];
        layout.custom = vec![CustomSection {
            id: 7,
            name: label("Mine"),
            rooms: vec![rid("t1"), rid("t2"), rid("dm1")],
        }];
        let out = built(Some(&layout));
        assert_eq!(
            out,
            [
                "# Starred",
                "t1",
                "chan1",
                "# Channels",
                "chan2",
                "starred1",
                "# tech",
                "rustroom",
                "techdm",
                "# voice",
                "v1",
                "# Mine",
                "t2",
                "dm1",
            ],
            "t1 starred and in custom: Starred; t2 custom over its sub-space; \
             dm1 custom over DM; techdm (a DM in a sub-space) stays in the sub-space"
        );
    }

    #[test]
    fn the_stored_order_of_starred_and_custom_rooms_is_kept() {
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.starred = vec![rid("chan2"), rid("chan1")];
        layout.custom = vec![CustomSection {
            id: 1,
            name: label("C"),
            rooms: vec![rid("t2"), rid("t1")],
        }];
        let out = built(Some(&layout));
        assert_eq!(&out[..3], ["# Starred", "chan2", "chan1"]);
        let at = out.iter().position(|l| l == "# C").expect("custom header");
        assert_eq!(&out[at..at + 3], ["# C", "t2", "t1"]);
    }

    #[test]
    fn sub_space_rows_are_not_rows_and_a_level_2_room_lands_in_its_level_1_section() {
        let out = built(None);
        for gone in ["tech", "voice", "rust"] {
            assert!(!out.iter().any(|l| l == gone), "{gone} drawn as a row");
        }
        let at = out
            .iter()
            .position(|l| l == "# tech")
            .expect("Tech section");
        assert!(out[at + 1..].iter().take(4).any(|l| l == "rustroom"));
    }

    #[test]
    fn the_spaces_own_id_is_left_out() {
        // A level-1 space is the selected one: the SDK lists it in its own
        // rooms, last (or not at all once the 2000-room cap cuts it off).
        let sp = vec![view("tech", 1, &["t1", "t2", "tech"])];
        for own_row in [space_row("tech"), room("tech")] {
            let rs = vec![room("t1"), own_row, room("t2")];
            let out = lines_of(&build(&sp[0], &sp, &rs, None));
            assert_eq!(out, ["# Channels", "t1", "t2"], "no row, no section for it");
        }
        // Cut off by the cap: the same result.
        let cut = vec![view("tech", 1, &["t1", "t2"])];
        let rs = vec![room("t1"), room("t2")];
        assert_eq!(
            lines_of(&build(&cut[0], &cut, &rs, None)),
            ["# Channels", "t1", "t2"]
        );
        // A level-0 space whose spaces list also holds a level-1 space that
        // is itself: never its own sub-space.
        let both = vec![view("a", 0, &["a", "r1"]), view("a", 1, &["a", "r1"])];
        let rs = vec![room("r1")];
        assert_eq!(
            lines_of(&build(&both[0], &both, &rs, None)),
            ["# Channels", "r1"]
        );
    }

    #[test]
    fn a_level_1_space_in_two_sub_spaces_is_placed_once() {
        let sp = vec![
            view("sp", 0, &["a", "b"]),
            view("a", 1, &["x", "a"]),
            view("b", 1, &["x", "b"]),
        ];
        let rs = vec![room("x")];
        assert_eq!(
            lines_of(&build(&sp[0], &sp, &rs, None)),
            ["# a", "x"],
            "the first sub-space wins; the other is empty and hidden"
        );
    }

    #[test]
    fn empty_built_in_sections_are_hidden_and_an_empty_custom_one_shows_its_hint() {
        let sp = vec![view("sp", 0, &["a", "empty"]), view("empty", 1, &["empty"])];
        let rs = vec![room("a")];
        assert_eq!(
            lines_of(&build(&sp[0], &sp, &rs, None)),
            ["# Channels", "a"]
        );
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.custom = vec![CustomSection {
            id: 3,
            name: label("Later"),
            rooms: vec![rid("gone")],
        }];
        assert_eq!(
            lines_of(&build(&sp[0], &sp, &rs, Some(&layout))),
            ["# Channels", "a", "# Later", "~hint"]
        );
        layout.collapsed = vec![SectionKey::Custom(3)];
        assert_eq!(
            lines_of(&build(&sp[0], &sp, &rs, Some(&layout))),
            ["# Channels", "a", "# Later"],
            "collapsed, the hint goes with the rooms"
        );
    }

    #[test]
    fn a_space_with_no_rooms_has_no_lines() {
        let sp = vec![view("sp", 0, &[])];
        assert!(build(&sp[0], &sp, &[], None).is_empty());
    }

    #[test]
    fn the_stored_order_is_kept_and_missing_keys_take_their_default_place() {
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.order = vec![
            SectionKey::Direct,
            SectionKey::SubSpace(rid("voice")),
            SectionKey::Channels,
        ];
        let heads: Vec<String> = built(Some(&layout))
            .into_iter()
            .filter(|l| l.starts_with('#'))
            .collect();
        // `tech` is missing from the stored order: it goes right after the
        // section that precedes it by default (Channels).
        assert_eq!(
            heads,
            ["# Direct messages", "# voice", "# Channels", "# tech"]
        );
    }

    #[test]
    fn unknown_stored_keys_are_not_drawn_and_not_a_problem() {
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.order = vec![
            SectionKey::Custom(99),
            SectionKey::SubSpace(rid("nowhere")),
            SectionKey::Starred,
            SectionKey::Direct,
        ];
        let heads: Vec<String> = built(Some(&layout))
            .into_iter()
            .filter(|l| l.starts_with('#'))
            .collect();
        assert_eq!(
            heads,
            ["# Channels", "# tech", "# voice", "# Direct messages"],
            "empty Starred is hidden even when stored first; the one stored key \
             is where it is by default"
        );
    }

    #[test]
    fn a_collapsed_section_keeps_its_header_and_drops_its_rooms() {
        let mut layout = SpaceLayout::new(rid("sp"));
        layout.collapsed = vec![SectionKey::SubSpace(rid("tech")), SectionKey::Channels];
        let out = built(Some(&layout));
        assert_eq!(
            out,
            [
                "# Channels",
                "# tech",
                "# voice",
                "v1",
                "# Direct messages",
                "dm1"
            ]
        );
    }

    // ---- badges ----

    fn counted(n: &str, notifications: u64, highlights: u64) -> RoomSummary {
        let mut r = room(n);
        r.unread_notifications = notifications;
        r.highlights = highlights;
        r
    }

    #[test]
    fn a_section_badge_follows_badge_for_over_the_summed_counts() {
        let a = counted("a", 3, 1);
        let b = counted("b", 2, 1);
        assert_eq!(section_badge(&[&a, &b]), Badge::Highlight(2));
        let c = counted("c", 4, 0);
        let d = counted("d", 1, 0);
        assert_eq!(section_badge(&[&c, &d]), Badge::Count(5));
        let mut marked = room("m");
        marked.marked_unread = true;
        assert_eq!(section_badge(&[&room("q"), &marked]), Badge::Dot);
        let mut quiet = room("u");
        quiet.unread_messages = 3;
        assert_eq!(section_badge(&[&quiet]), Badge::Dot);
        assert_eq!(section_badge(&[&room("z")]), Badge::None);
        assert_eq!(section_badge(&[]), Badge::None);
    }

    #[test]
    fn highlights_2_and_notifications_5_make_highlight_2() {
        let a = counted("a", 3, 2);
        let b = counted("b", 2, 0);
        assert_eq!(section_badge(&[&a, &b]), Badge::Highlight(2));
    }

    #[test]
    fn muted_rooms_are_left_out_of_the_sums_like_the_core_does() {
        let mut loud = counted("a", 0, 0);
        loud.unread_notifications = 2;
        let mut muted = counted("b", 9, 4);
        muted.notification_mode = Some(RoomNotificationModeView::Mute);
        assert_eq!(section_badge(&[&loud, &muted]), Badge::Count(2));
        assert_eq!(section_badge(&[&muted]), Badge::None);
    }

    #[test]
    fn sums_saturate() {
        let a = counted("a", u64::MAX, 0);
        let b = counted("b", 5, 0);
        assert_eq!(section_badge(&[&a, &b]), Badge::Count(u64::MAX));
    }

    #[test]
    fn only_a_collapsed_header_carries_a_badge() {
        let sp = vec![view("sp", 0, &["a", "b"])];
        let rs = vec![counted("a", 5, 2), counted("b", 0, 0)];
        let mut layout = SpaceLayout::new(rid("sp"));
        let badge_of = |layout: &SpaceLayout| -> Badge {
            match &build(&sp[0], &sp, &rs, Some(layout))[0] {
                Line::Header(h) => h.badge,
                other => panic!("not a header: {other:?}"),
            }
        };
        assert_eq!(badge_of(&layout), Badge::None, "expanded: no header badge");
        layout.collapsed = vec![SectionKey::Channels];
        assert_eq!(badge_of(&layout), Badge::Highlight(2));
    }

    // ---- windowing ----

    fn mixed_lines() -> Vec<RoomSummary> {
        (0..20).map(|i| room(&format!("r{i}"))).collect()
    }

    /// Header, 3 rooms, header, hint, header, 3 rooms ... in comfortable
    /// heights: 32, 64, 64, 64, 32, 36, 32, 64, ...
    fn mixed<'a>(rooms: &'a [RoomSummary]) -> Vec<Line<'a>> {
        let header = |key| {
            Line::Header(Header {
                key,
                name: "h",
                collapsed: false,
                badge: Badge::None,
            })
        };
        let mut out = vec![header(SectionKey::Channels)];
        out.extend(rooms[0..3].iter().map(Line::Room));
        out.push(header(SectionKey::Custom(1)));
        out.push(Line::EmptyHint(SectionKey::Custom(1)));
        out.push(header(SectionKey::Direct));
        out.extend(rooms[3..8].iter().map(Line::Room));
        out
    }

    #[test]
    fn heights_follow_the_density() {
        let rooms = mixed_lines();
        let lines = mixed(&rooms);
        let heights: Vec<f32> = lines
            .iter()
            .map(|l| line_height(l, Density::Comfortable))
            .collect();
        assert_eq!(&heights[..7], [32.0, 64.0, 64.0, 64.0, 32.0, 36.0, 32.0]);
        let compact: Vec<f32> = lines
            .iter()
            .map(|l| line_height(l, Density::Compact))
            .collect();
        assert_eq!(&compact[..7], [28.0, 44.0, 44.0, 44.0, 28.0, 32.0, 28.0]);
    }

    #[test]
    fn offsets_are_prefix_sums_ending_with_the_total() {
        let rooms = mixed_lines();
        let lines = mixed(&rooms);
        let offsets = line_offsets(&lines, Density::Comfortable);
        assert_eq!(offsets.len(), lines.len() + 1);
        assert_eq!(&offsets[..5], [0.0, 32.0, 96.0, 160.0, 224.0]);
        let total: f32 = lines
            .iter()
            .map(|l| line_height(l, Density::Comfortable))
            .sum();
        assert_eq!(*offsets.last().expect("a total"), total);
        assert_eq!(line_offsets(&[], Density::Comfortable), [0.0]);
    }

    /// Offsets of lines 32, 64, 64, 36, 32, 64: 0, 32, 96, 160, 196, 228, 292.
    const OFFSETS: [f32; 7] = [0.0, 32.0, 96.0, 160.0, 196.0, 228.0, 292.0];

    #[test]
    fn visible_lines_returns_exactly_the_lines_that_cross_the_viewport() {
        // 50..150 crosses line 1 (32..96) and line 2 (96..160), not 0 or 3.
        assert_eq!(visible_lines(&OFFSETS, 50.0, 100.0, 0), 1..3);
        // Exactly line 2 fills 96..160: neighbours touch but do not cross.
        assert_eq!(visible_lines(&OFFSETS, 96.0, 64.0, 0), 2..3);
        // One pixel more at the bottom brings line 3 in.
        assert_eq!(visible_lines(&OFFSETS, 96.0, 65.0, 0), 2..4);
        // One pixel more at the top brings line 1 in.
        assert_eq!(visible_lines(&OFFSETS, 95.0, 64.0, 0), 1..3);
        // The first page.
        assert_eq!(visible_lines(&OFFSETS, 0.0, 33.0, 0), 0..2);
    }

    #[test]
    fn overscan_adds_lines_on_both_sides_and_stops_at_the_ends() {
        assert_eq!(visible_lines(&OFFSETS, 96.0, 64.0, 1), 1..4);
        assert_eq!(visible_lines(&OFFSETS, 96.0, 64.0, 10), 0..6);
        assert_eq!(visible_lines(&OFFSETS, 0.0, 10.0, 2), 0..3);
    }

    #[test]
    fn nan_negative_and_stale_offsets_are_clamped() {
        let first = visible_lines(&OFFSETS, 0.0, 100.0, 0);
        assert_eq!(visible_lines(&OFFSETS, f32::NAN, 100.0, 0), first);
        assert_eq!(visible_lines(&OFFSETS, -500.0, 100.0, 0), first);
        assert_eq!(visible_lines(&OFFSETS, f32::NEG_INFINITY, 100.0, 0), first);
        // The list shrank under the offset: the last page.
        let last = visible_lines(&OFFSETS, 1.0e9, 100.0, 0);
        assert_eq!(last, 3..6, "line 3 still crosses the last page");
        assert_eq!(visible_lines(&OFFSETS, f32::INFINITY, 100.0, 0), last);
        // A bad viewport counts as zero high.
        assert_eq!(visible_lines(&OFFSETS, 0.0, f32::NAN, 0), 0..0);
        assert_eq!(visible_lines(&OFFSETS, 0.0, f32::INFINITY, 0), 0..0);
        assert_eq!(visible_lines(&OFFSETS, 0.0, -3.0, 0), 0..0);
        assert_eq!(visible_lines(&[0.0], 5.0, 100.0, 3), 0..0);
        assert_eq!(visible_lines(&[], 5.0, 100.0, 3), 0..0);
    }

    #[test]
    fn spacers_and_built_lines_add_up_to_the_total() {
        let total = *OFFSETS.last().expect("a total");
        for (offset, viewport, overscan) in [
            (0.0, 100.0, 0),
            (50.0, 100.0, 1),
            (96.0, 64.0, 0),
            (1.0e9, 100.0, 0),
            (f32::NAN, 100.0, 2),
            (100.0, 1000.0, 0),
        ] {
            let range = visible_lines(&OFFSETS, offset, viewport, overscan);
            let (top, bottom) = line_spacers(&OFFSETS, &range);
            let built = OFFSETS[range.end] - OFFSETS[range.start];
            assert_eq!(
                top + built + bottom,
                total,
                "{offset} {viewport} {overscan}"
            );
        }
        // The same over real lines.
        let rooms = mixed_lines();
        let lines = mixed(&rooms);
        let offsets = line_offsets(&lines, Density::Compact);
        let range = visible_lines(&offsets, 100.0, 120.0, 1);
        let (top, bottom) = line_spacers(&offsets, &range);
        let built: f32 = lines[range.clone()]
            .iter()
            .map(|l| line_height(l, Density::Compact))
            .sum();
        let total: f32 = lines.iter().map(|l| line_height(l, Density::Compact)).sum();
        assert!((top + built + bottom - total).abs() < 0.01);
    }

    #[test]
    fn equal_heights_give_what_visible_range_gives() {
        let offsets: Vec<f32> = (0..=1000).map(|i| i as f32 * 64.0).collect();
        for (offset, viewport) in [
            (0.0, 800.0),
            (63.0, 800.0),
            (64.0, 800.0),
            (12_345.0, 640.0),
            (1.0e9, 800.0),
            (f32::NAN, 800.0),
            (-4.0, 800.0),
        ] {
            assert_eq!(
                visible_lines(&offsets, offset, viewport, OVERSCAN_ROWS),
                visible_range(offset, viewport, 64.0, 1000, OVERSCAN_ROWS),
                "{offset} {viewport}"
            );
        }
    }

    // ---- the app ----

    fn app() -> (App, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(4096);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        app.set_sidebar_store(Box::new(MemorySidebar::default()));
        (app, rx)
    }

    fn feed(app: &mut App, event: CoreEvent) {
        let _ = app.update(Message::Core(Box::new(event)));
    }

    /// An app logged in as `@a:x` with its layout loaded, the spaces and the
    /// rooms of the fixtures, and the space `sp` selected.
    fn space_app(total: Option<u32>) -> (App, mpsc::Receiver<Command>) {
        let (mut app, rx) = app();
        feed(
            &mut app,
            CoreEvent::LoggedIn {
                user_id: UserId::from("@a:x"),
                device_id: DeviceId::from("D"),
                restored: false,
            },
        );
        let _ = app.update(Message::SidebarLoaded(SidebarAnswer::new(
            UserId::from("@a:x"),
            SidebarLoad::Layout(SidebarLayout::new(UserId::from("@a:x"))),
        )));
        feed(&mut app, CoreEvent::Spaces(spaces()));
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: rooms() }]),
        );
        feed(&mut app, CoreEvent::RoomListLoaded { total_rooms: total });
        let _ = app.update(Message::Spaces(SpacesMessage::Select(Some(rid("sp")))));
        (app, rx)
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(c) = rx.try_recv() {
            out.push(c);
        }
        out
    }

    fn drawn_names(app: &App) -> Vec<String> {
        lines_of(&app.section_lines().expect("sections show"))
    }

    // ---- when sections show ----

    #[test]
    fn sections_show_with_a_space_the_all_tab_a_blank_search_and_the_expanded_list() {
        let (mut app, _rx) = space_app(None);
        assert!(app.sections_shown());
        assert!(app.section_lines().is_some());
        // A blank search of spaces only is still blank.
        let _ = app.update(Message::SearchChanged("   ".to_owned()));
        assert!(app.sections_shown());
    }

    #[test]
    fn home_a_search_another_tab_or_the_rail_keep_the_flat_list() {
        let flat = |app: &App| !app.sections_shown() && app.section_lines().is_none();
        let (mut app, _rx) = space_app(None);
        let _ = app.update(Message::SearchChanged("t1".to_owned()));
        assert!(flat(&app), "a search");
        let _ = app.update(Message::SearchChanged(String::new()));
        assert!(app.sections_shown());
        for tab in [RoomListTab::People, RoomListTab::Rooms, RoomListTab::Spaces] {
            let _ = app.update(Message::TabSelected(tab));
            assert!(flat(&app), "{tab:?}");
        }
        let _ = app.update(Message::TabSelected(RoomListTab::All));
        assert!(app.sections_shown());
        app.ui_settings.room_list = RoomListChoice::Rail;
        assert!(flat(&app), "the rail");
        app.ui_settings.room_list = RoomListChoice::Expanded;
        let _ = app.update(Message::Spaces(SpacesMessage::Select(None)));
        assert!(flat(&app), "Home");
    }

    #[test]
    fn the_flat_list_is_the_rooms_in_their_order_as_before() {
        let (mut app, _rx) = space_app(None);
        let _ = app.update(Message::Spaces(SpacesMessage::Select(None)));
        let names: Vec<String> = app
            .rooms_as_drawn()
            .iter()
            .map(|r| r.name.clone())
            .collect();
        let want: Vec<String> = app.rooms.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, want);
    }

    // ---- keyboard ----

    fn press(app: &mut App, shortcut: crate::keys::Shortcut) {
        let _ = app.update(Message::Shortcut {
            shortcut,
            captured: false,
        });
    }

    fn collapse(app: &mut App, key: SectionKey) {
        assert!(
            app.apply_sidebar(Op::ToggleSection {
                space: rid("sp"),
                key,
            })
            .is_ok()
        );
    }

    #[test]
    fn alt_down_goes_from_the_last_room_of_a_section_to_the_first_of_the_next_drawn_one() {
        use crate::keys::Shortcut;
        let (mut app, _rx) = space_app(None);
        collapse(&mut app, SectionKey::SubSpace(rid("tech")));
        assert_eq!(
            drawn_names(&app),
            [
                "# Channels",
                "chan1",
                "chan2",
                "starred1",
                "# tech",
                "# voice",
                "v1",
                "# Direct messages",
                "dm1"
            ]
        );
        let _ = app.update(Message::SelectRoom(rid("starred1")));
        press(&mut app, Shortcut::NextRoom);
        assert_eq!(app.selected, Some(rid("v1")), "Tech is collapsed: skipped");
        press(&mut app, Shortcut::NextRoom);
        assert_eq!(app.selected, Some(rid("dm1")));
        press(&mut app, Shortcut::NextRoom);
        assert_eq!(app.selected, Some(rid("chan1")), "wraps to the first drawn");
        press(&mut app, Shortcut::PreviousRoom);
        assert_eq!(app.selected, Some(rid("dm1")), "and back");
        press(&mut app, Shortcut::PreviousRoom);
        press(&mut app, Shortcut::PreviousRoom);
        assert_eq!(app.selected, Some(rid("starred1")));
    }

    #[test]
    fn ctrl_shift_arrows_walk_the_drawn_rooms_too() {
        use crate::keys::Shortcut;
        let (mut app, _rx) = space_app(None);
        collapse(&mut app, SectionKey::SubSpace(rid("tech")));
        let mut unread = room("v1");
        unread.unread_messages = 2;
        let mut hidden = room("t1");
        hidden.unread_messages = 2;
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![
                ListDiff::Set {
                    index: 3,
                    value: unread,
                },
                ListDiff::Set {
                    index: 0,
                    value: hidden,
                },
            ]),
        );
        let _ = app.update(Message::SelectRoom(rid("chan1")));
        press(&mut app, Shortcut::NextUnread);
        assert_eq!(app.selected, Some(rid("v1")));
        press(&mut app, Shortcut::NextUnread);
        assert_eq!(
            app.selected,
            Some(rid("v1")),
            "t1 is in a collapsed section"
        );
    }

    #[test]
    fn the_keys_walk_the_flat_list_when_sections_do_not_show() {
        use crate::keys::Shortcut;
        let (mut app, _rx) = space_app(None);
        let _ = app.update(Message::SearchChanged("x".to_owned()));
        let _ = app.update(Message::SelectRoom(rid("t1")));
        press(&mut app, Shortcut::NextRoom);
        assert_eq!(app.selected, Some(rid("chan1")), "the list's own order");
    }

    // ---- load all ----

    #[test]
    fn the_load_all_rule() {
        assert!(!must_load_all(10, None));
        assert!(!must_load_all(10, Some(10)));
        assert!(!must_load_all(10, Some(3)));
        assert!(must_load_all(10, Some(11)));
        assert!(must_load_all(0, Some(1)));
    }

    #[test]
    fn sectioned_mode_asks_for_a_page_per_loaded_page_until_everything_is_in() {
        let (mut app, mut rx) = space_app(Some(40));
        let loads = |rx: &mut mpsc::Receiver<Command>| {
            drain(rx)
                .into_iter()
                .filter(|c| matches!(c, Command::LoadMoreRooms))
                .count()
        };
        // Selecting the space asks the core for its rooms; its answer
        // arrives as a list and a total. 12 loaded of 40, and the viewport is
        // nowhere near the end: the next page is still asked for.
        let _ = drain(&mut rx);
        feed(
            &mut app,
            CoreEvent::RoomListLoaded {
                total_rooms: Some(40),
            },
        );
        assert_eq!(loads(&mut rx), 1);
        // The same length again: not asked twice.
        feed(
            &mut app,
            CoreEvent::RoomListLoaded {
                total_rooms: Some(40),
            },
        );
        assert_eq!(loads(&mut rx), 0);
        // A page arrives: asked once more.
        let page: Vec<RoomSummary> = (0..20).map(|i| room(&format!("p{i}"))).collect();
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![ListDiff::Append { values: page }]),
        );
        assert_eq!(loads(&mut rx), 1);
        let page: Vec<RoomSummary> = (20..28).map(|i| room(&format!("p{i}"))).collect();
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![ListDiff::Append { values: page }]),
        );
        assert_eq!(app.rooms.len(), 40);
        assert_eq!(loads(&mut rx), 0, "len == total: done");
    }

    #[test]
    fn sectioned_mode_pages_even_with_the_viewport_far_from_the_end() {
        let (mut app, mut rx) = space_app(Some(1500));
        let _ = drain(&mut rx);
        let many: Vec<RoomSummary> = (0..500).map(|i| room(&format!("m{i}"))).collect();
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: many }]),
        );

        feed(
            &mut app,
            CoreEvent::RoomListLoaded {
                total_rooms: Some(1500),
            },
        );
        assert_eq!(
            drain(&mut rx)
                .iter()
                .filter(|c| matches!(c, Command::LoadMoreRooms))
                .count(),
            1,
            "500 of 1500 loaded, the viewport is at the top, and one page is asked for \
             once per loaded length"
        );
    }

    #[test]
    fn the_flat_list_still_pages_only_near_its_end() {
        let (mut app, mut rx) = space_app(Some(5000));
        let _ = app.update(Message::Spaces(SpacesMessage::Select(None)));
        let _ = drain(&mut rx);
        let many: Vec<RoomSummary> = (0..500).map(|i| room(&format!("m{i}"))).collect();
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: many }]),
        );
        feed(
            &mut app,
            CoreEvent::RoomListLoaded {
                total_rooms: Some(5000),
            },
        );
        assert!(
            drain(&mut rx)
                .iter()
                .all(|c| !matches!(c, Command::LoadMoreRooms)),
            "the viewport is at the top of 500 rooms"
        );
    }

    #[test]
    fn expanding_a_section_asks_for_the_avatars_of_the_rooms_it_shows() {
        use twicecat_core::api::AvatarId;
        let (mut app, mut rx) = space_app(None);
        let tech = SectionKey::SubSpace(rid("tech"));
        collapse(&mut app, tech.clone());
        let mut with_avatar = room("t1");
        with_avatar.avatar = Some(AvatarId::from("mxc://x/t1"));
        feed(
            &mut app,
            CoreEvent::RoomListChanged(vec![ListDiff::Set {
                index: 0,
                value: with_avatar,
            }]),
        );
        let asked = |rx: &mut mpsc::Receiver<Command>| {
            drain(rx)
                .into_iter()
                .any(|c| matches!(c, Command::FetchAvatars(ids) if ids.contains(&AvatarId::from("mxc://x/t1"))))
        };
        assert!(!asked(&mut rx), "t1 is in a collapsed section: not in view");
        let _ = app.update(Message::Spaces(SpacesMessage::ToggleSection(tech)));
        assert!(asked(&mut rx), "expanded: now in view");
    }

    // ---- toggling ----

    #[test]
    fn a_header_click_toggles_the_section_and_saves() {
        let (mut app, _rx) = space_app(None);
        let key = SectionKey::SubSpace(rid("voice"));
        let _ = app.update(Message::Spaces(SpacesMessage::ToggleSection(key.clone())));
        assert!(drawn_names(&app).iter().all(|l| l != "v1"));
        let layout = app.sidebar.layout().expect("a layout");
        assert_eq!(layout.spaces[0].collapsed, vec![key.clone()]);
        let _ = app.update(Message::Spaces(SpacesMessage::ToggleSection(key)));
        assert!(drawn_names(&app).iter().any(|l| l == "v1"));
    }

    // ---- the options cards: right-click, "⋯" and the menu flows (Task sbc.9) ----

    mod menu {
        use twicecat_core::api::sidebar::limits::MAX_CUSTOM_SECTIONS;

        use super::*;
        use crate::keys::Shortcut;
        use crate::sidebar_menu::{
            Entry, Purpose, SectionPick, SidebarMenu, SidebarMenuMessage as M, Target,
        };
        use crate::sidebar_store::Call;

        fn menu_app() -> (App, MemorySidebar, mpsc::Receiver<Command>) {
            let (mut app, rx) = space_app(None);
            let store = MemorySidebar::default();
            app.set_sidebar_store(Box::new(store.clone()));
            (app, store, rx)
        }

        fn send(app: &mut App, message: M) {
            let _ = app.update(Message::SidebarMenu(message));
        }

        fn saves(store: &MemorySidebar) -> usize {
            store
                .calls()
                .iter()
                .filter(|c| matches!(c, Call::Save(..)))
                .count()
        }

        fn record(app: &App) -> SpaceLayout {
            app.sidebar
                .layout()
                .and_then(|l| l.spaces.iter().find(|s| s.space == rid("sp")))
                .cloned()
                .expect("a record for the space")
        }

        fn entry(app: &mut App, entry: Entry) {
            send(app, M::Entry(entry));
        }

        fn name(app: &mut App, text: &str) {
            send(app, M::NameInput(text.to_owned()));
            send(app, M::NameSubmit);
        }

        fn make_section(app: &mut App, section: &str, rooms: &[&str]) {
            send(app, M::OpenRoom(rid(rooms[0])));
            send(app, M::Section(SectionPick::New));
            name(app, section);
            for room in &rooms[1..] {
                let id = record(app).custom.last().expect("a section").id;
                send(app, M::OpenRoom(rid(room)));
                send(app, M::Section(SectionPick::Existing(id)));
            }
            assert!(app.sidebar_menu.is_none());
        }

        #[test]
        fn a_right_click_or_the_dots_open_the_card_of_a_drawn_room_or_header() {
            let (mut app, _store, _rx) = menu_app();
            send(&mut app, M::OpenRoom(rid("chan1")));
            assert_eq!(
                app.sidebar_menu,
                Some(SidebarMenu::Card {
                    target: Target::Room {
                        space: rid("sp"),
                        room: rid("chan1")
                    },
                    error: None
                })
            );
            send(&mut app, M::Close);
            send(&mut app, M::OpenSection(SectionKey::SubSpace(rid("tech"))));
            assert_eq!(
                app.sidebar_menu,
                Some(SidebarMenu::Card {
                    target: Target::Section {
                        space: rid("sp"),
                        key: SectionKey::SubSpace(rid("tech"))
                    },
                    error: None
                })
            );
            send(&mut app, M::Close);
            // Not drawn: a section that is hidden, a room that is not here.
            send(&mut app, M::OpenSection(SectionKey::Starred));
            send(&mut app, M::OpenRoom(rid("nobody")));
            assert!(app.sidebar_menu.is_none());
            // A room in a collapsed section is not drawn.
            collapse(&mut app, SectionKey::Direct);
            send(&mut app, M::OpenRoom(rid("dm1")));
            assert!(app.sidebar_menu.is_none());
        }

        #[test]
        fn no_card_opens_in_the_flat_list_or_while_customizing_is_off_or_a_layer_is_up() {
            let (mut app, _store, _rx) = menu_app();
            let _ = app.update(Message::SearchChanged("chan".to_owned()));
            send(&mut app, M::OpenRoom(rid("chan1")));
            send(&mut app, M::OpenSection(SectionKey::Channels));
            assert!(app.sidebar_menu.is_none(), "a search keeps the flat list");
            let _ = app.update(Message::SearchChanged(String::new()));
            let (mut app2, _store2, _rx2) = menu_app();
            app2.switcher.open = true;
            send(&mut app2, M::OpenRoom(rid("chan1")));
            assert!(app2.sidebar_menu.is_none(), "the switcher is over the list");
            let (mut app3, _store3, _rx3) = menu_app();
            let _ = app3.update(Message::SidebarLoaded(SidebarAnswer::new(
                UserId::from("@a:x"),
                SidebarLoad::Newer,
            )));
            app3.sidebar.unload();
            send(&mut app3, M::OpenRoom(rid("chan1")));
            assert!(app3.sidebar_menu.is_none(), "no layout loaded");
        }

        #[test]
        fn star_on_a_rooms_card_toggles_its_star_and_saves_once() {
            let (mut app, store, _rx) = menu_app();
            send(&mut app, M::OpenRoom(rid("chan2")));
            entry(&mut app, Entry::Star);
            assert!(app.sidebar_menu.is_none(), "the card closes");
            assert_eq!(saves(&store), 1);
            assert_eq!(
                &drawn_names(&app)[..3],
                ["# Starred", "chan2", "# Channels"]
            );
            // The card of a starred room offers Unstar.
            send(&mut app, M::OpenRoom(rid("chan2")));
            entry(&mut app, Entry::Unstar);
            assert_eq!(saves(&store), 2);
            assert_eq!(drawn_names(&app)[0], "# Channels");
            assert!(drawn_names(&app).iter().all(|l| l != "# Starred"));
        }

        #[test]
        fn a_new_section_from_a_room_holds_the_room_and_survives_in_the_layout() {
            let (mut app, store, _rx) = menu_app();
            send(&mut app, M::OpenRoom(rid("chan2")));
            send(&mut app, M::Section(SectionPick::New));
            assert!(matches!(
                &app.sidebar_menu,
                Some(SidebarMenu::Name(d)) if matches!(d.purpose, Purpose::NewSection { room: Some(_), .. })
            ));
            name(&mut app, "  Ops ");
            assert!(app.sidebar_menu.is_none());
            assert_eq!(saves(&store), 1);
            let out = drawn_names(&app);
            let at = out.iter().position(|l| l == "# Ops").expect("Ops header");
            assert_eq!(out[at + 1], "chan2");
            let saved = record(&app);
            assert_eq!(saved.custom[0].name.as_str(), "Ops");
            assert_eq!(saved.custom[0].rooms, [rid("chan2")]);
            // Ops sits before Direct messages (its default place).
            assert!(
                at < out
                    .iter()
                    .position(|l| l == "# Direct messages")
                    .unwrap_or(usize::MAX)
            );
        }

        #[test]
        fn a_new_section_from_a_header_is_empty_and_shows_its_hint() {
            let (mut app, store, _rx) = menu_app();
            send(&mut app, M::OpenSection(SectionKey::Channels));
            entry(&mut app, Entry::NewSection);
            name(&mut app, "Later");
            assert_eq!(saves(&store), 1);
            let out = drawn_names(&app);
            let at = out.iter().position(|l| l == "# Later").expect("header");
            assert_eq!(out[at + 1], "~hint");
        }

        #[test]
        fn a_blank_or_bad_section_name_keeps_the_dialog_open_with_the_reason() {
            let (mut app, store, _rx) = menu_app();
            send(&mut app, M::OpenSection(SectionKey::Channels));
            entry(&mut app, Entry::NewSection);
            for bad in ["", "   ", &"x".repeat(33), "a\nb"] {
                name(&mut app, bad);
                match &app.sidebar_menu {
                    Some(SidebarMenu::Name(d)) => assert!(d.error.is_some(), "{bad:?}"),
                    other => panic!("the dialog closed: {other:?}"),
                }
            }
            assert_eq!(saves(&store), 0);
            send(&mut app, M::Escape);
            assert!(app.sidebar_menu.is_none());
        }

        #[test]
        fn rename_keeps_the_rooms_and_move_down_in_a_section_moves_only_inside_it() {
            let (mut app, store, _rx) = menu_app();
            make_section(&mut app, "Ops", &["chan1", "chan2"]);
            assert_eq!(saves(&store), 2);
            let before = record(&app);
            // Move down on chan1 (the first of the section).
            send(&mut app, M::OpenRoom(rid("chan1")));
            entry(&mut app, Entry::MoveDown);
            assert_eq!(saves(&store), 3);
            let after = record(&app);
            assert_eq!(after.custom[0].rooms, [rid("chan2"), rid("chan1")]);
            assert_eq!(after.order, before.order);
            assert_eq!(after.starred, before.starred);
            assert_eq!(after.collapsed, before.collapsed);
            // The last room cannot go down: no save.
            send(&mut app, M::OpenRoom(rid("chan1")));
            entry(&mut app, Entry::MoveDown);
            assert_eq!(saves(&store), 3);
            // Rename: the rooms stay.
            send(&mut app, M::OpenSection(SectionKey::Custom(0)));
            entry(&mut app, Entry::Rename);
            match &app.sidebar_menu {
                Some(SidebarMenu::Name(d)) => assert_eq!(d.text, "Ops", "the field has the name"),
                other => panic!("no dialog: {other:?}"),
            }
            name(&mut app, "Work");
            assert_eq!(record(&app).custom[0].name.as_str(), "Work");
            assert_eq!(record(&app).custom[0].rooms, [rid("chan2"), rid("chan1")]);
        }

        #[test]
        fn move_down_on_a_headers_card_moves_the_section_over_the_drawn_ones() {
            let (mut app, store, _rx) = menu_app();
            make_section(&mut app, "Ops", &["chan1"]);
            let heads = |app: &App| -> Vec<String> {
                drawn_names(app)
                    .into_iter()
                    .filter(|l| l.starts_with('#'))
                    .collect()
            };
            // Channels, tech, voice, Ops, Direct messages.
            assert_eq!(
                heads(&app),
                [
                    "# Channels",
                    "# tech",
                    "# voice",
                    "# Ops",
                    "# Direct messages"
                ]
            );
            let saved = saves(&store);
            send(&mut app, M::OpenSection(SectionKey::Custom(0)));
            entry(&mut app, Entry::MoveUp);
            assert_eq!(saves(&store), saved + 1);
            assert_eq!(
                heads(&app),
                [
                    "# Channels",
                    "# tech",
                    "# Ops",
                    "# voice",
                    "# Direct messages"
                ]
            );
            // Down from the last drawn section does nothing and saves nothing.
            send(&mut app, M::OpenSection(SectionKey::Direct));
            entry(&mut app, Entry::MoveDown);
            assert_eq!(saves(&store), saved + 1);
            // Starred is hidden while empty: Ops moved to the top steps over
            // it and a later star puts Starred where the order says.
            for _ in 0..3 {
                send(&mut app, M::OpenSection(SectionKey::Custom(0)));
                entry(&mut app, Entry::MoveUp);
            }
            assert_eq!(heads(&app)[0], "# Ops");
        }

        #[test]
        fn delete_returns_the_rooms_to_their_default_sections() {
            let (mut app, store, _rx) = menu_app();
            make_section(&mut app, "Ops", &["chan1", "t1"]);
            send(&mut app, M::OpenSection(SectionKey::Custom(0)));
            entry(&mut app, Entry::Collapse);
            send(&mut app, M::OpenSection(SectionKey::Custom(0)));
            entry(&mut app, Entry::MoveUp);
            let saved = saves(&store);
            send(&mut app, M::OpenSection(SectionKey::Custom(0)));
            entry(&mut app, Entry::Delete);
            assert_eq!(saves(&store), saved + 1);
            let out = drawn_names(&app);
            assert!(out.iter().all(|l| l != "# Ops"));
            let at = |name: &str| out.iter().position(|l| l == name).expect("drawn");
            assert!(at("# Channels") < at("chan1") && at("chan1") < at("# tech"));
            assert!(at("# tech") < at("t1") && at("t1") < at("# voice"));
            let rec = record(&app);
            assert!(rec.custom.is_empty());
            assert!(!rec.order.contains(&SectionKey::Custom(0)));
            assert!(!rec.collapsed.contains(&SectionKey::Custom(0)));
        }

        #[test]
        fn back_to_default_and_collapse_and_expand_work_from_the_cards() {
            let (mut app, _store, _rx) = menu_app();
            make_section(&mut app, "Ops", &["chan1"]);
            send(&mut app, M::OpenRoom(rid("chan1")));
            entry(&mut app, Entry::BackToDefault);
            assert!(record(&app).custom[0].rooms.is_empty());
            assert_eq!(
                drawn_names(&app).iter().filter(|l| *l == "~hint").count(),
                1
            );
            send(&mut app, M::OpenSection(SectionKey::Channels));
            entry(&mut app, Entry::Collapse);
            assert!(record(&app).collapsed.contains(&SectionKey::Channels));
            send(&mut app, M::OpenSection(SectionKey::Channels));
            entry(&mut app, Entry::Expand);
            assert!(!record(&app).collapsed.contains(&SectionKey::Channels));
        }

        #[test]
        fn a_21st_section_is_refused_with_the_reason_and_the_dialog_stays() {
            let (mut app, store, _rx) = menu_app();
            for n in 0..MAX_CUSTOM_SECTIONS {
                send(&mut app, M::OpenSection(SectionKey::Channels));
                entry(&mut app, Entry::NewSection);
                name(&mut app, &format!("S{n}"));
                assert!(app.sidebar_menu.is_none(), "section {n}");
            }
            let saved = saves(&store);
            send(&mut app, M::OpenSection(SectionKey::Channels));
            entry(&mut app, Entry::NewSection);
            name(&mut app, "One more");
            match &app.sidebar_menu {
                Some(SidebarMenu::Name(d)) => {
                    assert_eq!(d.error, Some("The sidebar layout is full"));
                }
                other => panic!("the dialog closed: {other:?}"),
            }
            assert_eq!(saves(&store), saved);
            assert_eq!(record(&app).custom.len(), MAX_CUSTOM_SECTIONS);
        }

        #[test]
        fn esc_closes_the_card_and_a_card_for_a_room_that_left_closes_itself() {
            let (mut app, _store, _rx) = menu_app();
            send(&mut app, M::OpenRoom(rid("chan1")));
            assert!(app.sidebar_menu.is_some());
            let _ = app.update(Message::Shortcut {
                shortcut: Shortcut::Escape,
                captured: false,
            });
            assert!(app.sidebar_menu.is_none());
            send(&mut app, M::OpenRoom(rid("chan1")));
            let rest: Vec<RoomSummary> = rooms()
                .into_iter()
                .filter(|r| r.id != rid("chan1"))
                .collect();
            feed(
                &mut app,
                CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: rest }]),
            );
            assert!(app.sidebar_menu.is_none(), "the room is not in the list");
            // A card of a section closes when its section is collapsed away
            // by a search that makes the list flat.
            send(&mut app, M::OpenSection(SectionKey::Channels));
            let _ = app.update(Message::SearchChanged("x".to_owned()));
            let _ = app.update(Message::Spaces(SpacesMessage::Select(None)));
            assert!(app.sidebar_menu.is_none());
        }
    }

    // ---- dragging rooms and sections (Task sbc.10) ----

    mod dragging {
        use iced::Point;

        use super::*;
        use crate::drag::Zone;
        use crate::keys::Shortcut;
        use crate::list_drop::ListKey;
        use crate::sidebar_menu::{Entry, SectionPick, SidebarMenuMessage as M};
        use crate::sidebar_store::Call;
        use crate::spaces::SpacesMessage as S;

        fn drag_app() -> (App, MemorySidebar, mpsc::Receiver<Command>) {
            let (mut app, rx) = space_app(None);
            let store = MemorySidebar::default();
            app.set_sidebar_store(Box::new(store.clone()));
            (app, store, rx)
        }

        /// An app whose layout has "Ops" (0) holding chan1 and t2 in that order,
        /// "Later" (1) empty, and starred1 starred. chan2 and t1 are free.
        fn arranged() -> (App, MemorySidebar, mpsc::Receiver<Command>) {
            let (mut app, store, rx) = drag_app();
            let label = |t: &str| SidebarLabel::try_from(t).expect("a name");
            for op in [
                Op::NewSection {
                    space: rid("sp"),
                    name: label("Ops"),
                    room: Some(rid("chan1")),
                },
                Op::MoveRoomToSection {
                    space: rid("sp"),
                    room: rid("t2"),
                    section: 0,
                },
                Op::NewSection {
                    space: rid("sp"),
                    name: label("Later"),
                    room: None,
                },
                Op::Star {
                    space: rid("sp"),
                    room: rid("starred1"),
                },
            ] {
                assert!(app.apply_sidebar(op).is_ok());
            }
            (app, store, rx)
        }

        fn saves(store: &MemorySidebar) -> usize {
            store
                .calls()
                .iter()
                .filter(|c| matches!(c, Call::Save(..)))
                .count()
        }

        fn record(app: &App) -> SpaceLayout {
            app.sidebar
                .layout()
                .and_then(|l| l.spaces.iter().find(|s| s.space == rid("sp")))
                .cloned()
                .expect("a record for the space")
        }

        fn send(app: &mut App, message: S) {
            let _ = app.update(Message::Spaces(message));
        }

        fn menu(app: &mut App, message: M) {
            let _ = app.update(Message::SidebarMenu(message));
        }

        fn room(n: &str) -> ListKey {
            ListKey::Room(rid(n))
        }

        fn head(key: SectionKey) -> ListKey {
            ListKey::Header(key)
        }

        /// Presses `item` and moves 30 px: a drag is running.
        fn start(app: &mut App, item: &ListKey) {
            send(app, S::ListEnter(item.clone()));
            send(app, S::ListPress(item.clone()));
            send(app, S::PointerMoved(Point::new(10.0, 10.0)));
            send(app, S::PointerMoved(Point::new(40.0, 10.0)));
            assert!(app.list_drag.is_dragging());
        }

        /// Drags `item` to `target` and lets go.
        fn drag(app: &mut App, item: &ListKey, target: &ListKey, zone: Zone) {
            start(app, item);
            send(app, S::ListOver(target.clone(), zone));
            send(app, S::PointerReleased);
            assert!(!app.list_drag.is_active());
        }

        /// How the drop of every kind of target compares with the card.
        #[test]
        fn every_kind_of_drop_gives_the_layout_of_the_matching_card_operation() {
            type Cards = fn(&mut App);
            type Row = (&'static str, ListKey, ListKey, Zone, Cards);
            let up_to_first = |app: &mut App| {
                // Direct messages up, one card Move up at a time.
                for _ in 0..8 {
                    menu(app, M::OpenSection(SectionKey::Direct));
                    menu(app, M::Entry(Entry::MoveUp));
                }
            };
            let table: Vec<Row> = vec![
                (
                    "a room onto Starred",
                    room("chan2"),
                    head(SectionKey::Starred),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("chan2")));
                        menu(app, M::Entry(Entry::Star));
                    },
                ),
                (
                    "a room onto a section of the user's own",
                    room("t1"),
                    head(SectionKey::Custom(0)),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("t1")));
                        menu(app, M::Section(SectionPick::Existing(0)));
                    },
                ),
                (
                    "a starred room onto a section of the user's own",
                    room("starred1"),
                    head(SectionKey::Custom(1)),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("starred1")));
                        menu(app, M::Section(SectionPick::Existing(1)));
                    },
                ),
                (
                    "a room onto an empty section's hint (its header)",
                    room("chan2"),
                    head(SectionKey::Custom(1)),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("chan2")));
                        menu(app, M::Section(SectionPick::Existing(1)));
                    },
                ),
                (
                    "a room of a section back onto Channels",
                    room("chan1"),
                    head(SectionKey::Channels),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("chan1")));
                        menu(app, M::Entry(Entry::BackToDefault));
                    },
                ),
                (
                    "a starred room back onto its default section",
                    room("starred1"),
                    head(SectionKey::Channels),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("starred1")));
                        menu(app, M::Entry(Entry::Unstar));
                    },
                ),
                (
                    "a room of a section back onto its sub-space",
                    room("t2"),
                    head(SectionKey::SubSpace(rid("tech"))),
                    Zone::Into,
                    |app| {
                        menu(app, M::OpenRoom(rid("t2")));
                        menu(app, M::Entry(Entry::BackToDefault));
                    },
                ),
                (
                    "a room before the first row of a section of the user's own",
                    room("chan2"),
                    room("chan1"),
                    Zone::Before,
                    |app| {
                        menu(app, M::OpenRoom(rid("chan2")));
                        menu(app, M::Section(SectionPick::Existing(0)));
                        for _ in 0..2 {
                            menu(app, M::OpenRoom(rid("chan2")));
                            menu(app, M::Entry(Entry::MoveUp));
                        }
                    },
                ),
                (
                    "a room between two rows of a section of the user's own",
                    room("chan2"),
                    room("chan1"),
                    Zone::After,
                    |app| {
                        menu(app, M::OpenRoom(rid("chan2")));
                        menu(app, M::Section(SectionPick::Existing(0)));
                        menu(app, M::OpenRoom(rid("chan2")));
                        menu(app, M::Entry(Entry::MoveUp));
                    },
                ),
                (
                    "a room of a section after the last row of the section",
                    room("chan1"),
                    room("t2"),
                    Zone::After,
                    |app| {
                        menu(app, M::OpenRoom(rid("chan1")));
                        menu(app, M::Entry(Entry::MoveDown));
                    },
                ),
                (
                    "a section header before the first header",
                    head(SectionKey::Direct),
                    head(SectionKey::Starred),
                    Zone::Before,
                    up_to_first,
                ),
                (
                    "a section header after the next one",
                    head(SectionKey::Custom(0)),
                    head(SectionKey::Custom(1)),
                    Zone::After,
                    |app| {
                        menu(app, M::OpenSection(SectionKey::Custom(0)));
                        menu(app, M::Entry(Entry::MoveDown));
                    },
                ),
            ];
            for (what, item, target, zone, cards) in table {
                let by_drag = {
                    let (mut app, store, _rx) = arranged();
                    let before = record(&app);
                    let saved = saves(&store);
                    drag(&mut app, &item, &target, zone);
                    assert_ne!(record(&app), before, "{what}: the drop did something");
                    assert_eq!(saves(&store), saved + 1, "{what}: one change, one save");
                    record(&app)
                };
                let by_card = {
                    let (mut app, store, _rx) = arranged();
                    let saved = saves(&store);
                    cards(&mut app);
                    assert!(saves(&store) > saved, "{what}: the card changed it");
                    record(&app)
                };
                assert_eq!(by_drag, by_card, "{what}");
            }
        }

        #[test]
        fn a_target_that_is_not_the_rooms_default_section_is_refused_and_drops_nothing() {
            let (mut app, store, _rx) = arranged();
            let before = record(&app);
            let saved = saves(&store);
            start(&mut app, &room("chan1"));
            send(&mut app, S::ListOver(head(SectionKey::Direct), Zone::Into));
            assert!(app.list_drag.target().is_none(), "no target");
            assert_eq!(
                app.list_drag.refused().map(|t| t.item.clone()),
                Some(head(SectionKey::Direct)),
                "drawn as not allowed"
            );
            send(&mut app, S::PointerReleased);
            assert_eq!(record(&app), before);
            assert_eq!(saves(&store), saved);
            assert!(!app.list_drag.is_active());
            // Leaving the place takes the refusal away.
            start(&mut app, &room("chan1"));
            send(&mut app, S::ListOver(head(SectionKey::Direct), Zone::Into));
            send(&mut app, S::ListExit(head(SectionKey::Direct)));
            assert!(app.list_drag.refused().is_none());
            // The sub-space header is refused for a Channels room too.
            send(
                &mut app,
                S::ListOver(head(SectionKey::SubSpace(rid("tech"))), Zone::Into),
            );
            assert!(app.list_drag.refused().is_some());
            send(&mut app, S::PointerReleased);
            assert_eq!(saves(&store), saved);
        }

        #[test]
        fn a_press_and_release_with_no_movement_is_still_a_click() {
            let (mut app, store, mut rx) = arranged();
            drain(&mut rx);
            // A row: opens the room, even after a wobble under 4 px.
            send(&mut app, S::ListEnter(room("chan2")));
            send(&mut app, S::ListPress(room("chan2")));
            send(&mut app, S::PointerMoved(Point::new(10.0, 10.0)));
            send(&mut app, S::PointerMoved(Point::new(12.0, 11.0)));
            send(&mut app, S::ListRelease(room("chan2")));
            send(&mut app, S::PointerReleased);
            assert_eq!(app.selected, Some(rid("chan2")));
            assert!(
                drain(&mut rx)
                    .iter()
                    .any(|c| matches!(c, Command::SelectRoom(id) if *id == rid("chan2")))
            );
            // A header: collapses it, and again expands it.
            let saved = saves(&store);
            for expect in [true, false] {
                let key = head(SectionKey::Direct);
                send(&mut app, S::ListPress(key.clone()));
                send(&mut app, S::ListRelease(key));
                send(&mut app, S::PointerReleased);
                assert_eq!(record(&app).collapsed.contains(&SectionKey::Direct), expect);
            }
            assert_eq!(saves(&store), saved + 2);
        }

        #[test]
        fn a_drag_is_no_click() {
            let (mut app, store, mut rx) = arranged();
            drain(&mut rx);
            start(&mut app, &room("chan2"));
            send(&mut app, S::ListRelease(room("chan2")));
            send(&mut app, S::PointerReleased);
            assert_ne!(app.selected, Some(rid("chan2")), "nothing opened");
            assert!(drain(&mut rx).is_empty());
            let saved = saves(&store);
            let key = head(SectionKey::Direct);
            start(&mut app, &key);
            send(&mut app, S::ListRelease(key));
            send(&mut app, S::PointerReleased);
            assert!(!record(&app).collapsed.contains(&SectionKey::Direct));
            assert_eq!(saves(&store), saved);
        }

        #[test]
        fn in_the_flat_list_or_without_a_layout_nothing_is_draggable() {
            // A search makes the list flat.
            let (mut app, _store, _rx) = arranged();
            let _ = app.update(Message::SearchChanged("chan".to_owned()));
            assert!(!app.sections_shown());
            send(&mut app, S::ListPress(room("chan2")));
            send(&mut app, S::PointerMoved(Point::new(0.0, 0.0)));
            send(&mut app, S::PointerMoved(Point::new(50.0, 50.0)));
            assert!(!app.list_drag.is_dragging());
            // So does the People tab.
            let (mut app, _store, _rx) = arranged();
            let _ = app.update(Message::TabSelected(RoomListTab::People));
            send(&mut app, S::ListPress(room("dm1")));
            send(&mut app, S::PointerMoved(Point::new(0.0, 0.0)));
            send(&mut app, S::PointerMoved(Point::new(50.0, 50.0)));
            assert!(!app.list_drag.is_dragging());
            // A layout that is protected cannot be arranged.
            let (mut app, _store, _rx) = drag_app();
            let _ = app.update(Message::SidebarLoaded(SidebarAnswer::new(
                UserId::from("@a:x"),
                SidebarLoad::Newer,
            )));
            app.sidebar.unload();
            send(&mut app, S::ListPress(room("chan2")));
            send(&mut app, S::PointerMoved(Point::new(0.0, 0.0)));
            send(&mut app, S::PointerMoved(Point::new(50.0, 50.0)));
            assert!(!app.list_drag.is_dragging());
        }

        #[test]
        fn esc_and_losing_the_focus_cancel_and_save_nothing() {
            for cancel in ["esc", "focus", "second press"] {
                let (mut app, store, _rx) = arranged();
                let saved = saves(&store);
                start(&mut app, &room("chan2"));
                send(
                    &mut app,
                    S::ListOver(head(SectionKey::Custom(0)), Zone::Into),
                );
                assert!(app.list_drag.target().is_some());
                match cancel {
                    "esc" => {
                        let _ = app.update(Message::Shortcut {
                            shortcut: Shortcut::Escape,
                            captured: false,
                        });
                    }
                    "focus" => {
                        let _ = app.update(Message::WindowFocus(false));
                    }
                    _ => send(&mut app, S::PointerPressed),
                }
                assert!(!app.list_drag.is_active(), "{cancel}");
                send(&mut app, S::PointerReleased);
                assert_eq!(saves(&store), saved, "{cancel}");
            }
        }

        #[test]
        fn a_release_with_no_target_or_off_the_target_saves_nothing() {
            let (mut app, store, _rx) = arranged();
            let saved = saves(&store);
            start(&mut app, &room("chan2"));
            send(&mut app, S::PointerReleased);
            start(&mut app, &room("chan2"));
            send(
                &mut app,
                S::ListOver(head(SectionKey::Custom(0)), Zone::Into),
            );
            send(&mut app, S::ListExit(head(SectionKey::Custom(0))));
            send(&mut app, S::PointerReleased);
            // Onto itself, and to where it already is.
            start(&mut app, &room("chan1"));
            send(&mut app, S::ListOver(room("chan1"), Zone::Before));
            assert!(app.list_drag.target().is_none());
            send(
                &mut app,
                S::ListOver(head(SectionKey::Custom(0)), Zone::Into),
            );
            assert!(app.list_drag.target().is_none(), "it is already there");
            send(&mut app, S::PointerReleased);
            assert_eq!(saves(&store), saved);
        }

        #[test]
        fn the_same_target_is_planned_once_until_the_pointer_leaves_or_things_change() {
            use crate::list_drop::plan_runs;
            let (mut app, _store, _rx) = arranged();
            start(&mut app, &room("chan2"));
            let runs = plan_runs();
            let over = |app: &mut App| {
                send(app, S::ListOver(head(SectionKey::Custom(0)), Zone::Into));
            };
            for _ in 0..5 {
                over(&mut app);
            }
            assert_eq!(plan_runs() - runs, 1, "five moves over it, one plan");
            send(&mut app, S::ListExit(head(SectionKey::Custom(0))));
            over(&mut app);
            assert_eq!(plan_runs() - runs, 2, "after leaving and coming back");
            // A change of the list forgets what was worked out.
            feed(
                &mut app,
                CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: rooms() }]),
            );
            over(&mut app);
            assert_eq!(plan_runs() - runs, 3, "after the rooms changed");
        }

        #[test]
        fn the_window_listener_runs_only_while_the_button_is_down_on_a_row_or_header() {
            use iced::advanced::subscription::into_recipes;
            let (mut app, _store, _rx) = arranged();
            let count = |app: &App| into_recipes(app.rail_drag_subscription()).len();
            assert_eq!(count(&app), 0, "idle");
            send(&mut app, S::ListEnter(room("chan2")));
            assert_eq!(count(&app), 0, "hover alone");
            send(&mut app, S::ListPress(room("chan2")));
            assert_eq!(count(&app), 1, "pressed");
            send(&mut app, S::PointerReleased);
            assert_eq!(count(&app), 0, "released");
        }

        #[test]
        fn a_row_that_leaves_the_list_ends_its_drag_and_hover() {
            let (mut app, store, _rx) = arranged();
            let saved = saves(&store);
            send(&mut app, S::ListEnter(room("chan2")));
            start(&mut app, &room("chan2"));
            send(
                &mut app,
                S::ListOver(head(SectionKey::Custom(0)), Zone::Into),
            );
            let rest: Vec<RoomSummary> = rooms()
                .into_iter()
                .filter(|r| r.id != rid("chan2"))
                .collect();
            feed(
                &mut app,
                CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: rest }]),
            );
            assert!(!app.list_drag.is_active());
            assert!(app.list_hover.is_none());
            send(&mut app, S::PointerReleased);
            assert_eq!(saves(&store), saved);
        }

        #[test]
        fn a_card_that_opens_during_a_press_ends_it_and_a_list_press_ends_a_rail_press() {
            let (mut app, _store, _rx) = arranged();
            send(&mut app, S::ListPress(room("chan2")));
            menu(&mut app, M::OpenRoom(rid("chan2")));
            assert!(app.sidebar_menu.is_some());
            assert!(!app.list_drag.is_active());
            menu(&mut app, M::Close);
            send(&mut app, S::Press(crate::spaces::RailKey::Space(rid("sp"))));
            send(&mut app, S::ListPress(room("chan2")));
            assert!(!app.rail_drag.is_active());
            assert!(app.list_drag.is_active());
        }

        #[test]
        fn pointer_moves_over_an_unchanged_list_rebuild_nothing_but_a_real_change_settles() {
            let (mut app, _store, _rx) = arranged();
            send(&mut app, S::ListEnter(room("chan2")));
            start(&mut app, &room("chan2"));
            send(
                &mut app,
                S::ListOver(head(SectionKey::Custom(0)), Zone::Into),
            );
            let builds = build_runs();
            for n in 0..50 {
                send(&mut app, S::PointerMoved(Point::new(40.0 + n as f32, 10.0)));
                send(&mut app, S::ListEnter(room("chan1")));
                send(&mut app, S::ListExit(room("chan1")));
                send(&mut app, S::Exit(crate::spaces::RailKey::Home));
            }
            assert_eq!(
                build_runs(),
                builds,
                "no rebuild for pointer moves or hover"
            );
            assert!(app.list_drag.is_dragging());
            // A scroll asks for avatars (its own build) and is not settled.
            let _ = app.update(Message::RoomListScrolled {
                offset_y: 10.0,
                viewport_h: 500.0,
            });
            // A room that leaves still ends the drag.
            let rest: Vec<RoomSummary> = rooms()
                .into_iter()
                .filter(|r| r.id != rid("chan2"))
                .collect();
            feed(
                &mut app,
                CoreEvent::RoomListChanged(vec![ListDiff::Reset { values: rest }]),
            );
            assert!(build_runs() > builds, "a change of the list settles");
            assert!(!app.list_drag.is_active());
        }

        #[test]
        fn a_rail_press_ends_a_tracked_list_press() {
            let (mut app, _store, _rx) = arranged();
            send(&mut app, S::ListPress(room("chan2")));
            send(&mut app, S::Press(crate::spaces::RailKey::Space(rid("sp"))));
            assert!(!app.list_drag.is_active());
            assert!(app.rail_drag.is_active());
        }

        #[test]
        fn a_click_works_again_after_a_cancelled_drag_and_after_the_window_press_cancel() {
            let (mut app, store, mut rx) = arranged();
            let click_room = |app: &mut App| {
                send(app, S::ListEnter(room("chan2")));
                send(app, S::ListPress(room("chan2")));
                send(app, S::ListRelease(room("chan2")));
                send(app, S::PointerReleased);
            };
            for cancel in ["esc", "focus", "second press"] {
                app.selected = None;
                drain(&mut rx);
                start(&mut app, &room("chan2"));
                match cancel {
                    "esc" => {
                        let _ = app.update(Message::Shortcut {
                            shortcut: Shortcut::Escape,
                            captured: false,
                        });
                    }
                    "focus" => {
                        let _ = app.update(Message::WindowFocus(false));
                    }
                    _ => send(&mut app, S::PointerPressed),
                }
                assert!(!app.list_drag.is_active(), "{cancel}");
                click_room(&mut app);
                assert_eq!(app.selected, Some(rid("chan2")), "{cancel}: the row opens");
            }
            // A press cancelled by the window press, then a header click.
            send(&mut app, S::ListPress(head(SectionKey::Direct)));
            send(&mut app, S::PointerPressed);
            let saved = saves(&store);
            send(&mut app, S::ListPress(head(SectionKey::Direct)));
            send(&mut app, S::ListRelease(head(SectionKey::Direct)));
            send(&mut app, S::PointerReleased);
            assert!(record(&app).collapsed.contains(&SectionKey::Direct));
            assert_eq!(saves(&store), saved + 1);
        }
    }
}
