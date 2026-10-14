//! The call on screen (plan Task 3, Q4): the call bar at the bottom of the
//! room list, and the participants panel beside the call's room.
//!
//! Both are slots that are an empty space when there is nothing to show, so
//! the room list and the room pane keep their place in the widget tree when a
//! call starts or ends.

use iced::widget::{
    Column, Row, button, column, container, pick_list, row, rule, scrollable, slider, space, stack,
    text,
};
use iced::{Background, Element, Fill, Length, border};

use crate::calls::{
    CallMessage, CleaningChoice, MAX_VOLUME, MUTED_MARK, PANEL_WIDTH, ParticipantRow,
};
use crate::layout::PaneLayout;
use crate::screens::main_view::display_name;
use crate::screens::room_list_pane::LOCK_MARK;
use crate::state::{App, Message};
use crate::theme;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::{
    self, PERSON_SLOT, Severity, active_icon_style, avatar, card_style, danger_style, icon_style,
    modal, muted, severity_tag_style, tip,
};
use crate::ui::tokens;
use crate::verification::elide_middle;
use twicecat_core::api::InputMode;

/// Width of the ring drawn around a speaking person's picture.
const SPEAKING_RING: f32 = 2.0;
/// How many people who declined are named before "and N more".
const DECLINED_NAMED: usize = 3;

fn call(message: CallMessage) -> Message {
    Message::Call(message)
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
        let picture = avatar(
            &ring.caller,
            &caller_name,
            room.filter(|r| r.is_dm).and_then(|r| r.avatar.as_ref()),
            &app.avatars,
        );
        let mut words = column![text(headline).size(18)].spacing(4);
        if let Some(under) = under {
            words = words.push(text(under).size(13).style(muted));
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
                .style(muted),
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
                        muted(t)
                    }
                })
                .into(),
        );
        if let Some(line) = declined_line(current) {
            lines.push(text(line).size(12).style(muted).into());
        }
        if let Some((line, talking)) = talk_line(app) {
            lines.push(
                text(line)
                    .size(12)
                    .style(
                        move |t: &iced::Theme| {
                            if talking { text::primary(t) } else { muted(t) }
                        },
                    )
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

/// [`tip`] with words that are built, not borrowed.
fn tip_owned<'a>(content: impl Into<Element<'a, Message>>, label: String) -> Element<'a, Message> {
    iced::widget::tooltip(
        content,
        text(label).size(tokens::CAPTION),
        iced::widget::tooltip::Position::Bottom,
    )
    .gap(tokens::XS)
    .padding(tokens::S)
    .style(card_style)
    .into()
}

/// How the rail's mic button looks. The four looks differ in icon or fill, so
/// a live microphone is never only a tooltip away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicLook {
    /// Open microphone (voice activity, or push-to-talk is not in use).
    Plain,
    /// Muted or deafened.
    Muted,
    /// Push-to-talk or toggle-to-talk, not transmitting.
    Gated,
    /// Push-to-talk or toggle-to-talk, transmitting.
    Talking,
}

impl MicLook {
    /// The icon on the button: the open mic only while sound can go out.
    pub const fn icon(self) -> Icon {
        match self {
            Self::Plain | Self::Talking => Icon::Mic,
            Self::Muted | Self::Gated => Icon::MicOff,
        }
    }
}

/// The encryption marker of the call in the rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockLock {
    /// Connected with end-to-end encrypted media.
    Encrypted,
    /// Connected without it.
    Unencrypted,
}

impl DockLock {
    /// The severity that colours the marker.
    pub const fn severity(self) -> Severity {
        match self {
            Self::Encrypted => Severity::Success,
            Self::Unencrypted => Severity::Warning,
        }
    }

    /// The marker's glyph.
    pub const fn icon(self) -> Icon {
        match self {
            Self::Encrypted => Icon::Lock,
            Self::Unencrypted => Icon::Warning,
        }
    }

