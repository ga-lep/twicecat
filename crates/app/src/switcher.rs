//! The room switcher (Ctrl+K, plan Q18, Task 16): a field and a ranked list
//! of rooms, driven from the keyboard.
//!
//! [`rank_rooms`] is pure. The model keeps the query, the highlighted row and
//! what is needed to put the room list back as it was: a query with few local
//! matches also asks the core to search (`SetRoomListQuery`), which changes
//! the sidebar list for as long as the switcher is open.

use std::fmt;

use iced::Task;
use tessera_core::api::{RoomId, RoomListQuery, RoomListTab, RoomSummary};

use crate::state::{App, Message, Screen};

/// Widget ID of the switcher's field.
pub const INPUT_ID: &str = "switcher-input";

/// Widget ID of the list's scrollable.
pub const LIST_ID: &str = "switcher-list";

/// Most rooms the list shows.
pub const MAX_RESULTS: usize = 20;
/// Longest query, in characters.
pub const MAX_QUERY_CHARS: usize = 100;
/// With fewer local matches than this, the core is asked to search too.
pub const MIN_LOCAL_MATCHES: usize = 3;

/// Lowercase, without accents: the form names and queries are compared in.
///
/// There is no normalisation crate in the app, so Latin letters with
/// diacritics (Latin-1 and Latin Extended-A) map to their base letter by a
/// table, and combining marks are dropped (a name that arrives decomposed).
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            '\u{300}'..='\u{36f}' => {}
            'ß' => out.push_str("ss"),
            'æ' => out.push_str("ae"),
            'œ' => out.push_str("oe"),
            c => out.push(base_letter(c)),
        }
    }
    out
}

fn base_letter(c: char) -> char {
    match c {
        'à'..='å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' | 'ŧ' => 't',
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// How well a folded name matches a folded query; lower is better.
fn rank(name: &str, query: &str) -> Option<u8> {
    if name.starts_with(query) {
        return Some(0);
    }
    let word_start = name
        .char_indices()
        .filter(|&(at, _)| {
            name[..at]
                .chars()
                .next_back()
                .is_none_or(|before| !before.is_alphanumeric())
        })
        .any(|(at, _)| name[at..].starts_with(query));
    if word_start {
        return Some(1);
    }
    name.contains(query).then_some(2)
}

/// The rooms that match `query`, best first: a name that starts with it, then
/// one with a word that starts with it, then any that contains it; in list
/// order within each group. Case- and accent-insensitive, at most
/// [`MAX_RESULTS`]. A blank query lists the first rooms. Spaces are left out
/// (they have no timeline).
pub fn rank_rooms<'a>(
    query: &str,
    rooms: impl IntoIterator<Item = &'a RoomSummary>,
) -> Vec<RoomId> {
    let query = fold(query.trim());
    let mut found: Vec<(u8, usize, &RoomId)> = Vec::new();
    for (order, room) in rooms.into_iter().filter(|room| !room.is_space).enumerate() {
        let tier = if query.is_empty() {
            Some(0)
        } else {
            rank(&fold(&room.name), &query)
        };
        if let Some(tier) = tier {
            found.push((tier, order, &room.id));
        }
    }
    found.sort_by_key(|&(tier, order, _)| (tier, order));
    found
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, _, id)| id.clone())
        .collect()
}

/// The switcher's state. `Debug` hides the query and the room names.
#[derive(Clone, Default)]
pub struct SwitcherModel {
    /// The switcher is open.
    pub open: bool,
    /// The field as typed.
    pub query: String,
    /// The highlighted row.
    pub selected: usize,
    /// The room list query to put back on close, when a search replaced it.
    pub restore: Option<RoomListQuery>,
    /// The rooms of the list when the switcher opened, so results the core's
    /// search adds are merged with them and not instead of them.
    pub seed: Vec<RoomSummary>,
}

impl fmt::Debug for SwitcherModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SwitcherModel")
            .field("open", &self.open)
            .field("query", &self.query.chars().count())
            .field("selected", &self.selected)
            .field("restore", &self.restore.is_some())
            .field("seed", &self.seed.len())
            .finish()
    }
}

/// A message of the switcher. `Debug` hides the typed text.
#[derive(Clone)]
pub enum SwitcherMessage {
    /// The field changed.
    Input(String),
    /// Highlight the next row.
    Next,
    /// Highlight the previous row.
    Previous,
    /// Enter: open the highlighted room.
    Submit,
    /// A row was clicked.
    Pick(RoomId),
    /// Close without choosing.
    Close,
}

