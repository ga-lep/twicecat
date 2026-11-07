//! The timeline pane: only the rows around the viewport are built, between
//! two spacers that stand in for the rest (plan Q14, Q15, Q20 to Q23).
//!
//! The model ([`TimelineView`]) decides which rows to build and how tall the
//! spacers are. Each built row sits in a `sensor` that reports its real
//! height, so estimates are replaced by measurements as rows come into view.
//! A `Set` diff keeps the old measurement of a replaced item; the sensor
//! reports the new height by itself when the rebuilt row has another size.

#[cfg(test)]
use std::cell::Cell;

use chrono::{Local, TimeZone};
use iced::widget::{
    Column, Row, button, column, container, image, mouse_area, progress_bar, row, scrollable,
    sensor, space, stack, text, tooltip,
};
use iced::{
    Background, Color, ContentFit, Element, Fill, Font, Length, Padding, Shrink, Theme, mouse,
};
use iced::{Border, border};
use twicecat_core::api::{
    Block, EventId, Inline, MediaContent, MediaKind, MessageDetails, MessageKind, MessageText,
    PaginationState, PlaceholderKind, ReactionGroup, ReplyPreview, ReplyState, RichText, RoomId,
    RoomMemberView, RoomSummary, SendStatus, Shield, ShieldCode, StateChange, TextStyle,
    ThreadSummaryView, TimelineItem, TimelineItemKind, TypingUser, UploadProgress, UserId,
    UtdReason,
};

use crate::actions::{self, ActionButtons, ActionState};
use crate::dates::{day_label, time_label};
use crate::media::{
    BOX_MAX_W, MediaModel, Thumb, card_name, has_caption, kind_label, media_box, size_label,
    type_label,
};
use crate::rich_text::{Ctx, Revealed, blocks_view};
use crate::state::Message;
use crate::text_safety::{
    MAX_BODY_CHARS, MAX_NAME_CHARS, multi_line_capped, name_or_placeholder, one_line_capped,
};
use crate::theme;
use crate::timeline::{
    CARD_H, DIVIDER_H, INDENT_PX, MARKER_H, OVERSCAN_PX, SIDE_PAD_PX, START_H, THREAD_ROW_H,
    TIMELINE_ID, TimelineView, is_continuation, spacer_heights, top_pad,
};
use crate::ui::icons::Icon;
use crate::ui::kit;
use crate::ui::tokens::{self, Density};
use crate::upload;

/// How many lines of a body are shown.
pub const MAX_BODY_LINES: usize = 200;

/// How many characters of an event type are shown.
const MAX_EVENT_TYPE_CHARS: usize = 64;
/// Longest reply quote line, in characters (the core already cuts at 200).
const MAX_QUOTE_CHARS: usize = 200;

/// What the pane needs besides the timeline to draw formatted text.
#[derive(Clone, Copy)]
pub struct RichEnv<'a> {
    /// The theme, for span colours (spans cannot read it while drawing).
    pub theme: &'a Theme,
    /// The signed-in user, whose mentions get the stronger pill.
    pub own_user: Option<&'a UserId>,
    /// Spoilers the user revealed.
    pub revealed: &'a Revealed,
    /// Hover and delete confirmation, for the action bar of a message.
    pub actions: &'a ActionState,
    /// Known rooms, for the names on room pills.
    pub rooms: &'a [RoomSummary],
    /// The open room's members, for the names on user pills.
    pub members: &'a [RoomMemberView],
    /// Thumbnails and saved files, for media messages.
    pub media: &'a MediaModel,
    /// The open room, which keys the media cache.
    pub room: Option<&'a RoomId>,
    /// The density, which sets the space above and between rows.
    pub density: Density,
}

/// What differs between the room's timeline pane and the thread panel's: the
/// scrollable's ID, the messages the scroll and the row sensors send, and the
/// actions a row offers.
#[derive(Clone, Copy)]
pub struct Pane {
    /// The scrollable's widget ID, which scroll tasks name.
    pub scroll_id: &'static str,
    /// The message for a scroll of the pane.
    pub on_scroll: fn(scrollable::Viewport) -> Message,
    /// The message for a measured row: the item's ID and its height.
    pub on_measure: fn(String, f32) -> Message,
    /// The pane is the thread panel: no **Reply** or **Edit** (they belong to
    /// the room's composer) and no thread summary row.
    pub thread: bool,
}

impl Pane {
    /// The room's timeline.
    pub const MAIN: Self = Self {
        scroll_id: TIMELINE_ID,
        on_scroll: Message::TimelineScrolled,
        on_measure: |id, height| Message::RowMeasured { id, height },
        thread: false,
    };

    /// The thread panel's timeline.
    pub const THREAD: Self = Self {
        scroll_id: crate::threads::THREAD_SCROLL_ID,
        on_scroll: |viewport| Message::Threads(crate::threads::ThreadsMessage::Scrolled(viewport)),
        on_measure: |id, height| {
            Message::Threads(crate::threads::ThreadsMessage::Measured { id, height })
        },
        thread: true,
    };

    /// The buttons a row of this pane offers, from what the core allows.
    fn limit(self, buttons: ActionButtons) -> ActionButtons {
        if self.thread {
            ActionButtons {
                reply: false,
                edit: false,
                ..buttons
            }
        } else {
            buttons
        }
    }
}

/// The pane for the selected room's timeline, in the local time zone.
pub fn view<'a>(timeline: &'a TimelineView, env: RichEnv<'_>) -> Element<'a, Message> {
    let now = Local::now().timestamp_millis();
    view_in(timeline, u64::try_from(now).unwrap_or(0), &Local, env)
}

/// [`view`] with the clock and the time zone given.
///
/// The scrollable's content is a column of the top spacer, the rows of
/// [`TimelineView::window`] and the bottom spacer, so its height is the
/// content height of the model. While older events load, a "Loading" badge
/// floats over the top edge instead of taking space in the content.
pub fn view_in<'a, Tz: TimeZone>(
    timeline: &'a TimelineView,
    now_ms: u64,
    tz: &Tz,
    env: RichEnv<'_>,
) -> Element<'a, Message> {
    view_pane(timeline, now_ms, tz, env, Pane::MAIN)
}

/// [`view_in`] for the pane `pane` names.
pub fn view_pane<'a, Tz: TimeZone>(
    timeline: &'a TimelineView,
    now_ms: u64,
    tz: &Tz,
    env: RichEnv<'_>,
    pane: Pane,
) -> Element<'a, Message> {
    let window = timeline.window();
    let (above, below) = spacer_heights(&timeline.prefix, &window);
    let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(window.len() + 2);
    rows.push(space().height(above).into());
    rows.extend(window.map(|i| built_row(timeline, i, now_ms, tz, env, pane)));
    rows.push(space().height(below).into());
    // `from_vec`, because `Column::push` drops zero-size children, and the
    // content is always the two spacers plus the window.
    let list = scrollable(Column::from_vec(rows).width(Fill))
        .id(pane.scroll_id)
        .anchor_bottom()
        .on_scroll(pane.on_scroll)
        .width(Fill)
        .height(Fill);
    // Always a stack with the list first: iced keeps a widget's state only
    // while the widget at that place in the tree keeps its type, so returning
    // the bare list when idle would reset the scroll offset to the bottom on
    // every pagination.
    let badge: Element<'a, Message> = if timeline.pagination == PaginationState::Loading {
        container(text("Loading\u{2026}").size(tokens::CAPTION).style(muted))
            .center_x(Fill)
            .padding(tokens::XS)
            .into()
    } else {
        space().into()
    };
    stack([list.into(), badge]).into()
}

const SEMIBOLD: Font = crate::ui::fonts::BODY_SEMIBOLD;

const ITALIC: Font = crate::ui::fonts::BODY_ITALIC;

use kit::muted;

fn accent(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme.palette().primary),
    }
}

/// Row `i` in a sensor that reports its height.
fn built_row<'a, Tz: TimeZone>(
    timeline: &'a TimelineView,
    i: usize,
    now_ms: u64,
    tz: &Tz,
    env: RichEnv<'_>,
    pane: Pane,
) -> Element<'a, Message> {
    #[cfg(test)]
    BUILT_ROWS.with(|built| built.set(built.get() + 1));
    let id = timeline.items[i].id.clone();
    let on_resize_id = id.clone();
    sensor(row_content(timeline, i, now_ms, tz, env, pane))
        .key(id.clone())
        .anticipate(OVERSCAN_PX)
        .on_show(move |size| (pane.on_measure)(id.clone(), size.height))
        .on_resize(move |size| (pane.on_measure)(on_resize_id.clone(), size.height))
        .into()
}