    /// The marker's words, for its tooltip.
    pub const fn words(self) -> &'static str {
        match self {
            Self::Encrypted => "Call is end-to-end encrypted",
            Self::Unencrypted => "Call is not encrypted",
        }
    }
}

/// What the rail's dock shows about the call, as data so that it can be
/// tested: everything the expanded [`bar`] says in words is a visible mark
/// here, with the words in its tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockCall {
    /// The call button's icon (a warning while reconnecting).
    pub phase_icon: Icon,
    /// The call phase in words.
    pub phase_words: String,
    /// The encryption marker, once connected.
    pub lock: Option<DockLock>,
    /// The mic button's look.
    pub mic: MicLook,
    /// The mic button's tooltip: mute or unmute, and the talk line.
    pub mic_words: String,
    /// Who declined the ring.
    pub declined: Option<String>,
}

/// The dock's call, if there is one.
pub fn dock_call(app: &App) -> Option<DockCall> {
    let current = app.calls.call.as_ref()?;
    let (mute, _) = mute_labels(current.muted, current.deafened);
    let reconnecting = matches!(current.phase, crate::calls::CallPhase::Reconnecting { .. });
    let talk = talk_line(app);
    let mic = if current.muted || current.deafened {
        MicLook::Muted
    } else {
        match &talk {
            Some((_, true)) => MicLook::Talking,
            Some((_, false)) => MicLook::Gated,
            None => MicLook::Plain,
        }
    };
    let mic_words = match talk {
        Some((line, _)) => format!("{mute}. {line}"),
        None => mute.to_owned(),
    };
    Some(DockCall {
        phase_icon: if reconnecting {
            Icon::Warning
        } else {
            Icon::Speaker
        },
        phase_words: current.phase.label(),
        lock: match current.phase {
            crate::calls::CallPhase::Connected { encrypted: true } => Some(DockLock::Encrypted),
            crate::calls::CallPhase::Connected { encrypted: false } => Some(DockLock::Unencrypted),
            _ => None,
        },
        mic,
        mic_words,
        declined: declined_line(current),
    })
}

/// A small coloured mark with its words in a tooltip.
fn marker<'a>(glyph: &'static str, severity: Severity, words: String) -> Element<'a, Message> {
    tip_owned(
        container(text(glyph).size(tokens::CAPTION))
            .padding([1.0, tokens::S])
            .style(severity_tag_style(severity)),
        words,
    )
}

/// The call bar for the rail: the same actions as icon buttons in a narrow
/// column (the call's room, mute, deafen, leave), each with a tooltip, and
/// the state the bar writes out as visible marks: the encryption lock, the
/// mic button's look while push-to-talk gates it, who declined, and the
/// notice with its own close button. An empty space without a call or
/// notice, like [`bar`].
pub fn rail_dock(app: &App) -> Element<'_, Message> {
    let calls = &app.calls;
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if let (Some(current), Some(dock)) = (&calls.call, dock_call(app)) {
        let (_, deafen) = mute_labels(current.muted, current.deafened);
        items.push(tip_owned(
            button(icon(dock.phase_icon))
                .on_press(call(CallMessage::ShowCall))
                .padding(tokens::S)
                .style(icon_style),
            dock.phase_words.clone(),
        ));
        if let Some(lock) = dock.lock {
            items.push(marker(
                lock.icon().glyph(),
                lock.severity(),
                lock.words().to_owned(),
            ));
        }
        if let Some(declined) = dock.declined.clone() {
            items.push(marker(Icon::People.glyph(), Severity::Warning, declined));
        }
        let mic_style: fn(&iced::Theme, button::Status) -> button::Style = match dock.mic {
            MicLook::Plain | MicLook::Gated => icon_style,
            MicLook::Muted => active_icon_style,
            MicLook::Talking => button::primary,
        };
        items.push(tip_owned(
            button(icon(dock.mic.icon()))
                .on_press(call(CallMessage::ToggleMute))
                .padding(tokens::S)
                .style(mic_style),
            dock.mic_words.clone(),
        ));
        items.push(tip(
            button(icon(Icon::Deafened))
                .on_press(call(CallMessage::ToggleDeafen))
                .padding(tokens::S)
                .style(if current.deafened {
                    active_icon_style
                } else {
                    icon_style
                }),
            deafen,
        ));
        items.push(tip(
            button(icon(Icon::Leave))
                .on_press_maybe(calls.pending.is_none().then_some(call(CallMessage::Leave)))
                .padding(tokens::S)
                .style(danger_style),
            "Leave the call",
        ));
    }
    if let Some(notice) = &calls.notice {
        // The warning mark only shows the words; closing is its own button.
        items.push(marker(
            Icon::Warning.glyph(),
            Severity::Danger,
            notice.clone(),
        ));
        items.push(tip(
            button(icon(Icon::Close))
                .on_press(call(CallMessage::DismissNotice))
                .padding(tokens::S)
                .style(icon_style),
            "Dismiss the notice",
        ));
    }
    if items.is_empty() {
        return space().height(0).into();
    }
    container(
        Column::from_vec(items)
            .spacing(tokens::XS)
            .align_x(iced::Center),
    )
    .padding(tokens::XS)
    .center_x(Fill)
    .style(|t: &iced::Theme| container::Style {
        background: Some(Background::Color(theme::semantic(t).selection)),
        border: border::rounded(tokens::MEDIUM),
        ..container::Style::default()
    })
    .into()
}

