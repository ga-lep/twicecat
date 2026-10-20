//! Spaces in the app (plan Q15, Task 13): the sidebar's model and the rule
//! for which spaces it lists.
//!
//! A space filters the room list; it is not a security boundary. A space's
//! admins can list any room as a child (Element allows the same), so being
//! under a space says nothing about who may join the room or who is in it,
//! and the UI never presents it that way.
//!
//! Space names belong to people: nothing here logs them, and no `Debug` of a
//! type here (the model, `RailKey`, `RailItem`, `RailFolder`) shows a room ID or
//! a name.

use std::collections::{HashMap, HashSet};
use std::fmt;

use iced::{Point, Subscription, Task, mouse, touch};
use twicecat_core::api::{
    Command, RailEntry, RoomId, RoomListQuery, SidebarLayout, SpaceUnreadView, SpaceView,
    UnreadCountsView,
};

use crate::drag::{Outcome, Target, Zone};
use crate::keys::Shortcut;
use crate::rail_drop;
use crate::sidebar::{Op as SidebarOp, SidebarState};
use crate::state::{App, Message};

/// Most spaces kept; the sidebar is not a place for thousands of buttons.
pub const MAX_SPACES: usize = 200;

/// Most per-space unread entries kept: the core reports level 0 and level 1
/// spaces, so a little more than [`MAX_SPACES`] is room for all of them.
const MAX_UNREAD_ENTRIES: usize = 4 * MAX_SPACES;

/// What an item of the rail stands for. `Debug` hides the room ID.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum RailKey {
    /// The **Home** slot.
    Home,
    /// A space.
    Space(RoomId),
    /// A folder, by its ID in the layout.
    Folder(u16),
}

impl fmt::Debug for RailKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Home => f.write_str("Home"),
            Self::Space(_) => f.write_str("Space(<redacted>)"),
            Self::Folder(id) => f.debug_tuple("Folder").field(id).finish(),
        }
    }
}

/// The spaces the user has joined, as the core last reported them, and the
/// unread counts the core rolled up for them.
#[derive(Default)]
pub struct SpacesModel {
    spaces: Vec<SpaceView>,
    unread: HashMap<RoomId, UnreadCountsView>,
    outside: UnreadCountsView,
}

impl fmt::Debug for SpacesModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpacesModel")
            .field("spaces", &self.spaces.len())
            .field("unread", &self.unread.len())
            .finish()
    }
}

/// A folder of the rail, with the spaces of it that the core reports.
/// `Debug` shows its number, whether it is open and how many spaces it
/// draws, never its name or a room ID.
#[derive(Clone, PartialEq, Eq)]
pub struct RailFolder<'a> {
    /// Its ID in the layout.
    pub id: u16,
    /// Its name, if the user gave it one.
    pub name: Option<&'a str>,
    /// Whether it is shown open.
    pub open: bool,
    /// Its spaces in the layout's order; stored ones the core does not
    /// report are left out here (and stay in the layout).
    pub spaces: Vec<&'a SpaceView>,
}

impl fmt::Debug for RailFolder<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RailFolder")
            .field("id", &self.id)
            .field("named", &self.name.is_some())
            .field("open", &self.open)
            .field("spaces", &self.spaces.len())
            .finish()
    }
}

/// One item of the rail below **Home**. `Debug` hides the room ID and the
/// folder's name.
#[derive(Clone, PartialEq, Eq)]
pub enum RailItem<'a> {
    /// A top-level space on the rail itself.
    Space(&'a SpaceView),
    /// A folder.
    Folder(RailFolder<'a>),
}

impl fmt::Debug for RailItem<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Space(_) => f.write_str("Space(<redacted>)"),
            Self::Folder(folder) => f.debug_tuple("Folder").field(folder).finish(),
        }
    }
}

