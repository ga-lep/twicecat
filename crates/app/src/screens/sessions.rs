//! The Sessions screen, its password prompt and the logout warning (plan Q13,
//! Task 13; restyled in UI/UX rework Task 14).
//!
//! The screen is a modal above the main view (see
//! [`crate::screens::main_view::view`]); the prompt and the warning are
//! modals over it. Views only read the app state: every change goes through
//! `App::update`.
//!
//! Session names and IDs come from the server and are untrusted: they pass
//! through [`crate::sessions`] (cleaned, capped) and are drawn with a break
//! inside a word when it has none, so no end of an ID is cut off. Every
//! sign-out keeps its confirmation: the button that asks is a different
//! button, in a different place, from the one that confirms, and the
//! destructive buttons sit apart from the others.

use chrono::Local;
use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, column, container, row, scrollable, space, stack, text, text_input,
};
use iced::{Element, Fill, Length};
use twicecat_core::api::{DeviceSummary, DeviceTrust, Password, RecoveryStatus};

use crate::security::SecurityModel;
use crate::sessions::{
    Approval, LOGOUT_WARNING, REAUTH_INPUT_ID, RENAME_INPUT_ID, SessionsMessage, SessionsModel,
    SignOutAsk, can_verify, device_id_line, last_active_text, recovery_line, row_title,
    trust_label,
};
use crate::state::{App, Message};
use crate::ui::fonts;
use crate::ui::kit::{
    self, Severity, banner, banner_with, danger_button, empty_state, ghost_button, modal, muted,
    primary_button, secondary_button, severity_tag,
};
use crate::ui::tokens;

/// Width of the Sessions card.
pub const CARD_WIDTH: f32 = 760.0;
/// Height of the Sessions card, with its padding.
const CARD_HEIGHT: f32 = 620.0;
/// The least height of the card's content, in a small window.
const MIN_CARD_HEIGHT: f32 = 240.0;
/// The padding `kit::modal` puts around the card's content.
const CARD_PADDING: f32 = 20.0;
/// Width of the password prompt and the logout warning.
const PROMPT_WIDTH: f32 = 520.0;
/// How text wraps here: a break inside a word when it has none, so the end of
/// a long name or ID is never cut off.
const WRAP: Wrapping = Wrapping::WordOrGlyph;

/// What the sign-out prompt says.
pub const REAUTH_TEXT: &str =
    "Your homeserver needs your account password to sign out these sessions.";

/// What the prompt says after a rejected password.
pub const WRONG_PASSWORD: &str = "Wrong password";

fn msg(message: SessionsMessage) -> Message {
    Message::Sessions(message)
}

/// Text that wraps on this screen's rules.
fn words<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(label).wrapping(WRAP)
}

fn caption<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a> {
    words(label).size(tokens::CAPTION).style(muted)
}

/// The layer for the Sessions screen, the password prompt and the logout
/// warning, when any is open.
pub fn layer(app: &App) -> Option<Element<'_, Message>> {
    let model = &app.sessions;
    let mut layers: Vec<Element<'_, Message>> = Vec::new();
    if model.open {
        layers.push(iced::widget::responsive(move |size| page(app, size.height)).into());
    }
    if model.open && model.reauth.is_some() {
        layers.push(reauth_card(model));
    }
    if model.logout_warning {
        layers.push(logout_card());
    }
    match layers.len() {
        0 => None,
        1 => layers.pop(),
        _ => Some(stack(layers).into()),
    }
}

