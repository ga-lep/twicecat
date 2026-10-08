//! The message composer: a `text_editor` buffer with markdown, mention and
//! emoji autocomplete, reply and edit modes, and typing notices (plan Q13,
//! Q17, Q18).
//!
//! Everything here is pure state: no command is sent from this module and no
//! clock is read. Callers pass the current [`Instant`], so tests script time.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::keyboard::{self, key};
use iced::widget::text_editor::{self, Action, Binding, Cursor, Edit, KeyPress, Position, Status};
use tessera_core::api::{
    Member, MentionRef, MessageKind, MessageText, OutgoingMessage, RoomId, TextFormat,
};

use crate::emoji;
use crate::state::Message;
use crate::text_safety::one_line_capped;

/// Least time between two `SearchMembers` commands (plan Q18). Each search
/// loads and sorts the room's member list in the core, so a burst of
/// keystrokes is coalesced into one trailing search.
pub const MEMBER_SEARCH_INTERVAL: Duration = Duration::from_millis(150);

/// Least time between two `SetTyping(true)` commands (plan Q13).
pub const TYPING_INTERVAL: Duration = Duration::from_secs(3);

/// Longest mention query, in characters.
pub const MAX_MENTION_QUERY: usize = 64;

/// Longest emoji shortcode query, in characters.
pub const MAX_EMOJI_QUERY: usize = 32;

/// Longest mention label (display name) inserted, in characters.
pub const MAX_LABEL_CHARS: usize = 64;

/// Most mentions kept for one message.
pub const MAX_MENTIONS: usize = 64;

/// A key the composer cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Enter or Return.
    Enter,
    /// Tab.
    Tab,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Escape.
    Escape,
    /// Any other key.
    Other,
}

/// The part of a key press the bindings depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    /// The key.
    pub key: Key,
    /// Shift is held.
    pub shift: bool,
}

/// What a key press does in the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Leave it to the editor's default bindings.
    Default,
    /// Send the message.
    Send,
    /// Insert a line break.
    Newline,
    /// Pick the highlighted suggestion.
    Pick,
    /// Highlight the next suggestion.
    Next,
    /// Highlight the previous suggestion.
    Previous,
    /// Close the suggestions, or cancel reply and edit.
    Escape,
}

/// The pure key bindings of plan Q17.
pub fn bind(input: KeyInput, suggestions_open: bool) -> Decision {
    match input.key {
        Key::Enter if input.shift => Decision::Newline,
        Key::Enter if suggestions_open => Decision::Pick,
        Key::Enter => Decision::Send,
        Key::Tab if suggestions_open => Decision::Pick,
        Key::Down if suggestions_open => Decision::Next,
        Key::Up if suggestions_open => Decision::Previous,
        Key::Escape => Decision::Escape,
        Key::Tab | Key::Up | Key::Down | Key::Other => Decision::Default,
    }
}

/// A composer key command that travels as a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerKey {
    /// Send the message.
    Send,
    /// Pick the highlighted suggestion.
    Pick,
    /// Highlight the next suggestion.
    Next,
    /// Highlight the previous suggestion.
    Previous,
    /// Close the suggestions, or cancel reply and edit.
    Escape,
}

impl Decision {
    /// The message for a decision that is not left to the editor.
    pub fn message(self) -> Option<Message> {
        let key = match self {
            Self::Default | Self::Newline => return None,
            Self::Send => ComposerKey::Send,
            Self::Pick => ComposerKey::Pick,
            Self::Next => ComposerKey::Next,
            Self::Previous => ComposerKey::Previous,
            Self::Escape => ComposerKey::Escape,
        };
        Some(Message::ComposerKey(key))
    }
}

/// Ctrl+V (paste, which may be a picture the editor cannot take) and the
/// chords of the `keys::SHORTCUTS` table (Ctrl+K, Ctrl+U, Alt+Up, ...), which
/// the editor must neither type nor use to move the cursor. Enter, Up, Down,
/// Tab and Esc are not chords: `bind` decides those.
fn shortcut(press: &KeyPress) -> Option<Message> {
    if let Some(found) = crate::keys::shortcut_for(
        &press.modified_key,
        press.modifiers,
        crate::keys::Platform::current(),
    ) && crate::keys::composer_takes(found)
    {
        return Some(Message::Shortcut {
            shortcut: found,
            captured: false,
        });
    }
    if !press.modifiers.command() || press.modifiers.alt() {
        return None;
    }
    let keyboard::Key::Character(ch) = press.modified_key.as_ref() else {
        return None;
    };
    ch.eq_ignore_ascii_case("v")
        .then_some(Message::PasteRequested)
}

/// The adapter for `text_editor::key_binding`.
pub fn binding(press: KeyPress, suggestions_open: bool) -> Option<Binding<Message>> {
    if !matches!(press.status, Status::Focused { .. }) {
        return None;
    }
    if let Some(message) = shortcut(&press) {
        return Some(Binding::Custom(message));
    }
    let key = match press.modified_key.as_ref() {
        keyboard::Key::Named(key::Named::Enter) => Key::Enter,
        keyboard::Key::Named(key::Named::Tab) => Key::Tab,
        keyboard::Key::Named(key::Named::ArrowUp) => Key::Up,
        keyboard::Key::Named(key::Named::ArrowDown) => Key::Down,
        keyboard::Key::Named(key::Named::Escape) => Key::Escape,
        _ => Key::Other,
    };
    let input = KeyInput {
        key,
        shift: press.modifiers.shift(),
    };
    match bind(input, suggestions_open) {
        Decision::Default => Binding::from_key_press(press),
        Decision::Newline => Some(Binding::Enter),
        other => other.message().map(Binding::Custom),
    }
}

/// An editor action whose `Debug` shows only its kind: `Edit::Paste` would
/// print the pasted text.
#[derive(Clone)]
pub struct EditorAction(pub Action);

impl fmt::Debug for EditorAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match &self.0 {
            Action::Move(_) => "Move",
            Action::Select(_) => "Select",
            Action::SelectWord => "SelectWord",
            Action::SelectLine => "SelectLine",
            Action::SelectAll => "SelectAll",
            Action::Edit(Edit::Insert(_)) => "Edit::Insert",
            Action::Edit(Edit::Paste(_)) => "Edit::Paste",
            Action::Edit(Edit::Enter) => "Edit::Enter",
            Action::Edit(Edit::Indent) => "Edit::Indent",
            Action::Edit(Edit::Unindent) => "Edit::Unindent",
            Action::Edit(Edit::Backspace) => "Edit::Backspace",
            Action::Edit(Edit::Delete) => "Edit::Delete",
            Action::Click(_) => "Click",
            Action::Drag(_) => "Drag",
            Action::Scroll { .. } => "Scroll",
        };
        write!(f, "EditorAction({kind})")
    }
}

/// Widget ID of the composer's editor, focused after Reply and Edit.
pub const COMPOSER_ID: &str = "composer";

/// What the next send does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mode {
    /// A new message.
    #[default]
    Normal,
    /// A reply to the timeline item with this ID.
    Reply(String),
    /// An edit of the timeline item with this ID.
    Edit(String),
}

/// A token being typed right before the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// `@` and what follows it.
    Mention(String),
    /// `:` and what follows it.
    Emoji(String),
}

/// The token that ends at the cursor, at (`line`, byte `column`) of `text`.
pub fn token_before_cursor(text: &str, line: usize, column: usize) -> Option<Token> {
    let line = text.split('\n').nth(line)?;
    token_in_line(line, column)
}

/// Whether a token may start after `previous` (`None` at the line start).
fn is_word_start(previous: Option<char>) -> bool {
    previous.is_none_or(|c| !c.is_alphanumeric() && !matches!(c, '_' | ':' | '@'))
}

