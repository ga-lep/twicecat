//! What a drop in a space's sectioned room list does (sidebar plan, Task 10):
//! the operations of the options cards, in the order that makes the drop.
//!
//! Like `rail_drop` for the rail, dragging adds no way to change the layout
//! that the cards of Task 9 do not have. A drop is worked out as a list of
//! [`Op`]s, each one a card produces (`Star`, `Unstar`, `Move to section`,
//! `Back to its default section`, `Move up`/`Move down` of a room inside a
//! section of the user's own, `Move up`/`Move down` of a section); the list is
//! done as one change and saved once.
//!
//! What each drop means:
//! - a room onto the Starred header: it is starred;
//! - a room onto the header of a section of the user's own (or onto its "Drop
//!   or move rooms here" hint): it goes to the end of that section;
//! - a room onto the header of a sub-space, Channels or Direct messages: it
//!   goes back to its default section, and only when that is its own default
//!   section. Anywhere else the target is refused (drawn as not allowed): the
//!   app has no way to put a room in a section that is not its default one;
//! - a room between two rows of a section of the user's own: it goes there,
//!   from wherever it was;
//! - a section header onto another header: the section goes before or after
//!   it among the sections drawn.
//!
//! Nothing here logs, and no `Debug` shows a room ID.

use std::fmt;

use twicecat_core::api::{RoomId, SectionKey, SidebarLayout};

use crate::drag::{Target, Zone};
use crate::sections::{Line, drawn_sections, spot_of};
use crate::sidebar::{self, Dir, Op};

#[cfg(test)]
thread_local! {
    /// How many times [`plan`] ran on this thread (tests count evaluations).
    static PLANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times [`plan`] ran on this thread.
#[cfg(test)]
pub(crate) fn plan_runs() -> usize {
    PLANS.with(std::cell::Cell::get)
}

/// What can be dragged in the sectioned list, and what it can be dropped on.
/// `Debug` hides the room ID.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ListKey {
    /// A room row.
    Room(RoomId),
    /// The header of a section (also the "Drop or move rooms here" hint of an
    /// empty section of the user's own).
    Header(SectionKey),
}

impl fmt::Debug for ListKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Room(_) => f.write_str("Room(<redacted>)"),
            Self::Header(key) => f.debug_tuple("Header").field(key).finish(),
        }
    }
}

/// What dropping would do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing: onto itself, to where it is, or onto something that takes no
    /// drop. The place is no target.
    Nothing,
    /// The target is refused: the place is drawn as not allowed.
    Refused,
    /// The operations to do, in order, as one change.
    Ops(Vec<Op>),
}

/// The zone of height `y` inside a slot `height` tall, by halves: rows and
/// headers have a top half (before) and a bottom half (after), and no middle.
pub fn halves(y: f32, height: f32) -> Zone {
    if height <= 0.0 || y < height / 2.0 {
        Zone::Before
    } else {
        Zone::After
    }
}