/// The card, 620 high or as high as the window leaves room for.
fn page(app: &App, window_height: f32) -> Element<'_, Message> {
    let model = &app.sessions;
    let now = u64::try_from(Local::now().timestamp_millis()).unwrap_or(0);
    let header = row![
        words("Sessions").size(tokens::HEADLINE).width(Fill),
        secondary_button(
            "Refresh",
            (!model.loading).then_some(msg(SessionsMessage::Refresh)),
        ),
        secondary_button("Close", Some(msg(SessionsMessage::Close))),
    ]
    .spacing(tokens::M)
    .align_y(iced::Center);
    // The bars under the header: a column that is always there and holds
    // what there is to say (the last failure, an approval, the recovery).
    let mut bars = Column::new().spacing(tokens::S);
    if let Some(notice) = &model.notice {
        bars = bars.push(banner_with(
            Severity::Danger,
            notice.as_str(),
            vec![ghost_button("Dismiss", Some(msg(SessionsMessage::DismissNotice))).into()],
        ));
    }
    if let Some(approval) = &model.approval {
        bars = bars.push(approval_bar(approval));
    }
    if let Some(line) = recovery_bar(&app.security) {
        bars = bars.push(line);
    }
    let list: Element<'_, Message> = match &model.devices {
        None => {
            if model.loading {
                empty_state("Loading\u{2026}", "Asking the server for your sessions.")
            } else {
                empty_state(
                    "The sessions could not be listed.",
                    "Press Refresh to try again.",
                )
            }
        }
        Some(devices) => {
            let mut rows = Column::new().spacing(tokens::M);
            for device in devices {
                rows = rows.push(device_row(model, device, now));
            }
            // The scrollbar takes room of its own, so a button at the
            // right edge of a row is never under it.
            scrollable(rows)
                .spacing(tokens::S)
                .width(Fill)
                .height(Fill)
                .into()
        }
    };
    // Under the list, apart from the header's buttons: signing out of
    // everything else. A column that is always there.
    let mut footer = Column::new().spacing(tokens::S);
    if model.other_devices().is_some_and(|n| n > 0) {
        footer = footer.push(sign_out_all(model));
    }
    let content = column![header, bars, list, footer].spacing(tokens::L);
    let height = (window_height - 2.0 * CARD_PADDING - 2.0 * tokens::L)
        .clamp(MIN_CARD_HEIGHT, CARD_HEIGHT - 2.0 * CARD_PADDING);
    modal(container(content).height(Length::Fixed(height)), CARD_WIDTH)
}

/// The Recovery line (plan Task 15): its words and colour say where recovery
/// stands, and its button, on a row of its own, is off while the core is busy
/// with a security action.
fn recovery_bar(security: &SecurityModel) -> Option<Element<'static, Message>> {
    let (label, action) = recovery_line(security.status.recovery)?;
    let severity = match security.status.recovery {
        RecoveryStatus::Enabled => Severity::Success,
        _ => Severity::Warning,
    };
    let actions: Vec<Element<'static, Message>> = action
        .map(|action| {
            let press = security
                .busy
                .is_none()
                .then(|| Message::Security(action.message()));
            secondary_button(action.label(), press).into()
        })
        .into_iter()
        .collect();
    Some(banner_with(severity, label, actions))
}

fn approval_bar(approval: &Approval) -> Element<'_, Message> {
    let text = format!(
        "Open {} to approve signing out? Sign in there, then press Refresh.",
        approval.host
    );
    banner_with(
        Severity::Warning,
        text,
        vec![
            ghost_button("Cancel", Some(msg(SessionsMessage::ApprovalDismissed))).into(),
            primary_button("Open", Some(msg(SessionsMessage::ApprovalConfirmed))).into(),
        ],
    )
}

fn sign_out_all(model: &SessionsModel) -> Element<'_, Message> {
    if model.confirm == Some(SignOutAsk::AllOthers) {
        return confirm(
            "Sign out of all other sessions? Each one will have to sign in again.",
            "Sign out of all",
        );
    }
    let press = model
        .pending
        .is_none()
        .then_some(msg(SessionsMessage::SignOutAllAsk));
    row![danger_button("Sign out of all other sessions", press)].into()
}

/// The question as a warning, then Cancel at the left and the sign-out far
/// from it at the right, on a row below the words: the button that asked was
/// somewhere else, so a second click on it cannot confirm.
fn confirm(question: &'static str, label: &'static str) -> Element<'static, Message> {
    column![
        banner(Severity::Warning, question),
        row![
            secondary_button("Cancel", Some(msg(SessionsMessage::SignOutCancelled))),
            space().width(Fill),
            danger_button(label, Some(msg(SessionsMessage::SignOutConfirmed))),
        ]
        .spacing(tokens::M)
        .align_y(iced::Center),
    ]
    .spacing(tokens::S)
    .into()
}

/// The trust of a session, in words and in the colour of its severity.
pub fn trust_severity(trust: DeviceTrust) -> Severity {
    match trust {
        DeviceTrust::Verified => Severity::Success,
        DeviceTrust::Unverified | DeviceTrust::NoKeys => Severity::Warning,
    }
}

