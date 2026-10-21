//! The call on screen (plan Task 3, Q4; UI/UX rework Task 11): the call dock
//! at the bottom of the room list, the participants panel beside the call's
//! room, and the incoming-call card.
//!
//! The dock and the panel are slots that are an empty space when there is
//! nothing to show, so the room list and the room pane keep their place in
//! the widget tree when a call starts or ends.
//!
//! What the microphone is doing is always drawn, never only a tooltip: an
//! open microphone, a muted or deafened one, and push-to-talk (or
//! toggle-to-talk) gated or transmitting differ by glyph and by fill. The
//! encryption of the call, who declined and a notice are visible marks too.

use iced::widget::button::Status;
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, button, column, container, pick_list, row, scrollable, slider, space, stack, text,
};
use iced::{Element, Fill, Length, Theme, border};

use crate::calls::{
    CallMessage, CallPhase, CleaningChoice, MAX_VOLUME, MUTED_MARK, PANEL_WIDTH, ParticipantRow,
};
use crate::layout::PaneLayout;
use crate::screens::main_view::display_name;
use crate::state::{App, Message};
use crate::theme;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::{
    self, Severity, active_icon_style, avatar_sized, card_style, danger_style, ghost_style,
    icon_style, modal, muted, open_icon_style, primary_style, secondary_style, severity_tag_style,
    speaking_ring, tip,
};
use crate::ui::tokens;
use crate::verification::elide_middle;
use twicecat_core::api::InputMode;

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
/// Side of the caller's picture on the incoming-call card.
const RING_AVATAR: f32 = 64.0;

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

/// Words that never lose a character to the edge: a glyph break where a word
/// (a user ID) has no other.
const WRAP: Wrapping = Wrapping::WordOrGlyph;

/// A button with an icon and words side by side, filling its width.
fn action_button<'a>(
    glyph: Icon,
    words: &'static str,
    style: fn(&Theme, Status) -> button::Style,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    button(
        container(
            row![text(glyph.glyph()), text(words).wrapping(Wrapping::None)]
                .spacing(tokens::S)
                .align_y(iced::Center),
        )
        .center_x(Fill),
    )
    .width(Fill)
    .padding([tokens::S, tokens::M])
    .style(style)
    .on_press_maybe(on_press)
    .into()
}

/// The base with the incoming-call card as the layer above it (plan Task 5):
/// always a stack of two, so the card coming and going keeps the widget
/// state under it.
///
/// The card: the caller's picture, the headline and the room under it, and
/// **Decline** then **Accept** (the Tab order) side by side with a gap, each
/// half the width, below the words so nothing covers them.
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
        let picture = avatar_sized(
            &ring.caller,
            &caller_name,
            room.filter(|r| r.is_dm).and_then(|r| r.avatar.as_ref()),
            &app.avatars,
            RING_AVATAR,
        );
        let mut words = column![
            text(headline)
                .size(tokens::HEADLINE)
                .width(Fill)
                .wrapping(WRAP)
        ]
        .spacing(tokens::XS)
        .width(Fill);
        if let Some(under) = under {
            words = words.push(
                text(under)
                    .size(tokens::BODY)
                    .width(Fill)
                    .wrapping(WRAP)
                    .style(muted),
            );
        }
        // The buttons wait while the card arms: a click already on its way
        // when the card came up was meant for what was under it.
        let arming = app.calls.arming();
        let idle = !ring.answered && !arming;
        let accept = action_button(
            Icon::Call,
            "Accept",
            primary_style,
            idle.then(|| call(CallMessage::Accept(ring.room_id.clone()))),
        );
        let decline = action_button(
            Icon::Close,
            "Decline",
            danger_style,
            idle.then(|| call(CallMessage::Decline(ring.room_id.clone()))),
        );
        let content = column![
            row![picture, words]
                .spacing(tokens::L)
                .align_y(iced::Center),
            row![decline, accept].spacing(tokens::L),
            column![
                // Always one line, so the buttons never move when it ends.
                text(if arming {
                    "Answer in a moment\u{2026}"
                } else {
                    "Ready to answer"
                })
                .size(tokens::CAPTION)
                .wrapping(WRAP)
                .style(muted),
                text("Esc hides this; the room keeps ringing.")
                    .size(tokens::CAPTION)
                    .wrapping(WRAP)
                    .style(muted),
                text("Keys: Ctrl+Shift+A accepts, Ctrl+Shift+X declines (Cmd on macOS).")
                    .size(tokens::CAPTION)
                    .wrapping(WRAP)
                    .style(muted),
            ]
            .spacing(tokens::XS),
        ]
        .spacing(tokens::L);
        modal(content, RING_CARD_WIDTH)
    });
    stack([base, card.unwrap_or_else(|| space().into())]).into()
}

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
    let line = match (
        bindings.key.as_deref(),
        bindings.mouse_button.as_deref(),
        bindings.gamepad_button.is_some(),
    ) {
        (Some(key), _, _) => format!(
            "Not talking: {verb} {}",
            crate::audio_settings::key_label(key)
        ),
        (None, Some(mouse), _) => format!(
            "Not talking: {verb} {}",
            crate::audio_settings::mouse_words(mouse)
        ),
        (None, None, true) => format!("Not talking: {verb} the gamepad button"),
        (None, None, false) => "Not talking: no key or button is set (Audio\u{2026})".to_owned(),
    };
    Some((line, false))
}