/// Works out dropping `item` on `target` in `space`'s sectioned list.
/// `lines` are the lines drawn, and `defaults` the same rooms placed with
/// no layout (every room in its default section).
pub fn plan(
    space: &RoomId,
    layout: &SidebarLayout,
    lines: &[Line<'_>],
    defaults: &[Line<'_>],
    item: &ListKey,
    target: &Target<ListKey>,
) -> Verdict {
    #[cfg(test)]
    PLANS.with(|n| n.set(n.get() + 1));
    if *item == target.item {
        return Verdict::Nothing;
    }
    match (item, &target.item) {
        (ListKey::Room(room), ListKey::Header(key)) => {
            room_onto_header(space, lines, defaults, room, key)
        }
        (ListKey::Room(room), ListKey::Room(near)) => {
            room_beside(space, layout, lines, room, near, target.zone)
        }
        (ListKey::Header(key), ListKey::Header(over)) => {
            header_over_header(space, layout, lines, key, over, target.zone)
        }
        (ListKey::Header(_), ListKey::Room(_)) => Verdict::Nothing,
    }
}

/// A room onto a header (any part of it).
fn room_onto_header(
    space: &RoomId,
    lines: &[Line<'_>],
    defaults: &[Line<'_>],
    room: &RoomId,
    key: &SectionKey,
) -> Verdict {
    let (Some(now), true) = (spot_of(lines, room), drawn_sections(lines).contains(key)) else {
        return Verdict::Nothing;
    };
    let (space, room) = (space.clone(), room.clone());
    let op = match key {
        SectionKey::Starred if now.section == SectionKey::Starred => return Verdict::Nothing,
        SectionKey::Starred => Op::Star { space, room },
        SectionKey::Custom(_) if now.section == *key => return Verdict::Nothing,
        SectionKey::Custom(section) => Op::MoveRoomToSection {
            space,
            room,
            section: *section,
        },
        // A built-in section takes a room only back: it is the room's default.
        SectionKey::Channels | SectionKey::Direct | SectionKey::SubSpace(_) => {
            let home = spot_of(defaults, &room).map(|spot| spot.section);
            if home.as_ref() != Some(key) {
                return Verdict::Refused;
            }
            match now.section {
                SectionKey::Starred => Op::Unstar { space, room },
                SectionKey::Custom(_) => Op::RoomToDefault { space, room },
                SectionKey::Channels | SectionKey::Direct | SectionKey::SubSpace(_) => {
                    return Verdict::Nothing;
                }
            }
        }
    };
    Verdict::Ops(vec![op])
}

/// A room onto a row of a section of the user's own: before or after it. The
/// card has Move to section, then Move up / Move down for as many steps as
/// it takes.
fn room_beside(
    space: &RoomId,
    layout: &SidebarLayout,
    lines: &[Line<'_>],
    room: &RoomId,
    near: &RoomId,
    zone: Zone,
) -> Verdict {
    let (Some(now), Some(there)) = (spot_of(lines, room), spot_of(lines, near)) else {
        return Verdict::Nothing;
    };
    let SectionKey::Custom(section) = there.section else {
        return Verdict::Nothing;
    };
    if zone == Zone::Into {
        return Verdict::Nothing;
    }
    let mut sim = layout.clone();
    let mut ops = Vec::new();
    if now.section != there.section {
        let op = Op::MoveRoomToSection {
            space: space.clone(),
            room: room.clone(),
            section,
        };
        if !matches!(sidebar::simulate(&mut sim, op.clone(), &[]), Ok(true)) {
            return Verdict::Nothing;
        }
        ops.push(op);
    }
    // The rooms of the section the list draws, and the one that is moving.
    let mut shown: Vec<RoomId> = there.rooms;
    if !shown.contains(room) {
        shown.push(room.clone());
    }
    for _ in 0..2 * shown.len() + 4 {
        let stored: Vec<RoomId> = sim
            .spaces
            .iter()
            .find(|record| record.space == *space)
            .and_then(|record| record.custom.iter().find(|c| c.id == section))
            .map(|c| {
                c.rooms
                    .iter()
                    .filter(|r| shown.contains(r))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let (Some(i), Some(t)) = (
            stored.iter().position(|r| r == room),
            stored.iter().position(|r| r == near),
        ) else {
            break;
        };
        let Some(dir) = step_toward(i, t, zone) else {
            break;
        };
        let op = Op::MoveRoomInSection {
            space: space.clone(),
            room: room.clone(),
            dir,
            drawn: stored,
        };
        if !matches!(sidebar::simulate(&mut sim, op.clone(), &[]), Ok(true)) {
            break;
        }
        ops.push(op);
    }
    verdict_of(ops)
}

/// A section header onto another header: before or after it.
fn header_over_header(
    space: &RoomId,
    layout: &SidebarLayout,
    lines: &[Line<'_>],
    key: &SectionKey,
    over: &SectionKey,
    zone: Zone,
) -> Verdict {
    let mut order = drawn_sections(lines);
    if zone == Zone::Into || !order.contains(key) || !order.contains(over) {
        return Verdict::Nothing;
    }
    let mut sim = layout.clone();
    let mut ops = Vec::new();
    for _ in 0..2 * order.len() + 4 {
        let (Some(i), Some(t)) = (
            order.iter().position(|k| k == key),
            order.iter().position(|k| k == over),
        ) else {
            break;
        };
        let Some(dir) = step_toward(i, t, zone) else {
            break;
        };
        let op = Op::MoveSection {
            space: space.clone(),
            key: key.clone(),
            dir,
            drawn: order.clone(),
        };
        match sidebar::simulate(&mut sim, op.clone(), &[]) {
            Ok(true) => {}
            Ok(false) => break,
            // The order cannot hold this many sections: not allowed.
            Err(_) => return Verdict::Refused,
        }
        let to = if dir == Dir::Up { i - 1 } else { i + 1 };
        order.swap(i, to);
        ops.push(op);
    }
    verdict_of(ops)
}

/// The step that brings the item at `i` closer to being right before (`Before`)
/// or right after (`After`) the one at `t`, or `None` when it is there.
fn step_toward(i: usize, t: usize, zone: Zone) -> Option<Dir> {
    match zone {
        Zone::Before if i + 1 == t => None,
        Zone::After if i == t + 1 => None,
        Zone::Into => None,
        Zone::Before | Zone::After if i < t => Some(Dir::Down),
        Zone::Before | Zone::After => Some(Dir::Up),
    }
}

fn verdict_of(ops: Vec<Op>) -> Verdict {
    if ops.is_empty() {
        Verdict::Nothing
    } else {
        Verdict::Ops(ops)
    }
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{
        CustomSection, RoomSummary, SidebarLabel, SpaceLayout, SpaceView, UserId,
    };

    use super::*;
    use crate::sections::{Spot, build};
    use crate::sidebar_menu::{Entry, SectionPick, room_op, section_op};

    fn rid(n: &str) -> RoomId {
        RoomId::from(format!("!{n}:x"))
    }

    fn room(n: &str) -> RoomSummary {
        RoomSummary::new(rid(n), n.to_owned())
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

    fn spaces() -> Vec<SpaceView> {
        vec![
            view("sp", 0, &["c1", "c2", "c3", "c4", "d1", "tech"]),
            view("tech", 1, &["t1", "t2", "tech"]),
        ]
    }

    fn rooms() -> Vec<RoomSummary> {
        let mut d1 = room("d1");
        d1.is_dm = true;
        let mut tech = room("tech");
        tech.is_space = true;
        vec![
            room("c1"),
            room("c2"),
            room("c3"),
            room("c4"),
            room("t1"),
            room("t2"),
            d1,
            tech,
        ]
    }

    fn custom(id: u16, name: &str, members: &[&str]) -> CustomSection {
        CustomSection {
            id,
            name: SidebarLabel::try_from(name).expect("a name"),
            rooms: members.iter().map(|r| rid(r)).collect(),
        }
    }

    /// The layout of the tests: Starred holds c3, "Ops" (0) holds c1 and c2 in
    /// that order, "Empty" (1) holds nothing. c4 stays in Channels.
    fn record() -> SpaceLayout {
        let mut rec = SpaceLayout::new(rid("sp"));
        rec.starred = vec![rid("c3")];
        rec.custom = vec![custom(0, "Ops", &["c1", "c2"]), custom(1, "Empty", &[])];
        rec
    }

    fn layout_of(rec: &SpaceLayout) -> SidebarLayout {
        let mut layout = SidebarLayout::new(UserId::from("@a:x"));
        layout.spaces = vec![rec.clone()];
        layout
    }

    /// Plans `item` on `target` against `rec`.
    fn planned(rec: &SpaceLayout, item: ListKey, target: ListKey, zone: Zone) -> Verdict {
        let spaces = spaces();
        let rooms = rooms();
        let lines = build(&spaces[0], &spaces, &rooms, Some(rec));
        let defaults = build(&spaces[0], &spaces, &rooms, None);
        plan(
            &rid("sp"),
            &layout_of(rec),
            &lines,
            &defaults,
            &item,
            &Target { item: target, zone },
        )
    }

    fn r(n: &str) -> ListKey {
        ListKey::Room(rid(n))
    }

    fn h(key: SectionKey) -> ListKey {
        ListKey::Header(key)
    }

    fn ops(verdict: Verdict) -> Vec<Op> {
        match verdict {
            Verdict::Ops(ops) => ops,
            other => panic!("no operations: {other:?}"),
        }
    }

    #[test]
    fn debug_hides_the_room_id() {
        assert_eq!(format!("{:?}", r("secret")), "Room(<redacted>)");
        assert!(format!("{:?}", h(SectionKey::Custom(3))).contains('3'));
    }

    #[test]
    fn halves_split_a_slot_in_two() {
        assert_eq!(halves(0.0, 56.0), Zone::Before);
        assert_eq!(halves(27.9, 56.0), Zone::Before);
        assert_eq!(halves(28.0, 56.0), Zone::After);
        assert_eq!(halves(1.0, 0.0), Zone::Before);
    }

    #[test]
    fn a_room_onto_the_starred_header_stars_it_and_a_starred_one_stays() {
        let rec = record();
        let verdict = planned(&rec, r("c1"), h(SectionKey::Starred), Zone::Into);
        assert_eq!(
            ops(verdict),
            [Op::Star {
                space: rid("sp"),
                room: rid("c1")
            }]
        );
        assert_eq!(
            planned(&rec, r("c3"), h(SectionKey::Starred), Zone::Into),
            Verdict::Nothing,
            "already starred"
        );
    }

    #[test]
    fn a_room_onto_a_custom_header_goes_to_the_end_of_that_section() {
        let rec = record();
        for target in [0, 1] {
            let verdict = planned(&rec, r("t1"), h(SectionKey::Custom(target)), Zone::Into);
            assert_eq!(
                ops(verdict),
                [Op::MoveRoomToSection {
                    space: rid("sp"),
                    room: rid("t1"),
                    section: target
                }]
            );
        }
        assert_eq!(
            planned(&rec, r("c1"), h(SectionKey::Custom(0)), Zone::Into),
            Verdict::Nothing,
            "already there"
        );
    }

    #[test]
    fn a_room_onto_a_default_header_goes_back_only_to_its_own_default_section() {
        let rec = record();
        // c1 is in Ops; its default is Channels.
        assert_eq!(
            ops(planned(&rec, r("c1"), h(SectionKey::Channels), Zone::Into)),
            [Op::RoomToDefault {
                space: rid("sp"),
                room: rid("c1")
            }]
        );
        // c3 is starred; its default is Channels: Unstar, as its card does.
        assert_eq!(
            ops(planned(&rec, r("c3"), h(SectionKey::Channels), Zone::Into)),
            [Op::Unstar {
                space: rid("sp"),
                room: rid("c3")
            }]
        );
        // Anywhere else it is refused.
        for (room, key) in [
            ("c1", SectionKey::Direct),
            ("c1", SectionKey::SubSpace(rid("tech"))),
            ("c3", SectionKey::Direct),
            ("t1", SectionKey::Channels),
            ("t1", SectionKey::Direct),
            ("d1", SectionKey::Channels),
            ("d1", SectionKey::SubSpace(rid("tech"))),
        ] {
            assert_eq!(
                planned(&rec, r(room), h(key.clone()), Zone::Into),
                Verdict::Refused,
                "{room} onto {key:?}"
            );
        }
        // Its own default section, where it already is: nothing to do.
        assert_eq!(
            planned(
                &rec,
                r("t1"),
                h(SectionKey::SubSpace(rid("tech"))),
                Zone::Into
            ),
            Verdict::Nothing
        );
        assert_eq!(
            planned(&rec, r("d1"), h(SectionKey::Direct), Zone::Into),
            Verdict::Nothing
        );
    }

    #[test]
    fn a_room_between_rows_of_a_custom_section_lands_there() {
        let rec = record();
        // From the sub-space: before c2.
        let verdict = planned(&rec, r("t1"), r("c2"), Zone::Before);
        assert_eq!(
            ops(verdict),
            [
                Op::MoveRoomToSection {
                    space: rid("sp"),
                    room: rid("t1"),
                    section: 0
                },
                Op::MoveRoomInSection {
                    space: rid("sp"),
                    room: rid("t1"),
                    dir: Dir::Up,
                    drawn: vec![rid("c1"), rid("c2"), rid("t1")]
                }
            ]
        );
        // After the last row: just the move to the section.
        let verdict = planned(&rec, r("t1"), r("c2"), Zone::After);
        assert_eq!(ops(verdict).len(), 1);
        // Inside the section: c2 before c1 is one step up; c1 after c2 is one
        // step down; c1 before c2 is already so.
        let up = ops(planned(&rec, r("c2"), r("c1"), Zone::Before));
        assert!(matches!(
            up.as_slice(),
            [Op::MoveRoomInSection { dir: Dir::Up, .. }]
        ));
        let down = ops(planned(&rec, r("c1"), r("c2"), Zone::After));
        assert!(matches!(
            down.as_slice(),
            [Op::MoveRoomInSection { dir: Dir::Down, .. }]
        ));
        assert_eq!(
            planned(&rec, r("c1"), r("c2"), Zone::Before),
            Verdict::Nothing
        );
        assert_eq!(
            planned(&rec, r("c2"), r("c1"), Zone::After),
            Verdict::Nothing
        );
        // A row of a built-in section takes no drop; nor does the middle.
        assert_eq!(
            planned(&rec, r("c1"), r("t2"), Zone::Before),
            Verdict::Nothing
        );
        assert_eq!(
            planned(&rec, r("t1"), r("c2"), Zone::Into),
            Verdict::Nothing
        );
    }

    #[test]
    fn a_section_onto_a_header_goes_before_or_after_it_among_the_drawn_ones() {
        use SectionKey::{Channels, Custom, Direct, Starred, SubSpace};
        let rec = record();
        // Drawn: Starred, Channels, tech, Ops, Empty, Direct.
        let tech = SubSpace(rid("tech"));
        let verdict = planned(&rec, h(Direct), h(Channels), Zone::Before);
        let steps = ops(verdict);
        assert_eq!(steps.len(), 4, "Direct steps up over Empty, Ops, tech");
        assert!(steps.iter().all(|op| matches!(
            op,
            Op::MoveSection {
                key: Direct,
                dir: Dir::Up,
                ..
            }
        )));
        // After the neighbour it is already behind is nothing.
        assert_eq!(
            planned(&rec, h(Channels), h(Starred), Zone::After),
            Verdict::Nothing
        );
        assert_eq!(
            planned(&rec, h(Channels), h(tech.clone()), Zone::Before),
            Verdict::Nothing
        );
        // One step down.
        let down = ops(planned(&rec, h(Custom(0)), h(Custom(1)), Zone::After));
        assert!(matches!(
            down.as_slice(),
            [Op::MoveSection { dir: Dir::Down, .. }]
        ));
        // A section onto a room takes no drop.
        assert_eq!(
            planned(&rec, h(Direct), r("c1"), Zone::Before),
            Verdict::Nothing
        );
    }

    #[test]
    fn nothing_is_planned_onto_itself_or_for_something_not_drawn() {
        let rec = record();
        assert_eq!(
            planned(&rec, r("c1"), r("c1"), Zone::Before),
            Verdict::Nothing
        );
        assert_eq!(
            planned(
                &rec,
                h(SectionKey::Direct),
                h(SectionKey::Direct),
                Zone::After
            ),
            Verdict::Nothing
        );
        // Not drawn: a room that is not in the list, a hidden section.
        assert_eq!(
            planned(&rec, r("nobody"), h(SectionKey::Custom(0)), Zone::Into),
            Verdict::Nothing
        );
        let plain = SpaceLayout::new(rid("sp"));
        assert_eq!(
            planned(&plain, r("c1"), h(SectionKey::Starred), Zone::Into),
            Verdict::Nothing,
            "Starred is hidden while empty"
        );
    }

    /// Every operation a drop gives is one that a card of Task 9 produces.
    #[test]
    fn every_dropped_operation_is_one_the_cards_produce() {
        let rec = record();
        let drawn = {
            let spaces = spaces();
            let rooms = rooms();
            drawn_sections(&build(&spaces[0], &spaces, &rooms, Some(&rec)))
        };
        let sp = rid("sp");
        let mut card: Vec<Op> = Vec::new();
        for room in ["c1", "c2", "c3", "c4", "t1", "t2", "d1"] {
            let spot = Spot {
                section: SectionKey::Channels,
                rooms: vec![rid("c1"), rid("c2"), rid(room)],
                index: 2,
            };
            for entry in [
                Entry::Star,
                Entry::Unstar,
                Entry::BackToDefault,
                Entry::MoveUp,
                Entry::MoveDown,
            ] {
                card.extend(room_op(&sp, &rid(room), entry, &spot));
            }
            for pick in [0, 1] {
                card.extend(crate::sidebar_menu::op_for_section_pick(
                    &sp,
                    &rid(room),
                    SectionPick::Existing(pick),
                ));
            }
        }
        for key in &drawn {
            for entry in [Entry::MoveUp, Entry::MoveDown] {
                card.extend(section_op(&sp, key, entry, &drawn));
            }
        }
        let kind = |op: &Op| std::mem::discriminant(op);
        let allowed: Vec<_> = card.iter().map(kind).collect();
        let mut seen = 0;
        let keys: Vec<ListKey> = ["c1", "c2", "c3", "c4", "t1", "t2", "d1"]
            .iter()
            .map(|n| r(n))
            .chain(drawn.iter().cloned().map(h))
            .collect();
        for item in &keys {
            for target in &keys {
                for zone in [Zone::Before, Zone::Into, Zone::After] {
                    if let Verdict::Ops(list) = planned(&rec, item.clone(), target.clone(), zone) {
                        for op in &list {
                            seen += 1;
                            assert!(allowed.contains(&kind(op)), "{op:?} is no card operation");
                        }
                    }
                }
            }
        }
        assert!(seen > 20, "the sweep covered the drops ({seen})");
    }
}
