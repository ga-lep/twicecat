//! Spaces in the app (plan Q15, Task 13): the sidebar's model and the rule
//! for which spaces it lists.
//!
//! A space filters the room list; it is not a security boundary. A space's
//! admins can list any room as a child (Element allows the same), so being
//! under a space says nothing about who may join the room or who is in it,
//! and the UI never presents it that way.
//!
//! Space names belong to people: nothing here logs them and `Debug` shows
//! counts only.

use std::fmt;

use iced::Task;
use twicecat_core::api::{Command, RoomId, RoomListQuery, SpaceView};

use crate::state::{App, Message};

/// Most spaces kept; the sidebar is not a place for thousands of buttons.
pub const MAX_SPACES: usize = 200;

/// The spaces the user has joined, as the core last reported them.
#[derive(Default)]
pub struct SpacesModel {
    spaces: Vec<SpaceView>,
}

impl fmt::Debug for SpacesModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpacesModel")
            .field("spaces", &self.spaces.len())
            .finish()
    }
}

/// One button of the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    /// The space.
    pub space: &'a SpaceView,
    /// A child of the top-level space above it.
    pub nested: bool,
}

impl SpacesModel {
    /// Replaces the list with the core's (capped at [`MAX_SPACES`]).
    pub fn set(&mut self, mut spaces: Vec<SpaceView>) {
        spaces.truncate(MAX_SPACES);
        self.spaces = spaces;
    }

    /// Forgets all spaces (logout).
    pub fn clear(&mut self) {
        self.spaces.clear();
    }

    /// The spaces, in the core's order.
    pub fn spaces(&self) -> &[SpaceView] {
        &self.spaces
    }

    /// Whether the user is in no space, so the sidebar is not drawn.
    pub fn is_empty(&self) -> bool {
        self.spaces.is_empty()
    }

    /// Whether `id` is one of the spaces.
    pub fn contains(&self, id: &RoomId) -> bool {
        self.spaces.iter().any(|space| space.id == *id)
    }

    /// The buttons below **Home**: every top-level space, and under the one
    /// that is selected (or holds the selected sub-space) its direct
    /// sub-spaces.
    pub fn entries(&self, selected: Option<&RoomId>) -> Vec<Entry<'_>> {
        entries(&self.spaces, selected)
    }
}

/// See [`SpacesModel::entries`]. The core lists a space and then its
/// sub-spaces, so a level-1 space belongs to the nearest level-0 space above
/// it; spaces deeper than level 1, and level-1 spaces before any level-0
/// one, are not shown.
pub fn entries<'a>(spaces: &'a [SpaceView], selected: Option<&RoomId>) -> Vec<Entry<'a>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spaces.len() {
        if spaces[i].level != 0 {
            i += 1;
            continue;
        }
        let end = spaces[i + 1..]
            .iter()
            .position(|space| space.level == 0)
            .map_or(spaces.len(), |n| i + 1 + n);
        let block = &spaces[i..end];
        out.push(Entry {
            space: &block[0],
            nested: false,
        });
        if selected.is_some_and(|sel| block.iter().any(|space| space.id == *sel)) {
            out.extend(
                block[1..]
                    .iter()
                    .filter(|space| space.level == 1)
                    .map(|space| Entry {
                        space,
                        nested: true,
                    }),
            );
        }
        i = end;
    }
    out
}

/// The message of the sidebar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpacesMessage {
    /// A space button, or **Home** (`None`).
    Select(Option<RoomId>),
}

impl App {
    /// A space button, or **Home** (`None`): the room list is filtered to the
    /// space, keeping the tab and the search. An unknown space is refused.
    pub(crate) fn select_space(&mut self, space: Option<RoomId>) -> Task<Message> {
        if space.as_ref().is_some_and(|id| !self.spaces.contains(id)) {
            return Task::none();
        }
        let query = RoomListQuery {
            tab: self.query.tab,
            search: self.query.search.clone(),
            space,
        };
        self.set_query(query)
    }

    /// The core's list of spaces: kept, their avatars requested once, and a
    /// selected space that is gone resets the filter to Home.
    pub(crate) fn on_spaces(&mut self, spaces: Vec<SpaceView>) -> Task<Message> {
        self.spaces.set(spaces);
        let ids = self.avatars.take_unrequested(
            self.spaces
                .spaces()
                .iter()
                .filter_map(|space| space.avatar.clone()),
        );
        let fetch = if ids.is_empty() {
            Task::none()
        } else {
            self.send(Command::FetchAvatars(ids))
        };
        let reset = match &self.query.space {
            Some(id) if !self.spaces.contains(id) => self.select_space(None),
            _ => Task::none(),
        };
        Task::batch([fetch, reset])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(id: &str, level: u32) -> SpaceView {
        SpaceView {
            id: RoomId::from(id),
            name: id.to_owned(),
            avatar: None,
            level,
            children: 0,
            rooms: Vec::new(),
        }
    }

    fn ids(entries: &[Entry<'_>]) -> Vec<(String, bool)> {
        entries
            .iter()
            .map(|e| (e.space.id.as_str().to_owned(), e.nested))
            .collect()
    }

    fn sample() -> Vec<SpaceView> {
        vec![
            space("!a:x", 0),
            space("!a1:x", 1),
            space("!a2:x", 1),
            space("!a2x:x", 2),
            space("!b:x", 0),
            space("!b1:x", 1),
        ]
    }

    #[test]
    fn home_shows_only_top_level_spaces() {
        let spaces = sample();
        assert_eq!(
            ids(&entries(&spaces, None)),
            [("!a:x".into(), false), ("!b:x".into(), false)]
        );
    }

    #[test]
    fn the_selected_space_shows_its_level_one_children() {
        let spaces = sample();
        let a = RoomId::from("!a:x");
        assert_eq!(
            ids(&entries(&spaces, Some(&a))),
            [
                ("!a:x".into(), false),
                ("!a1:x".into(), true),
                ("!a2:x".into(), true),
                ("!b:x".into(), false),
            ]
        );
    }

    #[test]
    fn a_selected_child_keeps_its_siblings_visible() {
        let spaces = sample();
        let child = RoomId::from("!b1:x");
        assert_eq!(
            ids(&entries(&spaces, Some(&child))),
            [
                ("!a:x".into(), false),
                ("!b:x".into(), false),
                ("!b1:x".into(), true),
            ]
        );
    }

    #[test]
    fn odd_lists_do_not_panic_or_invent_parents() {
        let none: Vec<SpaceView> = Vec::new();
        assert!(entries(&none, None).is_empty());
        // A level-1 space before any level-0 one has no parent.
        let orphans = vec![space("!o:x", 1), space("!t:x", 0)];
        assert_eq!(
            ids(&entries(&orphans, Some(&RoomId::from("!o:x")))),
            [("!t:x".into(), false)]
        );
        // An unknown selection opens nothing.
        let spaces = sample();
        assert_eq!(entries(&spaces, Some(&RoomId::from("!zz:x"))).len(), 2);
    }

    #[test]
    fn the_model_caps_and_answers_membership() {
        let mut model = SpacesModel::default();
        assert!(model.is_empty());
        model.set(
            (0..MAX_SPACES + 5)
                .map(|i| space(&format!("!s{i}:x"), 0))
                .collect(),
        );
        assert_eq!(model.spaces().len(), MAX_SPACES);
        assert!(model.contains(&RoomId::from("!s0:x")));
        assert!(!model.contains(&RoomId::from("!s9999:x")));
        let dbg = format!("{model:?}");
        assert!(!dbg.contains("!s0:x"), "{dbg}");
        model.clear();
        assert!(model.is_empty());
    }
}
