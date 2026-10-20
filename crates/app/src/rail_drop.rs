//! What a drop on the rail does (sidebar plan, Task 7): the operations of
//! the options card, in the order that makes the drop.
//!
//! Dragging adds no way to change the layout that the options card of Task 6
//! does not have. A drop is worked out as a list of [`Op`]s, each one of
//! those the card produces (`Move up`, `Move down`, `Move to folder`, `New
//! folder`, `Remove from folder`): "before this one" is as many `Move up` or
//! `Move down` steps as it takes, and "make a folder with this space" is a
//! new folder around the target followed by moving the dragged space into it.
//! The list is done as one change, saved once. The tests below check, for
//! each kind of target, that the card's buttons in the same order give the
//! same layout.
//!
//! Nothing here logs, and no `Debug` shows a room ID.

use twicecat_core::api::{RailEntry, RoomId, SidebarLayout};

use crate::drag::{Target, Zone};
use crate::sidebar::{self, Dir, FolderTarget, Op};
use crate::spaces::RailKey;

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

/// The operations that make dropping `item` on `target` (empty when the drop
/// does nothing: onto itself, a folder into a folder, a move to where it
/// already is). `known` are the top-level spaces the core reports, as for
/// [`sidebar::SidebarModel::apply_known`].
///
/// What each drop means:
/// - before or after a top-level item: the dragged one goes there, out of its
///   folder first when it was in one;
/// - into a folder: the space goes to the end of the folder;
/// - into a space: a new folder is made around that space and the dragged
///   space goes into it;
/// - onto a space inside a folder: the dragged space joins that folder (or
///   is reordered inside it when it is from there);
/// - a folder can only be moved before or after another top-level item.
pub fn plan(
    layout: &SidebarLayout,
    known: &[RoomId],
    item: &RailKey,
    target: &Target<RailKey>,
) -> Vec<Op> {
    #[cfg(test)]
    PLANS.with(|n| n.set(n.get() + 1));
    let mut ops = Vec::new();
    if item == &target.item || *item == RailKey::Home || target.item == RailKey::Home {
        return ops;
    }
    let mut sim = layout.clone();
    let view = sidebar::materialized(layout, known);
    let (Some(item_at), Some(target_at)) =
        (place(&view, known, item), place(&view, known, &target.item))
    else {
        return ops;
    };
    let zone = target.zone;
    match (item, &target.item) {
        (RailKey::Folder(_), _) => {
            // A folder only moves among the top-level items.
            if matches!(target_at, Place::Nested(_)) || zone == Zone::Into {
                return ops;
            }
            reorder(&mut sim, known, item, target, &mut ops);
        }
        (RailKey::Space(id), RailKey::Space(t)) => match target_at {
            Place::Nested(g) => {
                if item_at == Place::Nested(g) {
                    if zone != Zone::Into {
                        reorder(&mut sim, known, item, target, &mut ops);
                    }
                } else {
                    ops.push(to_folder(id, g));
                }
            }
            Place::Top => match zone {
                Zone::Into => {
                    let around = Op::MoveToFolder {
                        id: t.clone(),
                        folder: FolderTarget::New(None),
                    };
                    if sidebar::simulate(&mut sim, around.clone(), known).is_err() {
                        return Vec::new();
                    }
                    ops.push(around);
                    let Some(made) = folder_of(&sim, t) else {
                        return Vec::new();
                    };
                    ops.push(to_folder(id, made));
                }
                Zone::Before | Zone::After => {
                    detach(&mut sim, known, id, item_at, &mut ops);
                    reorder(&mut sim, known, item, target, &mut ops);
                }
            },
        },
        (RailKey::Space(id), RailKey::Folder(g)) => match zone {
            Zone::Into => {
                if item_at != Place::Nested(*g) {
                    ops.push(to_folder(id, *g));
                }
            }
            Zone::Before | Zone::After => {
                detach(&mut sim, known, id, item_at, &mut ops);
                reorder(&mut sim, known, item, target, &mut ops);
            }
        },
        (RailKey::Home, _) | (_, RailKey::Home) => {}
    }
    ops
}