/// The participants panel beside the call's room, or an empty slot.
///
/// With `layout.panel_fills` (a narrow window) it takes the room pane's
/// width, and its header carries the trust tag of a direct message, since the
/// room header is out of sight.
pub fn panel_slot(app: &App, layout: PaneLayout) -> Element<'_, Message> {
    if !layout.call || !app.calls.panel_shown(app.selected.as_ref()) {
        return space().width(0).into();
    }
    let rows = app.calls.rows();
    let mut head: Vec<Element<'_, Message>> = vec![crate::screens::main_view::panel_title(
        app,
        text(format!("In the call ({})", rows.len())).size(18),
        layout.panel_fills,
    )];
    if let Some(trust) = layout
        .panel_fills
        .then(|| crate::screens::main_view::trust_strip(app))
        .flatten()
    {
        head.push(trust);
    }
    head.push(
        button(text("\u{2715}"))
            .on_press(call(CallMessage::TogglePanel))
            .style(button::secondary)
            .into(),
    );
    let header = Row::with_children(head).spacing(8).align_y(iced::Center);
    let list = Column::with_children(rows.into_iter().map(|r| participant(app, r))).spacing(10);
    let body = column![header, scrollable(list).height(Fill)]
        .spacing(8)
        .width(Fill)
        .height(Fill);
    row![
        rule::vertical(1),
        container(body).padding(8).width(Fill).height(Fill)
    ]
    .width(if layout.panel_fills {
        Length::Fill
    } else {
        Length::Fixed(PANEL_WIDTH)
    })
    .height(Fill)
    .into()
}