/// How the microphone is, as the dock draws it. The five looks differ in
/// glyph, and the ones that can be mistaken for each other differ in fill
/// too, so a live microphone is never only a tooltip away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicLook {
    /// Open microphone (voice activity, or push-to-talk is not in use).
    Plain,
    /// Muted or deafened: takes precedence over everything else.
    Muted,
    /// Push-to-talk or toggle-to-talk on a key, not transmitting.
    GatedKey,
    /// Push-to-talk or toggle-to-talk on a gamepad button, not transmitting.
    GatedPad,
    /// Push-to-talk or toggle-to-talk, transmitting.
    Talking,
}

impl MicLook {
    /// Every look.
    pub const ALL: [Self; 5] = [
        Self::Plain,
        Self::Muted,
        Self::GatedKey,
        Self::GatedPad,
        Self::Talking,
    ];

    /// The glyph: the open microphone only while sound can go out (or is
    /// left to your voice), a crossed one for muted, and the key or pad that
    /// opens it while push-to-talk gates the microphone.
    pub const fn icon(self) -> Icon {
        match self {
            Self::Plain | Self::Talking => Icon::Mic,
            Self::Muted => Icon::MicOff,
            Self::GatedKey => Icon::Keyboard,
            Self::GatedPad => Icon::Gamepad,
        }
    }

    /// The button style of the look: the open microphone's success outline,
    /// the toggle-on fill, the outlined fill, and the accent fill, so no two
    /// looks share one.
    fn button_style(self) -> fn(&Theme, Status) -> button::Style {
        match self {
            Self::Plain => open_icon_style,
            Self::Muted => active_icon_style,
            Self::GatedKey | Self::GatedPad => secondary_style,
            Self::Talking => primary_style,
        }
    }
}

/// The phase's severity: Joining in warning, Connected in success,
/// Reconnecting in danger.
pub const fn phase_severity(phase: CallPhase) -> Severity {
    match phase {
        CallPhase::Joining => Severity::Warning,
        CallPhase::Connected { .. } => Severity::Success,
        CallPhase::Reconnecting { .. } => Severity::Danger,
    }
}

/// The phase's glyph in the rail, where there are no words beside it: the
/// glyphs differ, so the colour is not the only sign.
pub const fn phase_icon(phase: CallPhase) -> Icon {
    match phase {
        CallPhase::Joining => Icon::More,
        CallPhase::Connected { .. } => Icon::Check,
        CallPhase::Reconnecting { .. } => Icon::Warning,
    }
}

/// The encryption marker of the call.
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
            Self::Unencrypted => Icon::LockOpen,
        }
    }

    /// The marker's words, for its tooltip.
    pub const fn words(self) -> &'static str {
        match self {
            Self::Encrypted => "Call is end-to-end encrypted",
            Self::Unencrypted => "Call is not encrypted",
        }
    }

    /// The words drawn beside the glyph in the expanded dock: none for the
    /// lock (the phase line says "encrypted"), a warning in words otherwise.
    pub const fn visible_words(self) -> Option<&'static str> {
        match self {
            Self::Encrypted => None,
            Self::Unencrypted => Some("Not encrypted"),
        }
    }
}