fn token_in_line(line: &str, column: usize) -> Option<Token> {
    let before = line.get(..column)?;
    // The run of non-blank characters that ends at the cursor, bounded so a
    // pasted megabyte line costs nothing.
    let start = before
        .char_indices()
        .rev()
        .take(MAX_MENTION_QUERY + 2)
        .take_while(|(_, c)| !c.is_whitespace())
        .last()
        .map_or(column, |(at, _)| at);
    let run = &before[start..];
    let starts_word = |at: usize| is_word_start(before[..start + at].chars().next_back());
    for (at, _) in run.match_indices('@') {
        let query = &run[at + 1..];
        let chars = query.chars().count();
        if (1..=MAX_MENTION_QUERY).contains(&chars) && !query.contains('@') && starts_word(at) {
            return Some(Token::Mention(query.to_owned()));
        }
    }
    let at = run.rfind(':')?;
    let query = &run[at + 1..];
    let chars = query.chars().count();
    ((2..=MAX_EMOJI_QUERY).contains(&chars)
        && query.chars().all(emoji::is_shortcode_char)
        && starts_word(at))
    .then(|| Token::Emoji(query.to_owned()))
}

/// A complete `:shortcode:` that ends at the cursor: the byte column where it
/// starts and its emoji.
pub fn closed_shortcode_before_cursor(line: &str, column: usize) -> Option<(usize, &'static str)> {
    let before = line.get(..column)?.strip_suffix(':')?;
    let body_len: usize = before
        .chars()
        .rev()
        .take_while(|c| emoji::is_shortcode_char(*c))
        .map(char::len_utf8)
        .sum();
    let start = before.len() - body_len;
    let opener = before[..start].strip_suffix(':')?;
    if !is_word_start(opener.chars().next_back()) {
        return None;
    }
    let glyph = emoji::exact(&line[start - 1..column])?;
    Some((start - 1, glyph))
}

/// What to do about the member search after a query change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchStep {
    /// Nothing.
    Idle,
    /// Send `SearchMembers` with this query now.
    Send(String),
    /// Call [`SearchGate::timer_fired`] after this long.
    Arm(Duration),
}

/// Rate limit with a trailing edge for member searches.
#[derive(Debug, Default)]
pub struct SearchGate {
    last_sent: Option<Instant>,
    wanted: Option<String>,
    pending: Option<String>,
    armed: bool,
}

impl SearchGate {
    /// The query now wanted.
    pub fn query(&mut self, query: &str, now: Instant) -> SearchStep {
        if self.wanted.as_deref() == Some(query) {
            return SearchStep::Idle;
        }
        self.wanted = Some(query.to_owned());
        let wait = self.wait(now);
        if wait.is_zero() {
            self.pending = None;
            self.last_sent = Some(now);
            return SearchStep::Send(query.to_owned());
        }
        self.pending = Some(query.to_owned());
        self.arm(wait)
    }

    fn wait(&self, now: Instant) -> Duration {
        self.last_sent.map_or(Duration::ZERO, |sent| {
            MEMBER_SEARCH_INTERVAL.saturating_sub(now.saturating_duration_since(sent))
        })
    }

    fn arm(&mut self, wait: Duration) -> SearchStep {
        if self.armed {
            SearchStep::Idle
        } else {
            self.armed = true;
            SearchStep::Arm(wait)
        }
    }

    /// The timer armed by [`SearchStep::Arm`] fired.
    pub fn timer_fired(&mut self, now: Instant) -> SearchStep {
        self.armed = false;
        let Some(query) = self.pending.take() else {
            return SearchStep::Idle;
        };
        let wait = self.wait(now);
        if wait.is_zero() {
            self.last_sent = Some(now);
            SearchStep::Send(query)
        } else {
            self.pending = Some(query);
            self.arm(wait)
        }
    }

    /// No query is wanted any more.
    pub fn cancel(&mut self) {
        self.wanted = None;
        self.pending = None;
    }
}

/// Throttle for `SetTyping` (plan Q13).
#[derive(Debug, Default)]
pub struct TypingGate {
    last_sent: Option<Instant>,
    active: bool,
}

impl TypingGate {
    /// The buffer was edited. `Some(flag)` is a `SetTyping` to send.
    pub fn edited(&mut self, non_empty: bool, now: Instant) -> Option<bool> {
        if !non_empty {
            return self.stop().then_some(false);
        }
        let due = self
            .last_sent
            .is_none_or(|sent| now.saturating_duration_since(sent) >= TYPING_INTERVAL);
        if self.active && !due {
            return None;
        }
        self.active = true;
        self.last_sent = Some(now);
        Some(true)
    }

    /// The user stopped for good (send, room switch). `true` when a
    /// `SetTyping(false)` is due.
    pub fn stop(&mut self) -> bool {
        let was_active = self.active;
        *self = Self::default();
        was_active
    }

    /// Forget everything without a notice (logout).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// The suggestion list under the editor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Suggestions {
    /// No list.
    #[default]
    Closed,
    /// Members for an `@query`; empty until the core answers.
    Mentions {
        /// The query the list belongs to.
        query: String,
        /// The members, at most [`emoji::MAX_SUGGESTIONS`].
        members: Vec<Member>,
        /// The highlighted row.
        selected: usize,
    },
    /// Emojis for a `:query`.
    Emoji {
        /// The query the list belongs to.
        query: String,
        /// The emojis.
        items: Vec<emoji::Suggestion>,
        /// The highlighted row.
        selected: usize,
    },
}

impl Suggestions {
    /// Number of rows.
    pub fn len(&self) -> usize {
        match self {
            Self::Closed => 0,
            Self::Mentions { members, .. } => members.len(),
            Self::Emoji { items, .. } => items.len(),
        }
    }

    /// Whether there is no row.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the list has rows to pick from, which changes what Enter does.
    pub fn is_open(&self) -> bool {
        !self.is_empty()
    }

    /// The highlighted row.
    pub fn selected(&self) -> usize {
        match self {
            Self::Closed => 0,
            Self::Mentions { selected, .. } | Self::Emoji { selected, .. } => *selected,
        }
    }
}

/// What a buffer change asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effects {
    /// The member search step.
    pub search: SearchStep,
    /// A `SetTyping` to send.
    pub typing: Option<bool>,
}

/// The composer state of the open room.
pub struct Composer {
    content: text_editor::Content,
    /// Mentions picked from the suggestions; only those whose label is still
    /// in the text are sent.
    pub mentions: Vec<MentionRef>,
    /// What the next send does.
    pub mode: Mode,
    /// The suggestion list.
    pub suggestions: Suggestions,
    /// Member search rate limit.
    pub search: SearchGate,
    /// Typing notice throttle.
    pub typing: TypingGate,
    /// The draft that an edit replaced in the buffer; kept in memory only,
    /// never logged (`Debug` shows no text).
    saved_draft: Option<String>,
}

// Manual impl: `Content`'s `Debug` dumps the whole buffer.
impl fmt::Debug for Composer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Composer")
            .field("len", &self.text().len())
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

impl Composer {
    /// An empty composer.
    pub fn new() -> Self {
        Self {
            content: text_editor::Content::new(),
            mentions: Vec::new(),
            mode: Mode::Normal,
            suggestions: Suggestions::Closed,
            search: SearchGate::default(),
            typing: TypingGate::default(),
            saved_draft: None,
        }
    }

    /// The buffer, for the editor widget.
    pub fn content(&self) -> &text_editor::Content {
        &self.content
    }

