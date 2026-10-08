//! The thread panel (plan Q16, Task 13): a second timeline beside the room's,
//! its composer, and how it follows the core.
//!
//! The panel shows one thread at a time, named by room and root event. The
//! core's `ThreadDiff` and `ThreadPagination` carry the same pair, and the
//! app applies them only while the pair is the open one: a late event for a
//! thread that was closed or replaced is dropped. A new root starts with a
//! fresh [`TimelineView`], so nothing of the old thread survives.
//!
//! Item IDs of the thread timeline are `thread:{root}:{n}`, unique across
//! threads, so the media cache and the action messages need no thread root.
//!
//! Thread text belongs to people: nothing here logs it and `Debug` shows
//! counts only.

use std::fmt;

use iced::Task;
use iced::widget::{operation, scrollable};
use tessera_core::api::{
    Command, CoreEvent, EventId, ListDiff, MessageKind, MessageText, OutgoingMessage,
    PaginationState, RoomId, TextFormat, ThreadSummaryView, TimelineItem,
};

use crate::screens::timeline_pane::sender_label;
use crate::state::{App, Message};
use crate::text_safety::one_line_capped;
use crate::timeline::{Effects, TimelineView};

/// Width of the panel, in pixels.
pub const PANEL_WIDTH: f32 = 340.0;

/// Widget ID of the panel's scrollable.
pub const THREAD_SCROLL_ID: &str = "thread-timeline";

/// Widget ID of the panel's text field.
pub const THREAD_INPUT_ID: &str = "thread-composer";

/// Characters of the latest sender's name kept in a summary line.
const MAX_SUMMARY_NAME_CHARS: usize = 24;
/// Characters of the latest reply's preview kept in a summary line.
const MAX_SUMMARY_PREVIEW_CHARS: usize = 60;

/// "N replies · name: preview", the line under a thread root. The name and
/// the preview come from other people: one line each, capped.
pub fn summary_text(summary: &ThreadSummaryView) -> String {
    let count = match summary.replies {
        1 => "1 reply".to_owned(),
        n => format!("{n} replies"),
    };
    let preview = summary
        .latest_preview
        .as_deref()
        .map(|p| one_line_capped(p, MAX_SUMMARY_PREVIEW_CHARS))
        .filter(|p| !p.trim().is_empty());
    let name = summary.latest_sender.as_ref().map(|sender| {
        one_line_capped(
            &sender_label(summary.latest_sender_name.as_deref(), false, sender),
            MAX_SUMMARY_NAME_CHARS,
        )
    });
    match (name, preview) {
        (Some(name), Some(preview)) => format!("{count} \u{b7} {name}: {preview}"),
        (Some(name), None) => format!("{count} \u{b7} {name}"),
        (None, _) => count,
    }
}

/// The message to send from the panel's text field: markdown into the thread
/// of `root`; `None` for a blank text.
pub fn outgoing(room_id: &RoomId, root: &EventId, draft: &str) -> Option<OutgoingMessage> {
    let text = draft.trim_end();
    if text.trim().is_empty() {
        return None;
    }
    Some(OutgoingMessage {
        room_id: room_id.clone(),
        text: MessageText::from(text),
        mentions: Vec::new(),
        reply_to: None,
        kind: MessageKind::Text,
        format: TextFormat::Markdown,
        thread_root: Some(root.clone()),
    })
}

/// The open thread: which one, its timeline and the text being typed.
pub struct ThreadPanel {
    /// Room of the thread.
    pub room_id: RoomId,
    /// The thread's root event.
    pub root: EventId,
    /// The thread's items, kept in step with the core's diffs.
    pub timeline: TimelineView,
    /// The text field's content, not sent yet.
    pub draft: String,
}

impl ThreadPanel {
    fn new(room_id: RoomId, root: EventId) -> Self {
        Self {
            room_id,
            root,
            timeline: TimelineView::default(),
            draft: String::new(),
        }
    }

    /// Whether this is the thread `room_id` and `root` name.
    pub fn is(&self, room_id: &RoomId, root: &EventId) -> bool {
        self.room_id == *room_id && self.root == *root
    }
}

impl fmt::Debug for ThreadPanel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadPanel")
            .field("room_id", &self.room_id)
            .field("root", &self.root)
            .field("timeline", &self.timeline)
            .field("draft", &self.draft.chars().count())
            .finish()
    }
}

/// The thread panel's state: the open thread, if any.
#[derive(Debug, Default)]
pub struct ThreadsModel {
    /// The open thread.
    pub panel: Option<ThreadPanel>,
}