fn row_content<'a, Tz: TimeZone>(
    timeline: &'a TimelineView,
    i: usize,
    now_ms: u64,
    tz: &Tz,
    env: RichEnv<'_>,
    pane: Pane,
) -> Element<'a, Message> {
    let items = &timeline.items;
    match &items[i].kind {
        TimelineItemKind::Message {
            sender,
            body,
            is_own,
            timestamp_ms,
            sent,
            details,
            ..
        } => {
            let echo = echo_of(*sent, details);
            let continuation = is_continuation(items, i);
            let mut content = Column::new().width(Fill);
            if !continuation {
                let name = sender_label(
                    details.sender_name.as_deref(),
                    details.sender_name_ambiguous,
                    sender,
                );
                content = content.push(header(name, time_label(*timestamp_ms, tz), details.shield));
            }
            if let Some(reply) = &details.reply_to {
                content = content.push(reply_quote(reply));
            }
            if let Some(media) = details.media.as_deref() {
                content = content.push(media_view(&items[i].id, media, env));
            }
            // A media message with no caption has no text: its body is the
            // file name, which the card or the image already shows.
            let shows_text = details
                .media
                .as_deref()
                .is_none_or(|media| has_caption(body, media));
            // The body sits below the header in its own widget, so it cannot
            // pass for a sender line. Rich text has no muted colour and no
            // small size, so it cannot copy the time next to a name either.
            if shows_text {
                let body = body_widget(
                    &items[i].id,
                    body,
                    details,
                    echo == Echo::Sending,
                    sender,
                    env,
                );
                // A continuation has no header to carry the shield, so it
                // sits beside the body (the same height: it only narrows the
                // body).
                content = content.push(match details.shield.filter(|_| continuation) {
                    Some(shield) => row![body, shield_view(shield)]
                        .spacing(6)
                        .align_y(iced::Alignment::Start)
                        .into(),
                    None => body,
                });
            } else if let Some(shield) = details.shield.filter(|_| continuation) {
                content = content.push(shield_view(shield));
            }
            if let Some(label) = edited_text(details) {
                content = content.push(text(label).size(tokens::CAPTION).style(muted));
            }
            let buttons = pane.limit(actions::action_buttons(details, *is_own));
            // The thread to open from the bar: only a message that already
            // heads one, and only in the room's pane.
            let thread_root = match (&details.thread, &items[i].event_id) {
                (Some(_), Some(root)) if !pane.thread => Some(root),
                _ => None,
            };
            if let (Some(summary), Some(root)) = (&details.thread, thread_root) {
                content = content.push(thread_row(root, summary));
            }
            if !details.reactions.is_empty() {
                content = content.push(reactions_row(&items[i].id, &details.reactions, buttons));
            }
            let upload = details.media.as_deref().and_then(|m| m.upload);
            if let Some(line) = echo_row(&items[i].id, echo, upload) {
                content = content.push(line);
            }
            if let Some(line) = receipts_row(items, i) {
                content = content.push(line);
            }
            let look = RowLook {
                highlighted: details.highlighted,
                hovered: bar_visible(env.actions, &items[i].id),
                flashed: timeline.flash.as_deref() == Some(items[i].id.as_str()),
            };
            let framed = framed(content.into(), continuation, look, env.density);
            with_actions(&items[i].id, framed, buttons, thread_root, env.actions)
        }
        TimelineItemKind::Placeholder {
            sender,
            sender_name,
            timestamp_ms,
            what,
            utd,
            ..
        } => {
            let continuation = is_continuation(items, i);
            let mut content = Column::new().width(Fill);
            if !continuation {
                let name = sender_label(sender_name.as_deref(), false, sender);
                content = content.push(header(name, time_label(*timestamp_ms, tz), None));
            }
            content = content.push(
                text(placeholder_line(*what, *utd))
                    .font(ITALIC)
                    .style(muted)
                    .width(Fill),
            );
            if let Some(line) = receipts_row(items, i) {
                content = content.push(line);
            }
            framed(
                content.into(),
                continuation,
                RowLook::default(),
                env.density,
            )
        }
        TimelineItemKind::State {
            sender,
            sender_name,
            change,
            ..
        } => {
            let actor = sender_label(sender_name.as_deref(), false, sender);
            let words = text(state_text(&actor, change))
                .size(tokens::CAPTION + 1.0)
                .style(muted)
                .width(Fill);
            // A call notice starts with the call icon; the words say the
            // same, so the icon is a cue and not the message.
            let words: Element<'a, Message> = match state_icon(change) {
                Some(icon) => row![
                    text(icon.glyph()).size(tokens::CAPTION + 1.0).style(muted),
                    words
                ]
                .spacing(tokens::XS)
                .into(),
                None => words.into(),
            };
            let mut line = Column::new().width(Fill).push(words);
            if let Some(receipts) = receipts_row(items, i) {
                line = line.push(receipts);
            }
            container(line)
                .padding(Padding {
                    top: 4.0,
                    right: SIDE_PAD_PX,
                    bottom: 4.0,
                    left: INDENT_PX,
                })
                .into()
        }
        TimelineItemKind::DateDivider { timestamp_ms } => {
            let label = day_label(*timestamp_ms, now_ms, tz);
            fixed_height(ruled(kit::tag(label)), DIVIDER_H)
        }
        TimelineItemKind::ReadMarker => fixed_height(
            ruled(
                text("New messages")
                    .size(tokens::CAPTION + 1.0)
                    .style(accent)
                    .into(),
            ),
            MARKER_H,
        ),
        TimelineItemKind::TimelineStart => fixed_height(
            text("This is the start of the conversation")
                .size(tokens::CAPTION + 1.0)
                .style(muted)
                .into(),
            START_H,
        ),
        TimelineItemKind::Other => space().height(0).into(),
    }
}

/// The line under a thread root, "N replies · name: preview"; clicking it
/// opens the thread panel.
fn thread_row<'a>(root: &EventId, summary: &ThreadSummaryView) -> Element<'a, Message> {
    button(
        row![
            text(Icon::Thread.glyph())
                .size(tokens::CAPTION + 1.0)
                .style(accent),
            text(crate::threads::summary_text(summary))
                .size(tokens::CAPTION + 1.0)
                .style(accent)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(tokens::XS),
    )
    .padding([2, 0])
    .height(Length::Fixed(THREAD_ROW_H))
    .style(button::text)
    .on_press(Message::Threads(crate::threads::ThreadsMessage::Open(
        root.clone(),
    )))
    .into()
}

/// The media of a message: an image in its fixed box, or a card.
fn media_view<'a>(id: &str, media: &MediaContent, env: RichEnv<'_>) -> Element<'a, Message> {
    match media.kind {
        MediaKind::Image => {
            let thumb = env
                .room
                .and_then(|room| env.media.thumb(&(room.clone(), id.to_owned())));
            image_box(id, media, thumb)
        }
        MediaKind::Video | MediaKind::Audio | MediaKind::File => {
            let open = env
                .room
                .is_some_and(|room| env.media.open_allowed(&(room.clone(), id.to_owned())));
            file_card(id, media, open)
        }
    }
}

/// A note in the middle of an image box.
fn box_note<'a>(label: &'static str) -> Element<'a, Message> {
    container(text(label).size(tokens::CAPTION + 1.0).style(muted))
        .center(Fill)
        .style(kit::card_style)
        .into()
}

/// An image in the box [`media_box`] gives, whatever has arrived: the box is
/// fixed by the sender's size, so the row never changes height when the
/// thumbnail comes (Q7). Clicking opens the viewer.
fn image_box<'a>(id: &str, media: &MediaContent, thumb: Option<&Thumb>) -> Element<'a, Message> {
    let size = media_box(media.width, media.height);
    let inner: Element<'a, Message> = match thumb {
        Some(Thumb::Ready(decoded)) => image(decoded.handle.clone())
            .width(Length::Fixed(size.width))
            .height(Length::Fixed(size.height))
            .content_fit(ContentFit::Contain)
            .into(),
        Some(Thumb::Unavailable) => box_note("No preview"),
        None => box_note("Loading\u{2026}"),
    };
    button(
        container(inner)
            .width(Length::Fixed(size.width))
            .height(Length::Fixed(size.height)),
    )
    .padding(0)
    .style(button::text)
    .on_press(Message::MediaClicked(id.to_owned()))
    .into()
}

/// A card for a file, video or audio message: kind, sanitised name, size and
/// type, **Save...** and, after a save of a safe file, **Open**.
fn file_card<'a>(id: &str, media: &MediaContent, open: bool) -> Element<'a, Message> {
    let detail = format!("{} \u{b7} {}", size_label(media.size), type_label(media));
    let mut actions = row![small_button(
        "Save\u{2026}",
        Message::SaveRequested(id.to_owned())
    )]
    .spacing(tokens::XS);
    if open {
        actions = actions.push(small_button("Open", Message::OpenRequested(id.to_owned())));
    }
    let label = column![
        text(card_name(&media.filename)).wrapping(text::Wrapping::None),
        text(detail)
            .size(12)
            .style(muted)
            .wrapping(text::Wrapping::None),
    ]
    .width(Fill);
    container(
        row![
            text(kind_label(media.kind))
                .size(tokens::CAPTION + 1.0)
                .style(muted)
                .width(44),
            label,
            actions
        ]
        .spacing(8)
        .align_y(iced::Center),
    )
    .padding(tokens::S)
    .width(Length::Fixed(BOX_MAX_W))
    .height(Length::Fixed(CARD_H))
    .style(kit::card_style)
    .into()
}

/// Whether the action bar of message `id` is drawn: the pointer is over it,
/// or its delete question is open.
pub fn bar_visible(state: &ActionState, id: &str) -> bool {
    state.hovered.as_deref() == Some(id) || state.confirm_delete.as_deref() == Some(id)
}

/// The text of a reaction chip: the key (one line, capped) and the count.
pub fn chip_label(group: &ReactionGroup) -> String {
    format!("{} {}", actions::key_label(&group.key), group.count)
}

/// The `key count` chips under a message, then a "+" chip. Chips toggle only
/// where the core allows reacting.
fn reactions_row<'a>(
    id: &str,
    groups: &[ReactionGroup],
    buttons: ActionButtons,
) -> Element<'a, Message> {
    let mut chips: Row<'a, Message> = Row::new();
    for group in groups {
        let mut chip = button(text(chip_label(group)).size(tokens::CAPTION + 1.0))
            .padding([2.0, tokens::S])
            .style(kit::chip_style(group.own));
        if buttons.react {
            chip = chip.on_press(Message::ReactionToggled {
                item: id.to_owned(),
                key: MessageText::from(group.key.as_str()),
            });
        }
        chips = chips.push(chip);
    }
    if buttons.react {
        chips = chips.push(kit::tip(
            button(text("+").size(tokens::CAPTION + 1.0))
                .padding([2.0, tokens::S])
                .style(kit::chip_style(false))
                .on_press(Message::PickerOpened(id.to_owned())),
            Icon::React.label(),
        ));
    }
    container(
        chips
            .spacing(tokens::XS)
            .wrap()
            .vertical_spacing(tokens::XS),
    )
    .padding(Padding {
        top: 2.0,
        ..Padding::ZERO
    })
    .into()
}

/// A small text button: **Retry**, **Discard**, **Cancel**, **Save...**.
fn small_button<'a>(label: &'static str, message: Message) -> Element<'a, Message> {
    button(text(label).size(tokens::CAPTION + 1.0))
        .padding([2.0, tokens::S])
        .style(kit::secondary_style)
        .on_press(message)
        .into()
}