    /// The text, lines joined with `\n`.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for (i, line) in self.content.lines().enumerate() {
            if i > 0 {
                text.push('\n');
            }
            text.push_str(&line.text);
        }
        text
    }

    /// Whether the buffer holds nothing at all.
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Applies an editor action and reports what to send.
    pub fn apply(&mut self, action: Action, now: Instant) -> Effects {
        let is_edit = action.is_edit();
        let closing_colon = matches!(action, Action::Edit(Edit::Insert(':')));
        self.content.perform(action);
        let mut typing = None;
        if is_edit {
            if closing_colon {
                self.complete_shortcode();
            }
            self.prune_mentions();
            typing = self.typing.edited(!self.content.is_empty(), now);
        }
        let search = self.refresh_suggestions(now);
        Effects { search, typing }
    }

    /// The text of line `index`.
    fn line_text(&self, index: usize) -> Option<String> {
        self.content.line(index).map(|line| line.text.into_owned())
    }

    /// The token that ends at the cursor, when nothing is selected.
    fn current_token(&self) -> Option<Token> {
        let cursor = self.content.cursor();
        if cursor.selection.is_some() {
            return None;
        }
        let line = self.line_text(cursor.position.line)?;
        token_in_line(&line, cursor.position.column)
    }

    /// Replaces the bytes `start..end` of `line` with `text`.
    fn replace(&mut self, line: usize, start: usize, end: usize, text: String) {
        self.content.move_to(Cursor {
            position: Position { line, column: end },
            selection: Some(Position {
                line,
                column: start,
            }),
        });
        self.content
            .perform(Action::Edit(Edit::Paste(Arc::new(text))));
    }

    /// Turns a just-closed `:shortcode:` before the cursor into its emoji.
    fn complete_shortcode(&mut self) {
        let cursor = self.content.cursor();
        let Some(line) = self.line_text(cursor.position.line) else {
            return;
        };
        let column = cursor.position.column;
        if let Some((start, glyph)) = closed_shortcode_before_cursor(&line, column) {
            self.replace(cursor.position.line, start, column, glyph.to_owned());
        }
    }

    fn prune_mentions(&mut self) {
        if self.mentions.is_empty() {
            return;
        }
        let text = self.text();
        self.mentions.retain(|m| text.contains(&m.label));
    }

    /// Re-reads the token at the cursor and updates the list.
    fn refresh_suggestions(&mut self, now: Instant) -> SearchStep {
        match self.current_token() {
            None => {
                self.suggestions = Suggestions::Closed;
                self.search.cancel();
                SearchStep::Idle
            }
            Some(Token::Mention(query)) => {
                let same = matches!(&self.suggestions,
                    Suggestions::Mentions { query: q, .. } if *q == query);
                if !same {
                    // The old rows stay until the answer for the new query
                    // arrives, so the list does not flicker.
                    let members = match std::mem::take(&mut self.suggestions) {
                        Suggestions::Mentions { members, .. } => members,
                        _ => Vec::new(),
                    };
                    self.suggestions = Suggestions::Mentions {
                        query: query.clone(),
                        members,
                        selected: 0,
                    };
                }
                self.search.query(&query, now)
            }
            Some(Token::Emoji(query)) => {
                self.search.cancel();
                let same = matches!(&self.suggestions,
                    Suggestions::Emoji { query: q, .. } if *q == query);
                if !same {
                    let items = emoji::suggest(&query);
                    self.suggestions = if items.is_empty() {
                        Suggestions::Closed
                    } else {
                        Suggestions::Emoji {
                            query,
                            items,
                            selected: 0,
                        }
                    };
                }
                SearchStep::Idle
            }
        }
    }

    /// Picks suggestion `index`, or the highlighted one.
    pub fn pick(&mut self, index: Option<usize>, now: Instant) -> Option<Effects> {
        let selected = index.unwrap_or_else(|| self.suggestions.selected());
        if selected >= self.suggestions.len() {
            return None;
        }
        let cursor = self.content.cursor();
        let line = cursor.position.line;
        let end = cursor.position.column;
        let (query, text, mention) = match (&self.suggestions, self.current_token()) {
            (Suggestions::Mentions { query, members, .. }, Some(Token::Mention(typed)))
                if *query == typed =>
            {
                let member = &members[selected];
                let label = mention_label(member);
                let mention = MentionRef {
                    label: label.clone(),
                    user_id: member.user_id.clone(),
                };
                // As Element (`createMentionParts`, trailing colon on by
                // default): ": " after a mention that starts the message.
                let starts_message = line == 0 && end == typed.len() + 1;
                let suffix = if starts_message { ": " } else { " " };
                (typed, format!("{label}{suffix}"), Some(mention))
            }
            (Suggestions::Emoji { query, items, .. }, Some(Token::Emoji(typed)))
                if *query == typed =>
            {
                (typed, items[selected].glyph.to_owned(), None)
            }
            _ => {
                // The cursor left the token: the list is stale.
                self.suggestions = Suggestions::Closed;
                return None;
            }
        };
        // The token is the sigil and the query, ending at the cursor.
        self.replace(line, end - (query.len() + 1), end, text);
        if let Some(mention) = mention
            && self.mentions.len() < MAX_MENTIONS
            && !self.mentions.contains(&mention)
        {
            self.mentions.push(mention);
        }
        self.suggestions = Suggestions::Closed;
        self.search.cancel();
        let typing = self.typing.edited(true, now);
        Some(Effects {
            search: SearchStep::Idle,
            typing,
        })
    }

    /// Moves the highlight by `delta`, wrapping.
    pub fn step(&mut self, delta: isize) {
        let len = self.suggestions.len();
        let Ok(count) = isize::try_from(len) else {
            return;
        };
        if count == 0 {
            return;
        }
        let current = isize::try_from(self.suggestions.selected()).unwrap_or(0);
        let next = usize::try_from((current + delta).rem_euclid(count)).unwrap_or(0);
        if let Suggestions::Mentions { selected, .. } | Suggestions::Emoji { selected, .. } =
            &mut self.suggestions
        {
            *selected = next;
        }
    }

    /// Escape: close the list, else leave reply or edit mode. `true` when
    /// something changed.
    pub fn escape(&mut self) -> bool {
        if self.suggestions != Suggestions::Closed {
            self.suggestions = Suggestions::Closed;
            self.search.cancel();
            return true;
        }
        match self.mode {
            Mode::Normal => false,
            Mode::Reply(_) => {
                self.mode = Mode::Normal;
                true
            }
            Mode::Edit(_) => {
                // The buffer holds the old message, not a draft.
                self.restore_draft();
                true
            }
        }
    }

    /// Leaves reply or edit mode, whatever the suggestion list does (the
    /// reply bar's ✕). A reply keeps the draft; an edit gives the previous
    /// draft back. `true` when a mode was left.
    pub fn cancel_mode(&mut self) -> bool {
        match self.mode {
            Mode::Normal => false,
            Mode::Reply(_) => {
                self.mode = Mode::Normal;
                true
            }
            Mode::Edit(_) => {
                self.restore_draft();
                true
            }
        }
    }

    /// Loads the plain `body` of message `item_id` into the buffer in edit
    /// mode, keeping the current draft for later. Editing another message
    /// while an edit is open keeps the first draft.
    pub fn begin_edit(&mut self, item_id: String, body: &str) {
        let draft = self.text();
        if !matches!(self.mode, Mode::Edit(_)) {
            self.saved_draft = Some(draft);
        }
        self.set_text(body);
        self.mentions.clear();
        self.suggestions = Suggestions::Closed;
        self.search.cancel();
        self.mode = Mode::Edit(item_id);
    }

    /// Resets the composer after a send: an edit gives the previous draft
    /// back, anything else empties the buffer. The typing throttle is the
    /// caller's to stop.
    pub fn finish_send(&mut self) {
        if matches!(self.mode, Mode::Edit(_)) {
            self.restore_draft();
        } else {
            self.clear();
        }
    }

    /// Empties the buffer, then puts back the draft an edit replaced.
    fn restore_draft(&mut self) {
        let draft = self.saved_draft.take();
        self.clear();
        if let Some(draft) = draft {
            self.set_text(&draft);
        }
    }

    /// Replaces the buffer with `text`, the cursor at its end.
    fn set_text(&mut self, text: &str) {
        self.content = text_editor::Content::with_text(text);
        self.content
            .perform(Action::Move(text_editor::Motion::DocumentEnd));
    }

    /// What to keep as the room's draft: the text and the picked mentions
    /// still in it, with the item being replied to. During an edit, the
    /// draft the edit set aside (the edit itself is never kept, so it cannot
    /// come back as a new message). `None` for blank text.
    pub fn draft_parts(&self) -> Option<(String, Vec<MentionRef>, Option<&str>)> {
        let (text, mentions, reply) = match &self.mode {
            Mode::Edit(_) => (self.saved_draft.clone()?, Vec::new(), None),
            Mode::Reply(id) => (self.text(), self.mentions.clone(), Some(id.as_str())),
            Mode::Normal => (self.text(), self.mentions.clone(), None),
        };
        if text.trim().is_empty() {
            return None;
        }
        let mentions = mentions
            .into_iter()
            .filter(|m| text.contains(&m.label))
            .collect();
        Some((text, mentions, reply))
    }

    /// Puts a stored draft into an untouched composer (empty, no reply or
    /// edit). Returns `false`, changing nothing, otherwise.
    pub fn load_draft(
        &mut self,
        text: &str,
        mentions: Vec<MentionRef>,
        reply_to: Option<String>,
    ) -> bool {
        if !self.text().is_empty() || self.mode != Mode::Normal {
            return false;
        }
        self.set_text(text);
        self.mentions = mentions;
        if let Some(item) = reply_to {
            self.mode = Mode::Reply(item);
        }
        true
    }

    /// The core's answer to a member search.
    pub fn set_members(&mut self, query: &str, members: Vec<Member>) {
        if let Suggestions::Mentions {
            query: wanted,
            members: rows,
            selected,
        } = &mut self.suggestions
            && wanted == query
        {
            let mut members = members;
            members.truncate(emoji::MAX_SUGGESTIONS);
            *rows = members;
            *selected = (*selected).min(rows.len().saturating_sub(1));
        }
    }

    /// Empties the buffer and returns to a new message. The typing throttle
    /// is the caller's to stop.
    pub fn clear(&mut self) {
        self.content = text_editor::Content::new();
        self.mentions.clear();
        self.mode = Mode::Normal;
        self.suggestions = Suggestions::Closed;
        self.search.cancel();
        self.saved_draft = None;
    }
}