/// The sum of `counts`, saturating; unread when any of them is.
pub fn sum_counts(counts: impl IntoIterator<Item = UnreadCountsView>) -> UnreadCountsView {
    counts
        .into_iter()
        .fold(UnreadCountsView::default(), |sum, c| UnreadCountsView {
            notifications: sum.notifications.saturating_add(c.notifications),
            highlights: sum.highlights.saturating_add(c.highlights),
            unread: sum.unread || c.unread,
        })
}

impl SpacesModel {
    /// Replaces the list with the core's (capped at [`MAX_SPACES`]).
    pub fn set(&mut self, mut spaces: Vec<SpaceView>) {
        spaces.truncate(MAX_SPACES);
        self.spaces = spaces;
    }

    /// Replaces the unread counts with the core's (capped).
    pub fn set_unread(&mut self, spaces: Vec<SpaceUnreadView>, outside: UnreadCountsView) {
        self.unread = spaces
            .into_iter()
            .take(MAX_UNREAD_ENTRIES)
            .map(|view| (view.space, view.counts))
            .collect();
        self.outside = outside;
    }

    /// Forgets all spaces and counts (logout).
    pub fn clear(&mut self) {
        self.spaces.clear();
        self.unread.clear();
        self.outside = UnreadCountsView::default();
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

    /// The unread counts of a space; none when the core sent none.
    pub fn unread_for(&self, id: &RoomId) -> UnreadCountsView {
        self.unread.get(id).copied().unwrap_or_default()
    }

    /// The unread counts of the rooms in no space (the **Home** badge).
    pub fn outside(&self) -> UnreadCountsView {
        self.outside
    }

    /// The counts of a folder: the sum over its spaces.
    pub fn folder_counts(&self, folder: &RailFolder<'_>) -> UnreadCountsView {
        sum_counts(folder.spaces.iter().map(|space| self.unread_for(&space.id)))
    }

    /// The top-level spaces, in the core's order. Sub-spaces are not on the
    /// rail (they are sections of the room list).
    pub fn entries(&self) -> Vec<&SpaceView> {
        self.spaces
            .iter()
            .filter(|space| space.level == 0)
            .collect()
    }

    /// The rail below **Home**: the layout's order and folders merged with
    /// the core's top-level spaces. The layout's order comes first; spaces
    /// the layout does not name follow in the core's order; a stored space
    /// the core does not report (or that is not top-level) is not drawn, and
    /// the layout is not changed. A space is drawn once: the first place the
    /// layout names it wins. A folder with none of its spaces known is not
    /// drawn.
    pub fn rail_items<'a>(&'a self, layout: Option<&'a SidebarLayout>) -> Vec<RailItem<'a>> {
        let top: HashMap<&RoomId, &SpaceView> = self
            .spaces
            .iter()
            .filter(|space| space.level == 0)
            .map(|space| (&space.id, space))
            .collect();
        let mut placed: HashSet<&RoomId> = HashSet::new();
        let mut out = Vec::new();
        for entry in layout.map_or(&[][..], |layout| layout.rail.as_slice()) {
            match entry {
                RailEntry::Space(id) => {
                    if let Some(space) = top.get(id)
                        && placed.insert(id)
                    {
                        out.push(RailItem::Space(space));
                    }
                }
                RailEntry::Folder(folder) => {
                    let mut spaces = Vec::new();
                    for id in &folder.spaces {
                        if let Some(space) = top.get(id)
                            && placed.insert(id)
                        {
                            spaces.push(*space);
                        }
                    }
                    if !spaces.is_empty() {
                        out.push(RailItem::Folder(RailFolder {
                            id: folder.id,
                            name: folder.name.as_ref().map(|name| name.as_str()),
                            open: folder.open,
                            spaces,
                        }));
                    }
                }
            }
        }
        for space in self.spaces.iter().filter(|space| space.level == 0) {
            if placed.insert(&space.id) {
                out.push(RailItem::Space(space));
            }
        }
        out
    }
}

/// The window event for the rail's drag: the cursor, and the left button
/// coming up.
pub(crate) fn pointer_message(event: iced::Event) -> Option<Message> {
    match event {
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Message::Spaces(SpacesMessage::PointerMoved(position)))
        }
        iced::Event::Touch(touch::Event::FingerMoved { position, .. }) => {
            Some(Message::Spaces(SpacesMessage::PointerMoved(position)))
        }
        iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        | iced::Event::Touch(touch::Event::FingerPressed { .. }) => {
            Some(Message::Spaces(SpacesMessage::PointerPressed))
        }
        iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        | iced::Event::Touch(touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. }) => {
            Some(Message::Spaces(SpacesMessage::PointerReleased))
        }
        _ => None,
    }
}