impl fmt::Debug for SwitcherMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(text) => write!(f, "Input({} chars)", text.chars().count()),
            Self::Next => f.write_str("Next"),
            Self::Previous => f.write_str("Previous"),
            Self::Submit => f.write_str("Submit"),
            Self::Pick(id) => f.debug_tuple("Pick").field(id).finish(),
            Self::Close => f.write_str("Close"),
        }
    }
}

impl SwitcherModel {
    /// The rooms to rank: the ones the list had when the switcher opened,
    /// then the list now without repeats.
    pub fn candidates<'a>(&'a self, live: &'a [RoomSummary]) -> Vec<&'a RoomSummary> {
        let mut all: Vec<&RoomSummary> = self.seed.iter().collect();
        for room in live {
            if !all.iter().any(|seen| seen.id == room.id) {
                all.push(room);
            }
        }
        all
    }

    /// The rows now, best first.
    pub fn results(&self, live: &[RoomSummary]) -> Vec<RoomId> {
        rank_rooms(&self.query, self.candidates(live))
    }

    /// The name of room `id`, wherever it is known.
    pub fn name<'a>(&'a self, live: &'a [RoomSummary], id: &RoomId) -> Option<&'a RoomSummary> {
        self.candidates(live)
            .into_iter()
            .find(|room| room.id == *id)
    }
}

impl App {
    /// Opens the switcher over the main view, with the field focused.
    pub(crate) fn open_switcher(&mut self) -> Task<Message> {
        if self.screen != Screen::Main || self.switcher.open || self.any_overlay_open() {
            return Task::none();
        }
        self.switcher = SwitcherModel {
            open: true,
            seed: self.rooms.clone(),
            ..SwitcherModel::default()
        };
        iced::widget::operation::focus(INPUT_ID)
    }

    pub(crate) fn on_switcher(&mut self, message: SwitcherMessage) -> Task<Message> {
        if !self.switcher.open {
            return Task::none();
        }
        match message {
            SwitcherMessage::Input(text) => self.switcher_input(text),
            SwitcherMessage::Next => self.switcher_step(1),
            SwitcherMessage::Previous => self.switcher_step(-1),
            SwitcherMessage::Submit => {
                let results = self.switcher.results(&self.rooms);
                match results.get(self.switcher.selected) {
                    Some(id) => self.switcher_choose(id.clone()),
                    None => Task::none(),
                }
            }
            SwitcherMessage::Pick(id) => self.switcher_choose(id),
            SwitcherMessage::Close => self.close_switcher(),
        }
    }

    fn switcher_input(&mut self, text: String) -> Task<Message> {
        self.switcher.query = text.chars().take(MAX_QUERY_CHARS).collect();
        self.switcher.selected = 0;
        let to_top = self.scroll_to_selected();
        let trimmed = self.switcher.query.trim().to_owned();
        let local = rank_rooms(&trimmed, self.switcher.candidates(&self.rooms)).len();
        if trimmed.is_empty() || local >= MIN_LOCAL_MATCHES {
            return to_top;
        }
        if self.switcher.restore.is_none() {
            self.switcher.restore = Some(self.query.clone());
        }
        Task::batch([
            to_top,
            self.set_query(RoomListQuery {
                tab: RoomListTab::All,
                search: trimmed,
                space: None,
            }),
        ])
    }

    fn switcher_step(&mut self, delta: isize) -> Task<Message> {
        let count = self.switcher.results(&self.rooms).len();
        if count > 0 {
            let next = (self.switcher.selected as isize + delta).rem_euclid(count as isize);
            self.switcher.selected = usize::try_from(next).unwrap_or(0);
        }
        self.scroll_to_selected()
    }

    /// Scrolls the list so the highlighted row is in view. Rows are one
    /// height, so the same fraction of the scroll range as of the rows keeps
    /// any row fully visible.
    fn scroll_to_selected(&self) -> Task<Message> {
        let count = self.switcher.results(&self.rooms).len();
        let y = if count > 1 {
            self.switcher.selected as f32 / (count - 1) as f32
        } else {
            0.0
        };
        iced::widget::operation::snap_to(
            LIST_ID,
            iced::widget::scrollable::RelativeOffset {
                x: None,
                y: Some(y),
            },
        )
    }

    /// Opens room `id` and closes the switcher.
    fn switcher_choose(&mut self, id: RoomId) -> Task<Message> {
        // A room only the opening list knew (the core's search left it out)
        // joins the list for the selection; the restored list replaces it.
        if !crate::room_list::contains(&self.rooms, &id)
            && let Some(room) = self.switcher.seed.iter().find(|room| room.id == id)
        {
            self.rooms.push(room.clone());
        }
        let select = self.update(Message::SelectRoom(id));
        let restore = self.end_switcher();
        Task::batch([
            restore,
            select,
            iced::widget::operation::focus(crate::composer::COMPOSER_ID),
        ])
    }