/// One person: the picture (ringed while they talk), the name, the marks,
/// and for other people the volume and cleaning controls.
fn participant(app: &App, row_view: ParticipantRow) -> Element<'_, Message> {
    let speaking = row_view.speaking;
    let picture = container(avatar(
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
        name = name.push(text("(you)").size(12).style(muted));
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
        details = details.push(text(status).size(12).style(muted));
    }
    if let Some(cleaning) = row_view.cleaning {
        details = details.push(text(cleaning).size(12).style(muted));
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
        // Ctrl + wheel is the zoom gesture, which iced's slider takes as a
        // step: the volume has the - and + buttons instead.
        let volume = crate::ui::guard::no_wheel(
            slider(0..=MAX_VOLUME, controls.volume, move |percent| {
                call(CallMessage::Volume {
                    user_id: slide_user.clone(),
                    percent,
                })
            })
            .step(5u16)
            .shift_step(1u16)
            .default(100u16)
            .on_release(call(CallMessage::VolumeReleased))
            .width(Fill),
        );
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
        details = details.push(kit::menu(
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
        ));
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

    fn app_with_call(
        phase: crate::calls::CallPhase,
        input_mode: Option<InputMode>,
        talking: bool,
    ) -> App {
        use crate::bridge::EventsSlot;
        use crate::state::Startup;
        use twicecat_core::api::{AudioSettings, RoomId};
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let (_events_tx, events_rx) = tokio::sync::mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        app.calls.call = Some(crate::calls::CurrentCall::new(RoomId::from("!r:x"), phase));
        if let Some(mode) = input_mode {
            app.calls.settings = Some(AudioSettings {
                input_mode: mode,
                ..AudioSettings::default()
            });
        }
        app.ptt.talking = talking;
        app
    }

    fn ptt(toggle: bool) -> InputMode {
        InputMode::PushToTalk {
            release_delay_ms: 0,
            toggle,
        }
    }

    const CONNECTED: crate::calls::CallPhase =
        crate::calls::CallPhase::Connected { encrypted: true };

    #[test]
    fn the_rail_dock_shows_whether_push_to_talk_is_transmitting() {
        for toggle in [false, true] {
            let talking =
                dock_call(&app_with_call(CONNECTED, Some(ptt(toggle)), true)).expect("a call");
            let gated =
                dock_call(&app_with_call(CONNECTED, Some(ptt(toggle)), false)).expect("a call");
            assert_eq!(talking.mic, MicLook::Talking);
            assert_eq!(gated.mic, MicLook::Gated);
            assert_ne!(talking.mic.icon(), gated.mic.icon());
            assert_ne!(talking, gated);
            assert!(
                talking.mic_words.contains("Talking"),
                "{}",
                talking.mic_words
            );
            assert!(
                gated.mic_words.contains("Not talking"),
                "{}",
                gated.mic_words
            );
        }
        let open = dock_call(&app_with_call(CONNECTED, None, false)).expect("a call");
        assert_eq!(open.mic, MicLook::Plain);
        assert_eq!(open.mic.icon(), Icon::Mic);
        let mut muted = app_with_call(CONNECTED, Some(ptt(false)), true);
        muted.calls.call.as_mut().expect("call").muted = true;
        assert_eq!(dock_call(&muted).expect("a call").mic, MicLook::Muted);
        let _: Element<'_, Message> = rail_dock(&muted);
    }

    #[test]
    fn the_rail_dock_marks_encryption_and_the_lack_of_it() {
        let lock = |phase| {
            dock_call(&app_with_call(phase, None, false))
                .expect("a call")
                .lock
        };
        assert_eq!(lock(CONNECTED), Some(DockLock::Encrypted));
        assert_eq!(
            lock(crate::calls::CallPhase::Connected { encrypted: false }),
            Some(DockLock::Unencrypted)
        );
        assert_eq!(lock(crate::calls::CallPhase::Joining), None);
        assert_ne!(DockLock::Encrypted.icon(), DockLock::Unencrypted.icon());
        assert_ne!(
            DockLock::Encrypted.severity(),
            DockLock::Unencrypted.severity()
        );
        assert_ne!(DockLock::Encrypted.words(), DockLock::Unencrypted.words());
    }

    #[test]
    fn the_rail_dock_shows_who_declined_and_the_notice() {
        let mut app = app_with_call(CONNECTED, None, false);
        assert_eq!(dock_call(&app).expect("a call").declined, None);
        app.calls.call.as_mut().expect("call").declined = vec![UserId::from("@u0:x")];
        assert_eq!(
            dock_call(&app).expect("a call").declined.as_deref(),
            Some("@u0:x declined")
        );
        app.calls.notice = Some("The call ended".to_owned());
        let _: Element<'_, Message> = rail_dock(&app);
        app.calls.call = None;
        let _: Element<'_, Message> = rail_dock(&app);
        assert!(dock_call(&app).is_none());
    }
}