/// The icon buttons of the hover bar of message `id` and what each sends:
/// Reply, Edit, React, Copy, Thread (only on a message that heads a thread,
/// whose root is `thread`), then Delete last, set apart by a gap of at least
/// `tokens::M` and drawn in the danger colour. Delete
/// only asks: the confirmation row has its own **Delete**.
pub fn bar_actions(
    id: &str,
    buttons: ActionButtons,
    thread: Option<&EventId>,
) -> Vec<(Icon, Message)> {
    let mut actions = Vec::new();
    if buttons.reply {
        actions.push((Icon::Reply, Message::ReplyRequested(id.to_owned())));
    }
    if buttons.edit {
        actions.push((Icon::Edit, Message::EditRequested(id.to_owned())));
    }
    if buttons.react {
        actions.push((Icon::React, Message::PickerOpened(id.to_owned())));
    }
    if buttons.copy {
        actions.push((Icon::Copy, Message::CopyRequested(id.to_owned())));
    }
    if let Some(root) = thread {
        actions.push((
            Icon::Thread,
            Message::Threads(crate::threads::ThreadsMessage::Open(root.clone())),
        ));
    }
    if buttons.delete {
        actions.push((Icon::Delete, Message::DeleteRequested(id.to_owned())));
    }
    actions
}

/// The hover bar of message `id`, or the delete question.
fn action_bar<'a>(
    id: &str,
    buttons: ActionButtons,
    thread: Option<&EventId>,
    confirming: bool,
) -> Element<'a, Message> {
    let mut bar = Row::new().spacing(tokens::XS).align_y(iced::Center);
    if confirming {
        bar = bar
            .push(text("Delete this message?").size(tokens::CAPTION + 1.0))
            .push(
                button(text("Delete").size(tokens::CAPTION + 1.0))
                    .padding([2.0, tokens::S])
                    .style(kit::danger_style)
                    .on_press(Message::DeleteConfirmed),
            )
            .push(small_button("Cancel", Message::DeleteCancelled));
    } else {
        for (index, (icon, message)) in bar_actions(id, buttons, thread).into_iter().enumerate() {
            if icon == Icon::Delete {
                // Delete is set apart so a slip on a neighbour cannot land
                // on it, and its glyph is in the danger colour.
                if index > 0 {
                    bar = bar.push(space().width(tokens::M));
                }
                bar = bar.push(kit::compact_danger_icon_button(icon, message));
            } else {
                bar = bar.push(kit::compact_icon_button(icon, message));
            }
        }
    }
    container(bar)
        .padding(2)
        .style(|theme: &Theme| container::Style {
            border: Border {
                radius: border::rounded(tokens::MEDIUM).radius,
                ..kit::card_style(theme).border
            },
            ..kit::card_style(theme)
        })
        .into()
}

/// Wraps a message row so that hovering it shows the action bar. The bar is
/// a stack layer over the row's top right corner: it never changes the row's
/// height, so the measurements stay valid.
fn with_actions<'a>(
    id: &str,
    row: Element<'a, Message>,
    buttons: ActionButtons,
    thread: Option<&EventId>,
    state: &ActionState,
) -> Element<'a, Message> {
    let confirming = state.confirm_delete.as_deref() == Some(id);
    let any = buttons.any() || thread.is_some();
    let content = if bar_visible(state, id) && (any || confirming) {
        let bar = container(action_bar(id, buttons, thread, confirming))
            .width(Fill)
            .align_right(Fill)
            .padding(Padding {
                right: SIDE_PAD_PX,
                ..Padding::ZERO
            });
        stack([row, bar.into()]).into()
    } else {
        row
    };
    mouse_area(content)
        .on_enter(Message::HoverEnter(id.to_owned()))
        .on_exit(Message::HoverExit(id.to_owned()))
        .into()
}

/// `content` with Element's emote prefix, "* name ", at the start of its
/// first paragraph, or on a line of its own before a list, quote or other
/// block.
pub fn with_emote_prefix(content: &RichText, name: &str) -> RichText {
    let prefix = Inline::Text {
        text: format!("* {name} "),
        style: TextStyle::default(),
    };
    let mut blocks = content.blocks.clone();
    match blocks.first_mut() {
        Some(Block::Paragraph(inlines)) => inlines.insert(0, prefix),
        _ => blocks.insert(0, Block::Paragraph(vec![prefix])),
    }
    RichText { blocks }
}

/// The body of a message: its formatted content, or the plain text when the
/// core sent none. An emote starts with "* name ", as in Element.
fn body_widget<'a>(
    id: &str,
    body: &str,
    details: &MessageDetails,
    dim: bool,
    sender: &UserId,
    env: RichEnv<'_>,
) -> Element<'a, Message> {
    let emote = (details.kind == MessageKind::Emote).then(|| {
        sender_label(
            details.sender_name.as_deref(),
            details.sender_name_ambiguous,
            sender,
        )
    });
    if details.content.blocks.is_empty() {
        let shown = match &emote {
            Some(name) => format!("* {name} {}", body_text(body)),
            None => body_text(body),
        };
        let plain = text(shown).width(Fill);
        return if dim {
            plain.style(muted).into()
        } else {
            plain.into()
        };
    }
    let ctx = Ctx {
        own_user: env.own_user,
        item: id,
        revealed: env.revealed,
        rooms: env.rooms,
        members: env.members,
    };
    match &emote {
        Some(name) => blocks_view(&with_emote_prefix(&details.content, name), &ctx, env.theme),
        None => blocks_view(&details.content, &ctx, env.theme),
    }
}

/// The marker under an edited message.
pub fn edited_text(details: &MessageDetails) -> Option<&'static str> {
    details.edited.then_some("(edited)")
}

/// Width of the upload progress bar, in pixels.
const UPLOAD_BAR_W: f32 = 160.0;

/// "42 %" for a fraction of one.
pub fn upload_percent(fraction: f32) -> String {
    format!("{} %", (fraction.clamp(0.0, 1.0) * 100.0).floor() as u32)
}

/// The status line of a local echo: muted while sending, in the danger
/// colour when failed, with **Retry** and **Discard**.
fn echo_row<'a>(
    id: &str,
    echo: Echo,
    upload: Option<UploadProgress>,
) -> Option<Element<'a, Message>> {
    let label = echo_text(echo)?;
    Some(match echo {
        Echo::Delivered => return None,
        // An upload on its way: a bar, and **Cancel** (a discard).
        Echo::Sending if upload.is_some() => {
            let progress = upload.map_or(0.0, upload::fraction);
            row![
                text(label).size(tokens::CAPTION).style(muted),
                progress_bar(0.0..=1.0, progress)
                    .length(Length::Fixed(UPLOAD_BAR_W))
                    .girth(Length::Fixed(6.0)),
                text(upload_percent(progress))
                    .size(tokens::CAPTION)
                    .style(muted),
                small_button("Cancel", Message::DiscardRequested(id.to_owned())),
            ]
            .spacing(8)
            .align_y(iced::Center)
            .into()
        }
        Echo::Sending => text(label).size(tokens::CAPTION).style(muted).into(),
        Echo::Failed { .. } => row![
            text(label).size(13).style(text::danger),
            small_button("Retry", Message::RetryRequested(id.to_owned())),
            small_button("Discard", Message::DiscardRequested(id.to_owned())),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into(),
    })
}

/// "Seen by" under item `i`, when other users' receipts point at it.
fn receipts_row<'a>(items: &[TimelineItem], i: usize) -> Option<Element<'a, Message>> {
    let line = seen_by_line(&items[i].read_by, |user| known_name(&items[..=i], user))?;
    Some(
        text(line)
            .size(tokens::CAPTION)
            .style(muted)
            .width(Fill)
            .into(),
    )
}

/// A message body ready to draw: line breaks kept, bidi controls replaced,
/// capped in lines and characters.
pub fn body_text(body: &str) -> String {
    multi_line_capped(body, MAX_BODY_CHARS, MAX_BODY_LINES)
}

/// A line centred in a row of a fixed height.
fn fixed_height<'a>(label: Element<'a, Message>, height: f32) -> Element<'a, Message> {
    container(label).center_x(Fill).center_y(height).into()
}

/// `label` centred between two rules.
fn ruled<'a>(label: Element<'a, Message>) -> Element<'a, Message> {
    row![kit::divider(), label, kit::divider()]
        .spacing(tokens::S)
        .align_y(iced::Center)
        .padding([0.0, tokens::L])
        .width(Fill)
        .into()
}

/// The icon a state line starts with: the call icon on a call notice.
pub fn state_icon(change: &StateChange) -> Option<Icon> {
    matches!(change, StateChange::CallStarted | StateChange::LegacyCall).then_some(Icon::Call)
}

/// The sender and time above a message.
fn header<'a>(name: String, time: String, shield: Option<Shield>) -> Element<'a, Message> {
    let mut line = row![
        text(name).font(SEMIBOLD),
        text(time).size(tokens::CAPTION).style(muted)
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    if let Some(shield) = shield {
        line = line.push(shield_view(shield));
    }
    line.into()
}

/// Side of the shield mark, in pixels.
const SHIELD_SIZE: f32 = 16.0;

/// The small shield of a message: a red or grey mark with the text of its
/// code as tooltip (plan Q10). The mark is a drawn badge, not a glyph, so it
/// does not depend on the fonts installed.
fn shield_view<'a>(shield: Shield) -> Element<'a, Message> {
    let (look, label) = shield_text(shield);
    // The colours are the theme's (`Semantic::shield_*`): the red one is the
    // danger colour, the grey one a neutral of its own, both with a glyph
    // that reaches 4.5:1 on its fill (tests in `theme.rs`).
    let colours = move |theme: &Theme| {
        let sem = theme::semantic(theme);
        match look {
            Look::Red => (sem.shield_red, sem.shield_red_glyph),
            Look::Grey => (sem.shield_grey, sem.shield_grey_glyph),
        }
    };
    let mark = container(text("!").size(11).style(move |theme: &Theme| text::Style {
        color: Some(colours(theme).1),
    }))
    .width(SHIELD_SIZE)
    .height(SHIELD_SIZE)
    .center_x(SHIELD_SIZE)
    .center_y(SHIELD_SIZE)
    .style(move |theme: &Theme| container::Style {
        background: Some(Background::Color(colours(theme).0)),
        border: iced::border::rounded(SHIELD_SIZE / 4.0),
        ..container::Style::default()
    });
    tooltip(
        mark,
        container(text(label).size(tokens::CAPTION))
            .padding(6)
            .style(kit::card_style),
        tooltip::Position::Top,
    )
    .into()
}

/// What decides the background of a message row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowLook {
    /// The message mentions the user.
    pub highlighted: bool,
    /// The pointer is over the row.
    pub hovered: bool,
    /// A reply quote jumped to this row a moment ago.
    pub flashed: bool,
}