/// What the dock shows about the call, as data so that it can be tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockCall {
    /// The call's room, cleaned and capped.
    pub room_name: String,
    /// The phase's severity.
    pub severity: Severity,
    /// The phase's glyph in the rail.
    pub phase_icon: Icon,
    /// The call phase in words.
    pub phase_words: String,
    /// The encryption marker, once connected.
    pub lock: Option<DockLock>,
    /// The microphone's look.
    pub mic: MicLook,
    /// The mic button's tooltip: mute or unmute, and the talk line.
    pub mic_words: String,
    /// The push-to-talk pill (push-to-talk and toggle-to-talk only): its
    /// look and words. Muted or deafened replaces what push-to-talk says.
    pub talk: Option<(MicLook, String)>,
    /// Deafened.
    pub deafened: bool,
    /// The deafen button's tooltip.
    pub deafen_words: &'static str,
    /// Who declined the ring.
    pub declined: Option<String>,
}

/// The dock's call, if there is one.
pub fn dock_call(app: &App) -> Option<DockCall> {
    let current = app.calls.call.as_ref()?;
    let (mute, deafen) = mute_labels(current.muted, current.deafened);
    let room_name = app
        .rooms
        .iter()
        .find(|r| r.id == current.room_id)
        .map_or_else(|| "Call".to_owned(), |r| display_name(&r.name));
    let line = talk_line(app);
    let off = current.muted || current.deafened;
    let gated = match app.calls.settings.as_ref() {
        Some(s)
            if s.push_to_talk.key.is_none()
                && s.push_to_talk.mouse_button.is_none()
                && s.push_to_talk.gamepad_button.is_some() =>
        {
            MicLook::GatedPad
        }
        _ => MicLook::GatedKey,
    };
    let mic = if off {
        MicLook::Muted
    } else {
        match &line {
            Some((_, true)) => MicLook::Talking,
            Some((_, false)) => gated,
            None => MicLook::Plain,
        }
    };
    let talk = line.as_ref().map(|(words, talking)| {
        if off {
            let words = if current.deafened {
                "Deafened"
            } else {
                "Muted"
            };
            (MicLook::Muted, words.to_owned())
        } else if *talking {
            (MicLook::Talking, words.clone())
        } else {
            (gated, words.clone())
        }
    });
    let mic_words = match line {
        Some((words, _)) => format!("{mute}. {words}"),
        None => mute.to_owned(),
    };
    Some(DockCall {
        room_name,
        severity: phase_severity(current.phase),
        phase_icon: phase_icon(current.phase),
        phase_words: current.phase.label(),
        lock: match current.phase {
            CallPhase::Connected { encrypted: true } => Some(DockLock::Encrypted),
            CallPhase::Connected { encrypted: false } => Some(DockLock::Unencrypted),
            _ => None,
        },
        mic,
        mic_words,
        talk,
        deafened: current.deafened,
        deafen_words: deafen,
        declined: declined_line(current),
    })
}

/// Text in the colour of `severity`, nudged until it reads on the raised
/// surface the dock sits on.
fn severity_text(severity: Severity) -> impl Fn(&Theme) -> text::Style {
    move |t| text::Style {
        color: Some(theme::readable_on(
            severity.color(t),
            theme::semantic(t).surface_raised,
        )),
    }
}

/// A small coloured mark with its words in a tooltip (and beside it, when
/// `words` has them).
fn marker<'a>(
    glyph: &'static str,
    severity: Severity,
    visible: Option<&'static str>,
    tooltip: String,
) -> Element<'a, Message> {
    let mut content = Row::new()
        .push(text(glyph).size(tokens::CAPTION))
        .spacing(tokens::XS)
        .align_y(iced::Center);
    if let Some(visible) = visible {
        content = content.push(text(visible).size(tokens::CAPTION).wrapping(Wrapping::None));
    }
    tip(
        container(content)
            .padding([1.0, tokens::S])
            .style(severity_tag_style(severity)),
        tooltip,
    )
}

/// The pill of the push-to-talk line: the accent with the open microphone
/// while talking, an outlined muted fill with the key or pad while gated, the
/// quiet fill with the crossed microphone while muted.
fn pill_style(look: MicLook) -> impl Fn(&Theme) -> container::Style {
    move |t| {
        let sem = theme::semantic(t);
        let (background, text_color) = match look {
            MicLook::Talking => {
                let accent = primary_style(t, Status::Active);
                (accent.background, accent.text_color)
            }
            MicLook::GatedKey | MicLook::GatedPad => (
                Some(iced::Background::Color(sem.selection)),
                t.palette().text,
            ),
            MicLook::Muted | MicLook::Plain => {
                (Some(iced::Background::Color(sem.surface)), t.palette().text)
            }
        };
        container::Style {
            background,
            text_color: Some(text_color),
            border: iced::Border {
                color: sem.border,
                width: 1.0,
                ..border::rounded(tokens::ROUND)
            },
            ..container::Style::default()
        }
    }
}

