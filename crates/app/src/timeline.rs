//! The timeline model: item list, row heights, scroll position and
//! back-pagination (plan Q14, Q16, Q22).
//!
//! Everything here is pure data and arithmetic, so `update` can be tested
//! without a window. The pane (a later task) reads [`TimelineView::window`] and
//! builds only those rows between two spacers.
//!
//! Heights: a row has a measured height once the pane reported it, an
//! estimate before that. `prefix[i]` is the top of item `i` in the content,
//! and `prefix[n]` the content height. The scrollable is anchored at the
//! bottom, so `offset_from_bottom` is the distance from the bottom edge.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use tessera_core::api::{
    Block, EventId, Inline, ListDiff, PaginationState, RichText, SendStatus, TimelineItem,
    TimelineItemKind,
};

/// How far outside the viewport rows are built, in pixels.
pub const OVERSCAN_PX: f32 = 600.0;
/// The most rows ever built, whatever the estimates say.
pub const MAX_BUILT_ROWS: usize = 120;
/// Same sender within this many milliseconds: no new header (Q22).
pub const CONTINUATION_WINDOW_MS: u64 = 5 * 60 * 1000;
/// At most this far from the bottom counts as "at the bottom".
pub const BOTTOM_EPS_PX: f32 = 2.0;
/// A correction smaller than this is not worth a scroll operation.
pub const CORRECTION_EPS_PX: f32 = 0.5;
/// A width change beyond this drops the measured heights.
pub const WIDTH_EPS_PX: f32 = 1.0;
/// After a pagination request the user must scroll this far before the app
/// asks again without an `Idle` event in between (the core drops a request
/// that finds another one running, silently).
pub const RETRY_SCROLL_PX: f32 = 100.0;
/// Viewport height assumed until the pane reports one.
pub const DEFAULT_VIEWPORT_H: f32 = 600.0;
/// Pane width assumed until the pane reports one.
pub const DEFAULT_WIDTH: f32 = 800.0;
/// Least time between two read receipts (plan Q12).
pub const RECEIPT_MIN_INTERVAL: Duration = Duration::from_secs(1);
/// Typing users kept for display: three are enough to tell "several".
pub const MAX_TYPING_KEPT: usize = 3;
/// The id of the timeline's scrollable.
pub const TIMELINE_ID: &str = "timeline";
/// Pages (of the core's 20 events) searched for a reply's original before
/// giving up: about 1,000 messages.
pub const JUMP_MAX_PAGES: u32 = 50;
/// How long the row jumped to stays highlighted.
pub const FLASH_DURATION: Duration = Duration::from_secs(2);
/// Where the row jumped to lands: this fraction of the viewport below its top.
const JUMP_TOP_FRACTION: f32 = 0.3;

/// Anchor candidates kept on each side of the anchor item.
const CANDIDATE_REACH: usize = 8;
/// A measured height above this is not believed.
const MAX_MEASURED_PX: f32 = 50_000.0;
/// Estimated lines are capped, so one huge message cannot give an absurd
/// height.
const MAX_ESTIMATED_LINES: usize = 500;

const LINE_H: f32 = 20.0;
const HEADER_H: f32 = 22.0;
/// Space above a row that starts with a header.
pub const TOP_PAD: f32 = 8.0;
/// Space above a continuation row.
pub const CONTINUATION_PAD: f32 = 2.0;
/// Left margin of the content column (room for the header and avatar).
pub const INDENT_PX: f32 = 64.0;
/// Right margin of the content column.
pub const SIDE_PAD_PX: f32 = 16.0;
const CHAR_W: f32 = 7.5;
const MIN_CHARS_PER_LINE: usize = 8;
const BLOCK_GAP: f32 = 6.0;
const REPLY_H: f32 = 44.0;
const REACTIONS_H: f32 = 30.0;
const RECEIPTS_H: f32 = 18.0;
const FAILURE_H: f32 = 22.0;
const EDITED_H: f32 = 18.0;
/// Height of a day divider row.
pub const DIVIDER_H: f32 = 36.0;
/// Estimated height of a state event row.
pub const STATE_H: f32 = 28.0;
/// Height of the read marker row.
pub const MARKER_H: f32 = 24.0;
/// Height of the start-of-room row.
pub const START_H: f32 = 40.0;
const RULE_H: f32 = 8.0;

fn clean_width(width: f32) -> f32 {
    if width.is_finite() {
        width.max(0.0)
    } else {
        DEFAULT_WIDTH
    }
}

fn chars_per_line(width: f32, indent_chars: usize) -> usize {
    let usable = (clean_width(width) - INDENT_PX - SIDE_PAD_PX).max(0.0);
    // Truncation is the point: whole characters only.
    let chars = (usable / CHAR_W) as usize;
    chars.saturating_sub(indent_chars).max(MIN_CHARS_PER_LINE)
}

/// Lines of a run of characters in a column of `cpl` characters.
fn wrapped(chars: usize, cpl: usize) -> usize {
    chars.div_ceil(cpl).max(1)
}

/// Lines used by `inlines`: forced breaks and newlines start a new line.
fn inline_lines(inlines: &[Inline], cpl: usize) -> usize {
    fn walk(inlines: &[Inline], run: &mut usize, lines: &mut usize, cpl: usize) {
        for inline in inlines {
            match inline {
                Inline::Text { text, .. } | Inline::Link { text, .. } => {
                    let mut parts = text.split('\n');
                    if let Some(first) = parts.next() {
                        *run += first.chars().count();
                    }
                    for part in parts {
                        *lines += wrapped(*run, cpl);
                        *run = part.chars().count();
                    }
                }
                Inline::Mention { label, .. } => *run += label.chars().count(),
                Inline::LineBreak => {
                    *lines += wrapped(*run, cpl);
                    *run = 0;
                }
                Inline::Spoiler(inner) => walk(inner, run, lines, cpl),
            }
        }
    }
    let (mut run, mut lines) = (0, 0);
    walk(inlines, &mut run, &mut lines, cpl);
    (lines + wrapped(run, cpl)).min(MAX_ESTIMATED_LINES)
}

fn block_height(block: &Block, cpl: usize) -> f32 {
    match block {
        Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
            inline_lines(inlines, cpl) as f32 * LINE_H
        }
        Block::Quote(blocks) => {
            blocks_height(blocks, cpl.saturating_sub(2).max(MIN_CHARS_PER_LINE))
        }
        Block::List { items, .. } => {
            let inner = cpl.saturating_sub(4).max(MIN_CHARS_PER_LINE);
            items.iter().map(|item| blocks_height(item, inner)).sum()
        }
        Block::Code { text } => {
            let lines: usize = text
                .split('\n')
                .map(|l| wrapped(l.chars().count(), cpl))
                .sum();
            lines.min(MAX_ESTIMATED_LINES) as f32 * LINE_H
        }
        Block::Rule => RULE_H,
    }
}

fn blocks_height(blocks: &[Block], cpl: usize) -> f32 {
    let gaps = blocks.len().saturating_sub(1) as f32 * BLOCK_GAP;
    blocks.iter().map(|b| block_height(b, cpl)).sum::<f32>() + gaps
}

fn rich_height(content: &RichText, body: &str, cpl: usize) -> f32 {
    if content.blocks.is_empty() {
        let lines: usize = body
            .split('\n')
            .map(|l| wrapped(l.chars().count(), cpl))
            .sum();
        lines.min(MAX_ESTIMATED_LINES) as f32 * LINE_H
    } else {
        blocks_height(&content.blocks, cpl)
    }
}

/// The height of a row before it was measured.
///
/// `continuation` rows have no header and less padding. The estimate grows
/// with the text, and is finite for any `width`.
pub fn estimate_height(item: &TimelineItem, continuation: bool, width: f32) -> f32 {
    estimate_unclamped(item, continuation, width).min(MAX_MEASURED_PX)
}

fn estimate_unclamped(item: &TimelineItem, continuation: bool, width: f32) -> f32 {
    let lead = if continuation {
        CONTINUATION_PAD
    } else {
        TOP_PAD + HEADER_H
    };
    let receipts = if item.read_by.is_empty() {
        0.0
    } else {
        RECEIPTS_H
    };
    let cpl = chars_per_line(width, 0);
    match &item.kind {
        TimelineItemKind::Message {
            body,
            details,
            sent,
            ..
        } => {
            let mut h = lead + rich_height(&details.content, body, cpl);
            if details.reply_to.is_some() {
                h += REPLY_H;
            }
            if details.edited {
                h += EDITED_H;
            }
            if !details.reactions.is_empty() {
                h += REACTIONS_H;
            }
            if details.send_failure.is_some() || *sent != SendStatus::Sent {
                h += FAILURE_H;
            }
            h + receipts
        }
        TimelineItemKind::Placeholder { .. } => lead + LINE_H + receipts,
        TimelineItemKind::DateDivider { .. } => DIVIDER_H,
        TimelineItemKind::ReadMarker => MARKER_H,
        TimelineItemKind::TimelineStart => START_H,
        TimelineItemKind::State { .. } => STATE_H + receipts,
        TimelineItemKind::Other => 0.0,
    }
}

/// The sender and time of a message or placeholder.
fn author(kind: &TimelineItemKind) -> Option<(&str, u64)> {
    match kind {
        TimelineItemKind::Message {
            sender,
            timestamp_ms,
            ..
        }
        | TimelineItemKind::Placeholder {
            sender,
            timestamp_ms,
            ..
        } => Some((sender.as_str(), *timestamp_ms)),
        _ => None,
    }
}

