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
use iced::font::{Style as FontStyle, Weight};
use iced::widget::{
    Column, Row, button, container, mouse_area, row, scrollable, sensor, space, stack, text,
};
use iced::{Background, Color, Element, Fill, Font, Padding, Theme};
use tessera_core::api::{
    MessageDetails, MessageText, PaginationState, PlaceholderKind, ReactionGroup, SendStatus,
    StateChange, TimelineItem, TimelineItemKind, TypingUser, UserId,
};

use crate::actions::{self, ActionButtons, ActionState};
use crate::dates::{day_label, time_label};
use crate::rich_text::{Ctx, Revealed, blocks_view};
use crate::state::Message;
use crate::text_safety::{
    MAX_BODY_CHARS, MAX_NAME_CHARS, multi_line_capped, name_or_placeholder, one_line_capped,
};
use crate::theme;
use crate::timeline::{
    CONTINUATION_PAD, DIVIDER_H, INDENT_PX, MARKER_H, OVERSCAN_PX, SIDE_PAD_PX, START_H,
    TIMELINE_ID, TOP_PAD, TimelineView, is_continuation, spacer_heights,
};

/// How many lines of a body are shown.
pub const MAX_BODY_LINES: usize = 200;

/// How many characters of an event type are shown.
const MAX_EVENT_TYPE_CHARS: usize = 64;

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
    let window = timeline.window();
    let (above, below) = spacer_heights(&timeline.prefix, &window);
    let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(window.len() + 2);
    rows.push(space().height(above).into());
    rows.extend(window.map(|i| built_row(timeline, i, now_ms, tz, env)));
    rows.push(space().height(below).into());
    // `from_vec`, because `Column::push` drops zero-size children, and the
    // content is always the two spacers plus the window.
    let list = scrollable(Column::from_vec(rows).width(Fill))
        .id(TIMELINE_ID)
        .anchor_bottom()
        .on_scroll(Message::TimelineScrolled)
        .width(Fill)
        .height(Fill);
    // Always a stack with the list first: iced keeps a widget's state only
    // while the widget at that place in the tree keeps its type, so returning
    // the bare list when idle would reset the scroll offset to the bottom on
    // every pagination.
    let badge: Element<'a, Message> = if timeline.pagination == PaginationState::Loading {
        container(text("Loading\u{2026}").size(13).style(muted))
            .center_x(Fill)
            .padding(4)
            .into()
    } else {
        space().into()
    };
    stack([list.into(), badge]).into()
}

const BOLD: Font = Font {
    weight: Weight::Bold,
    ..Font::DEFAULT
};

const ITALIC: Font = Font {
    style: FontStyle::Italic,
    ..Font::DEFAULT
};

fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme::semantic(theme).muted),
    }
}

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
) -> Element<'a, Message> {
    #[cfg(test)]
    BUILT_ROWS.with(|built| built.set(built.get() + 1));
    let id = timeline.items[i].id.clone();
    let on_resize_id = id.clone();
    sensor(row_content(timeline, i, now_ms, tz, env))
        .key(id.clone())
        .anticipate(OVERSCAN_PX)
        .on_show(move |size| Message::RowMeasured {
            id: id.clone(),
            height: size.height,
        })
        .on_resize(move |size| Message::RowMeasured {
            id: on_resize_id.clone(),
            height: size.height,
        })
        .into()
}