/// Where an item of the rail is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// On the rail itself.
    Top,
    /// A space inside this folder.
    Nested(u16),
}

fn to_folder(id: &RoomId, folder: u16) -> Op {
    Op::MoveToFolder {
        id: id.clone(),
        folder: FolderTarget::Existing(folder),
    }
}

/// Where `key` is drawn, or `None` when the rail does not draw it.
fn place(view: &SidebarLayout, known: &[RoomId], key: &RailKey) -> Option<Place> {
    for entry in &view.rail {
        match (entry, key) {
            (RailEntry::Space(id), RailKey::Space(k)) if id == k && known.contains(id) => {
                return Some(Place::Top);
            }
            (RailEntry::Folder(f), RailKey::Folder(k))
                if f.id == *k && sidebar::is_drawn(entry, known) =>
            {
                return Some(Place::Top);
            }
            (RailEntry::Folder(f), RailKey::Space(k)) if f.spaces.contains(k) => {
                return known.contains(k).then_some(Place::Nested(f.id));
            }
            _ => {}
        }
    }
    None
}

/// The folder that holds `space`.
fn folder_of(layout: &SidebarLayout, space: &RoomId) -> Option<u16> {
    layout.rail.iter().find_map(|entry| match entry {
        RailEntry::Folder(f) if f.spaces.contains(space) => Some(f.id),
        _ => None,
    })
}

/// Takes a space out of its folder first (the card's "Remove from folder").
fn detach(sim: &mut SidebarLayout, known: &[RoomId], id: &RoomId, at: Place, ops: &mut Vec<Op>) {
    if matches!(at, Place::Nested(_)) {
        let op = Op::RemoveFromFolder(id.clone());
        if sidebar::simulate(sim, op.clone(), known).is_ok() {
            ops.push(op);
        }
    }
}

/// The drawn items that `key` is among: the rail, or its folder.
fn siblings(view: &SidebarLayout, known: &[RoomId], key: &RailKey) -> Vec<RailKey> {
    if let RailKey::Space(id) = key
        && let Some(folder) = view.rail.iter().find_map(|entry| match entry {
            RailEntry::Folder(f) if f.spaces.contains(id) => Some(f),
            _ => None,
        })
    {
        return folder
            .spaces
            .iter()
            .filter(|s| known.contains(s))
            .map(|s| RailKey::Space(s.clone()))
            .collect();
    }
    view.rail
        .iter()
        .filter(|entry| sidebar::is_drawn(entry, known))
        .map(|entry| match entry {
            RailEntry::Space(id) => RailKey::Space(id.clone()),
            RailEntry::Folder(f) => RailKey::Folder(f.id),
        })
        .collect()
}

