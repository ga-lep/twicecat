//! The call on screen (plan Task 3, Q4): the call bar at the bottom of the
//! room list, and the participants panel beside the call's room.
//!
//! Both are slots that are an empty space when there is nothing to show, so
//! the room list and the room pane keep their place in the widget tree when a
//! call starts or ends.

use iced::widget::{
    Column, button, column, container, pick_list, row, rule, scrollable, slider, space, stack, text,
};
use iced::{Background, Element, Fill, Length, border};

use crate::calls::{
    CallMessage, CleaningChoice, MAX_VOLUME, MUTED_MARK, PANEL_WIDTH, ParticipantRow,
};
use crate::screens::main_view::display_name;
use crate::screens::room_dialogs::{PERSON_SLOT, modal, person_avatar};
use crate::screens::room_list_pane::LOCK_MARK;
use crate::state::{App, Message};
use crate::theme;
use crate::verification::elide_middle;
use twicecat_core::api::InputMode;

/// Width of the ring drawn around a speaking person's picture.
const SPEAKING_RING: f32 = 2.0;
/// How many people who declined are named before "and N more".
const DECLINED_NAMED: usize = 3;

fn call(message: CallMessage) -> Message {
    Message::Call(message)
}

fn muted_style(theme: &iced::Theme) -> text::Style {
    text::Style {
        color: Some(theme::semantic(theme).muted),
    }
}

/// The words on the call bar's mute and deafen buttons.
pub fn mute_labels(muted: bool, deafened: bool) -> (&'static str, &'static str) {
    let mute = if muted || deafened { "Unmute" } else { "Mute" };
    let deafen = if deafened { "Undeafen" } else { "Deafen" };
    (mute, deafen)
}

/// The line about who declined our ring: "Bob declined", up to three
/// names (the call room's member name, else the user ID) and then a count.
pub fn declined_line(call: &crate::calls::CurrentCall) -> Option<String> {
    let declined = &call.declined;
    if declined.is_empty() {
        return None;
    }
    let named: Vec<String> = declined
        .iter()
        .take(DECLINED_NAMED)
        .map(|user| {
            call.member(user)
                .and_then(|m| m.display_name.as_deref())
                .filter(|n| !n.trim().is_empty())
                .map_or_else(|| elide_middle(user.as_str(), 64), |n| elide_middle(n, 64))
        })
        .collect();
    let rest = declined.len().saturating_sub(DECLINED_NAMED);
    Some(if rest == 0 {
        format!("{} declined", named.join(", "))
    } else {
        format!("{} and {rest} more declined", named.join(", "))
    })
}

/// Width of the incoming-call card.
const RING_CARD_WIDTH: f32 = 380.0;

/// The words of the incoming-call card: the headline and the line under it.
/// A DM's name is the caller's; a group room names the room under it.
pub fn ring_lines(caller: &str, room_name: Option<&str>, is_dm: bool) -> (String, Option<String>) {
    let caller = elide_middle(caller, 64);
    let headline = format!("{caller} is calling");
    let under = match (is_dm, room_name) {
        (false, Some(room)) => Some(format!("in {}", display_name(room))),
        _ => None,
    };
    (headline, under)
}