/// A message of the thread summary rows and the panel.
#[derive(Debug, Clone)]
pub enum ThreadsMessage {
    /// A thread summary row was clicked.
    Open(EventId),
    /// The cross, or Esc.
    Close,
    /// The text field changed; the text is not logged.
    Draft(MessageText),
    /// Enter, or **Send**, in the text field.
    Submit,
    /// **Attach...** in the panel.
    Attach,
    /// The panel's timeline scrolled, or its size changed.
    Scrolled(scrollable::Viewport),
    /// The panel measured a row.
    Measured {
        /// The item's ID.
        id: String,
        /// Its height in pixels.
        height: f32,
    },
}

impl App {
    /// A message of the thread summary rows and the panel.
    pub(crate) fn on_threads(&mut self, message: ThreadsMessage) -> Task<Message> {
        match message {
            ThreadsMessage::Open(root) => self.open_thread(root),
            ThreadsMessage::Close => self.close_thread_panel(),
            ThreadsMessage::Draft(text) => {
                if let Some(panel) = &mut self.threads.panel {
                    text.expose().clone_into(&mut panel.draft);
                }
                Task::none()
            }
            ThreadsMessage::Submit => self.submit_thread(),
            ThreadsMessage::Attach => self.attach_to_thread(),
            ThreadsMessage::Scrolled(viewport) => {
                let bounds = viewport.bounds();
                // Anchored at the bottom, like the room's timeline.
                let effects = match &mut self.threads.panel {
                    Some(panel) => panel.timeline.scrolled(
                        viewport.absolute_offset().y,
                        bounds.height,
                        bounds.width,
                    ),
                    None => return Task::none(),
                };
                self.thread_effects(effects)
            }
            ThreadsMessage::Measured { id, height } => {
                let effects = match &mut self.threads.panel {
                    Some(panel) => panel.timeline.row_measured(&id, height),
                    None => return Task::none(),
                };
                self.thread_effects(effects)
            }
        }
    }

    /// Opens the thread rooted at `root` in the selected room: one
    /// `OpenThread`, and a fresh model unless it is the open one already.
    fn open_thread(&mut self, root: EventId) -> Task<Message> {
        let Some(room_id) = self.selected.clone() else {
            return Task::none();
        };
        if self
            .threads
            .panel
            .as_ref()
            .is_some_and(|panel| panel.is(&room_id, &root))
        {
            return operation::focus(THREAD_INPUT_ID);
        }
        self.drop_thread_uploads();
        self.threads.panel = Some(ThreadPanel::new(room_id.clone(), root.clone()));
        Task::batch([
            self.send(Command::OpenThread { room_id, root }),
            operation::focus(THREAD_INPUT_ID),
        ])
    }

    /// Closes the panel and the core's thread, when one is open.
    pub(crate) fn close_thread_panel(&mut self) -> Task<Message> {
        if self.threads.panel.take().is_none() {
            return Task::none();
        }
        self.drop_thread_uploads();
        self.send(Command::CloseThread)
    }

    /// Uploads started from the room's composer, a drop or a paste go to the
    /// room, not to a thread: unless a confirmation is open already (its
    /// target stays), the thread root is cleared.
    pub(crate) fn uploads_for_room(&mut self) {
        if self.uploads.dialog.is_none() {
            self.uploads.thread_root = None;
        }
    }

    /// Forgets the panel without telling the core: the room was left, or the
    /// session ended, and the core closes its thread by itself.
    pub(crate) fn reset_threads(&mut self) {
        self.threads.panel = None;
        self.drop_thread_uploads();
    }

    /// An upload confirmation that was opened from the panel goes with it.
    fn drop_thread_uploads(&mut self) {
        if self.uploads.thread_root.take().is_some() {
            self.uploads.dialog = None;
        }
    }

    fn submit_thread(&mut self) -> Task<Message> {
        let Some(panel) = &mut self.threads.panel else {
            return Task::none();
        };
        let Some(message) = outgoing(&panel.room_id, &panel.root, &panel.draft) else {
            return Task::none();
        };
        panel.draft.clear();
        self.send(Command::Send(message))
    }

    /// **Attach...** in the panel: the files go into the thread.
    fn attach_to_thread(&mut self) -> Task<Message> {
        let Some(root) = self.threads.panel.as_ref().map(|panel| panel.root.clone()) else {
            return Task::none();
        };
        self.uploads.thread_root = Some(root);
        self.attach_requested()
    }