/// Whether row `i` continues the sender's run above it (Q22): the item right
/// above is a message or placeholder of the same sender, less than five
/// minutes apart. A divider, state event, marker or start item in between
/// ends the run.
pub fn is_continuation(items: &[TimelineItem], i: usize) -> bool {
    let (Some(prev), Some(item)) = (i.checked_sub(1).and_then(|p| items.get(p)), items.get(i))
    else {
        return false;
    };
    match (author(&prev.kind), author(&item.kind)) {
        (Some((a, ta)), Some((b, tb))) => a == b && ta.abs_diff(tb) < CONTINUATION_WINDOW_MS,
        _ => false,
    }
}

/// The height of row `i`: measured, else estimated.
fn row_height(
    items: &[TimelineItem],
    i: usize,
    measured: &HashMap<String, f32>,
    width: f32,
) -> f32 {
    let item = &items[i];
    measured
        .get(&item.id)
        .copied()
        .unwrap_or_else(|| estimate_height(item, is_continuation(items, i), width))
}

/// Tops of all rows: `prefix[i]` is where item `i` starts, `prefix[n]` is the
/// content height.
pub fn rebuild_prefix(
    items: &[TimelineItem],
    measured: &HashMap<String, f32>,
    width: f32,
) -> Vec<f32> {
    let mut prefix = Vec::with_capacity(items.len() + 1);
    let mut top = 0.0;
    prefix.push(top);
    for i in 0..items.len() {
        top += row_height(items, i, measured, width);
        prefix.push(top);
    }
    prefix
}

fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// The content-space y of the viewport top for an offset from the bottom,
/// with the offset clamped to what the content allows.
fn viewport_top(total: f32, offset_from_bottom: f32, viewport_h: f32) -> f32 {
    let vh = finite_or_zero(viewport_h);
    let max_offset = (total - vh).max(0.0);
    let offset = if offset_from_bottom.is_nan() {
        0.0
    } else {
        offset_from_bottom.clamp(0.0, max_offset)
    };
    (total - offset - vh).max(0.0)
}

/// The index of the item that contains content y `y`.
fn item_at(prefix: &[f32], y: f32) -> usize {
    let n = prefix.len().saturating_sub(1);
    prefix[..n].partition_point(|&p| p <= y).saturating_sub(1)
}

/// The rows to build for a viewport: those that touch it, widened by
/// `overscan_px` on both sides, at most [`MAX_BUILT_ROWS`] (the row at the
/// viewport top is always kept). Bad numbers are treated as zero.
pub fn visible_window(
    prefix: &[f32],
    offset_from_bottom: f32,
    viewport_h: f32,
    overscan_px: f32,
) -> Range<usize> {
    let n = prefix.len().saturating_sub(1);
    if n == 0 {
        return 0..0;
    }
    let total = prefix[n];
    let overscan = finite_or_zero(overscan_px);
    let top = viewport_top(total, offset_from_bottom, viewport_h);
    let bottom = (top + finite_or_zero(viewport_h)).min(total).max(top);
    let first = item_at(prefix, top - overscan);
    let end = prefix[..n]
        .partition_point(|&p| p < bottom + overscan)
        .clamp(first + 1, n);
    if end - first <= MAX_BUILT_ROWS {
        return first..end;
    }
    let start = item_at(prefix, top)
        .saturating_sub(MAX_BUILT_ROWS / 4)
        .max(first);
    start..(start + MAX_BUILT_ROWS).min(n)
}

/// Heights of the spacers above and below the built `window`, so that
/// spacers plus built rows equal the content height.
pub fn spacer_heights(prefix: &[f32], window: &Range<usize>) -> (f32, f32) {
    let n = prefix.len().saturating_sub(1);
    if n == 0 {
        return (0.0, 0.0);
    }
    let start = window.start.min(n);
    let end = window.end.clamp(start, n);
    (prefix[start], prefix[n] - prefix[end])
}

/// The top visible item and where its top lies relative to the viewport top
/// (zero or negative when it is cut). `None` at the bottom, where the view
/// follows new messages instead.
pub fn record_anchor(
    items: &[TimelineItem],
    prefix: &[f32],
    offset_from_bottom: f32,
    viewport_h: f32,
) -> Option<(String, f32)> {
    if items.is_empty() || prefix.len() != items.len() + 1 {
        return None;
    }
    let total = prefix[items.len()];
    let max_offset = (total - finite_or_zero(viewport_h)).max(0.0);
    if offset_from_bottom.is_nan() || offset_from_bottom.min(max_offset) <= BOTTOM_EPS_PX {
        return None;
    }
    let top = viewport_top(total, offset_from_bottom, viewport_h);
    let i = item_at(prefix, top);
    Some((items[i].id.clone(), prefix[i] - top))
}

/// The anchor and its neighbours (alternating below and above, nearest first),
/// each with its position relative to the viewport top. Taken before a change,
/// so that [`anchor_correction`] still has an item to hold on to when the
/// anchor itself disappears.
pub fn anchor_candidates(
    items: &[TimelineItem],
    prefix: &[f32],
    anchor: &(String, f32),
    offset_from_bottom: f32,
    viewport_h: f32,
) -> Vec<(String, f32)> {
    if prefix.len() != items.len() + 1 {
        return Vec::new();
    }
    let Some(k) = items.iter().position(|i| i.id == anchor.0) else {
        return Vec::new();
    };
    let top = viewport_top(prefix[items.len()], offset_from_bottom, viewport_h);
    let mut out = Vec::with_capacity(2 * CANDIDATE_REACH + 1);
    let mut push = |i: usize| out.push((items[i].id.clone(), prefix[i] - top));
    push(k);
    for d in 1..=CANDIDATE_REACH {
        if k + d < items.len() {
            push(k + d);
        }
        if let Some(i) = k.checked_sub(d) {
            push(i);
        }
    }
    out
}

/// The bottom offset that keeps the first surviving candidate where it was in
/// the viewport, or `None` when the view should stay as it is: at the bottom,
/// no candidate survives, or the difference is under half a pixel.
pub fn anchor_correction(
    candidates: &[(String, f32)],
    items: &[TimelineItem],
    prefix: &[f32],
    offset_from_bottom: f32,
    viewport_h: f32,
) -> Option<f32> {
    if offset_from_bottom.is_nan()
        || offset_from_bottom <= BOTTOM_EPS_PX
        || prefix.len() != items.len() + 1
    {
        return None;
    }
    let total = prefix[items.len()];
    let max_offset = (total - finite_or_zero(viewport_h)).max(0.0);
    // Only the first surviving candidate counts: a later one would hold a
    // different item and could ask for a different offset.
    let (j, pos) = candidates
        .iter()
        .find_map(|(id, pos)| Some((items.iter().position(|i| i.id == *id)?, *pos)))?;
    let top = prefix[j] - pos;
    let wanted = (total - finite_or_zero(viewport_h) - top).clamp(0.0, max_offset);
    ((wanted - offset_from_bottom).abs() > CORRECTION_EPS_PX).then_some(wanted)
}

/// Q16: ask for older events when nothing is loading, the start of the room
/// is not reached, no request is pending in this `Idle` period, and the top of
/// the content is within one viewport above the viewport (`top_gap_px` is that
/// distance; negative when the content is shorter than the viewport).
pub fn should_paginate(
    state: PaginationState,
    top_gap_px: f32,
    viewport_h: f32,
    already_requested: bool,
) -> bool {
    state == PaginationState::Idle
        && !already_requested
        && top_gap_px.is_finite()
        && top_gap_px <= viewport_h
}

/// What to do about the read receipt now (see [`TimelineView::read_step`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadStep {
    /// Nothing.
    Idle,
    /// Send `MarkRead { fully_read: false }` now.
    Send,
    /// Too soon after the last one: ask again after this long.
    Wait(Duration),
}

/// What the app has to do after a change of the view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Effects {
    /// Scroll the timeline to this offset from the bottom.
    pub scroll_to: Option<f32>,
    /// Send `PaginateBackwards`.
    pub paginate: bool,
    /// A jump reached this item: end its highlight after [`FLASH_DURATION`].
    pub flash: Option<String>,
    /// A jump gave up: the original is not in the room's loadable history.
    pub jump_failed: bool,
}

/// A search for a reply's original (see [`TimelineView::jump_to`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    /// The event looked for.
    pub event_id: EventId,
    /// Pages that may still be asked for.
    pub pages_left: u32,
    /// A page was asked for in this `Idle` period.
    asked: bool,
}

/// Applies a diff, taking ownership so that a `Reset` moves the new list in
/// instead of copying it. An out-of-range index is ignored, as in
/// [`ListDiff::apply`].
fn apply_diff(list: &mut Vec<TimelineItem>, diff: ListDiff<TimelineItem>) {
    match diff {
        ListDiff::Append { mut values } => list.append(&mut values),
        ListDiff::Clear => list.clear(),
        ListDiff::PushFront { value } => list.insert(0, value),
        ListDiff::PushBack { value } => list.push(value),
        ListDiff::PopFront => {
            if !list.is_empty() {
                list.remove(0);
            }
        }
        ListDiff::PopBack => {
            list.pop();
        }
        ListDiff::Insert { index, value } => {
            if index <= list.len() {
                list.insert(index, value);
            }
        }
        ListDiff::Set { index, value } => {
            if let Some(slot) = list.get_mut(index) {
                *slot = value;
            }
        }
        ListDiff::Remove { index } => {
            if index < list.len() {
                list.remove(index);
            }
        }
        ListDiff::Truncate { length } => list.truncate(length),
        ListDiff::Reset { values } => *list = values,
    }
}