/// Steps `item` up or down, one drawn neighbour at a time, until it is right
/// before or right after the target: the card's Move up and Move down.
fn reorder(
    sim: &mut SidebarLayout,
    known: &[RoomId],
    item: &RailKey,
    target: &Target<RailKey>,
    ops: &mut Vec<Op>,
) {
    for _ in 0..2 * known.len() + 4 {
        let view = sidebar::materialized(sim, known);
        let list = siblings(&view, known, item);
        let (Some(i), Some(t)) = (
            list.iter().position(|k| k == item),
            list.iter().position(|k| *k == target.item),
        ) else {
            return;
        };
        let dir = match target.zone {
            Zone::Before if i + 1 == t => return,
            Zone::After if i == t + 1 => return,
            Zone::Into => return,
            Zone::Before if i < t => Dir::Down,
            Zone::After if i < t => Dir::Down,
            Zone::Before | Zone::After => Dir::Up,
        };
        let op = match item {
            RailKey::Space(id) => Op::MoveSpace {
                id: id.clone(),
                dir,
            },
            RailKey::Folder(id) => Op::MoveFolder { id: *id, dir },
            RailKey::Home => return,
        };
        if !matches!(sidebar::simulate(sim, op.clone(), known), Ok(true)) {
            return;
        }
        ops.push(op);
    }
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::{FolderLayout, UserId};

    use super::*;
    use crate::sidebar::{Refused, SidebarModel, SidebarState};
    use crate::sidebar_menu::{
        Entry, FolderPick, Purpose, Target as MenuTarget, op_for, op_for_name, op_for_pick,
    };
    use crate::sidebar_store::{Call, MemorySidebar, SidebarLoad};

    /// A valid room ID for a short name.
    fn rid(id: &str) -> RoomId {
        RoomId::from(format!("!{id}:x"))
    }

    fn sp(id: &str) -> RailKey {
        RailKey::Space(rid(id))
    }

    fn entry(id: &str) -> RailEntry {
        RailEntry::Space(rid(id))
    }

    fn folder(id: u16, spaces: &[&str]) -> RailEntry {
        RailEntry::Folder(FolderLayout {
            id,
            name: None,
            open: true,
            spaces: spaces.iter().map(|s| rid(s)).collect(),
        })
    }

    fn layout(rail: Vec<RailEntry>) -> SidebarLayout {
        let mut layout = SidebarLayout::new(UserId::from("@me:x"));
        layout.rail = rail;
        layout
    }

    fn known(ids: &[&str]) -> Vec<RoomId> {
        ids.iter().map(|s| rid(s)).collect()
    }

    fn at(item: RailKey, zone: Zone) -> Target<RailKey> {
        Target { item, zone }
    }

    /// A button of the options card, as a person would press it.
    #[derive(Debug, Clone)]
    enum Menu {
        Up(RailKey),
        Down(RailKey),
        Remove(&'static str),
        ToFolder(&'static str, u16),
        NewFolder(&'static str),
    }

    fn card(key: &RailKey) -> MenuTarget {
        match key {
            RailKey::Space(id) => MenuTarget::Space(id.clone()),
            RailKey::Folder(id) => MenuTarget::Folder(*id),
            RailKey::Home => unreachable!("Home has no card"),
        }
    }

    fn menu_op(button: &Menu) -> Op {
        match button {
            Menu::Up(key) => op_for(&card(key), Entry::MoveUp),
            Menu::Down(key) => op_for(&card(key), Entry::MoveDown),
            Menu::Remove(id) => op_for(&MenuTarget::Space(rid(id)), Entry::RemoveFromFolder),
            Menu::ToFolder(id, folder) => op_for_pick(&rid(id), FolderPick::Existing(*folder)),
            Menu::NewFolder(id) => Some(op_for_name(&Purpose::NewFolder(rid(id)), None)),
        }
        .expect("the card offers it")
    }

    /// A model holding `layout`, and the store that records its saves.
    fn model(layout: &SidebarLayout) -> (SidebarModel, MemorySidebar) {
        let store = MemorySidebar::default();
        let account = layout.account.clone();
        let mut model = SidebarModel::default();
        let _ = model.load_for(account.clone(), &store);
        model.loaded(&account, SidebarLoad::Layout(layout.clone()));
        (model, store)
    }

    fn saves(store: &MemorySidebar) -> usize {
        store
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Save(..)))
            .count()
    }

    /// One case of the table: the drop, and the card's buttons that do the
    /// same.
    struct Case {
        name: &'static str,
        rail: Vec<RailEntry>,
        known: &'static [&'static str],
        item: RailKey,
        target: Target<RailKey>,
        buttons: Vec<Menu>,
    }

    fn cases() -> Vec<Case> {
        use Menu::*;
        use Zone::*;
        let abcd: &[&str] = &["a", "b", "c", "d"];
        let in_f1: &[&str] = &["x", "y", "z", "a", "b"];
        let f1 = || vec![folder(1, &["x", "y", "z"]), entry("a"), entry("b")];
        vec![
            Case {
                name: "a space before one further down",
                rail: Vec::new(),
                known: abcd,
                item: sp("a"),
                target: at(sp("c"), Before),
                buttons: vec![Down(sp("a"))],
            },
            Case {
                name: "a space after one further down",
                rail: Vec::new(),
                known: abcd,
                item: sp("a"),
                target: at(sp("c"), After),
                buttons: vec![Down(sp("a")), Down(sp("a"))],
            },
            Case {
                name: "a space before one further up",
                rail: Vec::new(),
                known: abcd,
                item: sp("d"),
                target: at(sp("a"), Before),
                buttons: vec![Up(sp("d")), Up(sp("d")), Up(sp("d"))],
            },
            Case {
                name: "a space after one further up",
                rail: Vec::new(),
                known: abcd,
                item: sp("c"),
                target: at(sp("a"), After),
                buttons: vec![Up(sp("c"))],
            },
            Case {
                name: "a space before its next neighbour is already there",
                rail: Vec::new(),
                known: abcd,
                item: sp("a"),
                target: at(sp("b"), Before),
                buttons: vec![],
            },
            Case {
                name: "a space after its previous neighbour is already there",
                rail: Vec::new(),
                known: abcd,
                item: sp("b"),
                target: at(sp("a"), After),
                buttons: vec![],
            },
            Case {
                name: "a stored space the core does not report is stepped over",
                rail: vec![entry("a"), entry("ghost"), entry("b")],
                known: &["a", "b"],
                item: sp("a"),
                target: at(sp("b"), After),
                buttons: vec![Down(sp("a"))],
            },
            Case {
                name: "a space into a space makes a folder around the target",
                rail: Vec::new(),
                known: abcd,
                item: sp("a"),
                target: at(sp("c"), Into),
                buttons: vec![NewFolder("c"), ToFolder("a", 0)],
            },
            Case {
                name: "the new folder takes the lowest free number",
                rail: vec![folder(0, &["x", "y", "z"]), entry("a"), entry("b")],
                known: in_f1,
                item: sp("a"),
                target: at(sp("b"), Into),
                buttons: vec![NewFolder("b"), ToFolder("a", 1)],
            },
            Case {
                name: "a space from a folder into a space makes a folder",
                rail: f1(),
                known: in_f1,
                item: sp("x"),
                target: at(sp("a"), Into),
                buttons: vec![NewFolder("a"), ToFolder("x", 0)],
            },
            Case {
                name: "a space into a folder",
                rail: f1(),
                known: in_f1,
                item: sp("a"),
                target: at(RailKey::Folder(1), Into),
                buttons: vec![ToFolder("a", 1)],
            },
            Case {
                name: "a space into the folder it is in does nothing",
                rail: f1(),
                known: in_f1,
                item: sp("x"),
                target: at(RailKey::Folder(1), Into),
                buttons: vec![],
            },
            Case {
                name: "a space onto a space inside a folder joins the folder",
                rail: f1(),
                known: in_f1,
                item: sp("a"),
                target: at(sp("y"), Before),
                buttons: vec![ToFolder("a", 1)],
            },
            Case {
                name: "a space from a folder onto a space in another folder",
                rail: vec![
                    folder(1, &["x", "y", "z"]),
                    folder(2, &["a", "b"]),
                    entry("c"),
                ],
                known: &["x", "y", "z", "a", "b", "c"],
                item: sp("x"),
                target: at(sp("b"), Into),
                buttons: vec![ToFolder("x", 2)],
            },
            Case {
                name: "a space inside a folder, before another one there",
                rail: f1(),
                known: in_f1,
                item: sp("z"),
                target: at(sp("x"), Before),
                buttons: vec![Up(sp("z")), Up(sp("z"))],
            },
            Case {
                name: "a space inside a folder, after another one there",
                rail: f1(),
                known: in_f1,
                item: sp("x"),
                target: at(sp("z"), After),
                buttons: vec![Down(sp("x")), Down(sp("x"))],
            },
            Case {
                name: "a space inside a folder, into another one there does nothing",
                rail: f1(),
                known: in_f1,
                item: sp("y"),
                target: at(sp("x"), Into),
                buttons: vec![],
            },
            Case {
                name: "a space out of a folder to before a rail item",
                rail: f1(),
                known: in_f1,
                item: sp("x"),
                target: at(sp("a"), Before),
                buttons: vec![Remove("x")],
            },
            Case {
                name: "a space out of a folder to after a later rail item",
                rail: f1(),
                known: in_f1,
                item: sp("x"),
                target: at(sp("b"), After),
                buttons: vec![Remove("x"), Down(sp("x")), Down(sp("x"))],
            },
            Case {
                name: "a space out of a folder to before the folder",
                rail: f1(),
                known: in_f1,
                item: sp("x"),
                target: at(RailKey::Folder(1), Before),
                buttons: vec![Remove("x"), Up(sp("x"))],
            },
            Case {
                name: "a space out of a two-space folder dissolves it",
                rail: vec![folder(1, &["x", "y"]), entry("a")],
                known: &["x", "y", "a"],
                item: sp("x"),
                target: at(RailKey::Folder(1), After),
                buttons: vec![Remove("x")],
            },
            Case {
                name: "a top-level space before a folder",
                rail: f1(),
                known: in_f1,
                item: sp("b"),
                target: at(RailKey::Folder(1), Before),
                buttons: vec![Up(sp("b")), Up(sp("b"))],
            },
            Case {
                name: "a folder after a later space",
                rail: f1(),
                known: in_f1,
                item: RailKey::Folder(1),
                target: at(sp("b"), After),
                buttons: vec![Down(RailKey::Folder(1)), Down(RailKey::Folder(1))],
            },
            Case {
                name: "a folder before an earlier space",
                rail: vec![entry("a"), entry("b"), folder(1, &["x", "y", "z"])],
                known: in_f1,
                item: RailKey::Folder(1),
                target: at(sp("a"), Before),
                buttons: vec![Up(RailKey::Folder(1)), Up(RailKey::Folder(1))],
            },
            Case {
                name: "a folder after another folder",
                rail: vec![folder(1, &["x", "y"]), folder(2, &["a", "b"]), entry("c")],
                known: &["x", "y", "a", "b", "c"],
                item: RailKey::Folder(1),
                target: at(RailKey::Folder(2), After),
                buttons: vec![Down(RailKey::Folder(1))],
            },
            Case {
                name: "a folder into a folder does nothing",
                rail: vec![folder(1, &["x", "y"]), folder(2, &["a", "b"])],
                known: &["x", "y", "a", "b"],
                item: RailKey::Folder(1),
                target: at(RailKey::Folder(2), Into),
                buttons: vec![],
            },
            Case {
                name: "a folder onto a space inside a folder does nothing",
                rail: vec![folder(1, &["x", "y"]), folder(2, &["a", "b"])],
                known: &["x", "y", "a", "b"],
                item: RailKey::Folder(1),
                target: at(sp("a"), Before),
                buttons: vec![],
            },
            Case {
                name: "a folder into a space does nothing",
                rail: vec![folder(1, &["x", "y"]), entry("c")],
                known: &["x", "y", "c"],
                item: RailKey::Folder(1),
                target: at(sp("c"), Into),
                buttons: vec![],
            },
            Case {
                name: "a space onto itself does nothing",
                rail: Vec::new(),
                known: abcd,
                item: sp("a"),
                target: at(sp("a"), Into),
                buttons: vec![],
            },
            Case {
                name: "a folder onto itself does nothing",
                rail: f1(),
                known: in_f1,
                item: RailKey::Folder(1),
                target: at(RailKey::Folder(1), After),
                buttons: vec![],
            },
        ]
    }

    #[test]
    fn every_drop_is_the_options_cards_buttons_in_order_and_gives_the_same_layout() {
        for case in cases() {
            let start = layout(case.rail.clone());
            let known = known(case.known);
            let ops = plan(&start, &known, &case.item, &case.target);
            let buttons: Vec<Op> = case.buttons.iter().map(menu_op).collect();
            assert_eq!(ops, buttons, "{}: the operations", case.name);

            // The layout the drop makes (one change) is the one the card makes
            // (one button at a time).
            let (mut dropped, store) = model(&start);
            let _ = dropped
                .apply_all_known(ops.clone(), &known, &store)
                .unwrap_or_else(|e| panic!("{}: refused {e:?}", case.name));
            let (mut pressed, menu_store) = model(&start);
            for op in buttons {
                let _ = pressed
                    .apply_known(op, &known, &menu_store)
                    .unwrap_or_else(|e| panic!("{}: the card was refused {e:?}", case.name));
            }
            assert_eq!(dropped.layout(), pressed.layout(), "{}", case.name);

            // Saved once, and not at all for a drop that does nothing.
            let expected = usize::from(!ops.is_empty());
            assert_eq!(saves(&store), expected, "{}: saves", case.name);
        }
    }

    #[test]
    fn dropping_onto_itself_or_a_folder_into_a_folder_saves_nothing() {
        let start = layout(vec![folder(1, &["x", "y"]), folder(2, &["a", "b"])]);
        let k = known(&["x", "y", "a", "b"]);
        let (mut m, store) = model(&start);
        for (item, target) in [
            (sp("x"), at(sp("x"), Zone::Before)),
            (RailKey::Folder(1), at(RailKey::Folder(1), Zone::Into)),
            (RailKey::Folder(1), at(RailKey::Folder(2), Zone::Into)),
        ] {
            let ops = plan(&start, &k, &item, &target);
            assert!(ops.is_empty(), "{ops:?}");
            assert!(m.apply_all_known(ops, &k, &store).is_ok());
        }
        assert_eq!(saves(&store), 0);
        assert_eq!(m.layout(), Some(&start));
    }

    #[test]
    fn home_and_items_that_are_not_drawn_are_no_drop() {
        let start = layout(Vec::new());
        let k = known(&["a", "b"]);
        let some = |key| plan(&start, &k, &key, &at(sp("b"), Zone::Before));
        assert!(some(RailKey::Home).is_empty());
        assert!(some(sp("nope")).is_empty());
        assert!(plan(&start, &k, &sp("a"), &at(RailKey::Home, Zone::After)).is_empty());
        assert!(plan(&start, &k, &sp("a"), &at(sp("nope"), Zone::After)).is_empty());
        assert!(plan(&start, &k, &sp("a"), &at(RailKey::Folder(7), Zone::Into)).is_empty());
    }

    #[test]
    fn a_drop_with_a_step_for_a_folder_that_is_gone_is_refused_and_keeps_nothing() {
        // The first step is good; the second names a folder that does not
        // exist (Refused::Unknown). The good step is not kept and nothing is
        // saved.
        let start = layout(vec![entry("a"), entry("b")]);
        let k = known(&["a", "b"]);
        let (mut m, store) = model(&start);
        let steps = vec![
            Op::MoveSpace {
                id: rid("a"),
                dir: Dir::Down,
            },
            Op::MoveToFolder {
                id: rid("a"),
                folder: FolderTarget::Existing(9),
            },
        ];
        assert_eq!(
            m.apply_all_known(steps, &k, &store).err(),
            Some(Refused::Unknown)
        );
        assert_eq!(m.layout(), Some(&start), "the first step was not kept");
        assert_eq!(saves(&store), 0);
    }

    #[test]
    fn a_drop_that_would_pass_a_limit_is_refused_as_full_and_keeps_nothing() {
        use twicecat_core::api::sidebar::limits::MAX_SPACE_RECORDS;
        use twicecat_core::api::{SectionKey, SpaceLayout};
        let mut start = layout(vec![entry("a"), entry("b")]);
        start.spaces = (0..MAX_SPACE_RECORDS)
            .map(|n| SpaceLayout::new(RoomId::from(format!("!s{n}:x"))))
            .collect();
        assert!(start.fits());
        let k = known(&["a", "b"]);
        let (mut m, store) = model(&start);
        // A good step, then one that adds a record past the limit.
        let steps = vec![
            Op::MoveSpace {
                id: rid("a"),
                dir: Dir::Down,
            },
            Op::ToggleSection {
                space: RoomId::from("!one-too-many:x"),
                key: SectionKey::Starred,
            },
        ];
        assert_eq!(
            m.apply_all_known(steps, &k, &store).err(),
            Some(Refused::Full)
        );
        assert_eq!(m.state(), SidebarState::Full);
        assert_eq!(m.layout(), Some(&start), "nothing kept");
        assert_eq!(saves(&store), 0, "nothing saved");
    }
}