    /// Core events of the open thread; events of any other are dropped.
    pub(crate) fn on_thread_diff(
        &mut self,
        room_id: &RoomId,
        root: &EventId,
        diffs: Vec<ListDiff<TimelineItem>>,
    ) -> Task<Message> {
        let Some(panel) = self.threads.panel.as_mut().filter(|p| p.is(room_id, root)) else {
            return Task::none();
        };
        let effects = panel.timeline.apply_diffs(diffs);
        self.thread_effects(effects)
    }

    /// Pagination state of the open thread; any other is dropped.
    pub(crate) fn on_thread_pagination(
        &mut self,
        room_id: &RoomId,
        root: &EventId,
        state: PaginationState,
    ) -> Task<Message> {
        let Some(panel) = self.threads.panel.as_mut().filter(|p| p.is(room_id, root)) else {
            return Task::none();
        };
        let effects = panel.timeline.pagination_changed(state);
        self.thread_effects(effects)
    }

    /// Turns what the panel's timeline asks for into a scroll task and a
    /// `PaginateThread`.
    fn thread_effects(&mut self, effects: Effects) -> Task<Message> {
        let Some(panel) = &self.threads.panel else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        if let Some(y) = effects.scroll_to {
            tasks.push(operation::scroll_to(
                THREAD_SCROLL_ID,
                scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(y),
                },
            ));
        }
        if effects.paginate {
            let (room_id, root) = (panel.room_id.clone(), panel.root.clone());
            tasks.push(self.send(Command::PaginateThread { room_id, root }));
        }
        tasks.push(self.fetch_visible_thread_media());
        Task::batch(tasks)
    }

    /// The thread messages among the core's events (called from `on_core`).
    pub(crate) fn thread_event(&mut self, event: CoreEvent) -> Task<Message> {
        match event {
            CoreEvent::ThreadDiff {
                room_id,
                root,
                diffs,
            } => self.on_thread_diff(&room_id, &root, diffs),
            CoreEvent::ThreadPagination {
                room_id,
                root,
                state,
            } => self.on_thread_pagination(&room_id, &root, state),
            _ => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::api::UserId;

    fn summary(replies: u32, name: Option<&str>, preview: Option<&str>) -> ThreadSummaryView {
        ThreadSummaryView {
            replies,
            latest_sender: Some(UserId::from("@carol:x")),
            latest_sender_name: name.map(str::to_owned),
            latest_preview: preview.map(str::to_owned),
        }
    }

    #[test]
    fn the_summary_line_reads_like_element() {
        assert_eq!(
            summary_text(&summary(3, Some("Carol"), Some("see you"))),
            "3 replies \u{b7} Carol: see you"
        );
        assert_eq!(
            summary_text(&summary(1, Some("Carol"), None)),
            "1 reply \u{b7} Carol"
        );
        assert_eq!(
            summary_text(&summary(2, Some("Carol"), Some("  "))),
            "2 replies \u{b7} Carol"
        );
        let nobody = ThreadSummaryView {
            replies: 5,
            latest_sender: None,
            latest_sender_name: None,
            latest_preview: Some("x".into()),
        };
        assert_eq!(summary_text(&nobody), "5 replies");
    }

    #[test]
    fn the_summary_line_is_one_capped_line() {
        let text = summary_text(&summary(
            2,
            Some("Eve\n\u{202E}evil"),
            Some(&format!("a\nb{}", "z".repeat(500))),
        ));
        assert!(
            !text.contains('\n') && !text.contains('\u{202E}'),
            "{text:?}"
        );
        assert!(text.chars().count() < 120, "{}", text.chars().count());
    }

    #[test]
    fn the_panel_sends_markdown_into_its_thread() {
        let room = RoomId::from("!r:x");
        let root = EventId::from("$root:x");
        let message = outgoing(&room, &root, "hello \n").expect("text");
        assert_eq!(message.text.expose(), "hello");
        assert_eq!(message.thread_root.as_ref(), Some(&root));
        assert_eq!(message.room_id, room);
        assert!(message.reply_to.is_none() && message.mentions.is_empty());
        assert_eq!(message.format, TextFormat::Markdown);
        assert!(outgoing(&room, &root, "  \n ").is_none());
        assert!(outgoing(&room, &root, "").is_none());
    }

    #[test]
    fn debug_shows_counts_not_text() {
        let mut panel = ThreadPanel::new(RoomId::from("!r:x"), EventId::from("$root:x"));
        panel.draft = "secret-draft".into();
        let dbg = format!("{panel:?}");
        assert!(!dbg.contains("secret-draft"), "{dbg}");
    }
}