/// The selected room's timeline and everything needed to draw a window of it.
pub struct TimelineView {
    /// The items, in order.
    pub items: Vec<TimelineItem>,
    /// Row heights reported by the pane, by item ID.
    pub measured: HashMap<String, f32>,
    /// Tops of the rows; one entry more than `items` (see [`rebuild_prefix`]).
    pub prefix: Vec<f32>,
    /// Distance of the viewport bottom from the content bottom.
    pub offset_from_bottom: f32,
    /// Height of the viewport.
    pub viewport_h: f32,
    /// Width of the viewport.
    pub width: f32,
    /// The top visible item and its position in the viewport; `None` at the
    /// bottom.
    pub anchor: Option<(String, f32)>,
    /// Back-pagination state of this room.
    pub pagination: PaginationState,
    /// The item for which a read receipt was last sent.
    pub last_marked: Option<String>,
    /// The bottom of the timeline was in view at some point since the room
    /// was opened, so the read marker may move when the room is left.
    pub bottom_seen: bool,
    /// When the last receipt was sent.
    last_receipt_at: Option<Instant>,
    /// A timer for a delayed receipt is running.
    pub(crate) receipt_timer_armed: bool,
    /// The offset of the last scroll operation the model asked for. Tasks are
    /// opaque, so tests read it here.
    pub last_scroll_request: Option<f32>,
    /// The offset from the bottom at which `PaginateBackwards` was sent in
    /// this `Idle` period.
    requested_at: Option<f32>,
    /// A running search for a reply's original.
    pub jump: Option<Jump>,
    /// The row highlighted after a jump.
    pub flash: Option<String>,
}

impl Default for TimelineView {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            measured: HashMap::new(),
            prefix: vec![0.0],
            offset_from_bottom: 0.0,
            viewport_h: DEFAULT_VIEWPORT_H,
            width: DEFAULT_WIDTH,
            anchor: None,
            pagination: PaginationState::Idle,
            last_marked: None,
            bottom_seen: false,
            last_receipt_at: None,
            receipt_timer_armed: false,
            last_scroll_request: None,
            requested_at: None,
            jump: None,
            flash: None,
        }
    }
}

// Manual impl: print sizes, never item content.
impl std::fmt::Debug for TimelineView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineView")
            .field("items", &self.items.len())
            .field("measured", &self.measured.len())
            .field("offset_from_bottom", &self.offset_from_bottom)
            .field("pagination", &self.pagination)
            .finish_non_exhaustive()
    }
}

impl TimelineView {
    /// Forgets the room's content for a new selection. The pane's size stays.
    pub fn reset(&mut self) {
        *self = Self {
            viewport_h: self.viewport_h,
            width: self.width,
            ..Self::default()
        };
    }

    /// The rows to build now.
    pub fn window(&self) -> Range<usize> {
        visible_window(
            &self.prefix,
            self.offset_from_bottom,
            self.viewport_h,
            OVERSCAN_PX,
        )
    }

    /// The item a read receipt should point at: the newest event from
    /// another user that the server has. Dividers, markers, local echoes and
    /// the user's own messages are skipped.
    pub fn newest_receipt_target(&self) -> Option<&str> {
        self.items
            .iter()
            .rev()
            .find(|item| match &item.kind {
                TimelineItemKind::Message { sent, is_own, .. } => {
                    *sent == SendStatus::Sent && !*is_own
                }
                TimelineItemKind::Placeholder { is_own, .. } => !*is_own,
                TimelineItemKind::State { .. } => true,
                TimelineItemKind::DateDivider { .. }
                | TimelineItemKind::ReadMarker
                | TimelineItemKind::TimelineStart
                | TimelineItemKind::Other => false,
            })
            .map(|item| item.id.as_str())
    }

    /// Decides about the read receipt (plan Q12): one is due when the window
    /// has focus, the bottom is in view and the newest target differs from
    /// the last one marked. At most one is sent per
    /// [`RECEIPT_MIN_INTERVAL`]; a receipt that comes too early asks for a
    /// timer once, and the caller calls [`TimelineView::read_timer_fired`]
    /// and asks again when it ends.
    pub fn read_step(&mut self, focused: bool, now: Instant) -> ReadStep {
        let at_bottom = self.offset_from_bottom <= BOTTOM_EPS_PX;
        let Some(target) = self.newest_receipt_target().map(str::to_owned) else {
            return ReadStep::Idle;
        };
        if !at_bottom {
            return ReadStep::Idle;
        }
        if !focused {
            return ReadStep::Idle;
        }
        self.bottom_seen = true;
        if self.last_marked.as_deref() == Some(target.as_str()) {
            return ReadStep::Idle;
        }
        if let Some(sent) = self.last_receipt_at {
            let waited = now.saturating_duration_since(sent);
            if waited < RECEIPT_MIN_INTERVAL {
                if self.receipt_timer_armed {
                    return ReadStep::Idle;
                }
                self.receipt_timer_armed = true;
                return ReadStep::Wait(RECEIPT_MIN_INTERVAL - waited);
            }
        }
        self.last_marked = Some(target);
        self.last_receipt_at = Some(now);
        ReadStep::Send
    }

    /// The timer asked for by [`ReadStep::Wait`] ended.
    pub fn read_timer_fired(&mut self) {
        self.receipt_timer_armed = false;
    }

    /// Whether the first item is the start of the room.
    fn has_start_item(&self) -> bool {
        matches!(
            self.items.first().map(|i| &i.kind),
            Some(TimelineItemKind::TimelineStart)
        )
    }

    fn total(&self) -> f32 {
        self.prefix.last().copied().unwrap_or(0.0)
    }

    /// Candidates to hold on to across a change, when scrolled up.
    fn candidates(&self) -> Vec<(String, f32)> {
        match &self.anchor {
            Some(anchor) if self.offset_from_bottom > BOTTOM_EPS_PX => anchor_candidates(
                &self.items,
                &self.prefix,
                anchor,
                self.offset_from_bottom,
                self.viewport_h,
            ),
            _ => Vec::new(),
        }
    }

    /// Re-anchors after the layout changed, then looks at pagination.
    fn settle(&mut self, candidates: &[(String, f32)]) -> Effects {
        let mut effects = Effects::default();
        if let Some(wanted) = anchor_correction(
            candidates,
            &self.items,
            &self.prefix,
            self.offset_from_bottom,
            self.viewport_h,
        ) {
            self.offset_from_bottom = wanted;
            self.last_scroll_request = Some(wanted);
            effects.scroll_to = Some(wanted);
        }
        self.anchor = record_anchor(
            &self.items,
            &self.prefix,
            self.offset_from_bottom,
            self.viewport_h,
        );
        effects.paginate = self.check_pagination();
        effects
    }

    /// Scrolls to the message of `event_id` and highlights it, or searches
    /// older history for it, a page at a time (at most [`JUMP_MAX_PAGES`]).
    pub fn jump_to(&mut self, event_id: EventId) -> Effects {
        self.jump = Some(Jump {
            event_id,
            pages_left: JUMP_MAX_PAGES,
            asked: false,
        });
        let mut effects = Effects::default();
        self.drive_jump(&mut effects, true);
        effects
    }

    /// The highlight timer of `id` ended.
    pub fn flash_ended(&mut self, id: &str) {
        if self.flash.as_deref() == Some(id) {
            self.flash = None;
        }
    }

    /// Advances a running jump. `items_changed` is true only for new items
    /// (or the jump's start): the core reports `Idle` before a page's items
    /// arrive, so the jump may give up only once they are in.
    fn drive_jump(&mut self, effects: &mut Effects, items_changed: bool) {
        let Some(jump) = &self.jump else {
            return;
        };
        let found = self
            .items
            .iter()
            .position(|item| item.event_id.as_ref() == Some(&jump.event_id));
        if let Some(i) = found {
            self.jump = None;
            effects.scroll_to = Some(self.scroll_to_item(i));
            let id = self.items[i].id.clone();
            self.flash = Some(id.clone());
            effects.flash = Some(id);
            return;
        }
        let at_start = self.has_start_item() || self.pagination == PaginationState::ReachedStart;
        let Some(jump) = &mut self.jump else {
            return;
        };
        if jump.asked {
            return;
        }
        if jump.pages_left == 0 || at_start {
            if items_changed {
                self.jump = None;
                effects.jump_failed = true;
            }
            return;
        }
        if self.pagination == PaginationState::Idle {
            jump.asked = true;
            jump.pages_left -= 1;
            effects.paginate = true;
        }
    }

    /// Puts item `i` [`JUMP_TOP_FRACTION`] of the viewport below its top (as
    /// near as the content allows) and returns the new offset.
    fn scroll_to_item(&mut self, i: usize) -> f32 {
        let total = self.total();
        let vh = self.viewport_h;
        let top = (self.prefix[i] - vh * JUMP_TOP_FRACTION).max(0.0);
        let max_offset = (total - vh).max(0.0);
        let offset = (total - vh - top).clamp(0.0, max_offset);
        self.offset_from_bottom = offset;
        self.last_scroll_request = Some(offset);
        self.anchor = record_anchor(&self.items, &self.prefix, offset, vh);
        offset
    }

    /// Q16, once per `Idle` period (see [`RETRY_SCROLL_PX`]).
    fn check_pagination(&mut self) -> bool {
        if self.has_start_item() {
            return false;
        }
        let gap = self.total() - self.offset_from_bottom - self.viewport_h;
        let asked = self
            .requested_at
            .is_some_and(|at| (self.offset_from_bottom - at).abs() < RETRY_SCROLL_PX);
        let go = should_paginate(self.pagination, gap, self.viewport_h, asked);
        if go {
            self.requested_at = Some(self.offset_from_bottom);
        }
        go
    }