/// The background of a message row: the jump flash, else the mention
/// highlight, else the hover background under the pointer (so each message
/// of a run from one sender shows where it starts and ends), else none.
pub fn row_background(sem: &theme::Semantic, look: RowLook) -> Option<Color> {
    if look.flashed {
        Some(sem.selection)
    } else if look.highlighted {
        Some(sem.message_highlight)
    } else if look.hovered {
        Some(sem.message_hover)
    } else {
        None
    }
}

/// The sender and the line of a reply quote, sanitised and capped.
pub fn reply_quote_text(reply: &ReplyPreview) -> (String, String) {
    match reply.state {
        ReplyState::Ready => {
            let name = sender_label(reply.sender_name.as_deref(), false, &reply.sender);
            let line = one_line_capped(&reply.body, MAX_QUOTE_CHARS);
            let line = if line.trim().is_empty() {
                "(no text)".to_owned()
            } else {
                line
            };
            (name, line)
        }
        ReplyState::Loading => (
            "Reply".to_owned(),
            "Loading the original message\u{2026}".to_owned(),
        ),
        ReplyState::Unavailable => (
            "Reply".to_owned(),
            "Original message unavailable".to_owned(),
        ),
    }
}

/// The quote above a reply, on the quote surface behind the accent bar.
/// Clicking it jumps to the original.
fn reply_quote<'a>(reply: &ReplyPreview) -> Element<'a, Message> {
    let (name, line) = reply_quote_text(reply);
    let quote = row![
        kit::quote_bar(),
        column![
            text(name).size(tokens::CAPTION).font(SEMIBOLD),
            text(line).size(tokens::CAPTION + 1.0).style(muted),
        ]
        .width(Fill),
    ]
    .spacing(tokens::S)
    .height(Shrink);
    let surface = container(quote)
        .padding([tokens::XS, tokens::S])
        .width(Fill)
        .style(kit::quote_surface_style);
    let padded = container(surface).padding(Padding {
        bottom: tokens::XS,
        ..Padding::ZERO
    });
    mouse_area(padded)
        .on_press(Message::ReplyQuoteClicked(reply.event_id.clone()))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// Pads a message row: indented, with less space above a continuation, on
/// the [`row_background`].
fn framed(
    content: Element<'_, Message>,
    continuation: bool,
    look: RowLook,
    density: Density,
) -> Element<'_, Message> {
    let padded = container(content).width(Fill).padding(Padding {
        top: top_pad(continuation, density),
        right: SIDE_PAD_PX,
        bottom: 0.0,
        left: INDENT_PX,
    });
    if look == RowLook::default() {
        return padded.into();
    }
    padded
        .style(move |theme: &Theme| container::Style {
            background: row_background(&theme::semantic(theme), look).map(Background::Color),
            ..container::Style::default()
        })
        .into()
}

/// The name to show for a sender: the display name, sanitised and capped, or
/// the user ID when there is none. An ambiguous name also shows the user ID.
pub fn sender_label(name: Option<&str>, ambiguous: bool, id: &UserId) -> String {
    let id_label = || name_or_placeholder(&id.to_string(), MAX_NAME_CHARS);
    let Some(name) = name else {
        return id_label();
    };
    let clean = one_line_capped(name, MAX_NAME_CHARS);
    if clean.trim().is_empty() {
        id_label()
    } else if ambiguous {
        format!("{clean} ({})", id_label())
    } else {
        clean
    }
}

/// How a shield looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// A warning.
    Red,
    /// A caution.
    Grey,
}

/// The look and the tooltip of an authenticity shield (plan Q10).
pub fn shield_text(shield: Shield) -> (Look, &'static str) {
    let (look, code) = match shield {
        Shield::Red(code) => (Look::Red, code),
        Shield::Grey(code) => (Look::Grey, code),
    };
    let text = match code {
        ShieldCode::AuthenticityNotGuaranteed => {
            "The authenticity of this encrypted message can't be guaranteed on this device"
        }
        ShieldCode::UnknownDevice => "Encrypted by an unknown or deleted device",
        ShieldCode::UnsignedDevice => "Encrypted by a device not verified by its owner",
        ShieldCode::UnverifiedIdentity => "Encrypted by an unverified user",
        ShieldCode::VerificationViolation => "Sender's identity changed",
        ShieldCode::MismatchedSender => {
            "The sender of this message does not match the device that sent it"
        }
        ShieldCode::SentInClear => "Sent in clear in an encrypted room",
    };
    (look, text)
}

/// The line of a message that could not be decrypted (plan Q11).
pub fn utd_text(reason: Option<UtdReason>) -> &'static str {
    match reason {
        Some(UtdReason::SentBeforeWeJoined) => "You don't have access to this message",
        Some(UtdReason::HistoricalMessageAndDeviceIsUnverified) => {
            "You need to verify this device for access to historical messages"
        }
        Some(UtdReason::HistoricalMessageAndBackupIsDisabled) => {
            "Historical messages are not available on this device"
        }
        Some(UtdReason::WithheldForUnverifiedOrInsecureDevice) => {
            "Your device doesn't meet the sender's security requirements"
        }
        Some(
            UtdReason::WithheldBySender | UtdReason::UnknownDevice | UtdReason::UnsignedDevice,
        ) => "The sender didn't share the key for this message",
        Some(UtdReason::VerificationViolation) => "Sender's verified identity was reset",
        // The key may still arrive.
        Some(UtdReason::Unknown) | None => "Waiting for this message",
    }
}

/// The line of a placeholder row: the reason-specific text for an
/// undecryptable message, else [`placeholder_text`].
pub fn placeholder_line(what: PlaceholderKind, utd: Option<UtdReason>) -> &'static str {
    match what {
        PlaceholderKind::UnableToDecrypt => utd_text(utd),
        _ => placeholder_text(what),
    }
}

/// The muted text of a placeholder row.
pub fn placeholder_text(what: PlaceholderKind) -> &'static str {
    match what {
        PlaceholderKind::Redacted => "Message deleted",
        PlaceholderKind::UnableToDecrypt => "Unable to decrypt this message",
        PlaceholderKind::Image => "sent an image (not shown yet)",
        PlaceholderKind::Video => "sent a video (not shown yet)",
        PlaceholderKind::Audio => "sent an audio clip (not shown yet)",
        PlaceholderKind::File => "sent a file (not shown yet)",
        PlaceholderKind::Sticker => "sent a sticker (not shown yet)",
        PlaceholderKind::Poll => "started a poll (not shown yet)",
        PlaceholderKind::Location => "shared a location (not shown yet)",
        PlaceholderKind::Unsupported => "sent something Twicecat cannot show yet",
    }
}

/// A person named in a state event: the name when there is one, else the ID.
fn who(name: Option<&str>, id: &UserId) -> String {
    sender_label(name, false, id)
}

/// The muted one-line text of a state event. `actor` is already sanitised
/// (see [`sender_label`]); every other name is sanitised here.
pub fn state_text(actor: &str, change: &StateChange) -> String {
    match change {
        StateChange::Joined => format!("{actor} joined"),
        StateChange::Left => format!("{actor} left"),
        StateChange::Invited {
            target,
            target_name,
        } => {
            format!("{actor} invited {}", who(target_name.as_deref(), target))
        }
        StateChange::Kicked {
            target,
            target_name,
        } => {
            format!("{actor} removed {}", who(target_name.as_deref(), target))
        }
        StateChange::Banned {
            target,
            target_name,
        } => {
            format!("{actor} banned {}", who(target_name.as_deref(), target))
        }
        StateChange::Unbanned {
            target,
            target_name,
        } => {
            format!("{actor} unbanned {}", who(target_name.as_deref(), target))
        }
        StateChange::InviteAccepted => format!("{actor} accepted the invite"),
        StateChange::InviteRejected => format!("{actor} declined the invite"),
        StateChange::InviteRevoked {
            target,
            target_name,
        } => format!(
            "{actor} withdrew the invite of {}",
            who(target_name.as_deref(), target)
        ),
        StateChange::Knocked => format!("{actor} asked to join"),
        StateChange::KnockAccepted {
            target,
            target_name,
        } => format!(
            "{actor} accepted the request of {} to join",
            who(target_name.as_deref(), target)
        ),
        StateChange::KnockRetracted => format!("{actor} withdrew their request to join"),
        StateChange::KnockDenied {
            target,
            target_name,
        } => format!(
            "{actor} denied the request of {} to join",
            who(target_name.as_deref(), target)
        ),
        StateChange::DisplayName { old, new } => {
            let before = old
                .as_deref()
                .map(|n| one_line_capped(n, MAX_NAME_CHARS))
                .filter(|n| !n.trim().is_empty());
            let before = before.as_deref().unwrap_or(actor);
            match new
                .as_deref()
                .map(|n| one_line_capped(n, MAX_NAME_CHARS))
                .filter(|n| !n.trim().is_empty())
            {
                Some(after) => format!("{before} changed their name to {after}"),
                None => format!("{before} removed their name"),
            }
        }
        StateChange::Avatar => format!("{actor} changed their avatar"),
        StateChange::RoomCreated => format!("{actor} created the room"),
        StateChange::RoomName { name } => {
            match name
                .as_deref()
                .map(|n| one_line_capped(n, MAX_NAME_CHARS))
                .filter(|n| !n.trim().is_empty())
            {
                Some(name) => format!("{actor} changed the room name to {name}"),
                None => format!("{actor} removed the room name"),
            }
        }
        StateChange::Topic => format!("{actor} changed the topic"),
        StateChange::RoomAvatar => format!("{actor} changed the room avatar"),
        StateChange::Encryption => format!("{actor} turned on encryption"),
        StateChange::CallStarted => format!("{actor} started a call"),
        StateChange::LegacyCall => format!("{actor} placed an old-style call Twicecat cannot join"),
        StateChange::Other { event_type } => format!(
            "{actor} changed the room ({})",
            one_line_capped(event_type, MAX_EVENT_TYPE_CHARS)
        ),
    }
}

