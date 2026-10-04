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
use iced::{Background, Element, Fill, Font, Padding, Theme};
use tessera_core::api::{
    MessageDetails, MessageText, PaginationState, PlaceholderKind, ReactionGroup, StateChange,
    TimelineItemKind, UserId,
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
    if timeline.pagination == PaginationState::Loading {
        let badge = container(text("Loading\u{2026}").size(13).style(muted))
            .center_x(Fill)
            .padding(4);
        stack([list.into(), badge.into()]).into()
    } else {
        list.into()
    }
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
            details,
            ..
        } => {
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
            content = content.push(body_widget(&items[i].id, body, details, env));
            let buttons = actions::action_buttons(details, *is_own);
            if !details.reactions.is_empty() {
                content = content.push(reactions_row(&items[i].id, &details.reactions, buttons));
            }
            let framed = framed(content.into(), continuation, details.highlighted);
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
            framed(content.into(), continuation, false)
        }
        TimelineItemKind::State {
            sender,
            sender_name,
            change,
            ..
        } => {
            let actor = sender_label(sender_name.as_deref(), false, sender);
            container(
                text(state_text(&actor, change))
                    .size(13)
                    .style(muted)
                    .width(Fill),
            )
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
    env: RichEnv<'_>,
) -> Element<'a, Message> {
    if details.content.blocks.is_empty() {
        return text(body_text(body)).width(Fill).into();
    }
    let ctx = Ctx {
        own_user: env.own_user,
        item: id,
        revealed: env.revealed,
    };
    blocks_view(&details.content, &ctx, env.theme)
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

/// Pads a message row: indented, with less space above a continuation, on a
/// highlight background when it mentions the user.
fn framed(
    content: Element<'_, Message>,
    continuation: bool,
    highlighted: bool,
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
    if highlighted {
        padded
            .style(|theme: &Theme| container::Style {
                background: Some(Background::Color(theme::semantic(theme).message_highlight)),
                ..container::Style::default()
            })
            .into()
    } else {
        padded.into()
    }
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

    use std::time::{Duration, Instant};

    use chrono::Utc;
    use tessera_core::api::{ListDiff, MessageDetails, SendStatus, TimelineItem};

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