/// The base with the incoming-call card as the layer above it (plan Task 5):
/// always a stack of two, so the card coming and going keeps the widget
/// state under it.
pub fn ring_layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let card = app.calls.card().map(|ring| {
        let room = app.rooms.iter().find(|r| r.id == ring.room_id);
        // A DM's room is named after the other person and has their picture.
        let is_dm = room.is_some_and(|r| r.is_dm);
        let caller_name = room.filter(|r| r.is_dm).map_or_else(
            || ring.caller.as_str().to_owned(),
            |r| display_name(&r.name),
        );
        let (headline, under) = ring_lines(&caller_name, room.map(|r| r.name.as_str()), is_dm);
        let picture = person_avatar(
            &ring.caller,
            &caller_name,
            room.filter(|r| r.is_dm).and_then(|r| r.avatar.as_ref()),
            &app.avatars,
        );
        let mut words = column![text(headline).size(18)].spacing(4);
        if let Some(under) = under {
            words = words.push(text(under).size(13).style(muted_style));
        }
        let idle = !ring.answered;
        let accept = button(text("Accept").size(14))
            .padding([6, 14])
            .style(button::primary)
            .on_press_maybe(idle.then(|| call(CallMessage::Accept(ring.room_id.clone()))));
        let decline = button(text("Decline").size(14))
            .padding([6, 14])
            .style(button::danger)
            .on_press_maybe(idle.then(|| call(CallMessage::Decline(ring.room_id.clone()))));
        let content = column![
            row![picture, words].spacing(12).align_y(iced::Center),
            row![space().width(Fill), decline, accept].spacing(8),
            text("Esc hides this; the room keeps ringing.")
                .size(12)
                .style(muted_style),
        ]
        .spacing(16);
        modal(content, RING_CARD_WIDTH)
    });
    stack([base, card.unwrap_or_else(|| space().into())]).into()
}

/// The call bar, or an empty slot without a call or a notice.
/// The call bar's push-to-talk line, and whether it says "talking": only in
/// push-to-talk or toggle-to-talk.
pub fn talk_line(app: &App) -> Option<(String, bool)> {
    let settings = app.calls.settings.as_ref()?;
    let InputMode::PushToTalk { toggle, .. } = settings.input_mode else {
        return None;
    };
    if app.ptt.talking {
        return Some(("Talking".to_owned(), true));
    }
    let bindings = &settings.push_to_talk;
    let verb = if toggle { "press" } else { "hold" };
    let line = match (bindings.key.as_deref(), bindings.gamepad_button.is_some()) {
        (Some(key), _) => format!(
            "Not talking: {verb} {}",
            crate::audio_settings::key_label(key)
        ),
        (None, true) => format!("Not talking: {verb} the gamepad button"),
        (None, false) => "Not talking: no key or button is set (Audio\u{2026})".to_owned(),
    };
    Some((line, false))
}

pub fn bar(app: &App) -> Element<'_, Message> {
    let calls = &app.calls;
    let mut lines: Vec<Element<'_, Message>> = Vec::new();
    if let Some(current) = &calls.call {
        let room_name = app
            .rooms
            .iter()
            .find(|r| r.id == current.room_id)
            .map_or_else(|| "Call".to_owned(), |r| display_name(&r.name));
        let mut title = row![
            button(text(room_name).size(14).wrapping(text::Wrapping::None))
                .on_press(call(CallMessage::ShowCall))
                .padding([2, 4])
                .style(button::text)
                .width(Fill)
                .clip(true)
        ]
        .spacing(4)
        .align_y(iced::Center);
        if matches!(
            current.phase,
            crate::calls::CallPhase::Connected { encrypted: true }
        ) {
            title = title.push(text(LOCK_MARK).size(12));
        }
        lines.push(title.into());
        let reconnecting = matches!(current.phase, crate::calls::CallPhase::Reconnecting { .. });
        lines.push(
            text(current.phase.label())
                .size(12)
                .style(move |t: &iced::Theme| {
                    if reconnecting {
                        text::danger(t)
                    } else {
                        muted_style(t)
                    }
                })
                .into(),
        );
        if let Some(line) = declined_line(current) {
            lines.push(text(line).size(12).style(muted_style).into());
        }
        if let Some((line, talking)) = talk_line(app) {
            lines.push(
                text(line)
                    .size(12)
                    .style(move |t: &iced::Theme| {
                        if talking {
                            text::primary(t)
                        } else {
                            muted_style(t)
                        }
                    })
                    .into(),
            );
        }
        let (mute, deafen) = mute_labels(current.muted, current.deafened);
        let control = |label: &'static str, message: CallMessage, active: bool| {
            button(text(label).size(13))
                .padding([4, 8])
                .style(if active {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press(call(message))
        };
        let leave = button(text("Leave").size(13))
            .padding([4, 8])
            .style(button::danger)
            .on_press_maybe(calls.pending.is_none().then_some(call(CallMessage::Leave)));
        lines.push(
            row![
                control(
                    mute,
                    CallMessage::ToggleMute,
                    current.muted || current.deafened
                ),
                control(deafen, CallMessage::ToggleDeafen, current.deafened),
                button(text("Audio\u{2026}").size(13))
                    .padding([4, 8])
                    .style(button::secondary)
                    .on_press(Message::Audio(crate::audio_settings::AudioMessage::Open)),
                space().width(Fill),
                leave,
            ]
            .spacing(4)
            .into(),
        );
    }
    if let Some(notice) = &calls.notice {
        lines.push(
            row![
                text(notice.as_str())
                    .size(12)
                    .style(text::danger)
                    .width(Fill),
                button(text("\u{2715}").size(12))
                    .padding([0, 6])
                    .style(button::secondary)
                    .on_press(call(CallMessage::DismissNotice)),
            ]
            .spacing(4)
            .align_y(iced::Center)
            .into(),
        );
    }
    if lines.is_empty() {
        return space().height(0).into();
    }
    container(Column::from_vec(lines).spacing(4))
        .padding(8)
        .width(Fill)
        .style(|t: &iced::Theme| container::Style {
            background: Some(Background::Color(theme::semantic(t).selection)),
            border: border::rounded(6.0),
            ..container::Style::default()
        })
        .into()
}