fn row_content<'a, Tz: TimeZone>(
    timeline: &'a TimelineView,
    i: usize,
    now_ms: u64,
    tz: &Tz,
    env: RichEnv<'_>,
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
                content = content.push(header(name, time_label(*timestamp_ms, tz)));
            }
            // The body sits below the header in its own widget, so it cannot
            // pass for a sender line. Rich text has no muted colour and no
            // small size, so it cannot copy the time next to a name either.
            content = content.push(body_widget(
                &items[i].id,
                body,
                details,
                echo == Echo::Sending,
                env,
            ));
            if let Some(label) = edited_text(details) {
                content = content.push(text(label).size(12).style(muted));
            }
            let buttons = actions::action_buttons(details, *is_own);
            if !details.reactions.is_empty() {
                content = content.push(reactions_row(&items[i].id, &details.reactions, buttons));
            }
            if let Some(line) = echo_row(&items[i].id, echo) {
                content = content.push(line);
            }
            if let Some(line) = receipts_row(items, i) {
                content = content.push(line);
            }
            let hovered = bar_visible(env.actions, &items[i].id);
            let framed = framed(content.into(), continuation, details.highlighted, hovered);
            with_actions(&items[i].id, framed, buttons, env.actions)
        }
        TimelineItemKind::Placeholder {
            sender,
            sender_name,
            timestamp_ms,
            what,
            ..
        } => {
            let continuation = is_continuation(items, i);
            let mut content = Column::new().width(Fill);
            if !continuation {
                let name = sender_label(sender_name.as_deref(), false, sender);
                content = content.push(header(name, time_label(*timestamp_ms, tz)));
            }
            content = content.push(
                text(placeholder_text(*what))
                    .font(ITALIC)
                    .style(muted)
                    .width(Fill),
            );
            if let Some(line) = receipts_row(items, i) {
                content = content.push(line);
            }
            framed(content.into(), continuation, false, false)
        }
        TimelineItemKind::State {
            sender,
            sender_name,
            change,
            ..
        } => {
            let actor = sender_label(sender_name.as_deref(), false, sender);
            let mut line = Column::new().width(Fill).push(
                text(state_text(&actor, change))
                    .size(13)
                    .style(muted)
                    .width(Fill),
            );
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
        TimelineItemKind::DateDivider { timestamp_ms } => fixed_height(
            text(day_label(*timestamp_ms, now_ms, tz))
                .size(13)
                .style(muted),
            DIVIDER_H,
        ),
        TimelineItemKind::ReadMarker => {
            fixed_height(text("New messages").size(13).style(accent), MARKER_H)
        }
        TimelineItemKind::TimelineStart => fixed_height(
            text("This is the start of the conversation")
                .size(13)
                .style(muted),
            START_H,
        ),
        TimelineItemKind::Other => space().height(0).into(),
    }
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
        let mut chip = button(text(chip_label(group)).size(13))
            .padding([2, 6])
            .style(if group.own {
                button::primary
            } else {
                button::secondary
            });
        if buttons.react {
            chip = chip.on_press(Message::ReactionToggled {
                item: id.to_owned(),
                key: MessageText::from(group.key.as_str()),
            });
        }
        chips = chips.push(chip);
    }
    if buttons.react {
        chips = chips.push(
            button(text("+").size(13))
                .padding([2, 6])
                .style(button::secondary)
                .on_press(Message::PickerOpened(id.to_owned())),
        );
    }
    container(chips.spacing(4).wrap().vertical_spacing(4))
        .padding(Padding {
            top: 2.0,
            ..Padding::ZERO
        })
        .into()
}

/// A small text button of the action bar.
fn bar_button<'a>(label: &'static str, message: Message) -> Element<'a, Message> {
    button(text(label).size(13))
        .padding([2, 6])
        .style(button::secondary)
        .on_press(message)
        .into()
}

/// The hover bar of message `id`, or the delete question.
fn action_bar<'a>(id: &str, buttons: ActionButtons, confirming: bool) -> Element<'a, Message> {
    let mut bar = Row::new().spacing(4).align_y(iced::Center);
    if confirming {
        bar = bar
            .push(text("Delete this message?").size(13))
            .push(
                button(text("Delete").size(13))
                    .padding([2, 6])
                    .style(button::danger)
                    .on_press(Message::DeleteConfirmed),
            )
            .push(bar_button("Cancel", Message::DeleteCancelled));
    } else {
        if buttons.reply {
            bar = bar.push(bar_button("Reply", Message::ReplyRequested(id.to_owned())));
        }
        if buttons.edit {
            bar = bar.push(bar_button("Edit", Message::EditRequested(id.to_owned())));
        }
        if buttons.react {
            bar = bar.push(bar_button("React", Message::PickerOpened(id.to_owned())));
        }
        if buttons.copy {
            bar = bar.push(bar_button("Copy", Message::CopyRequested(id.to_owned())));
        }
        if buttons.delete {
            bar = bar.push(bar_button(
                "Delete",
                Message::DeleteRequested(id.to_owned()),
            ));
        }
    }
    container(bar)
        .padding(2)
        .style(container::bordered_box)
        .into()
}

/// Wraps a message row so that hovering it shows the action bar. The bar is
/// a stack layer over the row's top right corner: it never changes the row's
/// height, so the measurements stay valid.
fn with_actions<'a>(
    id: &str,
    row: Element<'a, Message>,
    buttons: ActionButtons,
    state: &ActionState,
) -> Element<'a, Message> {
    let confirming = state.confirm_delete.as_deref() == Some(id);
    let content = if bar_visible(state, id) && (buttons.any() || confirming) {
        let bar = container(action_bar(id, buttons, confirming))
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

/// The body of a message: its formatted content, or the plain text when the
/// core sent none.
fn body_widget<'a>(
    id: &str,
    body: &str,
    details: &MessageDetails,
    dim: bool,
    env: RichEnv<'_>,
) -> Element<'a, Message> {
    if details.content.blocks.is_empty() {
        let plain = text(body_text(body)).width(Fill);
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
    };
    blocks_view(&details.content, &ctx, env.theme)
}