fn listener(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    pointer_message(event)
}

/// The message of the sidebar.
#[derive(Debug, Clone, PartialEq)]
pub enum SpacesMessage {
    /// A space button, or **Home** (`None`).
    Select(Option<RoomId>),
    /// The pointer entered an item of the rail.
    Enter(RailKey),
    /// The pointer left an item of the rail.
    Exit(RailKey),
    /// The left button went down on an item of the rail.
    Press(RailKey),
    /// The left button came up over an item of the rail: a click when it
    /// went down on the same item and the pointer has not left it since.
    Release(RailKey),
    /// While a drag runs: the pointer moved over an item, in this part of it.
    Over(RailKey, Zone),
    /// While the button is down: the pointer moved (window coordinates).
    PointerMoved(Point),
    /// The left button, or a finger, went down anywhere in the window while a
    /// press or drag was tracked: its release was missed, so it is cancelled.
    PointerPressed,
    /// The left button, or the finger, came up anywhere in the window.
    PointerReleased,
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

    /// The pointer on the rail: hover, and a click as a press and a release
    /// on one item with no leaving in between.
    pub(crate) fn on_rail(&mut self, message: SpacesMessage) -> Task<Message> {
        match message {
            SpacesMessage::Select(space) => self.select_space(space),
            SpacesMessage::Enter(key) => {
                self.rail_hover = Some(key);
                Task::none()
            }
            SpacesMessage::Exit(key) => {
                if self.rail_hover.as_ref() == Some(&key) {
                    self.rail_hover = None;
                }
                self.rail_drag.leave(&key);
                Task::none()
            }
            SpacesMessage::Press(key) => {
                let draggable = key != RailKey::Home
                    && matches!(
                        self.sidebar.state(),
                        SidebarState::Loaded | SidebarState::Full
                    );
                self.rail_drag.press(key, draggable);
                Task::none()
            }
            SpacesMessage::Release(key) => match self.rail_drag.release_on(&key) {
                Outcome::Click(key) => self.rail_click(key),
                _ => Task::none(),
            },
            SpacesMessage::Over(key, zone) => {
                self.rail_over(key, zone);
                Task::none()
            }
            SpacesMessage::PointerMoved(at) => {
                let _ = self.rail_drag.moved(at);
                Task::none()
            }
            SpacesMessage::PointerPressed => {
                // A button is down again: the release of the one tracked was
                // missed. Nothing is dropped.
                let _ = self.rail_drag.cancel();
                Task::none()
            }
            SpacesMessage::PointerReleased => match self.rail_drag.end() {
                Outcome::Drop { item, target } => self.rail_drop(&item, &target),
                _ => Task::none(),
            },
        }
    }

    /// The top-level spaces the core reports, for the drop planner.
    fn known_spaces(&self) -> Vec<RoomId> {
        self.spaces.entries().iter().map(|s| s.id.clone()).collect()
    }

    /// The pointer is over `key`, in `zone`, during a drag: it is a target
    /// only if dropping there would change something.
    fn rail_over(&mut self, key: RailKey, zone: Zone) {
        let Some(item) = self.rail_drag.dragged().cloned() else {
            return;
        };
        let target = Target { item: key, zone };
        if self.rail_drag.seen(&target) {
            return;
        }
        let known = self.known_spaces();
        let does_something = self
            .sidebar
            .layout()
            .is_some_and(|layout| !rail_drop::plan(layout, &known, &item, &target).is_empty());
        self.rail_drag.hover(does_something.then_some(target));
    }