    /// Applies a batch of diffs from the core.
    pub fn apply_diffs(&mut self, diffs: Vec<ListDiff<TimelineItem>>) -> Effects {
        let candidates = self.candidates();
        for diff in diffs {
            apply_diff(&mut self.items, diff);
        }
        if self.measured.len() > self.items.len() + 256 {
            let live: HashSet<&str> = self.items.iter().map(|i| i.id.as_str()).collect();
            self.measured.retain(|id, _| live.contains(id.as_str()));
        }
        self.prefix = rebuild_prefix(&self.items, &self.measured, self.width);
        let mut effects = self.settle(&candidates);
        self.drive_jump(&mut effects, true);
        effects
    }

    /// Records the measured height of a row. Unknown items and heights that
    /// are not a positive finite number are ignored.
    pub fn row_measured(&mut self, id: &str, height: f32) -> Effects {
        if !(height.is_finite() && height > 0.0) {
            return Effects::default();
        }
        let height = height.min(MAX_MEASURED_PX);
        let Some(i) = self.items.iter().position(|item| item.id == id) else {
            return Effects::default();
        };
        let delta = height - (self.prefix[i + 1] - self.prefix[i]);
        if self.measured.get(id) == Some(&height) {
            return Effects::default();
        }
        let candidates = self.candidates();
        self.measured.insert(id.to_owned(), height);
        for top in &mut self.prefix[i + 1..] {
            *top += delta;
        }
        self.settle(&candidates)
    }

    /// The pane scrolled or changed size. A width change beyond one pixel
    /// drops the measured heights, because lines wrap differently.
    pub fn scrolled(&mut self, offset_from_bottom: f32, viewport_h: f32, width: f32) -> Effects {
        if viewport_h.is_finite() && viewport_h > 0.0 {
            self.viewport_h = viewport_h;
        }
        let width_changed =
            width.is_finite() && width > 0.0 && (width - self.width).abs() > WIDTH_EPS_PX;
        if width_changed {
            self.width = width;
            self.measured.clear();
            self.prefix = rebuild_prefix(&self.items, &self.measured, self.width);
        }
        self.offset_from_bottom = finite_or_zero(offset_from_bottom);
        self.anchor = record_anchor(
            &self.items,
            &self.prefix,
            self.offset_from_bottom,
            self.viewport_h,
        );
        Effects {
            paginate: self.check_pagination(),
            ..Effects::default()
        }
    }