/// [`pill_style`] for the pill as a button: the same look in every status,
/// with the outline in the focus colour while pointed at or pressed.
fn pill_button_style(look: MicLook) -> impl Fn(&Theme, Status) -> button::Style {
    move |t, status| {
        let base = pill_style(look)(t);
        let mut border = base.border;
        if matches!(status, Status::Hovered | Status::Pressed) {
            border.color = theme::semantic(t).focus;
            border.width = 2.0;
        }
        button::Style {
            background: base.background,
            text_color: base.text_color.unwrap_or(t.palette().text),
            border,
            ..button::Style::default()
        }
    }
}

/// The text style of the call's own tint: the severity colour on the dock.
fn phase_dot<'a>(severity: Severity) -> Element<'a, Message> {
    text(Icon::Dot.glyph())
        .size(tokens::CAPTION)
        .style(severity_text(severity))
        .into()
}

/// The mic button: the look's glyph and fill, always drawn.
fn mic_button<'a>(look: MicLook, words: String) -> Element<'a, Message> {
    tip(
        button(icon(look.icon()))
            .on_press(call(CallMessage::ToggleMute))
            .padding(tokens::S)
            .style(look.button_style()),
        words,
    )
}

/// The call dock: a card on the raised surface with the phase and the lock,
/// the room, who declined, the push-to-talk line and the controls; below it
/// the call's notice as a danger banner with its own close button. An empty
/// space without a call or a notice, so the slot keeps its place.
pub fn dock(app: &App) -> Element<'_, Message> {
    let calls = &app.calls;
    let density = app.density();
    let mut blocks: Vec<Element<'_, Message>> = Vec::new();
    if let Some(dock) = dock_call(app) {
        let mut lines: Vec<Element<'_, Message>> = Vec::new();
        let mut phase = Row::new()
            .push(phase_dot(dock.severity))
            .push(
                text(dock.phase_words.clone())
                    .size(tokens::CAPTION)
                    .width(Fill)
                    .wrapping(WRAP)
                    .style(severity_text(dock.severity)),
            )
            .spacing(tokens::XS)
            .align_y(iced::Center);
        if let Some(lock) = dock.lock {
            phase = phase.push(marker(
                lock.icon().glyph(),
                lock.severity(),
                lock.visible_words(),
                lock.words().to_owned(),
            ));
        }
        lines.push(phase.into());
        lines.push(tip(
            button(
                text(dock.room_name.clone())
                    .size(tokens::BODY)
                    .width(Fill)
                    .wrapping(WRAP),
            )
            .on_press(call(CallMessage::ShowCall))
            .padding([tokens::XS, tokens::S])
            .width(Fill)
            .style(ghost_style),
            "Show the call's room and people",
        ));
        if let Some(declined) = dock.declined.clone() {
            lines.push(
                row![
                    text(Icon::People.glyph()).size(tokens::CAPTION),
                    text(declined)
                        .size(tokens::CAPTION)
                        .width(Fill)
                        .wrapping(WRAP)
                ]
                .spacing(tokens::XS)
                .align_y(iced::Center)
                .into(),
            );
        }
        if let Some((look, words)) = dock.talk.clone() {
            // The pill is a button: it opens the Talking page of the audio
            // card, where the key is set. It is laid out as the container
            // it was.
            lines.push(
                button(
                    row![
                        text(look.icon().glyph()).size(tokens::CAPTION),
                        text(words).size(tokens::CAPTION).width(Fill).wrapping(WRAP)
                    ]
                    .spacing(tokens::XS)
                    .align_y(iced::Center),
                )
                .on_press(Message::Audio(crate::audio_settings::AudioMessage::OpenAt(
                    crate::audio_settings::AudioPage::Talking,
                )))
                .padding([tokens::XS, tokens::M])
                .width(Fill)
                .style(pill_button_style(look))
                .into(),
            );
        }
        let deafen = kit::toggle_icon_button_labelled(
            if dock.deafened {
                Icon::Deafened
            } else {
                Icon::Speaker
            },
            dock.deafened,
            dock.deafen_words,
            call(CallMessage::ToggleDeafen),
        );
        let audio = tip(
            button(icon(Icon::Settings))
                .on_press(Message::Audio(crate::audio_settings::AudioMessage::Open))
                .padding(tokens::S)
                .style(icon_style),
            "Audio settings",
        );
        // The gap between Audio and Leave never closes: Leave is not a stray
        // click from another control.
        lines.push(
            row![
                mic_button(dock.mic, dock.mic_words.clone()),
                deafen,
                crate::screens::stage::camera_button(app),
                crate::screens::stage::share_button(app),
            ]
            .spacing(tokens::S)
            .align_y(iced::Center)
            .into(),
        );
        lines.push(
            row![
                audio,
                space().width(Fill),
                kit::danger_button(
                    "Leave",
                    calls.pending.is_none().then_some(call(CallMessage::Leave))
                ),
            ]
            .spacing(tokens::S)
            .align_y(iced::Center)
            .into(),
        );
        blocks.push(
            container(Column::from_vec(lines).spacing(density.gap()).width(Fill))
                .padding(density.pad())
                .width(Fill)
                .style(card_style)
                .into(),
        );
    }
    if let Some(notice) = &calls.notice {
        blocks.push(kit::banner_with(
            Severity::Danger,
            notice.as_str(),
            vec![kit::icon_button(
                Icon::Close,
                call(CallMessage::DismissNotice),
            )],
        ));
    }
    if blocks.is_empty() {
        return space().height(0).into();
    }
    Column::from_vec(blocks)
        .spacing(density.gap())
        .width(Fill)
        .into()
}