/// The participants panel beside the call's room, or an empty slot.
pub fn panel_slot(app: &App) -> Element<'_, Message> {
    if !app.calls.panel_shown(app.selected.as_ref()) {
        return space().width(0).into();
    }
    let rows = app.calls.rows();
    let header = row![
        text(format!("In the call ({})", rows.len()))
            .size(18)
            .width(Fill),
        button(text("\u{2715}"))
            .on_press(call(CallMessage::TogglePanel))
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center);
    let list = Column::with_children(rows.into_iter().map(|r| participant(app, r))).spacing(10);
    let body = column![header, scrollable(list).height(Fill)]
        .spacing(8)
        .width(Fill)
        .height(Fill);
    row![
        rule::vertical(1),
        container(body).padding(8).width(Fill).height(Fill)
    ]
    .width(PANEL_WIDTH)
    .height(Fill)
    .into()
}

/// One person: the picture (ringed while they talk), the name, the marks,
/// and for other people the volume and cleaning controls.
fn participant(app: &App, row_view: ParticipantRow) -> Element<'_, Message> {
    let speaking = row_view.speaking;
    let picture = container(person_avatar(
        &row_view.user_id,
        &row_view.name,
        row_view.avatar.as_ref(),
        &app.avatars,
    ))
    .padding(SPEAKING_RING)
    .style(move |t: &iced::Theme| container::Style {
        border: border::rounded(PERSON_SLOT / 2.0 + SPEAKING_RING)
            .width(SPEAKING_RING)
            .color(if speaking {
                theme::semantic(t).highlight
            } else {
                iced::Color::TRANSPARENT
            }),
        ..container::Style::default()
    });
    let mut name = row![text(row_view.name.clone()).wrapping(text::Wrapping::None)]
        .spacing(4)
        .align_y(iced::Center);
    if row_view.is_local {
        name = name.push(text("(you)").size(12).style(muted_style));
    }
    if row_view.muted {
        name = name.push(text(MUTED_MARK).size(12));
    }
    let mut details = column![name].spacing(2).width(Fill);
    let status = match (speaking, row_view.muted) {
        (_, true) => Some("Muted"),
        (true, false) => Some("Speaking"),
        (false, false) => None,
    };
    if let Some(status) = status {
        details = details.push(text(status).size(12).style(muted_style));
    }
    if let Some(cleaning) = row_view.cleaning {
        details = details.push(text(cleaning).size(12).style(muted_style));
    }
    if let Some(controls) = row_view.controls {
        let user = row_view.user_id.clone();
        let step = |label: &'static str, up: bool| {
            button(text(label).size(13))
                .padding([0, 8])
                .style(button::secondary)
                .on_press(call(CallMessage::VolumeStep {
                    user_id: user.clone(),
                    up,
                }))
        };
        let slide_user = row_view.user_id.clone();
        let volume = slider(0..=MAX_VOLUME, controls.volume, move |percent| {
            call(CallMessage::Volume {
                user_id: slide_user.clone(),
                percent,
            })
        })
        .step(5u16)
        .shift_step(1u16)
        .default(100u16)
        .on_release(call(CallMessage::VolumeReleased))
        .width(Fill);
        details = details.push(
            row![
                step("\u{2212}", false),
                volume,
                step("+", true),
                text(format!("{}\u{a0}%", controls.volume))
                    .size(12)
                    .width(Length::Fixed(44.0)),
            ]
            .spacing(4)
            .align_y(iced::Center),
        );
        let cleaning_user = row_view.user_id.clone();
        details = details.push(
            pick_list(
                CleaningChoice::ALL,
                Some(controls.cleaning),
                move |choice| {
                    call(CallMessage::Cleaning {
                        user_id: cleaning_user.clone(),
                        choice,
                    })
                },
            )
            .text_size(12)
            .padding([2, 6]),
        );
    }
    row![picture, details]
        .spacing(8)
        .align_y(iced::Alignment::Start)
        .into()
}