    /// The drag ended on a target: the same operations the options card
    /// offers, done as one change and saved once. A refused change leaves the
    /// layout as it was.
    fn rail_drop(&mut self, item: &RailKey, target: &Target<RailKey>) -> Task<Message> {
        let known = self.known_spaces();
        let Some(layout) = self.sidebar.layout() else {
            return Task::none();
        };
        let ops = rail_drop::plan(layout, &known, item, target);
        if ops.is_empty() {
            return Task::none();
        }
        let task = match self
            .sidebar
            .apply_all_known(ops, &known, &*self.sidebar_store)
        {
            Ok(task) => task.discard(),
            Err(_) => Task::none(),
        };
        self.clear_stale_rail_keys();
        task
    }

    /// Esc during a drag cancels it. The key is used here, so nothing else
    /// closes. `None` when no drag runs.
    pub(crate) fn rail_escape(&mut self, shortcut: Shortcut) -> Option<Task<Message>> {
        if shortcut != Shortcut::Escape || !self.rail_drag.is_dragging() {
            return None;
        }
        let _ = self.rail_drag.cancel();
        // The composer's copy of the key must not close another layer.
        self.escape_spent = false;
        Some(Task::none())
    }

    /// The window's pointer, listened to only while the left button is down
    /// on an item of the rail (the end of a drag is often outside every
    /// `mouse_area`, which only sees a release over itself).
    pub fn rail_drag_subscription(&self) -> Subscription<Message> {
        if self.rail_drag.is_active() {
            iced::event::listen_with(listener)
        } else {
            Subscription::none()
        }
    }

    /// A click on an item of the rail.
    fn rail_click(&mut self, key: RailKey) -> Task<Message> {
        match key {
            RailKey::Home => self.select_space(None),
            RailKey::Space(id) => self.select_space(Some(id)),
            // Refused only with no layout loaded, when no folder shows.
            RailKey::Folder(id) => self
                .apply_sidebar(SidebarOp::ToggleFolder(id))
                .unwrap_or_else(|_| Task::none()),
        }
    }

    /// Forgets the hover and press of an item the rail no longer draws (and
    /// closes the options card or dialog about it): a
    /// space that is gone or inside a closed folder, and a folder that is
    /// gone, dissolved or has none of its spaces reported. Its pointer-exit
    /// would never come.
    pub(crate) fn clear_stale_rail_keys(&mut self) {
        // The layout or the spaces changed: what was worked out is stale.
        self.rail_drag.forget_seen();
        let mut drawn: HashSet<RailKey> = HashSet::from([RailKey::Home]);
        for item in self.spaces.rail_items(self.sidebar.layout()) {
            match item {
                RailItem::Space(space) => {
                    drawn.insert(RailKey::Space(space.id.clone()));
                }
                RailItem::Folder(folder) => {
                    drawn.insert(RailKey::Folder(folder.id));
                    if folder.open {
                        drawn.extend(folder.spaces.iter().map(|s| RailKey::Space(s.id.clone())));
                    }
                }
            }
        }
        if self
            .rail_hover
            .as_ref()
            .is_some_and(|key| !drawn.contains(key))
        {
            self.rail_hover = None;
        }
        self.rail_drag.retain(|key| drawn.contains(key));
        self.close_sidebar_menu_if_gone();
    }

    /// The core's unread counts per space and outside spaces.
    pub(crate) fn on_space_unread(
        &mut self,
        spaces: Vec<SpaceUnreadView>,
        outside: UnreadCountsView,
    ) {
        self.spaces.set_unread(spaces, outside);
    }