/// How many characters of a name in "Seen by" and the typing line are shown.
pub const MAX_SHORT_NAME_CHARS: usize = 24;
/// How many characters of a user ID are shown next to an ambiguous name.
const MAX_SHORT_ID_CHARS: usize = 48;
/// How many names "Seen by" lists before "+N".
pub const MAX_SEEN_NAMES: usize = 3;
/// How many earlier items are searched for the name of a user.
const NAME_SEARCH_REACH: usize = 500;

/// The part of a user ID between `@` and the first `:`, sanitised and
/// capped; the whole ID when that part is empty.
fn localpart(id: &UserId) -> String {
    let full = id.to_string();
    let local = full
        .strip_prefix('@')
        .and_then(|rest| rest.split(':').next())
        .filter(|l| !l.is_empty())
        .unwrap_or(&full);
    name_or_placeholder(local, MAX_SHORT_NAME_CHARS)
}

/// A short name: the display name (sanitised, capped), followed by the user
/// ID when the name is ambiguous; the localpart when there is no usable name.
fn short_name(name: Option<&str>, ambiguous: bool, id: &UserId) -> String {
    let clean = name
        .map(|n| one_line_capped(n, MAX_SHORT_NAME_CHARS))
        .filter(|n| !n.trim().is_empty());
    match clean {
        Some(clean) if ambiguous => {
            format!(
                "{clean} ({})",
                name_or_placeholder(&id.to_string(), MAX_SHORT_ID_CHARS)
            )
        }
        Some(clean) => clean,
        None => localpart(id),
    }
}

/// The text above the composer about who is typing (plan Q13), or `None`
/// when nobody is. Names with the same text show the user ID too.
pub fn typing_line(users: &[TypingUser]) -> Option<String> {
    let label = |user: &TypingUser| {
        let clean = user
            .name
            .as_deref()
            .map(|n| one_line_capped(n, MAX_SHORT_NAME_CHARS));
        let twin = clean.as_ref().is_some_and(|c| {
            users.iter().any(|other| {
                other.user_id != user.user_id
                    && other
                        .name
                        .as_deref()
                        .is_some_and(|n| one_line_capped(n, MAX_SHORT_NAME_CHARS) == *c)
            })
        });
        short_name(user.name.as_deref(), twin, &user.user_id)
    };
    match users {
        [] => None,
        [one] => Some(format!("{} is typing\u{2026}", label(one))),
        [a, b] => Some(format!("{} and {} are typing\u{2026}", label(a), label(b))),
        _ => Some("Several people are typing\u{2026}".to_owned()),
    }
}

/// "Seen by A, B, C +N" for the users whose receipt points at an item
/// (plan Q12). `known` gives a user's display name and whether it is
/// ambiguous, when the timeline knows it.
pub fn seen_by_line(
    read_by: &[UserId],
    known: impl Fn(&UserId) -> Option<(String, bool)>,
) -> Option<String> {
    if read_by.is_empty() {
        return None;
    }
    let names: Vec<String> = read_by
        .iter()
        .take(MAX_SEEN_NAMES)
        .map(|id| match known(id) {
            Some((name, ambiguous)) => short_name(Some(&name), ambiguous, id),
            None => short_name(None, false, id),
        })
        .collect();
    let rest = read_by.len().saturating_sub(MAX_SEEN_NAMES);
    Some(if rest > 0 {
        format!("Seen by {} +{rest}", names.join(", "))
    } else {
        format!("Seen by {}", names.join(", "))
    })
}

/// The display name a message in `items` gave for `user`, searching the
/// newest items first, and whether it is ambiguous.
pub fn known_name(items: &[TimelineItem], user: &UserId) -> Option<(String, bool)> {
    items
        .iter()
        .rev()
        .take(NAME_SEARCH_REACH)
        .find_map(|item| match &item.kind {
            TimelineItemKind::Message {
                sender, details, ..
            } if *sender == *user => details
                .sender_name
                .clone()
                .map(|n| (n, details.sender_name_ambiguous)),
            _ => None,
        })
}

/// The delivery state of a message, for its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Echo {
    /// Confirmed by the server, or received from it.
    Delivered,
    /// Queued or in flight.
    Sending,
    /// Failed; Retry and Discard are offered.
    Failed {
        /// The queue retries by itself when the network returns.
        recoverable: bool,
    },
}

/// The [`Echo`] of a message (plan Q11).
pub fn echo_of(sent: SendStatus, details: &MessageDetails) -> Echo {
    match sent {
        SendStatus::Sent => Echo::Delivered,
        SendStatus::Sending => Echo::Sending,
        SendStatus::Failed => Echo::Failed {
            recoverable: details.send_failure.is_some_and(|f| f.recoverable),
        },
    }
}

/// The status line under a local echo.
pub fn echo_text(echo: Echo) -> Option<&'static str> {
    match echo {
        Echo::Delivered => None,
        Echo::Sending => Some("\u{25F7} Sending\u{2026}"),
        Echo::Failed { recoverable: false } => Some("Failed to send"),
        Echo::Failed { recoverable: true } => Some("Failed to send. Will retry when back online"),
    }
}