    /// Esc: closes the switcher and gives the focus back to the composer.
    pub(crate) fn close_switcher(&mut self) -> Task<Message> {
        let restore = self.end_switcher();
        Task::batch([
            restore,
            iced::widget::operation::focus(crate::composer::COMPOSER_ID),
        ])
    }

    /// Closes the model and puts the room list query back.
    fn end_switcher(&mut self) -> Task<Message> {
        let restore = self.switcher.restore.take();
        self.switcher = SwitcherModel::default();
        match restore {
            Some(query) => self.set_query(query),
            None => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rooms(names: &[&str]) -> Vec<RoomSummary> {
        names
            .iter()
            .enumerate()
            .map(|(n, name)| RoomSummary::new(RoomId::from(format!("!{n}:x")), (*name).to_owned()))
            .collect()
    }

    fn names(query: &str, list: &[RoomSummary]) -> Vec<String> {
        rank_rooms(query, list)
            .iter()
            .map(|id| {
                list.iter()
                    .find(|r| r.id == *id)
                    .expect("ranked room is in the list")
                    .name
                    .clone()
            })
            .collect()
    }

    #[test]
    fn prefix_then_word_start_then_substring() {
        let list = rooms(&["Backend", "Team Ops", "Operations", "Cooperation", "Ops"]);
        // "Operations" and "Ops" start with it, "Team Ops" has a word that
        // does, "Cooperation" only contains it.
        assert_eq!(
            names("op", &list),
            ["Operations", "Ops", "Team Ops", "Cooperation"]
        );
        assert_eq!(names("ops", &list), ["Ops", "Team Ops"]);
        assert_eq!(names("ack", &list), ["Backend"]);
    }

    #[test]
    fn list_order_breaks_ties() {
        let list = rooms(&["beta two", "beta one", "alpha beta"]);
        assert_eq!(names("beta", &list), ["beta two", "beta one", "alpha beta"]);
    }

    #[test]
    fn case_and_accents_do_not_matter() {
        let list = rooms(&["Café Crème", "ÉCOLE", "Straße", "Zoë"]);
        assert_eq!(names("cafe", &list), ["Café Crème"]);
        assert_eq!(names("CAFÉ", &list), ["Café Crème"]);
        assert_eq!(names("ecole", &list), ["ÉCOLE"]);
        assert_eq!(names("é", &list), ["ÉCOLE", "Café Crème", "Straße", "Zoë"]);
        assert_eq!(names("strasse", &list), ["Straße"]);
        assert_eq!(names("zoe", &list), ["Zoë"]);
        // A name that arrives decomposed (e + combining acute).
        let decomposed = rooms(&["Cafe\u{301}"]);
        assert_eq!(names("cafe", &decomposed), ["Cafe\u{301}"]);
        assert_eq!(names("café", &decomposed), ["Cafe\u{301}"]);
    }

    #[test]
    fn a_blank_query_lists_the_first_rooms_and_the_list_is_capped() {
        let many: Vec<String> = (0..30).map(|n| format!("room {n}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let list = rooms(&refs);
        assert_eq!(rank_rooms("", &list).len(), MAX_RESULTS);
        assert_eq!(rank_rooms("   ", &list).len(), MAX_RESULTS);
        assert_eq!(rank_rooms("room", &list).len(), MAX_RESULTS);
        assert_eq!(names("", &list)[0], "room 0");
        // The cap applies after ranking: the best 20, not the first 20 found.
        let mut late = rooms(&refs);
        late.push(RoomSummary::new(RoomId::from("!z:x"), "zzz room".into()));
        let ranked = rank_rooms("zzz", &late);
        assert_eq!(ranked, [RoomId::from("!z:x")]);
    }

    #[test]
    fn spaces_are_left_out_and_nothing_matches_nothing() {
        let mut list = rooms(&["Work", "Work space"]);
        list[1].is_space = true;
        assert_eq!(names("work", &list), ["Work"]);
        assert!(names("play", &list).is_empty());
    }

    #[test]
    fn the_cap_keeps_the_best_matches() {
        let mut list: Vec<String> = (0..25).map(|n| format!("xx ab {n}")).collect();
        list.push("ab best".to_owned());
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        let rooms = rooms(&refs);
        let ranked = names("ab", &rooms);
        assert_eq!(ranked.len(), MAX_RESULTS);
        assert_eq!(
            ranked[0], "ab best",
            "the prefix match comes first, past the cap"
        );
    }

    #[test]
    fn fold_is_lowercase_without_accents() {
        assert_eq!(fold("ÀÉÎÕÜ Ñandú Łódź"), "aeiou nandu lodz");
        assert_eq!(fold("plain"), "plain");
    }
}