/// The dock for the rail: a 72 px column of the phase mark (the call's room),
/// Mute, Deafen and Leave with their words in tooltips, and the state the
/// expanded dock writes out as visible marks: the encryption lock or its
/// warning, who declined, the microphone's look, and the notice with a close
/// button of its own. An empty space without a call or notice, like [`dock`].
pub fn rail_dock(app: &App) -> Element<'_, Message> {
    let calls = &app.calls;
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if let (Some(_), Some(dock)) = (&calls.call, dock_call(app)) {
        let severity = dock.severity;
        items.push(tip(
            button(text(dock.phase_icon.glyph()))
                .on_press(call(CallMessage::ShowCall))
                .padding(tokens::S)
                .style(move |t: &Theme, status| {
                    let mut style = icon_style(t, status);
                    if status != Status::Disabled {
                        style.text_color = theme::readable_on(
                            severity.color(t),
                            theme::semantic(t).surface_raised,
                        );
                    }
                    style
                }),
            format!("{}. {}", dock.room_name, dock.phase_words),
        ));
        if let Some(lock) = dock.lock {
            items.push(marker(
                lock.icon().glyph(),
                lock.severity(),
                None,
                lock.words().to_owned(),
            ));
        }
        if let Some(declined) = dock.declined.clone() {
            items.push(marker(
                Icon::People.glyph(),
                Severity::Warning,
                None,
                declined,
            ));
        }
        items.push(mic_button(dock.mic, dock.mic_words.clone()));
        items.push(tip(
            button(icon(if dock.deafened {
                Icon::Deafened
            } else {
                Icon::Speaker
            }))
            .on_press(call(CallMessage::ToggleDeafen))
            .padding(tokens::S)
            .style(if dock.deafened {
                active_icon_style
            } else {
                icon_style
            }),
            dock.deafen_words,
        ));
        items.push(crate::screens::stage::camera_button(app));
        items.push(crate::screens::stage::share_button(app));
        // A gap before Leave, so it is not a stray click from Deafen.
        items.push(space().height(tokens::M).into());
        items.push(tip(
            button(icon(Icon::Leave))
                .on_press_maybe(calls.pending.is_none().then_some(call(CallMessage::Leave)))
                .padding(tokens::S)
                .style(danger_style),
            "Leave the call",
        ));
    }
    if let Some(notice) = &calls.notice {
        // The mark only shows the words; closing is its own button.
        items.push(marker(
            Icon::Warning.glyph(),
            Severity::Danger,
            None,
            notice.clone(),
        ));
        items.push(kit::icon_button(
            Icon::Close,
            call(CallMessage::DismissNotice),
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
    .style(card_style)
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
        text(format!("In the call ({})", rows.len())).size(tokens::TITLE),
        layout.panel_fills,
    )];
    if let Some(trust) = layout
        .panel_fills
        .then(|| crate::screens::main_view::trust_strip(app))
        .flatten()
    {
        head.push(trust);
    }
    head.push(kit::icon_button(
        Icon::Close,
        call(CallMessage::TogglePanel),
    ));
    let header = Row::with_children(head)
        .spacing(tokens::S)
        .align_y(iced::Center);
    let list = Column::with_children(rows.into_iter().map(|r| participant(app, r)))
        .spacing(tokens::M)
        .width(Fill);
    let body = column![header, scrollable(list).height(Fill)]
        .spacing(tokens::S)
        .width(Fill)
        .height(Fill);
    row![
        kit::vertical_divider(),
        container(body)
            .padding(tokens::S)
            .width(Fill)
            .height(Fill)
            .style(kit::surface_style)
    ]
    .width(if layout.panel_fills {
        Length::Fill
    } else {
        Length::Fixed(PANEL_WIDTH)
    })
    .height(Fill)
    .into()
}