#[cfg(test)]
thread_local! {
    /// Rows built on this thread, for the virtualization test.
    static BUILT_ROWS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_upload_percent_is_a_whole_number() {
        assert_eq!(upload_percent(0.0), "0 %");
        assert_eq!(upload_percent(0.426), "42 %");
        assert_eq!(upload_percent(1.0), "100 %");
        assert_eq!(upload_percent(7.0), "100 %");
        assert_eq!(upload_percent(-1.0), "0 %");
    }

    use twicecat_core::api::EventId;

    use super::*;

    #[test]
    fn placeholders_have_distinct_text() {
        let all = [
            PlaceholderKind::Redacted,
            PlaceholderKind::UnableToDecrypt,
            PlaceholderKind::Image,
            PlaceholderKind::Video,
            PlaceholderKind::Audio,
            PlaceholderKind::File,
            PlaceholderKind::Sticker,
            PlaceholderKind::Poll,
            PlaceholderKind::Location,
            PlaceholderKind::Unsupported,
        ];
        let texts: Vec<_> = all.iter().map(|k| placeholder_text(*k)).collect();
        for (i, t) in texts.iter().enumerate() {
            assert!(!t.is_empty());
            assert!(!texts[..i].contains(t), "{t}");
        }
        assert_eq!(
            placeholder_text(PlaceholderKind::Redacted),
            "Message deleted"
        );
        assert_eq!(
            placeholder_text(PlaceholderKind::UnableToDecrypt),
            "Unable to decrypt this message"
        );
        assert_eq!(
            placeholder_text(PlaceholderKind::Image),
            "sent an image (not shown yet)"
        );
    }

    #[test]
    fn shields_have_the_look_and_wording_of_their_code() {
        let codes = [
            (
                ShieldCode::AuthenticityNotGuaranteed,
                "The authenticity of this encrypted message can't be guaranteed on this device",
            ),
            (
                ShieldCode::UnknownDevice,
                "Encrypted by an unknown or deleted device",
            ),
            (
                ShieldCode::UnsignedDevice,
                "Encrypted by a device not verified by its owner",
            ),
            (
                ShieldCode::UnverifiedIdentity,
                "Encrypted by an unverified user",
            ),
            (
                ShieldCode::VerificationViolation,
                "Sender's identity changed",
            ),
            (
                ShieldCode::MismatchedSender,
                "The sender of this message does not match the device that sent it",
            ),
            (
                ShieldCode::SentInClear,
                "Sent in clear in an encrypted room",
            ),
        ];
        for (code, wording) in codes {
            assert_eq!(shield_text(Shield::Red(code)), (Look::Red, wording));
            assert_eq!(shield_text(Shield::Grey(code)), (Look::Grey, wording));
        }
    }

    #[test]
    fn every_utd_reason_has_its_line() {
        let wait = "Waiting for this message";
        let shared = "The sender didn't share the key for this message";
        let cases = [
            (None, wait),
            (Some(UtdReason::Unknown), wait),
            (
                Some(UtdReason::SentBeforeWeJoined),
                "You don't have access to this message",
            ),
            (
                Some(UtdReason::HistoricalMessageAndDeviceIsUnverified),
                "You need to verify this device for access to historical messages",
            ),
            (
                Some(UtdReason::HistoricalMessageAndBackupIsDisabled),
                "Historical messages are not available on this device",
            ),
            (
                Some(UtdReason::WithheldForUnverifiedOrInsecureDevice),
                "Your device doesn't meet the sender's security requirements",
            ),
            (Some(UtdReason::WithheldBySender), shared),
            (Some(UtdReason::UnknownDevice), shared),
            (Some(UtdReason::UnsignedDevice), shared),
            (
                Some(UtdReason::VerificationViolation),
                "Sender's verified identity was reset",
            ),
        ];
        for (reason, line) in cases {
            assert_eq!(utd_text(reason), line, "{reason:?}");
        }
    }

    #[test]
    fn only_an_undecryptable_placeholder_uses_the_reason_line() {
        let utd = Some(UtdReason::WithheldBySender);
        assert_eq!(
            placeholder_line(PlaceholderKind::UnableToDecrypt, utd),
            utd_text(utd)
        );
        assert_eq!(
            placeholder_line(PlaceholderKind::UnableToDecrypt, None),
            "Waiting for this message"
        );
        assert_eq!(
            placeholder_line(PlaceholderKind::Redacted, utd),
            placeholder_text(PlaceholderKind::Redacted)
        );
    }

    #[test]
    fn sender_labels() {
        let id = UserId::from("@a:x");
        assert_eq!(sender_label(Some("Alice"), false, &id), "Alice");
        assert_eq!(sender_label(None, false, &id), "@a:x");
        assert_eq!(sender_label(Some("  \u{3000}"), false, &id), "@a:x");
        assert_eq!(sender_label(Some(""), false, &id), "@a:x");
        assert_eq!(sender_label(Some("Alice"), true, &id), "Alice (@a:x)");
        assert_eq!(
            sender_label(Some("Al\nice\u{202E}"), false, &id),
            "Al\u{23CE}ice\u{FFFD}"
        );
        let long = "n".repeat(1000);
        assert_eq!(sender_label(Some(&long), false, &id).chars().count(), 256);
        let evil = UserId::from("@e\nvil:x");
        assert_eq!(sender_label(None, false, &evil), "@e\u{23CE}vil:x");
    }

    #[test]
    fn state_texts() {
        let t = |c: StateChange| state_text("Alice", &c);
        let target = |n: Option<&str>| (UserId::from("@b:x"), n.map(str::to_owned));
        let (id, name) = target(Some("Bob"));
        assert_eq!(t(StateChange::Joined), "Alice joined");
        assert_eq!(t(StateChange::Left), "Alice left");
        assert_eq!(
            t(StateChange::Invited {
                target: id.clone(),
                target_name: name.clone()
            }),
            "Alice invited Bob"
        );
        let (id2, none) = target(None);
        assert_eq!(
            t(StateChange::Kicked {
                target: id2,
                target_name: none
            }),
            "Alice removed @b:x"
        );
        assert_eq!(
            t(StateChange::DisplayName {
                old: Some("Al".into()),
                new: Some("Ally".into())
            }),
            "Al changed their name to Ally"
        );
        assert_eq!(
            t(StateChange::DisplayName {
                old: None,
                new: Some("Ally".into())
            }),
            "Alice changed their name to Ally"
        );
        assert_eq!(
            t(StateChange::DisplayName {
                old: None,
                new: None
            }),
            "Alice removed their name"
        );
        assert_eq!(
            t(StateChange::RoomName {
                name: Some("Lounge".into())
            }),
            "Alice changed the room name to Lounge"
        );
        assert_eq!(
            t(StateChange::RoomName { name: None }),
            "Alice removed the room name"
        );
    }

    #[test]
    fn every_state_change_names_the_actor_and_is_one_line() {
        let (id, name) = (UserId::from("@b:x"), Some("Bo\nb\u{202E}".to_owned()));
        let target = || (id.clone(), name.clone());
        let changes = [
            StateChange::Joined,
            StateChange::Left,
            StateChange::Invited {
                target: target().0,
                target_name: target().1,
            },
            StateChange::Kicked {
                target: target().0,
                target_name: target().1,
            },
            StateChange::Banned {
                target: target().0,
                target_name: target().1,
            },
            StateChange::Unbanned {
                target: target().0,
                target_name: target().1,
            },
            StateChange::InviteAccepted,
            StateChange::InviteRejected,
            StateChange::InviteRevoked {
                target: target().0,
                target_name: target().1,
            },
            StateChange::Knocked,
            StateChange::KnockAccepted {
                target: target().0,
                target_name: target().1,
            },
            StateChange::KnockRetracted,
            StateChange::KnockDenied {
                target: target().0,
                target_name: target().1,
            },
            StateChange::Avatar,
            StateChange::RoomCreated,
            StateChange::Topic,
            StateChange::RoomAvatar,
            StateChange::Encryption,
            StateChange::CallStarted,
            StateChange::LegacyCall,
            StateChange::Other {
                event_type: "m.x\n.y".into(),
            },
        ];
        for change in changes {
            let out = state_text("Alice", &change);
            assert!(out.starts_with("Alice "), "{out}");
            assert!(!out.contains(['\n', '\u{202E}']), "{out:?}");
        }
    }

    // ---- Task 13: receipts, typing, local echoes ----

    fn typist(id: &str, name: Option<&str>) -> TypingUser {
        TypingUser {
            user_id: UserId::from(id),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn typing_line_for_zero_one_two_and_many_users() {
        assert_eq!(typing_line(&[]), None);
        assert_eq!(
            typing_line(&[typist("@a:x", Some("Alice"))]).as_deref(),
            Some("Alice is typing\u{2026}")
        );
        assert_eq!(
            typing_line(&[typist("@a:x", Some("Alice")), typist("@b:x", None)]).as_deref(),
            Some("Alice and b are typing\u{2026}")
        );
        let three = [
            typist("@a:x", Some("Alice")),
            typist("@b:x", Some("Bob")),
            typist("@c:x", Some("Cy")),
        ];
        assert_eq!(
            typing_line(&three).as_deref(),
            Some("Several people are typing\u{2026}")
        );
    }

    #[test]
    fn typing_names_are_sanitised_capped_and_disambiguated() {
        let line = typing_line(&[typist("@a:x", Some("Al\nice\u{202E}"))]).expect("a line");
        assert!(!line.contains(['\n', '\u{202E}']), "{line:?}");
        let long = "n".repeat(500);
        let line = typing_line(&[typist("@a:x", Some(&long))]).expect("a line");
        assert!(line.chars().count() < 60, "{}", line.chars().count());
        // Two users with the same name: the IDs tell them apart.
        let line = typing_line(&[typist("@a:x", Some("Sam")), typist("@b:x", Some("Sam"))])
            .expect("a line");
        assert_eq!(line, "Sam (@a:x) and Sam (@b:x) are typing\u{2026}");
        // A blank name falls back to the localpart.
        let line = typing_line(&[typist("@carol:x", Some("  "))]).expect("a line");
        assert_eq!(line, "carol is typing\u{2026}");
    }

    fn ids(list: &[&str]) -> Vec<UserId> {
        list.iter().map(|u| UserId::from(*u)).collect()
    }

    #[test]
    fn seen_by_lists_three_names_then_a_count() {
        let known = |id: &UserId| match id.as_str() {
            "@a:x" => Some(("Alice".to_owned(), false)),
            "@d:x" => Some(("Dan".to_owned(), true)),
            _ => None,
        };
        assert_eq!(seen_by_line(&[], known), None);
        assert_eq!(
            seen_by_line(&ids(&["@a:x"]), known).as_deref(),
            Some("Seen by Alice")
        );
        assert_eq!(
            seen_by_line(&ids(&["@a:x", "@bob:x", "@d:x"]), known).as_deref(),
            Some("Seen by Alice, bob, Dan (@d:x)")
        );
        assert_eq!(
            seen_by_line(&ids(&["@a:x", "@b:x", "@c:x", "@e:x", "@f:x"]), known).as_deref(),
            Some("Seen by Alice, b, c +2")
        );
    }

    #[test]
    fn seen_by_names_are_sanitised_and_capped() {
        let evil = |_: &UserId| Some(("Bo\nb\u{202E}".to_owned(), false));
        let line = seen_by_line(&ids(&["@b:x"]), evil).expect("a line");
        assert!(!line.contains(['\n', '\u{202E}']), "{line:?}");
        let long = |_: &UserId| Some(("n".repeat(500), false));
        let line = seen_by_line(&ids(&["@b:x"]), long).expect("a line");
        assert!(line.chars().count() < 60);
        let odd = seen_by_line(&ids(&["@:x"]), |_| None).expect("a line");
        assert_eq!(odd, "Seen by @:x");
    }

    #[test]
    fn known_names_come_from_the_senders_before_the_item() {
        let mut named = TimelineItem::new(
            "m".to_owned(),
            TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: String::new(),
                is_own: false,
                timestamp_ms: 0,
                sent: SendStatus::Sent,
                details: Box::new(MessageDetails {
                    sender_name: Some("Alice".into()),
                    sender_name_ambiguous: true,
                    ..MessageDetails::default()
                }),
            },
        );
        named.read_by = ids(&["@a:x"]);
        let items = vec![named];
        assert_eq!(
            known_name(&items, &UserId::from("@a:x")),
            Some(("Alice".to_owned(), true))
        );
        assert_eq!(known_name(&items, &UserId::from("@z:x")), None);
    }

    #[test]
    fn echo_state_follows_the_delivery_status_and_failure() {
        let plain = MessageDetails::default();
        assert_eq!(echo_of(SendStatus::Sent, &plain), Echo::Delivered);
        assert_eq!(echo_of(SendStatus::Sending, &plain), Echo::Sending);
        assert_eq!(
            echo_of(SendStatus::Failed, &plain),
            Echo::Failed { recoverable: false }
        );
        let recoverable = MessageDetails {
            send_failure: Some(SendFailure { recoverable: true }),
            ..MessageDetails::default()
        };
        assert_eq!(
            echo_of(SendStatus::Failed, &recoverable),
            Echo::Failed { recoverable: true }
        );
    }

    #[test]
    fn a_hovered_row_gets_the_hover_background_and_a_mention_keeps_its_own() {
        let sem = theme::semantic(&theme::built_in());
        let bg = |highlighted, hovered| {
            row_background(
                &sem,
                RowLook {
                    highlighted,
                    hovered,
                    flashed: false,
                },
            )
        };
        assert_eq!(bg(false, false), None);
        assert_eq!(bg(false, true), Some(sem.message_hover));
        assert_eq!(bg(true, false), Some(sem.message_highlight));
        assert_eq!(bg(true, true), Some(sem.message_highlight));
    }

    #[test]
    fn a_flashed_row_wins_over_the_mention_and_the_hover() {
        let sem = theme::semantic(&theme::built_in());
        let look = RowLook {
            highlighted: true,
            hovered: true,
            flashed: true,
        };
        assert_eq!(row_background(&sem, look), Some(sem.selection));
    }

    fn preview(state: ReplyState, name: Option<&str>, body: &str) -> ReplyPreview {
        ReplyPreview {
            event_id: EventId::from("$e"),
            sender: UserId::from(if state == ReplyState::Ready {
                "@bob:x"
            } else {
                ""
            }),
            sender_name: name.map(str::to_owned),
            body: body.to_owned(),
            state,
        }
    }

    #[test]
    fn the_reply_quote_names_the_sender_and_shows_one_line() {
        let ready = preview(ReplyState::Ready, Some("Bob"), "the question");
        assert_eq!(
            reply_quote_text(&ready),
            ("Bob".to_owned(), "the question".to_owned())
        );
        let no_name = preview(ReplyState::Ready, None, "hi");
        assert_eq!(reply_quote_text(&no_name).0, "@bob:x");
        let empty = preview(ReplyState::Ready, Some("Bob"), "");
        assert_eq!(reply_quote_text(&empty).1, "(no text)");
        let loading = preview(ReplyState::Loading, None, "");
        assert_eq!(
            reply_quote_text(&loading).1,
            "Loading the original message\u{2026}"
        );
        let gone = preview(ReplyState::Unavailable, None, "");
        assert_eq!(reply_quote_text(&gone).1, "Original message unavailable");
    }

    #[test]
    fn the_reply_quote_is_sanitised_and_capped() {
        let evil = preview(
            ReplyState::Ready,
            Some("Mallory\u{202E}evil"),
            &format!("line\u{202E}one{}", "x".repeat(500)),
        );
        let (name, body) = reply_quote_text(&evil);
        assert!(!name.contains('\u{202E}') && !body.contains('\u{202E}'));
        assert!(body.chars().count() <= 201, "{}", body.chars().count());
    }

    #[test]
    fn an_emote_starts_with_a_star_and_the_name_like_element() {
        let text = |t: &str| Inline::Text {
            text: t.to_owned(),
            style: TextStyle::default(),
        };
        let waves = RichText {
            blocks: vec![Block::Paragraph(vec![text("waves")])],
        };
        assert_eq!(
            with_emote_prefix(&waves, "carol").blocks,
            vec![Block::Paragraph(vec![text("* carol "), text("waves")])]
        );
        let list = RichText {
            blocks: vec![Block::Quote(vec![Block::Paragraph(vec![text("q")])])],
        };
        let out = with_emote_prefix(&list, "carol").blocks;
        assert_eq!(out[0], Block::Paragraph(vec![text("* carol ")]));
        assert_eq!(out[1..], list.blocks[..]);
    }

    #[test]
    fn only_edited_messages_get_the_marker() {
        let edited = MessageDetails {
            edited: true,
            ..MessageDetails::default()
        };
        assert_eq!(edited_text(&edited), Some("(edited)"));
        assert_eq!(edited_text(&MessageDetails::default()), None);
    }

    #[test]
    fn echo_texts() {
        assert_eq!(echo_text(Echo::Delivered), None);
        assert!(echo_text(Echo::Sending).is_some_and(|t| t.contains("Sending")));
        let failed = echo_text(Echo::Failed { recoverable: false }).expect("text");
        assert!(!failed.contains("back online"));
        let again = echo_text(Echo::Failed { recoverable: true }).expect("text");
        assert!(again.contains("Will retry when back online"));
    }

    #[test]
    fn rows_with_echo_states_and_receipts_build() {
        let mut view = TimelineView::default();
        let echo = |id: &str, sent, failure| {
            TimelineItem::new(
                id.to_owned(),
                TimelineItemKind::Message {
                    sender: UserId::from("@me:x"),
                    body: "hello".into(),
                    is_own: true,
                    timestamp_ms: 0,
                    sent,
                    details: Box::new(MessageDetails {
                        send_failure: failure,
                        ..MessageDetails::default()
                    }),
                },
            )
        };
        let mut seen = echo("seen", SendStatus::Sent, None);
        seen.read_by = ids(&["@a:x", "@b:x", "@c:x", "@d:x"]);
        let _ = view.apply_diffs(vec![ListDiff::Append {
            values: vec![
                seen,
                echo("sending", SendStatus::Sending, None),
                echo(
                    "failed",
                    SendStatus::Failed,
                    Some(SendFailure { recoverable: true }),
                ),
            ],
        }]);
        let theme = Theme::Light;
        let revealed = Revealed::new();
        let actions = ActionState::default();
        let env = RichEnv {
            theme: &theme,
            own_user: None,
            revealed: &revealed,
            actions: &actions,
            rooms: &[],
            members: &[],
            media: &NO_MEDIA,
            room: None,
            density: Density::Comfortable,
        };
        BUILT_ROWS.with(|built| built.set(0));
        let _ = view_in(&view, 0, &chrono::Utc, env);
        assert_eq!(BUILT_ROWS.with(Cell::get), 3);
    }

    #[test]
    fn the_scroll_state_survives_a_pagination() {
        use iced::advanced::widget::Tree;

        let theme = Theme::Light;
        let revealed = Revealed::new();
        let actions = ActionState::default();
        let env = RichEnv {
            theme: &theme,
            own_user: None,
            revealed: &revealed,
            actions: &actions,
            rooms: &[],
            members: &[],
            media: &NO_MEDIA,
            room: None,
            density: Density::Comfortable,
        };
        let mut view = TimelineView::default();
        let mut tree = Tree::new(view_in(&view, 0, &chrono::Utc, env));
        let (root, list) = (tree.tag, tree.children[0].tag);
        for state in [PaginationState::Loading, PaginationState::Idle] {
            view.pagination = state;
            tree.diff(view_in(&view, 0, &chrono::Utc, env));
            // Same tags: `Tree::diff` keeps the scrollable's state (its
            // offset) instead of making a new one.
            assert_eq!(tree.tag, root, "{state:?}");
            assert_eq!(tree.children[0].tag, list, "{state:?}");
        }
    }

    use std::time::{Duration, Instant};

    use ::image as image_crate;
    use chrono::Utc;
    use twicecat_core::api::{ListDiff, SendFailure};

    use crate::timeline::MAX_BUILT_ROWS;

    const NOW: u64 = 1_790_000_000_000;

    fn big_timeline(n: usize) -> TimelineView {
        let items: Vec<TimelineItem> = (0..n)
            .map(|i| {
                let kind = if i % 50 == 0 {
                    TimelineItemKind::DateDivider {
                        timestamp_ms: NOW - 86_400_000 * (i as u64 / 50 % 400),
                    }
                } else if i % 25 == 0 {
                    TimelineItemKind::State {
                        sender: UserId::from("@a:x"),
                        sender_name: Some("Alice".into()),
                        timestamp_ms: NOW,
                        change: StateChange::Joined,
                    }
                } else {
                    TimelineItemKind::Message {
                        sender: UserId::from(if i % 3 == 0 { "@a:x" } else { "@b:x" }),
                        body: format!("message {i}\nsecond line of message {i}"),
                        is_own: false,
                        timestamp_ms: NOW - (n - i) as u64 * 1000,
                        sent: SendStatus::Sent,
                        details: Box::new(MessageDetails::default()),
                    }
                };
                TimelineItem::new(format!("i{i}"), kind)
            })
            .collect();
        let mut timeline = TimelineView::default();
        timeline.apply_diffs(vec![ListDiff::Reset { values: items }]);
        timeline
    }

    fn theme() -> Theme {
        crate::theme::built_in()
    }

    fn env<'a>(theme: &'a Theme, revealed: &'a Revealed) -> RichEnv<'a> {
        RichEnv {
            theme,
            own_user: None,
            revealed,
            actions: &NO_ACTIONS,
            rooms: &[],
            members: &[],
            media: &NO_MEDIA,
            room: None,
            density: Density::Comfortable,
        }
    }

    static NO_MEDIA: std::sync::LazyLock<MediaModel> =
        std::sync::LazyLock::new(MediaModel::default);

    static NO_ACTIONS: std::sync::LazyLock<ActionState> =
        std::sync::LazyLock::new(ActionState::default);

    fn built_by(timeline: &TimelineView) -> usize {
        let (theme, revealed) = (theme(), Revealed::new());
        built_by_with(timeline, env(&theme, &revealed))
    }

    fn built_by_with(timeline: &TimelineView, env: RichEnv<'_>) -> usize {
        BUILT_ROWS.with(|c| c.set(0));
        let element = view_in(timeline, NOW, &Utc, env);
        drop(element);
        BUILT_ROWS.with(Cell::get)
    }

    #[test]
    fn only_the_window_is_built_for_10000_items() {
        let mut timeline = big_timeline(10_000);
        for offset in [0.0, 12_345.0, 150_000.0, f32::MAX] {
            timeline.offset_from_bottom = offset;
            let built = built_by(&timeline);
            assert_eq!(built, timeline.window().len(), "offset {offset}");
            assert!(built > 0 && built <= MAX_BUILT_ROWS, "{built}");
        }
    }

    /// The median time to build the view of 10,000 items.
    fn median_build_time() -> Duration {
        let timeline = big_timeline(10_000);
        let (theme, revealed) = (theme(), Revealed::new());
        let _ = built_by(&timeline);
        let mut times: Vec<Duration> = (0..15)
            .map(|_| {
                let start = Instant::now();
                drop(view_in(&timeline, NOW, &Utc, env(&theme, &revealed)));
                start.elapsed()
            })
            .collect();
        times.sort();
        let median = times[times.len() / 2];
        println!("timeline_pane view build, median of 15: {median:?}");
        median
    }

    /// The view of 10,000 items is built in far less than a second
    /// (docs/testing.md, R2): it only draws the window, not the list. How fast
    /// it is, is `perf_building_the_view_for_10000_items`.
    #[test]
    fn building_the_view_for_10000_items_does_not_take_seconds() {
        let median = median_build_time();
        assert!(median < Duration::from_secs(1), "{median:?}");
    }

    /// Within 0.5 ms (0.1 to 0.16 ms measured on a laptop and on CI; the 16 ms
    /// frame budget is far above). An indicator, not a gate.
    #[test]
    fn perf_building_the_view_for_10000_items() {
        twicecat_perf::record_duration(
            "app.timeline_pane.view_10000_items",
            median_build_time(),
            Duration::from_micros(500),
        );
    }

    #[test]
    fn formatted_messages_build_in_both_themes_with_every_item_kind() {
        use twicecat_core::api::{Block, Inline, MentionTarget, RichText, TextStyle};
        let rich = RichText {
            blocks: vec![
                Block::Paragraph(vec![
                    Inline::Text {
                        text: "hi ".into(),
                        style: TextStyle {
                            bold: true,
                            ..TextStyle::default()
                        },
                    },
                    Inline::Link {
                        text: "bank.example".into(),
                        url: "https://evil.example/".into(),
                        style: TextStyle::default(),
                    },
                    Inline::Mention {
                        target: MentionTarget::User(UserId::from("@a:x")),
                        label: "A".into(),
                    },
                    Inline::Spoiler(vec![Inline::LineBreak]),
                ]),
                Block::Code { text: "x".into() },
            ],
        };
        let mut timeline = TimelineView::default();
        let item = |id: &str, content: RichText| {
            TimelineItem::new(
                id.to_owned(),
                TimelineItemKind::Message {
                    sender: UserId::from("@a:x"),
                    body: "plain fallback".into(),
                    is_own: false,
                    timestamp_ms: NOW,
                    sent: SendStatus::Sent,
                    details: Box::new(MessageDetails {
                        content,
                        ..MessageDetails::default()
                    }),
                },
            )
        };
        timeline.apply_diffs(vec![ListDiff::Reset {
            values: vec![
                item("a", rich.clone()),
                item("b", rich),
                item("c", RichText::default()),
            ],
        }]);
        let (theme, revealed) = (crate::theme::built_in(), Revealed::new());
        assert_eq!(built_by_with(&timeline, env(&theme, &revealed)), 3);
    }

    #[test]
    fn a_body_stays_multi_line_but_cannot_use_bidi_or_break_the_cap() {
        let out = body_text("hi\r\n@alice:x: I agree\u{2028}ok\u{202E}");
        assert_eq!(
            out,
            format!(
                "hi\n@alice:x: I agree\nok{}",
                crate::text_safety::REPLACEMENT
            )
        );
        let long = "b".repeat(10_000);
        assert_eq!(body_text(&long).chars().count(), MAX_BODY_CHARS);
        let lines = "x\n".repeat(1000);
        assert_eq!(body_text(&lines).lines().count(), MAX_BODY_LINES);
    }

    fn group(key: &str, count: u32, own: bool) -> ReactionGroup {
        ReactionGroup {
            key: key.to_owned(),
            count,
            own,
            senders: Vec::new(),
        }
    }

    #[test]
    fn a_chip_shows_a_sanitised_capped_key_and_the_count() {
        assert_eq!(chip_label(&group("\u{1F44D}", 3, false)), "\u{1F44D} 3");
        assert_eq!(
            chip_label(&group("a\nb\u{202E}", 1, true)),
            "a\u{23CE}b\u{FFFD} 1"
        );
        let long = chip_label(&group(&"k".repeat(500), 2, false));
        assert_eq!(long.chars().count(), actions::MAX_KEY_CHARS + 2);
    }

    #[test]
    fn the_bar_shows_for_the_hovered_message_or_a_pending_delete() {
        let mut state = ActionState::default();
        assert!(!bar_visible(&state, "a"));
        state.hovered = Some("a".into());
        assert!(bar_visible(&state, "a") && !bar_visible(&state, "b"));
        state.confirm_delete = Some("b".into());
        assert!(bar_visible(&state, "b"));
    }

    #[test]
    fn rows_with_actions_and_reactions_build_without_changing_the_window() {
        use twicecat_core::api::ItemActions;
        let details = MessageDetails {
            actions: ItemActions {
                can_reply: true,
                can_edit: true,
                can_redact: true,
                can_react: true,
                can_copy: true,
            },
            reactions: vec![group("\u{1F44D}", 2, true), group("x", 1, false)],
            ..MessageDetails::default()
        };
        let mut timeline = TimelineView::default();
        timeline.apply_diffs(vec![ListDiff::Reset {
            values: vec![TimelineItem::new(
                "m".to_owned(),
                TimelineItemKind::Message {
                    sender: UserId::from("@a:x"),
                    body: "hi".into(),
                    is_own: true,
                    timestamp_ms: NOW,
                    sent: SendStatus::Sent,
                    details: Box::new(details),
                },
            )],
        }]);
        let (theme, revealed) = (theme(), Revealed::new());
        let hovered = ActionState {
            hovered: Some("m".into()),
            ..ActionState::default()
        };
        let confirming = ActionState {
            confirm_delete: Some("m".into()),
            ..ActionState::default()
        };
        for state in [&ActionState::default(), &hovered, &confirming] {
            let env = RichEnv {
                actions: state,
                ..env(&theme, &revealed)
            };
            assert_eq!(built_by_with(&timeline, env), 1);
        }
    }

    // ---- Phase 5 Task 9: media rows ----

    fn laid_out(element: Element<'_, Message>) -> iced::Size {
        use iced::advanced::layout::Limits;
        use iced::advanced::renderer::Headless;
        use iced::advanced::widget::Tree;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a current-thread runtime");
        let renderer = runtime
            .block_on(iced::Renderer::new(
                iced::Font::DEFAULT,
                iced::Pixels(16.0),
                Some("tiny-skia"),
            ))
            .expect("the headless tiny-skia renderer is not available");
        let mut element = element;
        let mut tree = Tree::new(element.as_widget());
        element
            .as_widget_mut()
            .layout(
                &mut tree,
                &renderer,
                &Limits::new(iced::Size::ZERO, iced::Size::new(1000.0, 2000.0)),
            )
            .size()
    }

    fn content(kind: MediaKind, w: Option<u32>, h: Option<u32>, name: &str) -> MediaContent {
        let mut media = MediaContent::new(kind, name.to_owned());
        media.width = w;
        media.height = h;
        media
    }

    #[test]
    fn an_image_box_has_one_size_whatever_has_arrived() {
        use crate::media::{Decoded, Thumb};
        let media = content(MediaKind::Image, Some(1600), Some(900), "p.png");
        let wanted = media_box(media.width, media.height);
        let (handle, w, h) = crate::media::decode_media(
            &{
                let img = image_crate::RgbaImage::new(40, 30);
                let mut out = std::io::Cursor::new(Vec::new());
                img.write_to(&mut out, image_crate::ImageFormat::Png)
                    .expect("encode");
                out.into_inner()
            },
            crate::media::decode_limits(),
        )
        .expect("decodes");
        let ready = Thumb::Ready(Decoded {
            handle,
            width: w,
            height: h,
        });
        for thumb in [None, Some(&Thumb::Unavailable), Some(&ready)] {
            assert_eq!(laid_out(image_box("i", &media, thumb)), wanted, "{thumb:?}");
        }
    }

    #[test]
    fn a_card_has_a_fixed_height_whatever_the_name_and_the_buttons() {
        let long = "x".repeat(5000);
        for name in ["a.txt", long.as_str(), "\u{202E}gnp.exe", ""] {
            for open in [false, true] {
                let media = content(MediaKind::File, None, None, name);
                let size = laid_out(file_card("i", &media, open));
                assert_eq!(size.height, CARD_H, "{name:?} {open}");
                assert_eq!(size.width, BOX_MAX_W);
            }
        }
    }

    #[test]
    fn the_estimate_matches_the_media_box_it_draws() {
        let image = content(MediaKind::Image, Some(1600), Some(900), "p.png");
        let box_h = media_box(image.width, image.height).height;
        assert_eq!(crate::timeline::media_height(&image), box_h);
        let file = content(MediaKind::Audio, None, None, "a.ogg");
        assert_eq!(crate::timeline::media_height(&file), CARD_H);
    }

    #[test]
    fn media_rows_build_with_and_without_a_caption() {
        let mut timeline = TimelineView::default();
        let item = |id: &str, kind: MediaKind, body: &str| {
            let details = MessageDetails {
                media: Some(Box::new(content(kind, Some(10), Some(10), "p.png"))),
                ..MessageDetails::default()
            };
            TimelineItem::new(
                id.to_owned(),
                TimelineItemKind::Message {
                    sender: UserId::from("@a:x"),
                    body: body.to_owned(),
                    is_own: false,
                    timestamp_ms: NOW,
                    sent: SendStatus::Sent,
                    details: Box::new(details),
                },
            )
        };
        timeline.apply_diffs(vec![ListDiff::Reset {
            values: vec![
                item("a", MediaKind::Image, "p.png"),
                item("b", MediaKind::Image, "look at this"),
                item("c", MediaKind::File, "p.png"),
                item("d", MediaKind::Video, "p.png"),
            ],
        }]);
        let (theme, revealed) = (theme(), Revealed::new());
        assert_eq!(built_by_with(&timeline, env(&theme, &revealed)), 4);
    }

    #[test]
    fn the_bar_offers_one_icon_per_allowed_action_and_sends_the_same_messages() {
        let all = ActionButtons {
            reply: true,
            edit: true,
            react: true,
            copy: true,
            delete: true,
        };
        let root = EventId::from("$root:x");
        let acts = bar_actions("i1", all, Some(&root));
        let icons: Vec<Icon> = acts.iter().map(|(icon, _)| *icon).collect();
        assert_eq!(
            icons,
            [
                Icon::Reply,
                Icon::Edit,
                Icon::React,
                Icon::Copy,
                Icon::Thread,
                Icon::Delete
            ],
            "delete last (set apart by the bar layout)"
        );
        for (icon, message) in acts {
            match (icon, message) {
                (Icon::Reply, Message::ReplyRequested(id))
                | (Icon::Edit, Message::EditRequested(id))
                | (Icon::React, Message::PickerOpened(id))
                | (Icon::Copy, Message::CopyRequested(id))
                | (Icon::Delete, Message::DeleteRequested(id)) => assert_eq!(id, "i1"),
                (Icon::Thread, Message::Threads(crate::threads::ThreadsMessage::Open(opened))) => {
                    assert_eq!(opened, root)
                }
                (icon, message) => panic!("{icon:?} sends {message:?}"),
            }
        }
        // Delete only asks: the question with its own Delete is the
        // confirmation, so no hover click removes a message.
        assert!(
            bar_actions("i1", all, None)
                .iter()
                .all(|(_, m)| !matches!(m, Message::DeleteConfirmed))
        );
        let none = ActionButtons::default();
        assert!(bar_actions("i1", none, None).is_empty());
        let only_react = ActionButtons {
            react: true,
            ..none
        };
        assert_eq!(bar_actions("i1", only_react, None).len(), 1);
    }

    #[test]
    fn delete_sits_a_full_step_away_from_the_other_bar_icons() {
        let none = ActionButtons::default();
        let width = |buttons: ActionButtons, thread: Option<&EventId>| {
            laid_out(action_bar("i", buttons, thread, false)).width
        };
        let copy = ActionButtons { copy: true, ..none };
        let delete = ActionButtons {
            delete: true,
            ..none
        };
        let both = ActionButtons {
            copy: true,
            delete: true,
            ..none
        };
        let root = EventId::from("$root:x");
        // What Delete adds next to Copy, less what Thread adds there, less
        // the two icons' own difference: the extra gap before Delete.
        let delete_adds = width(both, None) - width(copy, None);
        let thread_adds = width(copy, Some(&root)) - width(copy, None);
        let own = width(delete, None) - width(none, Some(&root));
        let extra = delete_adds - thread_adds - own;
        assert!(
            extra >= tokens::M - 0.01,
            "Delete is only {extra} px further from Copy than Thread is"
        );
    }

    #[test]
    fn a_call_state_line_has_the_call_icon_and_other_state_lines_have_none() {
        assert_eq!(state_icon(&StateChange::CallStarted), Some(Icon::Call));
        assert_eq!(state_icon(&StateChange::LegacyCall), Some(Icon::Call));
        assert_eq!(state_icon(&StateChange::Joined), None);
    }
}