/// The marker under an edited message.
pub fn edited_text(details: &MessageDetails) -> Option<&'static str> {
    details.edited.then_some("(edited)")
}

/// The status line of a local echo: muted while sending, in the danger
/// colour when failed, with **Retry** and **Discard**.
fn echo_row<'a>(id: &str, echo: Echo) -> Option<Element<'a, Message>> {
    let label = echo_text(echo)?;
    Some(match echo {
        Echo::Delivered => return None,
        Echo::Sending => text(label).size(12).style(muted).into(),
        Echo::Failed { .. } => row![
            text(label).size(13).style(text::danger),
            bar_button("Retry", Message::RetryRequested(id.to_owned())),
            bar_button("Discard", Message::DiscardRequested(id.to_owned())),
        ]
        .spacing(8)
        .align_y(iced::Center)
        .into(),
    })
}

/// "Seen by" under item `i`, when other users' receipts point at it.
fn receipts_row<'a>(items: &[TimelineItem], i: usize) -> Option<Element<'a, Message>> {
    let line = seen_by_line(&items[i].read_by, |user| known_name(&items[..=i], user))?;
    Some(text(line).size(12).style(muted).width(Fill).into())
}

/// A message body ready to draw: line breaks kept, bidi controls replaced,
/// capped in lines and characters.
pub fn body_text(body: &str) -> String {
    multi_line_capped(body, MAX_BODY_CHARS, MAX_BODY_LINES)
}

/// A line of text centred in a row of a fixed height.
fn fixed_height<'a>(label: iced::widget::Text<'a>, height: f32) -> Element<'a, Message> {
    container(label).center_x(Fill).center_y(height).into()
}

/// The sender and time above a message.
fn header<'a>(name: String, time: String) -> Element<'a, Message> {
    row![text(name).font(BOLD), text(time).size(12).style(muted)]
        .spacing(8)
        .align_y(iced::Center)
        .into()
}

/// The background of a message row: the mention highlight, else the hover
/// background under the pointer (so each message of a run from one sender
/// shows where it starts and ends), else none.
pub fn row_background(sem: &theme::Semantic, highlighted: bool, hovered: bool) -> Option<Color> {
    if highlighted {
        Some(sem.message_highlight)
    } else if hovered {
        Some(sem.message_hover)
    } else {
        None
    }
}

/// Pads a message row: indented, with less space above a continuation, on
/// the [`row_background`].
fn framed(
    content: Element<'_, Message>,
    continuation: bool,
    highlighted: bool,
    hovered: bool,
) -> Element<'_, Message> {
    let padded = container(content).width(Fill).padding(Padding {
        top: if continuation {
            CONTINUATION_PAD
        } else {
            TOP_PAD
        },
        right: SIDE_PAD_PX,
        bottom: 0.0,
        left: INDENT_PX,
    });
    if !highlighted && !hovered {
        return padded.into();
    }
    padded
        .style(move |theme: &Theme| container::Style {
            background: row_background(&theme::semantic(theme), highlighted, hovered)
                .map(Background::Color),
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
        PlaceholderKind::Unsupported => "sent something Tessera cannot show yet",
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
        let sem = theme::LIGHT_SEMANTIC;
        assert_eq!(row_background(&sem, false, false), None);
        assert_eq!(row_background(&sem, false, true), Some(sem.message_hover));
        assert_eq!(
            row_background(&sem, true, false),
            Some(sem.message_highlight)
        );
        assert_eq!(
            row_background(&sem, true, true),
            Some(sem.message_highlight)
        );
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

    use chrono::Utc;
    use tessera_core::api::{ListDiff, SendFailure};

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
        crate::theme::theme_for(iced::theme::Mode::Light)
    }

    fn env<'a>(theme: &'a Theme, revealed: &'a Revealed) -> RichEnv<'a> {
        RichEnv {
            theme,
            own_user: None,
            revealed,
            actions: &NO_ACTIONS,
        }
    }

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

    #[test]
    fn building_the_view_for_10000_items_is_fast() {
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
        assert!(median < Duration::from_millis(16), "{median:?}");
    }

    #[test]
    fn formatted_messages_build_in_both_themes_with_every_item_kind() {
        use tessera_core::api::{Block, Inline, MentionTarget, RichText, TextStyle};
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
        for mode in [iced::theme::Mode::Light, iced::theme::Mode::Dark] {
            let (theme, revealed) = (crate::theme::theme_for(mode), Revealed::new());
            assert_eq!(built_by_with(&timeline, env(&theme, &revealed)), 3);
        }
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
        use tessera_core::api::ItemActions;
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
}