    /// The core reported a new pagination state of this room.
    pub fn pagination_changed(&mut self, state: PaginationState) -> Effects {
        self.pagination = state;
        if state != PaginationState::Loading {
            // A new period: a request may be sent again.
            self.requested_at = None;
            if let Some(jump) = &mut self.jump {
                jump.asked = false;
            }
        }
        let mut effects = Effects {
            paginate: self.check_pagination(),
            ..Effects::default()
        };
        self.drive_jump(&mut effects, false);
        effects
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use tessera_core::api::{
        Block, Command, CoreEvent, EventId, Inline, ListDiff, MessageDetails, PaginationState,
        PlaceholderKind, ReactionGroup, RichText, RoomId, RoomSummary, SendStatus, StateChange,
        TextStyle, TimelineItem, TimelineItemKind, UserId,
    };
    use tokio::sync::mpsc;

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::state::{App, Message, Startup};

    const MIN: u64 = 60_000;

    fn msg_with(id: &str, sender: &str, ts: u64, details: MessageDetails) -> TimelineItem {
        TimelineItem::new(
            id.to_owned(),
            TimelineItemKind::Message {
                sender: UserId::from(sender),
                body: String::new(),
                is_own: false,
                timestamp_ms: ts,
                sent: SendStatus::Sent,
                details: Box::new(details),
            },
        )
    }

    fn msg(id: &str, sender: &str, ts: u64) -> TimelineItem {
        msg_with(id, sender, ts, MessageDetails::default())
    }

    fn text_block(len: usize) -> Block {
        Block::Paragraph(vec![Inline::Text {
            text: "x".repeat(len),
            style: TextStyle::default(),
        }])
    }

    fn rich(blocks: Vec<Block>) -> MessageDetails {
        MessageDetails {
            content: RichText { blocks },
            ..MessageDetails::default()
        }
    }

    fn divider(id: &str) -> TimelineItem {
        TimelineItem::new(
            id.to_owned(),
            TimelineItemKind::DateDivider { timestamp_ms: 0 },
        )
    }

    fn state_item(id: &str) -> TimelineItem {
        TimelineItem::new(
            id.to_owned(),
            TimelineItemKind::State {
                sender: UserId::from("@a:x"),
                sender_name: None,
                timestamp_ms: 0,
                change: StateChange::Joined,
            },
        )
    }

    fn placeholder(id: &str, sender: &str, ts: u64) -> TimelineItem {
        TimelineItem::new(
            id.to_owned(),
            TimelineItemKind::Placeholder {
                sender: UserId::from(sender),
                sender_name: None,
                is_own: false,
                timestamp_ms: ts,
                what: PlaceholderKind::Redacted,
                utd: None,
            },
        )
    }

    fn plain(kind: TimelineItemKind) -> TimelineItem {
        TimelineItem::new("p".to_owned(), kind)
    }

    // ---- estimate_height --------------------------------------------------

    #[test]
    fn a_header_makes_a_message_taller_than_a_continuation() {
        let item = msg_with("a", "@a:x", 0, rich(vec![text_block(20)]));
        assert!(estimate_height(&item, false, 800.0) > estimate_height(&item, true, 800.0));
    }

    #[test]
    fn narrower_panes_wrap_into_more_lines() {
        let item = msg_with("a", "@a:x", 0, rich(vec![text_block(600)]));
        assert!(estimate_height(&item, false, 300.0) > estimate_height(&item, false, 1200.0));
    }

    #[test]
    fn height_never_shrinks_when_the_text_grows() {
        let mut last = 0.0_f32;
        for len in (0..3000).step_by(7) {
            let item = msg_with("a", "@a:x", 0, rich(vec![text_block(len)]));
            let h = estimate_height(&item, false, 640.0);
            assert!(h >= last, "len {len}: {h} < {last}");
            last = h;
        }
    }

    #[test]
    fn more_blocks_and_newlines_are_taller() {
        let one = msg_with("a", "@a:x", 0, rich(vec![text_block(10)]));
        let three = msg_with(
            "a",
            "@a:x",
            0,
            rich(vec![text_block(10), text_block(10), text_block(10)]),
        );
        assert!(estimate_height(&three, false, 800.0) > estimate_height(&one, false, 800.0));
        let breaks = msg_with(
            "a",
            "@a:x",
            0,
            rich(vec![Block::Paragraph(vec![
                Inline::Text {
                    text: "a".into(),
                    style: TextStyle::default(),
                },
                Inline::LineBreak,
                Inline::Text {
                    text: "b".into(),
                    style: TextStyle::default(),
                },
            ])]),
        );
        assert!(estimate_height(&breaks, false, 800.0) > estimate_height(&one, false, 800.0));
    }

    #[test]
    fn reactions_and_receipts_add_a_row_each() {
        let base = msg_with("a", "@a:x", 0, rich(vec![text_block(10)]));
        let mut reacted = base.clone();
        if let TimelineItemKind::Message { details, .. } = &mut reacted.kind {
            details.reactions.push(ReactionGroup {
                key: "+".into(),
                count: 1,
                own: false,
                senders: Vec::new(),
            });
        }
        let mut read = base.clone();
        read.read_by.push(UserId::from("@b:x"));
        let h = estimate_height(&base, false, 800.0);
        assert!(estimate_height(&reacted, false, 800.0) > h);
        assert!(estimate_height(&read, false, 800.0) > h);
    }

    #[test]
    fn every_kind_has_a_finite_height_even_for_hostile_widths() {
        let items = [
            msg("m", "@a:x", 0),
            divider("d"),
            state_item("s"),
            placeholder("p", "@a:x", 0),
            plain(TimelineItemKind::ReadMarker),
            plain(TimelineItemKind::TimelineStart),
            plain(TimelineItemKind::Other),
        ];
        for item in &items {
            for width in [f32::NAN, -5.0, 0.0, 1.0, f32::INFINITY, 800.0] {
                let h = estimate_height(item, false, width);
                assert!(h.is_finite() && h >= 0.0, "{item:?} at {width}: {h}");
            }
        }
    }

    // ---- is_continuation --------------------------------------------------

    #[test]
    fn same_sender_under_five_minutes_continues() {
        let items = [msg("a", "@a:x", 0), msg("b", "@a:x", 4 * MIN + 59_000)];
        assert!(!is_continuation(&items, 0));
        assert!(is_continuation(&items, 1));
        let items = [msg("a", "@a:x", 0), msg("b", "@a:x", 5 * MIN)];
        assert!(!is_continuation(&items, 1));
    }

    #[test]
    fn a_different_sender_does_not_continue() {
        let items = [msg("a", "@a:x", 0), msg("b", "@b:x", 1)];
        assert!(!is_continuation(&items, 1));
    }

    #[test]
    fn a_divider_state_or_marker_in_between_breaks_the_run() {
        for between in [
            divider("d"),
            state_item("s"),
            plain(TimelineItemKind::ReadMarker),
            plain(TimelineItemKind::TimelineStart),
            plain(TimelineItemKind::Other),
        ] {
            let items = [msg("a", "@a:x", 0), between, msg("b", "@a:x", 1)];
            assert!(!is_continuation(&items, 2));
        }
    }

    #[test]
    fn placeholders_group_with_messages_of_the_same_sender() {
        let items = [
            msg("a", "@a:x", 0),
            placeholder("p", "@a:x", 1),
            placeholder("q", "@a:x", 2),
        ];
        assert!(is_continuation(&items, 1));
        assert!(is_continuation(&items, 2));
        let items = [msg("a", "@a:x", 0), placeholder("p", "@b:x", 1)];
        assert!(!is_continuation(&items, 1));
    }

    #[test]
    fn continuation_is_false_for_the_first_item_and_out_of_range() {
        let items = [msg("a", "@a:x", 0)];
        assert!(!is_continuation(&items, 0));
        assert!(!is_continuation(&items, 5));
        assert!(!is_continuation(&[], 0));
    }

    #[test]
    fn a_timestamp_going_backwards_is_measured_by_distance() {
        let items = [msg("a", "@a:x", 10 * MIN), msg("b", "@a:x", 10 * MIN - 1)];
        assert!(is_continuation(&items, 1));
        let items = [msg("a", "@a:x", 10 * MIN), msg("b", "@a:x", 0)];
        assert!(!is_continuation(&items, 1));
    }

    // ---- rebuild_prefix ---------------------------------------------------

    #[test]
    fn the_prefix_has_one_more_entry_than_the_items() {
        let items = [msg("a", "@a:x", 0), divider("d"), msg("b", "@b:x", 0)];
        let prefix = rebuild_prefix(&items, &HashMap::new(), 800.0);
        assert_eq!(prefix.len(), 4);
        assert_eq!(prefix[0], 0.0);
        assert!(prefix.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(rebuild_prefix(&[], &HashMap::new(), 800.0), vec![0.0]);
    }

    #[test]
    fn measured_heights_replace_estimates() {
        let items = [msg("a", "@a:x", 0), msg("b", "@b:x", 0)];
        let measured = HashMap::from([("a".to_owned(), 100.0), ("b".to_owned(), 50.0)]);
        assert_eq!(
            rebuild_prefix(&items, &measured, 800.0),
            vec![0.0, 100.0, 150.0]
        );
    }

    // ---- visible_window ---------------------------------------------------

    /// Ten items of 100 px each: prefix 0, 100, ... 1000.
    fn hundreds(n: usize) -> Vec<f32> {
        (0..=n).map(|i| i as f32 * 100.0).collect()
    }

    #[test]
    fn the_window_at_the_bottom_ends_at_the_last_item() {
        assert_eq!(visible_window(&hundreds(10), 0.0, 300.0, 0.0), 7..10);
        assert_eq!(visible_window(&hundreds(10), 0.0, 300.0, 100.0), 6..10);
    }

    #[test]
    fn the_window_in_the_middle_covers_the_viewport_and_overscan() {
        // The viewport spans y 450..750: items 4 to 7.
        assert_eq!(visible_window(&hundreds(10), 250.0, 300.0, 0.0), 4..8);
        assert_eq!(visible_window(&hundreds(10), 250.0, 300.0, 100.0), 3..9);
    }

    #[test]
    fn the_window_at_the_top_starts_at_zero() {
        assert_eq!(visible_window(&hundreds(10), 700.0, 300.0, 0.0), 0..3);
        assert_eq!(visible_window(&hundreds(10), 700.0, 300.0, 50.0), 0..4);
    }

    #[test]
    fn an_empty_list_has_an_empty_window() {
        assert_eq!(visible_window(&[0.0], 0.0, 300.0, 600.0), 0..0);
        assert_eq!(visible_window(&[], 0.0, 300.0, 600.0), 0..0);
    }

    #[test]
    fn a_viewport_taller_than_the_content_shows_everything() {
        assert_eq!(visible_window(&hundreds(3), 0.0, 900.0, 0.0), 0..3);
        assert_eq!(visible_window(&hundreds(3), 500.0, 900.0, 0.0), 0..3);
    }

    #[test]
    fn a_stale_offset_is_clamped_to_the_content() {
        assert_eq!(visible_window(&hundreds(10), 99_999.0, 300.0, 0.0), 0..3);
    }

    #[test]
    fn nan_and_negative_input_never_panics() {
        let prefix = hundreds(10);
        for offset in [f32::NAN, -50.0, f32::INFINITY, f32::NEG_INFINITY] {
            for vh in [f32::NAN, -1.0, 0.0, f32::INFINITY, 300.0] {
                for overscan in [f32::NAN, -1.0, 0.0, f32::INFINITY, 600.0] {
                    let w = visible_window(&prefix, offset, vh, overscan);
                    assert!(w.start <= w.end && w.end <= 10, "{w:?}");
                    assert!(w.len() <= MAX_BUILT_ROWS);
                }
            }
        }
    }

    #[test]
    fn the_window_never_exceeds_the_row_cap() {
        let prefix: Vec<f32> = (0..=5000).map(|i| i as f32).collect(); // 1 px rows
        let w = visible_window(&prefix, 2000.0, 800.0, OVERSCAN_PX);
        assert_eq!(w.len(), MAX_BUILT_ROWS);
        // The row at the viewport top (y = 5000 - 2000 - 800 = 2200) is built.
        assert!(w.contains(&2200));
    }

    // ---- spacer_heights ---------------------------------------------------

    #[test]
    fn spacers_keep_the_content_height() {
        let prefix = hundreds(10);
        for window in [0..0, 0..10, 3..8, 0..3, 7..10] {
            let (top, bottom) = spacer_heights(&prefix, &window);
            let built = prefix[window.end] - prefix[window.start];
            assert_eq!(top + built + bottom, 1000.0, "{window:?}");
        }
        assert_eq!(spacer_heights(&[0.0], &(0..0)), (0.0, 0.0));
        assert_eq!(spacer_heights(&[], &(0..0)), (0.0, 0.0));
        // A window past the end is clamped.
        let (top, bottom) = spacer_heights(&prefix, &(8..40));
        assert_eq!((top, bottom), (800.0, 0.0));
    }

    // ---- anchor -----------------------------------------------------------

    fn layout(ids: &[&str], heights: &HashMap<&str, f32>) -> (Vec<TimelineItem>, Vec<f32>) {
        let items: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| msg(id, &format!("@u{i}:x"), 0))
            .collect();
        let measured = ids
            .iter()
            .map(|id| ((*id).to_owned(), heights.get(id).copied().unwrap_or(100.0)))
            .collect();
        let prefix = rebuild_prefix(&items, &measured, 800.0);
        (items, prefix)
    }

    const IDS: [&str; 10] = ["m0", "m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9"];

    /// Ten rows of 100 px, 300 px viewport, scrolled 250 px up from the
    /// bottom: the viewport spans y 450..750 and `m4` is cut by 50 px.
    fn scrolled_up() -> (Vec<TimelineItem>, Vec<f32>, Vec<(String, f32)>) {
        let (items, prefix) = layout(&IDS, &HashMap::new());
        let anchor = record_anchor(&items, &prefix, 250.0, 300.0).expect("scrolled up");
        let candidates = anchor_candidates(&items, &prefix, &anchor, 250.0, 300.0);
        (items, prefix, candidates)
    }

    #[test]
    fn the_anchor_is_the_top_visible_item_and_its_position() {
        let (items, prefix) = layout(&IDS, &HashMap::new());
        assert_eq!(
            record_anchor(&items, &prefix, 250.0, 300.0),
            Some(("m4".to_owned(), -50.0))
        );
        assert_eq!(record_anchor(&items, &prefix, 2.0, 300.0), None);
        assert_eq!(record_anchor(&items, &prefix, 0.0, 300.0), None);
        assert_eq!(record_anchor(&[], &[0.0], 100.0, 300.0), None);
    }

    #[test]
    fn prepending_needs_no_correction() {
        let (_, _, candidates) = scrolled_up();
        let ids = [
            "n0", "n1", "n2", "m0", "m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9",
        ];
        let (items, prefix) = layout(&ids, &HashMap::new());
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 250.0, 300.0),
            None
        );
    }

    #[test]
    fn appending_while_scrolled_up_grows_the_offset_by_the_appended_height() {
        let (_, _, candidates) = scrolled_up();
        let mut ids = IDS.to_vec();
        ids.extend(["m10", "m11"]);
        let (items, prefix) = layout(&ids, &HashMap::new());
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 250.0, 300.0),
            Some(450.0)
        );
    }

    #[test]
    fn an_item_growing_below_the_anchor_grows_the_offset() {
        let (_, _, candidates) = scrolled_up();
        let (items, prefix) = layout(&IDS, &HashMap::from([("m8", 140.0)]));
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 250.0, 300.0),
            Some(290.0)
        );
    }

    #[test]
    fn an_item_growing_above_the_anchor_needs_no_correction() {
        let (_, _, candidates) = scrolled_up();
        let (items, prefix) = layout(&IDS, &HashMap::from([("m1", 180.0)]));
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 250.0, 300.0),
            None
        );
    }

    #[test]
    fn at_the_bottom_there_is_no_correction() {
        let (items, prefix) = layout(&IDS, &HashMap::new());
        let candidates = [("m4".to_owned(), -50.0)];
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 0.0, 300.0),
            None
        );
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 2.0, 300.0),
            None
        );
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, f32::NAN, 300.0),
            None
        );
    }

    #[test]
    fn a_removed_anchor_falls_back_to_the_nearest_surviving_item() {
        let (_, _, candidates) = scrolled_up();
        let ids = ["m0", "m1", "m2", "m3", "m5", "m6", "m7", "m8", "m9", "m10"];
        let (items, prefix) = layout(&ids, &HashMap::new());
        // `m5` was 50 px below the viewport top; it stays there, and the one
        // appended row pushes the bottom offset up by 100 - 100 + 100.
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 250.0, 300.0),
            Some(350.0)
        );
    }

    #[test]
    fn with_no_surviving_candidate_there_is_no_correction() {
        let (_, _, candidates) = scrolled_up();
        let (items, prefix) = layout(&["z0", "z1"], &HashMap::new());
        assert_eq!(
            anchor_correction(&candidates, &items, &prefix, 250.0, 300.0),
            None
        );
    }

    #[test]
    fn candidates_start_with_the_anchor_then_alternate_outwards() {
        let (items, prefix) = layout(&IDS, &HashMap::new());
        let c = anchor_candidates(&items, &prefix, &("m4".to_owned(), -50.0), 250.0, 300.0);
        let ids: Vec<_> = c.iter().map(|(id, _)| id.as_str()).take(5).collect();
        assert_eq!(ids, ["m4", "m5", "m3", "m6", "m2"]);
        assert_eq!(c[0].1, -50.0);
        assert_eq!(c[1].1, 50.0);
        // An anchor that is not in the list gives nothing.
        assert!(
            anchor_candidates(&items, &prefix, &("zz".to_owned(), 0.0), 250.0, 300.0).is_empty()
        );
    }

    // ---- should_paginate (Q16) -------------------------------------------

    #[test]
    fn pagination_follows_the_q16_table() {
        use PaginationState::{Idle, Loading, ReachedStart};
        // (state, gap above the viewport, already asked) -> send
        let table = [
            (Idle, 500.0, false, true),  // within one viewport of the top
            (Idle, 600.0, false, true),  // exactly one viewport
            (Idle, 601.0, false, false), // further away
            (Idle, 0.0, false, true),    // at the very top
            (Idle, -400.0, false, true), // content shorter than the viewport
            (Idle, 100.0, true, false),  // already asked in this Idle period
            (Loading, 100.0, false, false),
            (ReachedStart, 100.0, false, false),
            (Idle, f32::NAN, false, false),
        ];
        for (state, gap, asked, expected) in table {
            assert_eq!(
                should_paginate(state, gap, 600.0, asked),
                expected,
                "{state:?} gap {gap} asked {asked}"
            );
        }
    }

    // ---- update-level behaviour ------------------------------------------

    fn rid(n: &str) -> RoomId {
        RoomId::from(format!("!{n}:x"))
    }

    fn app() -> (App, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(256);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        let _ = app.update(Message::Core(Box::new(CoreEvent::RoomListChanged(vec![
            ListDiff::Reset {
                values: vec![
                    RoomSummary::new(rid("a"), "a".to_owned()),
                    RoomSummary::new(rid("b"), "b".to_owned()),
                ],
            },
        ]))));
        let _ = app.update(Message::SelectRoom(rid("a")));
        (app, rx)
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(c) = rx.try_recv() {
            out.push(c);
        }
        out
    }

    fn paginations(commands: &[Command]) -> usize {
        commands
            .iter()
            .filter(|c| matches!(c, Command::PaginateBackwards { .. }))
            .count()
    }

    fn diff(room: &str, diffs: Vec<ListDiff<TimelineItem>>) -> Message {
        Message::Core(Box::new(CoreEvent::TimelineDiff {
            room_id: rid(room),
            diffs,
        }))
    }

    fn pagination(room: &str, state: PaginationState) -> Message {
        Message::Core(Box::new(CoreEvent::Pagination {
            room_id: rid(room),
            state,
        }))
    }

    fn many(n: usize) -> Vec<TimelineItem> {
        (0..n)
            .map(|i| msg(&format!("m{i}"), &format!("@u{i}:x"), 0))
            .collect()
    }

    fn reset(items: Vec<TimelineItem>) -> Message {
        diff("a", vec![ListDiff::Reset { values: items }])
    }

    #[test]
    fn diffs_apply_to_the_selected_room_only() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(3)));
        let _ = app.update(diff("b", vec![ListDiff::Clear]));
        assert_eq!(app.timeline.items.len(), 3);
        assert_eq!(app.timeline.prefix.len(), 4);
        let _ = app.update(diff("a", vec![ListDiff::PopBack]));
        assert_eq!(app.timeline.items.len(), 2);
        assert_eq!(app.timeline.prefix.len(), 3);
    }

    #[test]
    fn selecting_a_room_resets_the_timeline_view() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(3)));
        let _ = app.update(Message::RowMeasured {
            id: "m0".into(),
            height: 77.0,
        });
        let _ = app.update(pagination("a", PaginationState::ReachedStart));
        let _ = app.update(Message::SelectRoom(rid("b")));
        assert!(app.timeline.items.is_empty());
        assert!(app.timeline.measured.is_empty());
        assert_eq!(app.timeline.prefix, vec![0.0]);
        assert_eq!(app.timeline.pagination, PaginationState::Idle);
        assert!(app.timeline.anchor.is_none());
    }

    #[test]
    fn a_measured_row_updates_the_prefix() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(3)));
        let before = app.timeline.prefix[3];
        let est = app.timeline.prefix[1];
        let _ = app.update(Message::RowMeasured {
            id: "m0".into(),
            height: est + 40.0,
        });
        assert_eq!(app.timeline.prefix[3], before + 40.0);
        assert_eq!(app.timeline.measured["m0"], est + 40.0);
    }

    #[test]
    fn bad_measurements_are_ignored() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(3)));
        let prefix = app.timeline.prefix.clone();
        for height in [f32::NAN, -3.0, f32::INFINITY, 0.0] {
            let _ = app.update(Message::RowMeasured {
                id: "m0".into(),
                height,
            });
        }
        let _ = app.update(Message::RowMeasured {
            id: "nope".into(),
            height: 10.0,
        });
        assert_eq!(app.timeline.prefix, prefix);
        assert!(app.timeline.measured.is_empty());
    }

    #[test]
    fn a_width_change_clears_the_measurements() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(3)));
        let _ = app.update(Message::RowMeasured {
            id: "m0".into(),
            height: 90.0,
        });
        let _ = app.on_timeline_scrolled(0.0, 600.0, app.timeline.width + 0.5);
        assert_eq!(app.timeline.measured.len(), 1, "within 1 px");
        let _ = app.on_timeline_scrolled(0.0, 600.0, app.timeline.width + 40.0);
        assert!(app.timeline.measured.is_empty());
    }

    #[test]
    fn appending_while_scrolled_up_requests_a_scroll_correction() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(40)));
        let _ = app.on_timeline_scrolled(1000.0, 600.0, 800.0);
        assert!(app.timeline.anchor.is_some());
        let before = app.timeline.prefix[40];
        let _ = app.update(diff(
            "a",
            vec![ListDiff::PushBack {
                value: msg("new", "@z:x", 0),
            }],
        ));
        let grown = app.timeline.prefix[41] - before;
        assert_eq!(app.timeline.last_scroll_request, Some(1000.0 + grown));
        assert_eq!(app.timeline.offset_from_bottom, 1000.0 + grown);
    }

    #[test]
    fn at_the_bottom_new_messages_request_no_scroll() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(40)));
        let _ = app.on_timeline_scrolled(0.0, 600.0, 800.0);
        assert!(app.timeline.anchor.is_none());
        let _ = app.update(diff(
            "a",
            vec![ListDiff::PushBack {
                value: msg("new", "@z:x", 0),
            }],
        ));
        assert_eq!(app.timeline.last_scroll_request, None);
    }

    #[test]
    fn prepending_while_scrolled_up_requests_no_scroll() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(40)));
        let _ = app.on_timeline_scrolled(1000.0, 600.0, 800.0);
        let _ = app.update(diff(
            "a",
            (0..20)
                .map(|i| ListDiff::PushFront {
                    value: msg(&format!("old{i}"), "@z:x", 0),
                })
                .collect(),
        ));
        assert_eq!(app.timeline.last_scroll_request, None);
        assert_eq!(app.timeline.offset_from_bottom, 1000.0);
    }

    #[test]
    fn a_taller_row_below_the_anchor_requests_a_scroll() {
        let (mut app, _rx) = app();
        let _ = app.update(reset(many(40)));
        let _ = app.on_timeline_scrolled(1000.0, 600.0, 800.0);
        let est = app.timeline.prefix[40] - app.timeline.prefix[39];
        let _ = app.update(Message::RowMeasured {
            id: "m39".into(),
            height: est + 40.0,
        });
        assert_eq!(app.timeline.last_scroll_request, Some(1040.0));
    }

    #[test]
    fn pagination_events_of_other_rooms_are_ignored() {
        let (mut app, _rx) = app();
        let _ = app.update(pagination("b", PaginationState::ReachedStart));
        assert_eq!(app.timeline.pagination, PaginationState::Idle);
        let _ = app.update(pagination("a", PaginationState::Loading));
        assert_eq!(app.timeline.pagination, PaginationState::Loading);
    }

    #[test]
    fn short_content_after_the_first_reset_requests_one_page() {
        let (mut app, mut rx) = app();
        let _ = drain(&mut rx);
        let _ = app.update(reset(many(2)));
        let sent = drain(&mut rx);
        assert_eq!(paginations(&sent), 1);
        assert!(
            sent.iter().any(
                |c| matches!(c, Command::PaginateBackwards { room_id } if *room_id == rid("a"))
            )
        );
        // Further batches in the same Idle period do not ask again.
        let _ = app.update(diff("a", vec![ListDiff::PopBack]));
        assert_eq!(paginations(&drain(&mut rx)), 0);
    }

    #[test]
    fn a_scripted_scroll_asks_once_per_idle_period() {
        let (mut app, mut rx) = app();
        let _ = app.update(reset(many(200)));
        let _ = drain(&mut rx);
        let vh = 600.0;
        // Far from the top: nothing.
        let _ = app.on_timeline_scrolled(0.0, vh, 800.0);
        let _ = app.on_timeline_scrolled(500.0, vh, 800.0);
        assert_eq!(paginations(&drain(&mut rx)), 0);
        let total = *app.timeline.prefix.last().unwrap();
        let near_top = total - vh - 300.0;
        // Near the top: one request, then quiet while the user wiggles.
        let _ = app.on_timeline_scrolled(near_top, vh, 800.0);
        let _ = app.on_timeline_scrolled(near_top + 10.0, vh, 800.0);
        let _ = app.on_timeline_scrolled(near_top - 10.0, vh, 800.0);
        assert_eq!(paginations(&drain(&mut rx)), 1);
        // Loading: nothing more, however much it scrolls.
        let _ = app.update(pagination("a", PaginationState::Loading));
        let _ = app.on_timeline_scrolled(near_top + 50.0, vh, 800.0);
        assert_eq!(paginations(&drain(&mut rx)), 0);
        // Back to Idle with the top still near: a new period, a new request.
        let _ = app.update(pagination("a", PaginationState::Idle));
        assert_eq!(paginations(&drain(&mut rx)), 1);
        let _ = app.on_timeline_scrolled(near_top + 20.0, vh, 800.0);
        assert_eq!(paginations(&drain(&mut rx)), 0);
        // The start of the room: never again.
        let _ = app.update(pagination("a", PaginationState::ReachedStart));
        let _ = app.on_timeline_scrolled(total - vh, vh, 800.0);
        assert_eq!(paginations(&drain(&mut rx)), 0);
    }

    #[test]
    fn a_dropped_request_is_repeated_after_the_user_scrolls_on() {
        let (mut app, mut rx) = app();
        let _ = app.update(reset(many(200)));
        let vh = 600.0;
        let total = *app.timeline.prefix.last().unwrap();
        let near_top = total - vh - 300.0;
        let _ = app.on_timeline_scrolled(near_top, vh, 800.0);
        let _ = drain(&mut rx);
        // The core dropped it silently: no event follows. The user moves on.
        let _ = app.on_timeline_scrolled(near_top + RETRY_SCROLL_PX + 1.0, vh, 800.0);
        assert_eq!(paginations(&drain(&mut rx)), 1);
    }

    #[test]
    fn nothing_is_asked_once_the_start_item_is_present() {
        let (mut app, mut rx) = app();
        let mut items = vec![plain(TimelineItemKind::TimelineStart)];
        items.extend(many(2));
        let _ = app.update(reset(items));
        assert_eq!(paginations(&drain(&mut rx)), 0);
    }

    #[test]
    fn reached_start_before_the_start_item_arrives_is_harmless() {
        let (mut app, mut rx) = app();
        let _ = app.update(reset(many(2)));
        let _ = drain(&mut rx);
        let _ = app.update(pagination("a", PaginationState::ReachedStart));
        assert_eq!(app.timeline.pagination, PaginationState::ReachedStart);
        let _ = app.update(diff(
            "a",
            vec![ListDiff::PushFront {
                value: plain(TimelineItemKind::TimelineStart),
            }],
        ));
        assert_eq!(paginations(&drain(&mut rx)), 0);
        assert!(matches!(
            app.timeline.items[0].kind,
            TimelineItemKind::TimelineStart
        ));
    }

    // ---- Q15a: timing -----------------------------------------------------

    fn synthetic(n: usize) -> Vec<TimelineItem> {
        (0..n)
            .map(|i| {
                let id = format!("item{i}");
                if i % 50 == 0 {
                    divider(&id)
                } else if i % 25 == 0 {
                    state_item(&id)
                } else {
                    let mut details = MessageDetails::default();
                    if i % 3 == 0 {
                        details.content = RichText {
                            blocks: vec![text_block(60), text_block(120), text_block(30)],
                        };
                    } else {
                        details.content = RichText {
                            blocks: vec![text_block(40)],
                        };
                    }
                    if i % 5 == 0 {
                        details.reactions = (0..2)
                            .map(|k| ReactionGroup {
                                key: format!("k{k}"),
                                count: 2,
                                own: false,
                                senders: Vec::new(),
                            })
                            .collect();
                    }
                    msg_with(
                        &id,
                        &format!("@u{}:x", i / 4 % 7),
                        i as u64 * 20_000,
                        details,
                    )
                }
            })
            .collect()
    }

    fn changed(i: usize) -> TimelineItem {
        let mut item = synthetic(i + 1).pop().expect("one item");
        if let TimelineItemKind::Message { details, .. } = &mut item.kind {
            details.reactions.push(ReactionGroup {
                key: "new".into(),
                count: 1,
                own: true,
                senders: Vec::new(),
            });
        }
        item.read_by.push(UserId::from("@r:x"));
        item
    }

    fn loaded_app(scrolled_up: bool) -> App {
        let (mut app, _rx) = app();
        let _ = app.update(reset(synthetic(10_000)));
        if scrolled_up {
            let _ = app.on_timeline_scrolled(30_000.0, 800.0, 800.0);
        }
        app
    }

    fn median(mut v: Vec<Duration>) -> Duration {
        v.sort();
        v[v.len() / 2]
    }

    fn time_shape(
        name: &str,
        scrolled_up: bool,
        prepare: impl Fn() -> Vec<ListDiff<TimelineItem>>,
    ) {
        let budget = Duration::from_millis(16);
        let mut runs = Vec::new();
        for run in 0..16 {
            let mut app = loaded_app(scrolled_up);
            let message = diff("a", prepare());
            let start = Instant::now();
            let task = app.update(message);
            let elapsed = start.elapsed();
            drop(task);
            if run > 0 {
                runs.push(elapsed);
            }
        }
        let m = median(runs);
        println!("timeline_batches_under_16ms: {name}: median {m:?}");
        assert!(m < budget, "{name}: median {m:?} is over {budget:?}");
    }

    #[test]
    fn timeline_batches_under_16ms() {
        time_shape("1 reset of 10,000", false, || {
            vec![ListDiff::Reset {
                values: synthetic(10_000),
            }]
        });
        time_shape("2 pagination (20 PushFront + Insert)", false, || {
            let mut d: Vec<_> = (0..20)
                .map(|i| ListDiff::PushFront {
                    value: msg(&format!("old{i}"), "@o:x", 0),
                })
                .collect();
            d.push(ListDiff::Insert {
                index: 5000,
                value: msg("ins", "@o:x", 0),
            });
            d
        });
        time_shape("3 PushBack of 5 while scrolled up", true, || {
            (0..5)
                .map(|i| ListDiff::PushBack {
                    value: msg(&format!("new{i}"), "@n:x", 0),
                })
                .collect()
        });
        time_shape("4 100 Set + 10 Remove/Insert", false, || {
            let mut d: Vec<_> = (0..100)
                .map(|k| ListDiff::Set {
                    index: 100 + k * 90,
                    value: changed(100 + k * 90),
                })
                .collect();
            for k in 0..10 {
                d.push(ListDiff::Remove { index: 7000 + k });
                d.push(ListDiff::Insert {
                    index: 7000 + k,
                    value: msg(&format!("re{k}"), "@r:x", 0),
                });
            }
            d
        });
        time_shape("5 Clear then Reset of 10,000", false, || {
            vec![
                ListDiff::Clear,
                ListDiff::Reset {
                    values: synthetic(10_000),
                },
            ]
        });
    }

    #[test]
    fn the_estimate_is_capped_however_many_blocks_there_are() {
        let blocks = (0..5_000).map(|_| text_block(10_000)).collect();
        let item = msg_with("a", "@a:x", 0, rich(blocks));
        assert_eq!(estimate_height(&item, false, 800.0), MAX_MEASURED_PX);
    }

    // ---- read receipts (plan Q12) -------------------------------------------

    fn own_msg(id: &str) -> TimelineItem {
        let mut item = msg(id, "@me:x", 0);
        if let TimelineItemKind::Message { is_own, .. } = &mut item.kind {
            *is_own = true;
        }
        item
    }

    fn sending_msg(id: &str) -> TimelineItem {
        let mut item = msg(id, "@them:x", 0);
        if let TimelineItemKind::Message { sent, .. } = &mut item.kind {
            *sent = SendStatus::Sending;
        }
        item
    }

    fn view_with(items: Vec<TimelineItem>) -> TimelineView {
        let mut view = TimelineView::default();
        let _ = view.apply_diffs(vec![ListDiff::Reset { values: items }]);
        view
    }

    #[test]
    fn the_newest_receipt_target_skips_dividers_echoes_and_own_messages() {
        let view = view_with(vec![
            msg("a", "@a:x", 0),
            msg("b", "@b:x", 0),
            own_msg("mine"),
            sending_msg("echo"),
            divider("d"),
            plain(TimelineItemKind::ReadMarker),
        ]);
        assert_eq!(view.newest_receipt_target(), Some("b"));
        assert_eq!(view_with(vec![divider("d")]).newest_receipt_target(), None);
        assert_eq!(
            view_with(vec![state_item("s")]).newest_receipt_target(),
            Some("s")
        );
        assert_eq!(
            view_with(vec![placeholder("p", "@a:x", 0)]).newest_receipt_target(),
            Some("p")
        );
    }

    #[test]
    fn a_receipt_is_sent_once_per_new_bottom_item() {
        let t0 = Instant::now();
        let mut view = view_with(vec![msg("a", "@a:x", 0)]);
        assert_eq!(view.read_step(true, t0), ReadStep::Send);
        assert_eq!(view.last_marked.as_deref(), Some("a"));
        // Same item again, at any later time: nothing.
        assert_eq!(
            view.read_step(true, t0 + Duration::from_secs(5)),
            ReadStep::Idle
        );
        // A new item, after the debounce: one more.
        let _ = view.apply_diffs(vec![ListDiff::Append {
            values: vec![msg("b", "@a:x", 0)],
        }]);
        assert_eq!(
            view.read_step(true, t0 + Duration::from_secs(5)),
            ReadStep::Send
        );
        assert_eq!(view.last_marked.as_deref(), Some("b"));
    }

    #[test]
    fn no_receipt_while_scrolled_up_or_unfocused() {
        let t0 = Instant::now();
        let mut view = view_with(vec![msg("a", "@a:x", 0)]);
        let _ = view.scrolled(300.0, 600.0, 800.0);
        assert_eq!(view.read_step(true, t0), ReadStep::Idle);
        assert!(!view.bottom_seen);
        let _ = view.scrolled(0.0, 600.0, 800.0);
        assert_eq!(view.read_step(false, t0), ReadStep::Idle);
        assert!(
            !view.bottom_seen,
            "an unfocused window has not seen the bottom"
        );
        // Focus returns: now it is sent.
        assert_eq!(view.read_step(true, t0), ReadStep::Send);
        assert!(view.bottom_seen);
    }

    #[test]
    fn receipts_are_debounced_to_one_per_second() {
        let t0 = Instant::now();
        let mut view = view_with(vec![msg("a", "@a:x", 0)]);
        assert_eq!(view.read_step(true, t0), ReadStep::Send);
        let _ = view.apply_diffs(vec![ListDiff::Append {
            values: vec![msg("b", "@a:x", 0)],
        }]);
        let at = t0 + Duration::from_millis(300);
        assert_eq!(
            view.read_step(true, at),
            ReadStep::Wait(Duration::from_millis(700))
        );
        // The timer is already armed: no second timer, no send.
        assert_eq!(view.read_step(true, at), ReadStep::Idle);
        view.read_timer_fired();
        assert_eq!(
            view.read_step(true, t0 + Duration::from_secs(1)),
            ReadStep::Send
        );
        assert_eq!(view.last_marked.as_deref(), Some("b"));
    }

    #[test]
    fn a_new_room_starts_fresh() {
        let t0 = Instant::now();
        let mut view = view_with(vec![msg("a", "@a:x", 0)]);
        assert_eq!(view.read_step(true, t0), ReadStep::Send);
        assert!(view.bottom_seen);
        view.reset();
        assert!(!view.bottom_seen);
        assert_eq!(view.last_marked, None);
    }

    // ---- jumping to a reply's original ------------------------------------

    fn with_event(mut item: TimelineItem, event: &str) -> TimelineItem {
        item.event_id = Some(EventId::from(event));
        item
    }

    /// 40 messages `m0`..`m39` with events `$0`..`$39`, scrolled to the bottom.
    fn loaded(n: usize) -> TimelineView {
        let items = (0..n)
            .map(|i| with_event(msg(&format!("m{i}"), "@a:x", i as u64), &format!("${i}")))
            .collect();
        let mut view = view_with(items);
        let _ = view.scrolled(0.0, 300.0, 800.0);
        view
    }

    fn page(view: &mut TimelineView, from: usize, to: usize) -> Effects {
        let diffs = (from..to)
            .map(|i| ListDiff::Insert {
                index: i - from,
                value: with_event(msg(&format!("old{i}"), "@a:x", 0), &format!("$old{i}")),
            })
            .collect();
        view.apply_diffs(diffs)
    }

    #[test]
    fn jumping_to_a_loaded_message_scrolls_to_it_and_flashes_it() {
        let mut view = loaded(40);
        let effects = view.jump_to(EventId::from("$5"));
        let wanted = effects.scroll_to.expect("a scroll");
        assert!(!effects.paginate && !effects.jump_failed);
        assert_eq!(effects.flash.as_deref(), Some("m5"));
        assert_eq!(view.flash.as_deref(), Some("m5"));
        // m5 is in the viewport, in its upper part.
        let total = view.total();
        let top = total - wanted - view.viewport_h;
        assert!(view.prefix[5] >= top && view.prefix[6] <= top + view.viewport_h);
        assert!(view.prefix[5] - top < view.viewport_h / 2.0);
    }

    #[test]
    fn jumping_to_an_unloaded_message_pages_back_until_it_arrives() {
        let mut view = loaded(40);
        let first = view.jump_to(EventId::from("$old3"));
        assert!(first.paginate && first.scroll_to.is_none() && !first.jump_failed);
        // The core reports Loading, then Idle before the page's items.
        let loading = view.pagination_changed(PaginationState::Loading);
        assert!(!loading.paginate);
        let idle = view.pagination_changed(PaginationState::Idle);
        assert!(idle.paginate, "the next page is asked at once");
        let _ = view.pagination_changed(PaginationState::Loading);
        // A page without the target: still searching.
        let miss = page(&mut view, 20, 40);
        assert!(miss.flash.is_none() && !miss.jump_failed);
        let _ = view.pagination_changed(PaginationState::Idle);
        let _ = view.pagination_changed(PaginationState::Loading);
        let hit = page(&mut view, 0, 20);
        assert_eq!(hit.flash.as_deref(), Some("old3"));
        assert!(hit.scroll_to.is_some());
        assert!(view.jump.is_none());
    }

    #[test]
    fn a_jump_gives_up_at_the_start_of_the_room() {
        let mut view = loaded(40);
        let _ = view.jump_to(EventId::from("$nowhere"));
        let _ = view.pagination_changed(PaginationState::Loading);
        let _ = view.pagination_changed(PaginationState::ReachedStart);
        let start = TimelineItem::new("start".into(), TimelineItemKind::TimelineStart);
        let effects = view.apply_diffs(vec![ListDiff::PushFront { value: start }]);
        assert!(effects.jump_failed);
        assert!(view.jump.is_none());
    }

    #[test]
    fn a_jump_gives_up_after_its_page_budget() {
        let mut view = loaded(40);
        let mut effects = view.jump_to(EventId::from("$nowhere"));
        let mut pages = 0;
        let mut next = 1000;
        while !effects.jump_failed {
            assert!(pages <= JUMP_MAX_PAGES, "never gave up");
            if effects.paginate {
                pages += 1;
            }
            let _ = view.pagination_changed(PaginationState::Loading);
            let idle = view.pagination_changed(PaginationState::Idle);
            pages += u32::from(idle.paginate);
            effects = page(&mut view, next, next + 20);
            next += 20;
        }
        assert_eq!(pages, JUMP_MAX_PAGES);
    }

    #[test]
    fn a_row_measured_before_the_last_page_arrives_does_not_end_the_jump() {
        let mut view = loaded(40);
        let mut effects = view.jump_to(EventId::from("$nowhere"));
        // Spend the budget, then reach the Idle that precedes the last page.
        while view.jump.as_ref().is_some_and(|j| j.pages_left > 0) {
            let _ = view.pagination_changed(PaginationState::Loading);
            effects = view.pagination_changed(PaginationState::Idle);
        }
        assert!(!effects.jump_failed);
        // The last page is in flight; its Idle comes before its items.
        let _ = view.pagination_changed(PaginationState::Loading);
        let _ = view.pagination_changed(PaginationState::Idle);
        let measured = view.row_measured("m39", 123.0);
        assert!(!measured.jump_failed && view.jump.is_some());
        assert!(page(&mut view, 900, 920).jump_failed);
    }

    #[test]
    fn a_flash_ends_only_for_its_own_row() {
        let mut view = loaded(10);
        let _ = view.jump_to(EventId::from("$2"));
        view.flash_ended("m3");
        assert_eq!(view.flash.as_deref(), Some("m2"));
        view.flash_ended("m2");
        assert_eq!(view.flash, None);
    }

    #[test]
    fn selecting_another_room_drops_the_jump_and_the_flash() {
        let mut view = loaded(10);
        let _ = view.jump_to(EventId::from("$2"));
        let _ = view.jump_to(EventId::from("$elsewhere"));
        view.reset();
        assert!(view.jump.is_none() && view.flash.is_none());
    }

    #[test]
    fn a_local_echo_estimate_has_room_for_its_status_line() {
        let sent = estimate_height(&msg("a", "@a:x", 0), false, 800.0);
        let sending = estimate_height(&sending_msg("a"), false, 800.0);
        assert!(sending > sent);
    }

    #[test]
    fn an_edited_message_estimate_has_room_for_its_marker() {
        let plain = estimate_height(&msg("a", "@a:x", 0), false, 800.0);
        let edited = msg_with(
            "a",
            "@a:x",
            0,
            MessageDetails {
                edited: true,
                ..MessageDetails::default()
            },
        );
        assert!(estimate_height(&edited, false, 800.0) > plain);
    }
}