    /// The core's list of spaces: kept, their avatars requested once, and a
    /// selected space that is gone resets the filter to Home.
    pub(crate) fn on_spaces(&mut self, spaces: Vec<SpaceView>) -> Task<Message> {
        self.spaces.set(spaces);
        self.clear_stale_rail_keys();
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
    use twicecat_core::api::{FolderLayout, SidebarLabel, UserId};

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

    fn counts(notifications: u64, highlights: u64, unread: bool) -> UnreadCountsView {
        UnreadCountsView {
            notifications,
            highlights,
            unread,
        }
    }

    fn model(spaces: Vec<SpaceView>) -> SpacesModel {
        let mut model = SpacesModel::default();
        model.set(spaces);
        model
    }

    fn layout(rail: Vec<RailEntry>) -> SidebarLayout {
        let mut layout = SidebarLayout::new(UserId::from("@me:x"));
        layout.rail = rail;
        layout
    }

    fn sp(id: &str) -> RailEntry {
        RailEntry::Space(RoomId::from(id))
    }

    fn folder(id: u16, open: bool, spaces: &[&str]) -> RailEntry {
        RailEntry::Folder(FolderLayout {
            id,
            name: None,
            open,
            spaces: spaces.iter().map(|s| RoomId::from(*s)).collect(),
        })
    }

    /// A readable form of the rail: `a`, or `[1: a b]` for a folder.
    fn shape(items: &[RailItem<'_>]) -> Vec<String> {
        items
            .iter()
            .map(|item| match item {
                RailItem::Space(s) => s.id.as_str().to_owned(),
                RailItem::Folder(f) => {
                    let ids: Vec<&str> = f.spaces.iter().map(|s| s.id.as_str()).collect();
                    format!("[{}: {}]", f.id, ids.join(" "))
                }
            })
            .collect()
    }

    fn sample() -> SpacesModel {
        model(vec![
            space("!a:x", 0),
            space("!a1:x", 1),
            space("!b:x", 0),
            space("!b1:x", 1),
            space("!c:x", 0),
        ])
    }

    #[test]
    fn entries_are_the_top_level_spaces_only() {
        let m = sample();
        let ids: Vec<&str> = m.entries().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["!a:x", "!b:x", "!c:x"]);
        assert!(SpacesModel::default().entries().is_empty());
    }

    #[test]
    fn without_a_layout_the_rail_is_the_cores_order() {
        let m = sample();
        assert_eq!(shape(&m.rail_items(None)), ["!a:x", "!b:x", "!c:x"]);
        let empty = layout(Vec::new());
        assert_eq!(shape(&m.rail_items(Some(&empty))), ["!a:x", "!b:x", "!c:x"]);
    }

    #[test]
    fn the_layout_order_is_kept() {
        let m = sample();
        let l = layout(vec![sp("!c:x"), sp("!a:x"), sp("!b:x")]);
        assert_eq!(shape(&m.rail_items(Some(&l))), ["!c:x", "!a:x", "!b:x"]);
    }

    #[test]
    fn a_new_space_is_appended_in_the_cores_order() {
        let m = sample();
        let l = layout(vec![sp("!c:x")]);
        assert_eq!(shape(&m.rail_items(Some(&l))), ["!c:x", "!a:x", "!b:x"]);
    }

    #[test]
    fn a_space_in_a_folder_is_not_repeated_at_top_level() {
        let m = sample();
        let l = layout(vec![sp("!c:x"), folder(7, false, &["!a:x", "!b:x"])]);
        assert_eq!(shape(&m.rail_items(Some(&l))), ["!c:x", "[7: !a:x !b:x]"]);
        // Named twice in the layout: drawn once, where it is named first.
        let twice = layout(vec![folder(1, true, &["!a:x"]), sp("!a:x")]);
        assert_eq!(
            shape(&m.rail_items(Some(&twice))),
            ["[1: !a:x]", "!b:x", "!c:x"]
        );
    }

    #[test]
    fn a_stored_space_the_core_does_not_report_is_skipped_but_stays_stored() {
        let m = sample();
        let l = layout(vec![
            sp("!gone:x"),
            folder(2, false, &["!gone2:x", "!b:x"]),
            folder(3, false, &["!gone3:x"]),
            // A sub-space is not on the rail.
            sp("!a1:x"),
        ]);
        let before = l.clone();
        assert_eq!(
            shape(&m.rail_items(Some(&l))),
            ["[2: !b:x]", "!a:x", "!c:x"]
        );
        assert_eq!(l, before, "the layout is not touched");
    }

    #[test]
    fn folder_name_and_open_state_come_from_the_layout() {
        let m = sample();
        let mut l = layout(vec![folder(4, true, &["!a:x"])]);
        let RailEntry::Folder(f) = &mut l.rail[0] else {
            unreachable!()
        };
        f.name = Some(SidebarLabel::try_from("Work").expect("a valid name"));
        let items = m.rail_items(Some(&l));
        let RailItem::Folder(f) = &items[0] else {
            panic!("a folder")
        };
        assert_eq!((f.id, f.name, f.open), (4, Some("Work"), true));
    }

    #[test]
    fn counts_sum_with_saturation_and_a_folder_is_the_sum_of_its_spaces() {
        let mut m = sample();
        m.set_unread(
            vec![
                SpaceUnreadView {
                    space: RoomId::from("!a:x"),
                    counts: counts(2, 1, true),
                },
                SpaceUnreadView {
                    space: RoomId::from("!b:x"),
                    counts: counts(5, 0, true),
                },
                SpaceUnreadView {
                    space: RoomId::from("!c:x"),
                    counts: counts(100, 9, true),
                },
            ],
            counts(0, 0, false),
        );
        let l = layout(vec![folder(1, false, &["!a:x", "!b:x"])]);
        let items = m.rail_items(Some(&l));
        let RailItem::Folder(f) = &items[0] else {
            panic!("a folder")
        };
        assert_eq!(m.folder_counts(f), counts(7, 1, true));
        assert_eq!(
            sum_counts([counts(u64::MAX, u64::MAX, false), counts(1, 1, false)]),
            counts(u64::MAX, u64::MAX, false)
        );
        assert_eq!(sum_counts([]), UnreadCountsView::default());
        assert_eq!(m.unread_for(&RoomId::from("!nope:x")), counts(0, 0, false));
    }

    #[test]
    fn debug_of_the_rail_items_shows_no_room_id_and_no_folder_name() {
        let m = model(vec![space("!secretroom:x", 0), space("!secretroom2:x", 0)]);
        let mut l = layout(vec![folder(1, true, &["!secretroom:x", "!secretroom2:x"])]);
        let RailEntry::Folder(f) = &mut l.rail[0] else {
            unreachable!()
        };
        f.name = Some(SidebarLabel::try_from("Payroll").expect("a valid name"));
        let items = m.rail_items(Some(&l));
        let mut texts = vec![format!("{items:?}")];
        if let RailItem::Folder(f) = &items[0] {
            texts.push(format!("{f:?}"));
        }
        texts.push(format!("{:?}", RailItem::Space(&m.spaces()[0])));
        for text in texts {
            for secret in ["!secretroom", "@a:x", "Payroll"] {
                assert!(!text.contains(secret), "{secret} is in {text}");
            }
        }
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
        model.set_unread(
            vec![SpaceUnreadView {
                space: RoomId::from("!s0:x"),
                counts: counts(1, 0, true),
            }],
            counts(3, 0, true),
        );
        let dbg = format!("{model:?}");
        assert!(!dbg.contains("!s0:x"), "{dbg}");
        assert!(!format!("{:?}", RailKey::Space(RoomId::from("!s0:x"))).contains("!s0:x"));
        model.clear();
        assert!(model.is_empty());
        assert_eq!(model.outside(), UnreadCountsView::default());
        assert_eq!(
            model.unread_for(&RoomId::from("!s0:x")),
            counts(0, 0, false)
        );
    }
}