#[cfg(test)]
mod tests {
    use twicecat_core::api::UserId;

    use super::*;

    #[test]
    fn the_ring_card_names_the_caller_and_a_group_room() {
        assert_eq!(
            ring_lines("Bob", Some("Bob"), true),
            ("Bob is calling".to_owned(), None)
        );
        assert_eq!(
            ring_lines("@bob:x", Some("Team\nroom"), false),
            (
                "@bob:x is calling".to_owned(),
                Some("in Team\u{23ce}room".to_owned())
            )
        );
        assert_eq!(ring_lines("@bob:x", None, false).1, None);
    }

    #[test]
    fn mute_and_deafen_labels_follow_the_state() {
        assert_eq!(mute_labels(false, false), ("Mute", "Deafen"));
        assert_eq!(mute_labels(true, false), ("Unmute", "Deafen"));
        assert_eq!(mute_labels(false, true), ("Unmute", "Undeafen"));
        assert_eq!(mute_labels(true, true), ("Unmute", "Undeafen"));
    }

    #[test]
    fn the_declined_line_names_three_then_counts() {
        use twicecat_core::api::{MembershipView, PowerLevelView, Role, RoomId, RoomMemberView};
        let mut call =
            crate::calls::CurrentCall::new(RoomId::from("!r:x"), crate::calls::CallPhase::Joining);
        assert_eq!(declined_line(&call), None);
        let users = |n: usize| -> Vec<UserId> {
            (0..n).map(|i| UserId::from(format!("@u{i}:x"))).collect()
        };
        call.declined = users(1);
        assert_eq!(declined_line(&call).as_deref(), Some("@u0:x declined"));
        call.declined = users(3);
        call.members = Some(vec![RoomMemberView {
            user_id: UserId::from("@u1:x"),
            display_name: Some("Bob".into()),
            avatar: None,
            power: PowerLevelView::Level(0),
            role: Role::User,
            membership: MembershipView::Joined,
        }]);
        assert_eq!(
            declined_line(&call).as_deref(),
            Some("@u0:x, Bob, @u2:x declined")
        );
        call.declined = users(5);
        assert_eq!(
            declined_line(&call).as_deref(),
            Some("@u0:x, Bob, @u2:x and 2 more declined")
        );
    }
}