/// A button with a tooltip of its own words, for the volume steps (whose
/// icons are "Add" and "Remove" elsewhere).
fn step_button<'a>(
    glyph: Icon,
    words: &'static str,
    user: &twicecat_core::api::UserId,
    up: bool,
) -> Element<'a, Message> {
    tip(
        button(icon(glyph))
            .padding([tokens::XS, tokens::S])
            .style(icon_style)
            .on_press(call(CallMessage::VolumeStep {
                user_id: user.clone(),
                up,
            })),
        words,
    )
}

/// One person: the picture (ringed while they talk), the name, the marks, the
/// status in words, and for other people a **More** button that folds out the
/// volume and cleaning controls under the row.
fn participant(app: &App, row_view: ParticipantRow) -> Element<'_, Message> {
    let speaking = row_view.speaking;
    let picture = speaking_ring(
        avatar_sized(
            &row_view.user_id,
            &row_view.name,
            row_view.avatar.as_ref(),
            &app.avatars,
            kit::PERSON_SLOT,
        ),
        speaking,
    );
    let mut name = Row::new()
        .push(text(row_view.name.clone()).width(Fill).wrapping(WRAP))
        .spacing(tokens::XS)
        .align_y(iced::Center);
    if row_view.is_local {
        name = name.push(text("(you)").size(tokens::CAPTION).style(muted));
    }
    if row_view.muted {
        name = name.push(text(MUTED_MARK).size(tokens::CAPTION));
    }
    let mut details = Column::new().push(name).spacing(2).width(Fill);
    // The status in words (and, for speaking, a glyph and the ring's colour),
    // so the ring is never the only sign.
    if row_view.muted {
        details = details.push(text("Muted").size(tokens::CAPTION).style(muted));
    } else if speaking {
        details = details.push(
            container(
                row![
                    text(Icon::Speaker.glyph()).size(tokens::CAPTION),
                    text("Speaking").size(tokens::CAPTION)
                ]
                .spacing(tokens::XS)
                .align_y(iced::Center),
            )
            .style(|t: &Theme| {
                let sem = theme::semantic(t);
                container::Style {
                    text_color: Some(theme::readable_on(sem.speaking, sem.surface)),
                    ..container::Style::default()
                }
            }),
        );
    }
    if let Some(cleaning) = row_view.cleaning {
        details = details.push(
            text(cleaning)
                .size(tokens::CAPTION)
                .width(Fill)
                .wrapping(WRAP)
                .style(muted),
        );
    }
    let open = row_view.controls.is_some() && app.calls.row_open(&row_view.user_id);
    let mut top = Row::new()
        .push(picture)
        .push(details)
        .spacing(tokens::S)
        .align_y(iced::Alignment::Start);
    if row_view.controls.is_some() {
        top = top.push(kit::toggle_icon_button(
            Icon::More,
            open,
            Message::CallRowMore(row_view.user_id.clone()),
        ));
    }
    // The controls slot is always there, an empty space while folded, so the
    // row's tree (and a slider being dragged) keeps its place.
    let controls: Element<'_, Message> = match row_view.controls.filter(|_| open) {
        Some(controls) => {
            let slide_user = row_view.user_id.clone();
            // Ctrl + wheel is the zoom gesture, which iced's slider takes as
            // a step: the volume has the - and + buttons instead, and the
            // guard hides the wheel from the slider.
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
            let cleaning_user = row_view.user_id.clone();
            column![
                kit::field(
                    "Volume",
                    row![
                        step_button(Icon::Minus, "Quieter", &row_view.user_id, false),
                        volume,
                        step_button(Icon::Plus, "Louder", &row_view.user_id, true),
                        text(format!("{}\u{a0}%", controls.volume))
                            .size(tokens::CAPTION)
                            .wrapping(Wrapping::None)
                            .width(Length::Fixed(44.0)),
                    ]
                    .spacing(tokens::XS)
                    .align_y(iced::Center),
                ),
                kit::field(
                    "Voice cleaning",
                    kit::menu(
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
                        .text_size(tokens::CAPTION)
                        .padding([tokens::XS, tokens::S])
                        .width(Fill)
                        .style(kit::menu_style),
                    ),
                ),
            ]
            .spacing(tokens::S)
            .into()
        }
        None => space().height(0).into(),
    };
    Column::from_vec(vec![top.into(), controls])
        .spacing(tokens::XS)
        .width(Fill)
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

    fn dock_of(app: &App) -> DockCall {
        dock_call(app).expect("a call")
    }

    /// The fill and glyph of a look's button, on the default theme.
    fn look_signature(look: MicLook) -> (Icon, Option<iced::Background>, iced::Color, u32) {
        let theme = crate::theme::built_in();
        let style = (look.button_style())(&theme, Status::Active);
        (
            look.icon(),
            style.background,
            style.border.color,
            style.border.width.to_bits(),
        )
    }

    #[test]
    fn an_open_microphone_is_outlined_and_not_filled_like_muted() {
        for theme in crate::theme::test_themes() {
            let open = (MicLook::Plain.button_style())(&theme, Status::Active);
            let muted = (MicLook::Muted.button_style())(&theme, Status::Active);
            assert!(open.border.width >= 1.0, "an outline");
            assert_ne!(open.border.color, muted.border.color);
            assert_ne!(open.background, muted.background);
            // Not the quiet ghost: it must carry weight of its own.
            let ghost = ghost_style(&theme, Status::Active);
            assert_ne!(open.border.width, ghost.border.width);
        }
    }

    #[test]
    fn push_to_talk_transmitting_and_gated_differ_by_glyph_and_fill() {
        for toggle in [false, true] {
            let talking = dock_of(&app_with_call(CONNECTED, Some(ptt(toggle)), true));
            let gated = dock_of(&app_with_call(CONNECTED, Some(ptt(toggle)), false));
            assert_eq!(talking.mic, MicLook::Talking);
            assert_eq!(gated.mic, MicLook::GatedKey);
            let (t_icon, t_fill, ..) = look_signature(talking.mic);
            let (g_icon, g_fill, ..) = look_signature(gated.mic);
            assert_ne!(t_icon, g_icon, "glyph");
            assert_ne!(t_fill, g_fill, "fill");
            // The pill says the same in words and in its own glyph.
            let (pill_look, pill_words) = talking.talk.clone().expect("a push-to-talk pill");
            assert_eq!(
                (pill_look, pill_words.as_str()),
                (MicLook::Talking, "Talking")
            );
            let (pill_look, pill_words) = gated.talk.clone().expect("a push-to-talk pill");
            assert_eq!(pill_look, MicLook::GatedKey);
            assert!(pill_words.starts_with("Not talking"), "{pill_words}");
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
    }

    #[test]
    fn no_two_microphone_looks_share_a_glyph_and_a_fill() {
        let all: Vec<_> = MicLook::ALL.iter().map(|l| look_signature(*l)).collect();
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "two looks are drawn the same");
            }
        }
        // Muted is never drawn with a push-to-talk glyph, nor the other way
        // round: the two states are opposites for the user.
        for gated in [MicLook::GatedKey, MicLook::GatedPad] {
            assert_ne!(MicLook::Muted.icon(), gated.icon());
            assert_ne!(gated.icon(), Icon::Mic);
        }
        assert_eq!(MicLook::Plain.icon(), Icon::Mic);
    }

    #[test]
    fn an_open_microphone_in_voice_activity_is_the_open_glyph() {
        let open = dock_of(&app_with_call(CONNECTED, None, false));
        assert_eq!(open.mic, MicLook::Plain);
        assert_eq!(open.mic.icon(), Icon::Mic);
        assert_eq!(open.talk, None, "no pill outside push-to-talk");
        let voice = dock_of(&app_with_call(
            CONNECTED,
            Some(InputMode::VoiceActivity {
                automatic: true,
                threshold_db: -40.0,
            }),
            false,
        ));
        assert_eq!(voice.mic, MicLook::Plain);
    }

    #[test]
    fn muted_and_deafened_take_precedence_over_push_to_talk() {
        for (muted, deafened, words) in [(true, false, "Muted"), (false, true, "Deafened")] {
            for talking in [false, true] {
                let mut app = app_with_call(CONNECTED, Some(ptt(false)), talking);
                let call = app.calls.call.as_mut().expect("call");
                call.muted = muted;
                call.deafened = deafened;
                let dock = dock_of(&app);
                assert_eq!(dock.mic, MicLook::Muted, "talking={talking}");
                assert_eq!(dock.mic.icon(), Icon::MicOff);
                assert_eq!(dock.talk, Some((MicLook::Muted, words.to_owned())));
            }
        }
    }

    #[test]
    fn a_gamepad_only_binding_draws_the_gamepad() {
        use twicecat_core::api::{AudioSettings, PushToTalkBindings};
        let mut app = app_with_call(CONNECTED, Some(ptt(false)), false);
        app.calls.settings = Some(AudioSettings {
            input_mode: ptt(false),
            push_to_talk: PushToTalkBindings {
                key: None,
                mouse_button: None,
                gamepad_button: Some("South".to_owned()),
            },
            ..AudioSettings::default()
        });
        assert_eq!(dock_of(&app).mic, MicLook::GatedPad);
    }

    #[test]
    fn the_dock_marks_encryption_and_the_lack_of_it_in_words_or_glyphs() {
        let lock = |phase| dock_of(&app_with_call(phase, None, false)).lock;
        assert_eq!(lock(CONNECTED), Some(DockLock::Encrypted));
        assert_eq!(
            lock(CallPhase::Connected { encrypted: false }),
            Some(DockLock::Unencrypted)
        );
        assert_eq!(lock(CallPhase::Joining), None);
        assert_ne!(DockLock::Encrypted.icon(), DockLock::Unencrypted.icon());
        // Not the notice mark's glyph either: the lock is open, not a warning.
        assert_ne!(DockLock::Unencrypted.icon(), Icon::Warning);
        assert_ne!(DockLock::Unencrypted.icon(), Severity::Warning.icon());
        assert_eq!(DockLock::Unencrypted.icon(), Icon::LockOpen);
        assert_ne!(
            DockLock::Encrypted.severity(),
            DockLock::Unencrypted.severity()
        );
        assert_ne!(DockLock::Encrypted.words(), DockLock::Unencrypted.words());
        // The warning is in words on the dock, not only in a tooltip.
        assert_eq!(DockLock::Unencrypted.visible_words(), Some("Not encrypted"));
    }

    #[test]
    fn the_phases_are_warning_success_and_danger_with_their_own_glyphs() {
        let phases = [
            (CallPhase::Joining, Severity::Warning),
            (CONNECTED, Severity::Success),
            (CallPhase::Reconnecting { attempt: 1 }, Severity::Danger),
        ];
        for (phase, severity) in phases {
            assert_eq!(phase_severity(phase), severity);
            assert!(!phase.label().is_empty());
            let dock = dock_of(&app_with_call(phase, None, false));
            assert_eq!(dock.severity, severity);
            assert_eq!(dock.phase_words, phase.label());
        }
        let icons: Vec<Icon> = phases.iter().map(|(p, _)| phase_icon(*p)).collect();
        assert_ne!(icons[0], icons[1]);
        assert_ne!(icons[1], icons[2]);
        assert_ne!(icons[0], icons[2]);
    }

    #[test]
    fn the_dock_shows_who_declined_and_the_notice_in_both_forms() {
        let mut app = app_with_call(CONNECTED, None, false);
        assert_eq!(dock_of(&app).declined, None);
        app.calls.call.as_mut().expect("call").declined = vec![UserId::from("@u0:x")];
        assert_eq!(dock_of(&app).declined.as_deref(), Some("@u0:x declined"));
        app.calls.notice = Some("The call ended".to_owned());
        let _: Element<'_, Message> = dock(&app);
        let _: Element<'_, Message> = rail_dock(&app);
        app.calls.call = None;
        let _: Element<'_, Message> = dock(&app);
        let _: Element<'_, Message> = rail_dock(&app);
        assert!(dock_call(&app).is_none());
        app.calls.notice = None;
        let _: Element<'_, Message> = dock(&app);
    }
}