fn device_row<'a>(
    model: &'a SessionsModel,
    device: &'a DeviceSummary,
    now_ms: u64,
) -> Element<'a, Message> {
    let id = &device.device_id;
    let mut tags = Row::new().spacing(tokens::S);
    if device.current {
        tags = tags.push(severity_tag(Severity::Info, "This device"));
    }
    tags = tags.push(severity_tag(
        trust_severity(device.trust),
        trust_label(device.trust),
    ));
    let mut body = column![
        words(row_title(
            device,
            model.devices.as_deref().unwrap_or_default()
        ))
        .size(tokens::BODY_LARGE)
        .width(Fill),
        tags,
        caption(device_id_line(id)).font(fonts::MONO).width(Fill),
        caption(last_active_text(device.last_seen_ms, now_ms, &Local)).width(Fill),
    ]
    .spacing(tokens::XS);
    if let Some(edit) = model.rename.as_ref().filter(|e| e.device_id == *id) {
        body = body.push(
            row![
                text_input("Session name", &edit.input)
                    .id(RENAME_INPUT_ID)
                    .on_input(|s| msg(SessionsMessage::RenameInput(s)))
                    .on_submit(msg(SessionsMessage::RenameSubmitted))
                    .padding(tokens::S),
                primary_button("Save", Some(msg(SessionsMessage::RenameSubmitted))),
                ghost_button("Cancel", Some(msg(SessionsMessage::RenameCancelled))),
            ]
            .spacing(tokens::S)
            .align_y(iced::Center),
        );
    } else if model.confirm == Some(SignOutAsk::One(id.clone())) {
        body = body.push(confirm("Sign out of this session?", "Sign out"));
    } else {
        let mut actions = row![].spacing(tokens::M).align_y(iced::Center);
        if can_verify(device) {
            actions = actions.push(ghost_button(
                "Verify",
                Some(msg(SessionsMessage::Verify(id.clone()))),
            ));
        }
        actions = actions.push(ghost_button(
            "Rename",
            Some(msg(SessionsMessage::RenameStart(id.clone()))),
        ));
        if !device.current {
            // At the far end, apart from the others by at least the row's gap.
            actions = actions.push(space().width(Fill)).push(danger_button(
                "Sign out",
                model
                    .pending
                    .is_none()
                    .then(|| msg(SessionsMessage::SignOutAsk(id.clone()))),
            ));
        }
        body = body.push(column![actions].padding(iced::Padding::ZERO.top(tokens::S)));
    }
    container(body)
        .padding(tokens::M)
        .width(Fill)
        .style(kit::card_style)
        .into()
}

fn reauth_card(model: &SessionsModel) -> Element<'_, Message> {
    let Some(prompt) = &model.reauth else {
        return space().into();
    };
    let submit = (!prompt.waiting && !prompt.password.expose().is_empty())
        .then_some(msg(SessionsMessage::ReauthSubmitted));
    // The wrong-password line has a slot of its own that is always there.
    let mut failed = Column::new();
    if prompt.failed {
        failed = failed.push(banner(Severity::Danger, WRONG_PASSWORD));
    }
    let status: Element<'_, Message> = if prompt.waiting {
        words("Signing out\u{2026}").into()
    } else {
        space().into()
    };
    let body = column![
        words("Enter your password").size(tokens::HEADLINE),
        banner(Severity::Warning, REAUTH_TEXT),
        failed,
        text_input("Password", prompt.password.expose())
            .id(REAUTH_INPUT_ID)
            .secure(true)
            .on_input_maybe(
                (!prompt.waiting)
                    .then_some(|s: String| msg(SessionsMessage::ReauthInput(Password::from(s)))),
            )
            .on_submit_maybe(submit.clone())
            .padding(tokens::S),
        row![
            secondary_button(
                "Cancel",
                (!prompt.waiting).then_some(msg(SessionsMessage::ReauthCancelled)),
            ),
            space().width(Fill),
            primary_button("Continue", submit),
        ]
        .spacing(tokens::M)
        .align_y(iced::Center),
        status,
    ]
    .spacing(tokens::M);
    modal(body, PROMPT_WIDTH)
}

fn logout_card<'a>() -> Element<'a, Message> {
    let body = column![
        words("Sign out?").size(tokens::HEADLINE),
        banner(Severity::Warning, LOGOUT_WARNING),
        // The way out of the danger first and alone; the sign-out is at the
        // far end of the row below, away from it.
        primary_button(
            "Set up recovery",
            Some(msg(SessionsMessage::LogoutSetupRecovery))
        ),
        row![
            secondary_button("Cancel", Some(msg(SessionsMessage::LogoutCancelled))),
            space().width(Fill),
            danger_button("Sign out anyway", Some(msg(SessionsMessage::LogoutAnyway))),
        ]
        .spacing(tokens::M)
        .align_y(iced::Center),
    ]
    .spacing(tokens::L);
    modal(body, PROMPT_WIDTH)
}