/// What Enter does with the composer text.
#[derive(Debug, PartialEq, Eq)]
pub enum Outgoing {
    /// Send this message.
    Send(OutgoingMessage),
    /// `/name` is no command Tessera knows: ask before sending the text as
    /// a message (Element's "Unknown Command" question).
    UnknownCommand(String),
    /// A known command, named here, without its text, such as `/me` alone
    /// (Element shows a command error).
    CommandError(String),
    /// Nothing to send.
    Nothing,
}

impl Outgoing {
    /// The message to send, if any.
    pub fn message(self) -> Option<OutgoingMessage> {
        match self {
            Self::Send(message) => Some(message),
            Self::UnknownCommand(_) | Self::CommandError(_) | Self::Nothing => None,
        }
    }
}

/// The text split as Element does (element-web v1.12.30
/// `editor/commands.tsx`, `slash-commands/SlashCommands.tsx`,
/// `SendMessageComposer.tsx`): a leading `/` (not `//`) starts a command whose
/// name runs to the first whitespace; `//` and `\/` send one slash as text.
#[derive(Debug, PartialEq, Eq)]
enum Parsed<'a> {
    Markdown(&'a str),
    Emote(&'a str),
    Plain(&'a str),
    Html(&'a str),
    Spoiler(&'a str),
    Rainbow(&'a str),
    RainbowEmote(&'a str),
    /// An emoticon command: the emoticon, then the text if any.
    Emoticon(&'static str, &'a str),
    Unknown(&'a str),
}

/// Element's emoticon commands (`slash-commands/emoticon.ts`), copied from
/// its `SlashCommands.tsx`.
fn emoticon(name: &str) -> Option<&'static str> {
    match name {
        "shrug" => Some("\u{AF}\\_(\u{30C4})_/\u{AF}"),
        "tableflip" => {
            Some("(\u{256F}\u{B0}\u{25A1}\u{B0}\u{FF09}\u{256F}\u{FE35} \u{253B}\u{2501}\u{253B}")
        }
        "unflip" => Some("\u{252C}\u{2500}\u{2500}\u{252C} \u{30CE}( \u{309C}-\u{309C}\u{30CE})"),
        "lenny" => Some("( \u{361}\u{B0} \u{35C}\u{296} \u{361}\u{B0})"),
        _ => None,
    }
}

fn parse_command(text: &str) -> Parsed<'_> {
    if text.starts_with("//") || text.starts_with("\\/") {
        return Parsed::Markdown(&text[1..]);
    }
    let Some(after) = text.strip_prefix('/') else {
        return Parsed::Markdown(text);
    };
    let (name, args) = match after.find(char::is_whitespace) {
        Some(i) => (&after[..i], after[i..].trim_start()),
        None => (after, ""),
    };
    match name {
        "me" => Parsed::Emote(args),
        "plain" => Parsed::Plain(args),
        "html" => Parsed::Html(args),
        "spoiler" => Parsed::Spoiler(args),
        "rainbow" => Parsed::Rainbow(args),
        "rainbowme" => Parsed::RainbowEmote(args),
        _ => match emoticon(name) {
            Some(prefix) => Parsed::Emoticon(prefix, args),
            None => Parsed::Unknown(name),
        },
    }
}

/// What to do with the composer text: send it, ask about an unknown command,
/// report a command without its text, or nothing (blank text). Pure.
pub fn outgoing(composer: &Composer, room_id: &RoomId) -> Outgoing {
    let text = composer.text();
    let parsed = parse_command(text.trim_end());
    let command = !matches!(parsed, Parsed::Markdown(_));
    let (kind, format, body) = match parsed {
        Parsed::Markdown(body) => (MessageKind::Text, TextFormat::Markdown, body),
        Parsed::Emote(body) => (MessageKind::Emote, TextFormat::Markdown, body),
        Parsed::Plain(body) => (MessageKind::Text, TextFormat::Plain, body),
        Parsed::Html(body) => (MessageKind::Text, TextFormat::Html, body),
        Parsed::Spoiler(body) => (MessageKind::Text, TextFormat::Spoiler, body),
        Parsed::Rainbow(body) => (MessageKind::Text, TextFormat::Rainbow, body),
        Parsed::RainbowEmote(body) => (MessageKind::Emote, TextFormat::Rainbow, body),
        Parsed::Emoticon(prefix, args) => {
            let body = if args.is_empty() {
                prefix.to_owned()
            } else {
                format!("{prefix} {args}")
            };
            return build(
                composer,
                room_id,
                MessageKind::Text,
                TextFormat::Plain,
                &body,
            )
            .map_or(Outgoing::Nothing, Outgoing::Send);
        }
        Parsed::Unknown(name) => return Outgoing::UnknownCommand(name.to_owned()),
    };
    match build(composer, room_id, kind, format, body) {
        Some(message) => Outgoing::Send(message),
        None if command => {
            // The name runs from the slash to the first whitespace.
            let name = text
                .trim_end()
                .trim_start_matches('/')
                .split(char::is_whitespace)
                .next()
                .unwrap_or_default();
            Outgoing::CommandError(name.to_owned())
        }
        None => Outgoing::Nothing,
    }
}

/// The whole text as a markdown message, slash included: "Send as message"
/// after an unknown command. `None` for blank text.
pub fn as_text(composer: &Composer, room_id: &RoomId) -> Option<OutgoingMessage> {
    let text = composer.text();
    build(
        composer,
        room_id,
        MessageKind::Text,
        TextFormat::Markdown,
        text.trim_end(),
    )
}

fn build(
    composer: &Composer,
    room_id: &RoomId,
    kind: MessageKind,
    format: TextFormat,
    body: &str,
) -> Option<OutgoingMessage> {
    if body.trim().is_empty() {
        return None;
    }
    // Picked mentions still in the text; the core links them in markdown and
    // lists them in `m.mentions` for every format, as Element does.
    let mentions = composer
        .mentions
        .iter()
        .filter(|m| body.contains(&m.label))
        .cloned()
        .collect();
    let reply_to = match &composer.mode {
        Mode::Reply(id) => Some(id.clone()),
        Mode::Normal | Mode::Edit(_) => None,
    };
    Some(OutgoingMessage {
        room_id: room_id.clone(),
        text: MessageText::from(body),
        mentions,
        reply_to,
        kind,
        format,
        thread_root: None,
    })
}

/// The label inserted for a member: `@` and the sanitised display name, or
/// the localpart when the name is missing or blank.
fn mention_label(member: &Member) -> String {
    let name = member
        .display_name
        .as_deref()
        .map(|n| one_line_capped(n, MAX_LABEL_CHARS))
        .filter(|n| !n.trim().is_empty());
    let name = name.unwrap_or_else(|| {
        let localpart = member
            .user_id
            .as_str()
            .trim_start_matches('@')
            .split(':')
            .next()
            .unwrap_or_default();
        one_line_capped(localpart, MAX_LABEL_CHARS)
    });
    format!("@{name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::api::UserId;

    fn input(key: Key, shift: bool) -> KeyInput {
        KeyInput { key, shift }
    }

    #[test]
    fn enter_sends_and_shift_enter_adds_a_line() {
        assert_eq!(bind(input(Key::Enter, false), false), Decision::Send);
        assert_eq!(bind(input(Key::Enter, true), false), Decision::Newline);
        assert_eq!(bind(input(Key::Enter, true), true), Decision::Newline);
    }

    #[test]
    fn enter_and_tab_pick_when_suggestions_are_open() {
        assert_eq!(bind(input(Key::Enter, false), true), Decision::Pick);
        assert_eq!(bind(input(Key::Tab, false), true), Decision::Pick);
        assert_eq!(bind(input(Key::Tab, false), false), Decision::Default);
    }

    #[test]
    fn arrows_move_only_in_an_open_list() {
        assert_eq!(bind(input(Key::Down, false), true), Decision::Next);
        assert_eq!(bind(input(Key::Up, false), true), Decision::Previous);
        assert_eq!(bind(input(Key::Down, false), false), Decision::Default);
        assert_eq!(bind(input(Key::Up, false), false), Decision::Default);
    }

    #[test]
    fn escape_and_other_keys() {
        assert_eq!(bind(input(Key::Escape, false), true), Decision::Escape);
        assert_eq!(bind(input(Key::Escape, false), false), Decision::Escape);
        assert_eq!(bind(input(Key::Other, false), true), Decision::Default);
    }

    #[test]
    fn decisions_map_to_messages() {
        assert!(Decision::Default.message().is_none());
        assert!(matches!(
            Decision::Send.message(),
            Some(Message::ComposerKey(ComposerKey::Send))
        ));
        assert!(Decision::Newline.message().is_none());
    }

    #[test]
    fn the_adapter_leaves_other_keys_to_the_editor() {
        let press = |key: keyboard::Key, shift: bool, text: Option<&str>| KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            modifiers: if shift {
                keyboard::Modifiers::SHIFT
            } else {
                keyboard::Modifiers::empty()
            },
            text: text.map(Into::into),
            status: Status::Focused { is_hovered: false },
        };
        let enter = keyboard::Key::Named(key::Named::Enter);
        assert!(matches!(
            binding(press(enter.clone(), false, None), false),
            Some(Binding::Custom(Message::ComposerKey(ComposerKey::Send)))
        ));
        assert!(matches!(
            binding(press(enter, true, None), false),
            Some(Binding::Enter)
        ));
        assert!(matches!(
            binding(
                press(keyboard::Key::Character("a".into()), false, Some("a")),
                false
            ),
            Some(Binding::Insert('a'))
        ));
    }

    /// A focused composer's key press.
    fn focused(key: keyboard::Key, modifiers: keyboard::Modifiers) -> KeyPress {
        KeyPress {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        }
    }

    #[test]
    fn shortcuts_typed_in_the_composer_do_not_steal_its_own_keys() {
        use crate::keys::Shortcut;
        use keyboard::Modifiers as M;
        let named = |n: key::Named| keyboard::Key::Named(n);
        let chord = |c: &str, m: M| Some(focused(keyboard::Key::Character(c.into()), m));
        let shortcut_of = |press: KeyPress| match binding(press, false) {
            Some(Binding::Custom(Message::Shortcut { shortcut, captured })) => {
                assert!(!captured);
                Some(shortcut)
            }
            _ => None,
        };
        // The chords are the app's.
        for (press, want) in [
            (chord("k", M::COMMAND), Shortcut::Switcher),
            (chord(",", M::COMMAND), Shortcut::Settings),
            (chord("u", M::COMMAND), Shortcut::Attach),
            (
                Some(focused(named(key::Named::ArrowDown), M::ALT)),
                Shortcut::NextRoom,
            ),
            (
                Some(focused(named(key::Named::ArrowUp), M::ALT)),
                Shortcut::PreviousRoom,
            ),
            (
                Some(focused(named(key::Named::ArrowDown), M::COMMAND | M::SHIFT)),
                Shortcut::NextUnread,
            ),
            (
                Some(focused(named(key::Named::ArrowUp), M::COMMAND | M::SHIFT)),
                Shortcut::PreviousUnread,
            ),
        ] {
            assert_eq!(shortcut_of(press.expect("press")), Some(want));
        }
        // Enter sends, Shift+Enter breaks the line, Up edits the last
        // message, Tab picks a suggestion: none of them is a shortcut.
        let enter = || focused(named(key::Named::Enter), M::empty());
        assert!(matches!(
            binding(enter(), false),
            Some(Binding::Custom(Message::ComposerKey(ComposerKey::Send)))
        ));
        assert!(matches!(
            binding(focused(named(key::Named::Enter), M::SHIFT), false),
            Some(Binding::Enter)
        ));
        assert!(matches!(
            binding(focused(named(key::Named::ArrowUp), M::empty()), false),
            Some(Binding::Move(_))
        ));
        assert!(matches!(
            binding(focused(named(key::Named::ArrowUp), M::empty()), true),
            Some(Binding::Custom(Message::ComposerKey(ComposerKey::Previous)))
        ));
        assert!(matches!(
            binding(focused(named(key::Named::Tab), M::empty()), true),
            Some(Binding::Custom(Message::ComposerKey(ComposerKey::Pick)))
        ));
        // Esc and a bare Tab stay with the composer's own handling.
        assert!(matches!(
            binding(focused(named(key::Named::Escape), M::empty()), false),
            Some(Binding::Custom(Message::ComposerKey(ComposerKey::Escape)))
        ));
        assert!(shortcut_of(focused(named(key::Named::Tab), M::empty())).is_none());
        // Paste is not in the table: it stays the composer's.
        assert!(matches!(
            binding(
                focused(keyboard::Key::Character("v".into()), M::COMMAND),
                false
            ),
            Some(Binding::Custom(Message::PasteRequested))
        ));
    }

    #[test]
    fn ctrl_v_and_ctrl_u_are_the_apps_paste_and_attach() {
        let press = |ch: &str, modifiers: keyboard::Modifiers| KeyPress {
            key: keyboard::Key::Character(ch.into()),
            modified_key: keyboard::Key::Character(ch.into()),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            modifiers,
            text: None,
            status: Status::Focused { is_hovered: false },
        };
        let ctrl = keyboard::Modifiers::COMMAND;
        assert!(matches!(
            binding(press("v", ctrl), false),
            Some(Binding::Custom(Message::PasteRequested))
        ));
        assert!(matches!(
            binding(press("V", ctrl | keyboard::Modifiers::SHIFT), false),
            Some(Binding::Custom(Message::PasteRequested))
        ));
        assert!(matches!(
            binding(press("u", ctrl), false),
            Some(Binding::Custom(Message::Shortcut {
                shortcut: crate::keys::Shortcut::Attach,
                captured: false
            }))
        ));
        // Without the modifier, and with Alt, the keys are ordinary.
        assert!(!matches!(
            binding(press("v", keyboard::Modifiers::empty()), false),
            Some(Binding::Custom(_))
        ));
        assert!(!matches!(
            binding(press("v", ctrl | keyboard::Modifiers::ALT), false),
            Some(Binding::Custom(_))
        ));
    }

    #[test]
    fn the_editor_action_debug_hides_pasted_text() {
        let paste = EditorAction(Action::Edit(Edit::Paste(Arc::new("secret-body-x".into()))));
        let out = format!("{paste:?}");
        assert!(!out.contains("secret-body-x"), "{out}");
        assert!(out.contains("Paste"), "{out}");
        let typed = EditorAction(Action::Edit(Edit::Insert('s')));
        let out = format!("{typed:?}");
        assert!(!out.contains('s') || out.contains("Insert"), "{out}");
        assert!(!out.contains("'s'"), "{out}");
    }

    fn token(text: &str, line: usize, column: usize) -> Option<Token> {
        token_before_cursor(text, line, column)
    }

    #[test]
    fn a_mention_token_at_line_start_and_after_space() {
        assert_eq!(token("@al", 0, 3), Some(Token::Mention("al".into())));
        assert_eq!(token("hi @al", 0, 6), Some(Token::Mention("al".into())));
        assert_eq!(token("@", 0, 1), None, "needs one character");
    }

    #[test]
    fn a_token_in_mid_word_is_none() {
        assert_eq!(token("mail a@b", 0, 8), None);
        assert_eq!(token("12:30", 0, 5), None);
        assert_eq!(token("foo:smile", 0, 9), None);
        assert_eq!(token("a @al b", 0, 7), None, "the cursor is past the token");
    }

    #[test]
    fn a_mention_query_may_hold_a_colon_for_a_user_id() {
        assert_eq!(
            token("@alice:exa", 0, 10),
            Some(Token::Mention("alice:exa".into()))
        );
    }

    #[test]
    fn a_token_after_punctuation() {
        assert_eq!(token("(@al", 0, 4), Some(Token::Mention("al".into())));
        assert_eq!(token("\"@al", 0, 4), Some(Token::Mention("al".into())));
        assert_eq!(token("(:smi", 0, 5), Some(Token::Emoji("smi".into())));
        assert_eq!(token("::smi", 0, 5), None);
    }

    #[test]
    fn an_emoji_token_needs_two_valid_characters() {
        assert_eq!(token(":s", 0, 2), None);
        assert_eq!(token(":sm", 0, 3), Some(Token::Emoji("sm".into())));
        assert_eq!(token("hi :+1", 0, 6), Some(Token::Emoji("+1".into())));
        assert_eq!(token(":SM", 0, 3), None, "lower case only");
        assert_eq!(token(":sm e", 0, 5), None);
    }

    #[test]
    fn tokens_on_other_lines() {
        let text = "first\nsecond @bo\nthird";
        assert_eq!(token(text, 1, 10), Some(Token::Mention("bo".into())));
        assert_eq!(token(text, 0, 5), None);
        assert_eq!(token(text, 2, 5), None);
        assert_eq!(token(text, 9, 0), None);
    }

    #[test]
    fn tokens_with_multi_byte_text() {
        // "é" and "日本" are 2 and 6 bytes: the column is a byte offset.
        let text = "é @日本";
        assert_eq!(
            token(text, 0, text.len()),
            Some(Token::Mention("日本".into()))
        );
        assert_eq!(token("日本@x", 0, 7), None, "after a letter");
        assert_eq!(token("é @日本", 0, 4), None, "inside a character");
        assert_eq!(token("é @日本", 0, 99), None);
    }

    #[test]
    fn a_long_query_is_not_a_token() {
        let long = format!("@{}", "a".repeat(MAX_MENTION_QUERY + 1));
        assert_eq!(token(&long, 0, long.len()), None);
        let ok = format!("@{}", "a".repeat(MAX_MENTION_QUERY));
        assert!(token(&ok, 0, ok.len()).is_some());
    }

    #[test]
    fn a_closed_shortcode_resolves_at_a_word_start() {
        assert_eq!(
            closed_shortcode_before_cursor("hi :smile:", 10),
            Some((3, "\u{1F604}"))
        );
        assert_eq!(
            closed_shortcode_before_cursor(":smile:", 7),
            Some((0, "\u{1F604}"))
        );
        assert_eq!(closed_shortcode_before_cursor("a:smile:", 8), None);
        assert_eq!(closed_shortcode_before_cursor(":nope_nope:", 11), None);
        assert_eq!(closed_shortcode_before_cursor("12:30:", 6), None);
        assert_eq!(closed_shortcode_before_cursor(":smile:", 3), None);
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn the_first_search_goes_out_at_once() {
        let t0 = Instant::now();
        let mut gate = SearchGate::default();
        assert_eq!(gate.query("a", t0), SearchStep::Send("a".into()));
        assert_eq!(
            gate.query("a", at(t0, 1000)),
            SearchStep::Idle,
            "same query"
        );
    }

    #[test]
    fn a_burst_is_coalesced_into_one_trailing_search() {
        let t0 = Instant::now();
        let mut gate = SearchGate::default();
        assert_eq!(gate.query("a", t0), SearchStep::Send("a".into()));
        assert_eq!(
            gate.query("al", at(t0, 40)),
            SearchStep::Arm(Duration::from_millis(110))
        );
        assert_eq!(gate.query("ali", at(t0, 80)), SearchStep::Idle);
        assert_eq!(gate.query("alic", at(t0, 120)), SearchStep::Idle);
        assert_eq!(
            gate.timer_fired(at(t0, 150)),
            SearchStep::Send("alic".into())
        );
        assert_eq!(gate.timer_fired(at(t0, 151)), SearchStep::Idle);
    }

    #[test]
    fn a_query_after_the_interval_is_sent_at_once() {
        let t0 = Instant::now();
        let mut gate = SearchGate::default();
        let _ = gate.query("a", t0);
        assert_eq!(gate.query("ab", at(t0, 150)), SearchStep::Send("ab".into()));
    }

    #[test]
    fn a_cancelled_query_is_not_sent_by_the_timer() {
        let t0 = Instant::now();
        let mut gate = SearchGate::default();
        let _ = gate.query("a", t0);
        let _ = gate.query("ab", at(t0, 10));
        gate.cancel();
        assert_eq!(gate.timer_fired(at(t0, 150)), SearchStep::Idle);
        // The same text can be asked again afterwards.
        assert_eq!(gate.query("ab", at(t0, 300)), SearchStep::Send("ab".into()));
    }

    #[test]
    fn an_early_timer_arms_again() {
        let t0 = Instant::now();
        let mut gate = SearchGate::default();
        let _ = gate.query("a", t0);
        let _ = gate.query("ab", at(t0, 10));
        assert_eq!(
            gate.timer_fired(at(t0, 100)),
            SearchStep::Arm(Duration::from_millis(50))
        );
    }

    #[test]
    fn typing_is_sent_at_most_every_three_seconds() {
        let t0 = Instant::now();
        let mut gate = TypingGate::default();
        assert_eq!(gate.edited(true, t0), Some(true));
        assert_eq!(gate.edited(true, at(t0, 500)), None);
        assert_eq!(gate.edited(true, at(t0, 2999)), None);
        assert_eq!(gate.edited(true, at(t0, 3000)), Some(true));
        assert_eq!(gate.edited(true, at(t0, 4000)), None);
    }

    #[test]
    fn emptying_the_buffer_stops_typing_once() {
        let t0 = Instant::now();
        let mut gate = TypingGate::default();
        assert_eq!(gate.edited(false, t0), None, "nothing to stop");
        assert_eq!(gate.edited(true, t0), Some(true));
        assert_eq!(gate.edited(false, at(t0, 100)), Some(false));
        assert_eq!(gate.edited(false, at(t0, 200)), None);
        // Typing again right away is allowed: the stop reset the state.
        assert_eq!(gate.edited(true, at(t0, 300)), Some(true));
    }

    #[test]
    fn stop_is_due_only_while_typing() {
        let t0 = Instant::now();
        let mut gate = TypingGate::default();
        assert!(!gate.stop());
        let _ = gate.edited(true, t0);
        assert!(gate.stop());
        assert!(!gate.stop());
        let _ = gate.edited(true, at(t0, 10));
        gate.reset();
        assert!(!gate.stop());
    }

    fn type_text(composer: &mut Composer, text: &str, now: Instant) {
        for c in text.chars() {
            let edit = if c == '\n' {
                Edit::Enter
            } else {
                Edit::Insert(c)
            };
            let _ = composer.apply(Action::Edit(edit), now);
        }
    }

    fn member(id: &str, name: Option<&str>) -> Member {
        Member {
            user_id: UserId::from(id),
            display_name: name.map(str::to_owned),
        }
    }

    #[test]
    fn typing_fills_the_buffer_and_debug_hides_it() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "secret-body-x\nline two", now);
        assert_eq!(c.text(), "secret-body-x\nline two");
        let out = format!("{c:?}");
        assert!(!out.contains("secret-body-x"), "{out}");
        assert!(out.contains("22"), "the length is shown: {out}");
        assert!(out.contains("Normal"), "{out}");
    }

    #[test]
    fn typing_an_at_sign_asks_for_members_and_the_answer_fills_the_list() {
        let t0 = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "hi @", t0);
        assert!(!c.suggestions.is_open());
        let fx = c.apply(Action::Edit(Edit::Insert('a')), t0);
        assert_eq!(fx.search, SearchStep::Send("a".into()));
        assert!(matches!(c.suggestions, Suggestions::Mentions { .. }));
        assert!(!c.suggestions.is_open(), "empty until the core answers");
        // A stale answer is ignored.
        c.set_members("zzz", vec![member("@z:x", None)]);
        assert!(!c.suggestions.is_open());
        c.set_members("a", vec![member("@alice:x", Some("Alice"))]);
        assert!(c.suggestions.is_open());
    }

    #[test]
    fn picking_a_member_replaces_the_token_and_records_the_mention() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "é hi @日", now);
        c.set_members("日", vec![member("@alice:x", Some("Alice \u{202E}Smith"))]);
        let fx = c.pick(None, now).expect("a suggestion was open");
        assert_eq!(fx.typing, None, "already typing");
        assert_eq!(c.text(), "é hi @Alice \u{FFFD}Smith ");
        assert_eq!(c.mentions.len(), 1);
        assert_eq!(c.mentions[0].label, "@Alice \u{FFFD}Smith");
        assert_eq!(c.mentions[0].user_id, UserId::from("@alice:x"));
        assert_eq!(c.suggestions, Suggestions::Closed);
    }

    #[test]
    fn only_a_mention_starting_the_message_gets_a_colon_like_element() {
        let now = Instant::now();
        let pick = |typed: &str, query: &str| {
            let mut c = Composer::new();
            type_text(&mut c, typed, now);
            c.set_members(query, vec![member("@alice:x", Some("Alice"))]);
            let _ = c.pick(None, now).expect("a suggestion");
            c.text()
        };
        assert_eq!(pick("@al", "al"), "@Alice: ");
        assert_eq!(pick("hi @al", "al"), "hi @Alice ");
        assert_eq!(
            pick("hi\n@al", "al"),
            "hi\n@Alice ",
            "start of a later line"
        );
    }

    #[test]
    fn a_member_without_a_name_is_inserted_by_localpart() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "@bo", now);
        c.set_members("bo", vec![member("@bob:example.org", None)]);
        let _ = c.pick(Some(0), now);
        assert_eq!(
            c.text(),
            "@bob: ",
            "a mention starting the message gets Element's colon"
        );
    }

    #[test]
    fn emoji_suggestions_pick_and_closing_colon_insert() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "ok :smi", now);
        assert!(c.suggestions.is_open());
        let _ = c.pick(None, now);
        assert!(c.text().starts_with("ok "));
        assert!(c.text().contains('\u{1F604}') || c.text().ends_with(|ch: char| !ch.is_ascii()));
        assert_eq!(c.suggestions, Suggestions::Closed);

        let mut d = Composer::new();
        type_text(&mut d, "go :rocket:", now);
        assert_eq!(d.text(), "go \u{1F680}");
    }

    #[test]
    fn the_highlight_wraps_and_a_click_picks_that_row() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, ":sm", now);
        let n = c.suggestions.len();
        assert!(n > 1);
        c.step(-1);
        assert_eq!(c.suggestions.selected(), n - 1);
        c.step(1);
        assert_eq!(c.suggestions.selected(), 0);
        assert!(c.pick(Some(n + 5), now).is_none(), "out of range");
    }

    #[test]
    fn escape_closes_the_list_then_leaves_reply_and_edit() {
        let now = Instant::now();
        let mut c = Composer::new();
        c.mode = Mode::Reply("item".into());
        type_text(&mut c, ":sm", now);
        assert!(c.escape());
        assert_eq!(c.suggestions, Suggestions::Closed);
        assert_eq!(c.mode, Mode::Reply("item".into()));
        assert!(c.escape());
        assert_eq!(c.mode, Mode::Normal);
        assert!(!c.escape());
        assert_eq!(c.text(), ":sm", "a reply keeps the draft");

        c.mode = Mode::Edit("item".into());
        assert!(c.escape());
        assert_eq!(c.text(), "", "an edit cancel drops the old text");
    }

    #[test]
    fn typing_effects_follow_the_buffer() {
        let t0 = Instant::now();
        let mut c = Composer::new();
        let fx = c.apply(Action::Edit(Edit::Insert('a')), t0);
        assert_eq!(fx.typing, Some(true));
        let fx = c.apply(Action::Edit(Edit::Insert('b')), t0 + Duration::from_secs(1));
        assert_eq!(fx.typing, None);
        let fx = c.apply(
            Action::Move(text_editor::Motion::Left),
            t0 + Duration::from_secs(9),
        );
        assert_eq!(fx.typing, None, "moving is not typing");
        let _ = c.apply(Action::Move(text_editor::Motion::Right), t0);
        let _ = c.apply(Action::Edit(Edit::Backspace), t0 + Duration::from_secs(9));
        let fx = c.apply(Action::Edit(Edit::Backspace), t0 + Duration::from_secs(9));
        assert_eq!(fx.typing, Some(false));
    }

    #[test]
    fn a_paste_is_one_edit_and_keeps_line_breaks() {
        let now = Instant::now();
        let mut c = Composer::new();
        let _ = c.apply(Action::Edit(Edit::Paste(Arc::new("a\nb\nc".into()))), now);
        assert_eq!(c.text(), "a\nb\nc");
    }

    fn composer_with(text: &str) -> Composer {
        let mut c = Composer::new();
        type_text(&mut c, text, Instant::now());
        c
    }

    fn room() -> RoomId {
        RoomId::from("!r:x")
    }

    #[test]
    fn outgoing_trims_trailing_blank_lines_and_refuses_blank_text() {
        let m = outgoing(&composer_with("**hi**\n\n  \n"), &room())
            .message()
            .expect("text");
        assert_eq!(m.text.expose(), "**hi**");
        assert_eq!(m.room_id, room());
        assert_eq!(m.kind, MessageKind::Text);
        assert!(m.reply_to.is_none() && m.mentions.is_empty());
        assert!(outgoing(&composer_with(""), &room()).message().is_none());
        assert!(
            outgoing(&composer_with(" \n\t\n"), &room())
                .message()
                .is_none()
        );
    }

    #[test]
    fn me_gives_an_emote() {
        let m = outgoing(&composer_with("/me waves"), &room())
            .message()
            .expect("emote");
        assert_eq!(m.kind, MessageKind::Emote);
        assert_eq!(m.text.expose(), "waves");
        assert!(
            outgoing(&composer_with("/me "), &room())
                .message()
                .is_none()
        );
        assert!(outgoing(&composer_with("/me"), &room()).message().is_none());
        // As in Element, `/mean` is not `/me` with an argument: it is an
        // unknown command.
        assert_eq!(
            outgoing(&composer_with("/mean"), &room()),
            Outgoing::UnknownCommand("mean".into())
        );
    }

    fn sent(text: &str) -> OutgoingMessage {
        outgoing(&composer_with(text), &room())
            .message()
            .unwrap_or_else(|| panic!("{text:?} sends nothing"))
    }

    #[test]
    fn html_and_plain_commands_set_the_format_like_element() {
        let html = sent(r#"/html <a href="https://matrix.org">click here</a>"#);
        assert_eq!(html.format, TextFormat::Html);
        assert_eq!(html.kind, MessageKind::Text);
        assert_eq!(
            html.text.expose(),
            r#"<a href="https://matrix.org">click here</a>"#
        );
        let plain = sent("/plain **not bold**");
        assert_eq!(plain.format, TextFormat::Plain);
        assert_eq!(plain.text.expose(), "**not bold**");
        assert_eq!(sent("**bold**").format, TextFormat::Markdown);
        assert_eq!(sent("/me waves").format, TextFormat::Markdown);
        assert!(
            outgoing(&composer_with("/html"), &room())
                .message()
                .is_none()
        );
        assert!(
            outgoing(&composer_with("/plain  "), &room())
                .message()
                .is_none()
        );
    }

    #[test]
    fn spoiler_and_emoticon_commands_like_element() {
        let spoiler = sent("/spoiler the butler did it");
        assert_eq!(spoiler.format, TextFormat::Spoiler);
        assert_eq!(spoiler.text.expose(), "the butler did it");
        let shrug = sent("/shrug");
        assert_eq!(shrug.format, TextFormat::Plain);
        assert_eq!(shrug.text.expose(), "\u{AF}\\_(\u{30C4})_/\u{AF}");
        assert_eq!(
            sent("/shrug fine").text.expose(),
            "\u{AF}\\_(\u{30C4})_/\u{AF} fine"
        );
        for command in ["/tableflip", "/unflip", "/lenny"] {
            let m = sent(command);
            assert_eq!(m.format, TextFormat::Plain, "{command}");
            assert!(!m.text.expose().starts_with('/'), "{command}");
        }
        assert_eq!(
            outgoing(&composer_with("/spoiler"), &room()),
            Outgoing::CommandError("spoiler".into())
        );
        let rainbow = sent("/rainbow hello");
        assert_eq!(
            (rainbow.kind, rainbow.format),
            (MessageKind::Text, TextFormat::Rainbow)
        );
        assert_eq!(rainbow.text.expose(), "hello");
        let rainbow_me = sent("/rainbowme waves");
        assert_eq!(
            (rainbow_me.kind, rainbow_me.format),
            (MessageKind::Emote, TextFormat::Rainbow)
        );
        assert_eq!(
            outgoing(&composer_with("/rainbow"), &room()),
            Outgoing::CommandError("rainbow".into())
        );
    }

    #[test]
    fn unknown_commands_ask_and_slashes_can_be_escaped() {
        for (text, name) in [
            ("/topic", "topic"),
            ("/foo bar", "foo"),
            ("/", ""),
            ("/ME hi", "ME"),
        ] {
            assert_eq!(
                outgoing(&composer_with(text), &room()),
                Outgoing::UnknownCommand(name.into()),
                "{text}"
            );
        }
        // `//` and `\/` send one slash, as text.
        assert_eq!(sent("//foo bar").text.expose(), "/foo bar");
        assert_eq!(sent("\\/foo").text.expose(), "/foo");
        assert_eq!(sent("a /foo").text.expose(), "a /foo");
    }

    #[test]
    fn sending_an_unknown_command_anyway_sends_the_whole_text() {
        let c = composer_with("/foo **bar**");
        let m = as_text(&c, &room()).expect("text");
        assert_eq!(m.text.expose(), "/foo **bar**");
        assert_eq!(
            (m.kind, m.format),
            (MessageKind::Text, TextFormat::Markdown)
        );
        assert!(as_text(&composer_with("  "), &room()).is_none());
    }

    #[test]
    fn commands_keep_their_mentions_like_element() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "/plain @al", now);
        c.set_members("al", vec![member("@alice:x", Some("Alice"))]);
        let _ = c.pick(None, now);
        let m = outgoing(&c, &room()).message().expect("plain");
        assert_eq!(m.mentions.len(), 1);
    }

    #[test]
    fn a_command_without_its_text_is_an_error_like_element() {
        for (text, name) in [
            ("/me", "me"),
            ("/me   ", "me"),
            ("/html", "html"),
            ("/plain \n ", "plain"),
            ("/rainbowme", "rainbowme"),
        ] {
            assert_eq!(
                outgoing(&composer_with(text), &room()),
                Outgoing::CommandError(name.into()),
                "{text:?}"
            );
        }
        assert_eq!(outgoing(&composer_with("  \n"), &room()), Outgoing::Nothing);
    }

    #[test]
    fn only_mentions_still_in_the_text_are_sent() {
        let now = Instant::now();
        let mut c = Composer::new();
        type_text(&mut c, "@al", now);
        c.set_members("al", vec![member("@alice:x", Some("Alice"))]);
        let _ = c.pick(None, now);
        type_text(&mut c, "and @bo", now);
        c.set_members("bo", vec![member("@bob:x", Some("Bob"))]);
        let _ = c.pick(None, now);
        let both = outgoing(&c, &room()).message().expect("text");
        assert_eq!(both.mentions.len(), 2);
        // Delete "@Bob " (5 characters).
        for _ in 0..5 {
            let _ = c.apply(Action::Edit(Edit::Backspace), now);
        }
        let one = outgoing(&c, &room()).message().expect("text");
        assert_eq!(one.mentions.len(), 1);
        assert_eq!(one.mentions[0].user_id, UserId::from("@alice:x"));
        assert_eq!(c.mentions.len(), 1, "the buffer edit also pruned the list");
    }

    #[test]
    fn reply_mode_fills_reply_to_and_edit_mode_does_not() {
        let mut c = composer_with("hello");
        c.mode = Mode::Reply("item-7".into());
        let m = outgoing(&c, &room()).message().expect("text");
        assert_eq!(m.reply_to.as_deref(), Some("item-7"));
        c.mode = Mode::Edit("item-8".into());
        let m = outgoing(&c, &room()).message().expect("text");
        assert_eq!(m.reply_to, None);
    }

    #[test]
    fn clear_resets_everything_but_the_typing_throttle() {
        let now = Instant::now();
        let mut c = composer_with("x :sm");
        c.mode = Mode::Reply("i".into());
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.mode, Mode::Normal);
        assert_eq!(c.suggestions, Suggestions::Closed);
        assert!(c.mentions.is_empty());
        let _ = now;
    }

    #[test]
    fn an_edit_loads_the_body_and_escape_gives_the_draft_back() {
        let mut c = composer_with("my draft");
        c.begin_edit("item".into(), "old text\nline two");
        assert_eq!(c.mode, Mode::Edit("item".into()));
        assert_eq!(c.text(), "old text\nline two");
        let cursor = c.content().cursor().position;
        assert_eq!((cursor.line, cursor.column), (1, 8), "cursor at the end");
        assert!(c.escape());
        assert_eq!(c.mode, Mode::Normal);
        assert_eq!(c.text(), "my draft");
        assert!(!c.escape());
    }

    #[test]
    fn editing_another_message_keeps_the_first_draft() {
        let mut c = composer_with("draft");
        c.begin_edit("a".into(), "one");
        c.begin_edit("b".into(), "two");
        assert_eq!(c.mode, Mode::Edit("b".into()));
        assert!(c.cancel_mode());
        assert_eq!(c.text(), "draft");
    }

    #[test]
    fn finishing_an_edit_restores_the_draft_and_other_sends_empty_the_buffer() {
        let mut c = composer_with("draft");
        c.begin_edit("a".into(), "one");
        c.finish_send();
        assert_eq!((c.mode.clone(), c.text()), (Mode::Normal, "draft".into()));
        c.mode = Mode::Reply("r".into());
        c.finish_send();
        assert_eq!((c.mode.clone(), c.text()), (Mode::Normal, String::new()));
    }

    #[test]
    fn cancel_mode_keeps_the_draft_of_a_reply_and_clear_drops_the_saved_one() {
        let mut c = composer_with("keep");
        c.mode = Mode::Reply("r".into());
        assert!(c.cancel_mode());
        assert_eq!((c.mode.clone(), c.text()), (Mode::Normal, "keep".into()));
        assert!(!c.cancel_mode());

        c.begin_edit("a".into(), "one");
        c.clear();
        c.begin_edit("b".into(), "two");
        c.clear();
        assert!(c.is_empty());
        c.begin_edit("c".into(), "three");
        assert!(c.cancel_mode());
        assert_eq!(c.text(), "", "clear() forgot the earlier draft");
    }
}
