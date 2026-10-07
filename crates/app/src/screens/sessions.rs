//! The Sessions screen, its password prompt and the logout warning.
//!
//! The screen is a full-window layer over the main view (see
//! [`crate::screens::main_view::view`]); the prompt and the warning are cards
//! over it. Views only read the app state: every change goes through
//! `App::update`.

use chrono::Local;
use iced::widget::{
    Column, button, column, container, opaque, row, scrollable, space, stack, text, text_input,
};
use iced::{Element, Fill, Theme};
use tessera_core::api::{DeviceSummary, DeviceTrust, Password};

use crate::screens::security::{card, secondary, wide};
use crate::security::SecurityModel;
use crate::sessions::{
    Approval, LOGOUT_WARNING, REAUTH_INPUT_ID, RENAME_INPUT_ID, SessionsMessage, SessionsModel,
    SignOutAsk, can_verify, device_id_line, device_title, last_active_text, recovery_line,
    trust_label,
};
use crate::state::{App, Message};
use crate::theme;

/// Width of the Sessions page.
const PAGE_WIDTH: f32 = 720.0;

/// What the sign-out prompt says.
pub const REAUTH_TEXT: &str =
    "Your homeserver needs your account password to sign out these sessions.";

/// What the prompt says after a rejected password.
pub const WRONG_PASSWORD: &str = "Wrong password";

fn msg(message: SessionsMessage) -> Message {
    Message::Sessions(message)
}

fn muted(label: String) -> Element<'static, Message> {
    text(label)
        .size(13)
        .style(|t: &Theme| text::Style {
            color: Some(theme::semantic(t).muted),
        })
        .into()
}

/// The layer for the Sessions screen, the password prompt and the logout
/// warning, when any is open.
pub fn layer(app: &App) -> Option<Element<'_, Message>> {
    let model = &app.sessions;
    let mut layers: Vec<Element<'_, Message>> = Vec::new();
    if model.open {
        layers.push(page(model, &app.security));
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

fn page<'a>(model: &'a SessionsModel, security: &SecurityModel) -> Element<'a, Message> {
    let now = u64::try_from(Local::now().timestamp_millis()).unwrap_or(0);
    let mut body = Column::new().spacing(12);
    body = body.push(
        row![
            text("Sessions").size(24),
            space().width(Fill),
            button(text("Refresh"))
                .on_press_maybe((!model.loading).then_some(msg(SessionsMessage::Refresh)))
                .style(button::secondary),
            button(text("Close"))
                .on_press(msg(SessionsMessage::Close))
                .style(button::secondary),
        ]
        .spacing(8)
        .align_y(iced::Center),
    );
    if let Some(notice) = &model.notice {
        body = body.push(
            row![
                text(notice.as_str()).style(text::danger),
                space().width(Fill),
                button(text("\u{2715}"))
                    .on_press(msg(SessionsMessage::DismissNotice))
                    .style(button::secondary),
            ]
            .spacing(8)
            .align_y(iced::Center),
        );
    }
    if let Some(approval) = &model.approval {
        body = body.push(approval_bar(approval));
    }
    if let Some(line) = recovery_bar(security) {
        body = body.push(line);
    }
    let Some(devices) = &model.devices else {
        let label = if model.loading {
            "Loading\u{2026}"
        } else {
            "The sessions could not be listed."
        };
        return page_card(body.push(text(label)).into());
    };
    let others = devices.iter().filter(|d| !d.current).count();
    let mut rows = Column::new().spacing(10);
    for device in devices {
        rows = rows.push(device_row(model, device, now));
    }
    body = body.push(scrollable(rows).height(Fill));
    if others > 0 {
        body = body.push(sign_out_all(model));
    }
    page_card(body.into())
}

/// The Recovery line (plan Task 15); its button is off while the core is
/// busy with a security action.
fn recovery_bar(security: &SecurityModel) -> Option<Element<'static, Message>> {
    let (label, action) = recovery_line(security.status.recovery)?;
    let mut bar = row![text(label), space().width(Fill)]
        .spacing(8)
        .align_y(iced::Center);
    if let Some(action) = action {
        let press = security
            .busy
            .is_none()
            .then(|| Message::Security(action.message()));
        bar = bar.push(
            button(text(action.label()))
                .on_press_maybe(press)
                .style(button::secondary),
        );
    }
    Some(
        container(bar)
            .padding(10)
            .width(Fill)
            .style(container::bordered_box)
            .into(),
    )
}

fn page_card(content: Element<'_, Message>) -> Element<'_, Message> {
    opaque(
        container(
            container(content)
                .padding(24)
                .max_width(PAGE_WIDTH)
                .height(Fill)
                .style(container::bordered_box),
        )
        .padding(16)
        .center_x(Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(theme.extended_palette().background.base.color.into()),
            ..container::Style::default()
        }),
    )
}

fn approval_bar(approval: &Approval) -> Element<'_, Message> {
    container(
        row![
            text(format!(
                "Open {} to approve signing out? Sign in there, then press Refresh.",
                approval.host
            )),
            space().width(Fill),
            button(text("Open")).on_press(msg(SessionsMessage::ApprovalConfirmed)),
            button(text("Cancel"))
                .on_press(msg(SessionsMessage::ApprovalDismissed))
                .style(button::secondary),
        ]
        .spacing(8)
        .align_y(iced::Center),
    )
    .padding(8)
    .into()
}

fn sign_out_all(model: &SessionsModel) -> Element<'_, Message> {
    if model.confirm == Some(SignOutAsk::AllOthers) {
        return confirm_row("Sign out of all other sessions?");
    }
    let press = model
        .pending
        .is_none()
        .then_some(msg(SessionsMessage::SignOutAllAsk));
    button(text("Sign out of all other sessions"))
        .on_press_maybe(press)
        .style(button::danger)
        .into()
}

fn confirm_row(question: &'static str) -> Element<'static, Message> {
    row![
        text(question),
        space().width(Fill),
        button(text("Sign out"))
            .on_press(msg(SessionsMessage::SignOutConfirmed))
            .style(button::danger),
        button(text("Cancel"))
            .on_press(msg(SessionsMessage::SignOutCancelled))
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .into()
}

fn trust_color(trust: DeviceTrust) -> impl Fn(&Theme) -> text::Style {
    move |t: &Theme| text::Style {
        color: Some(match trust {
            DeviceTrust::Verified => t.palette().success,
            DeviceTrust::Unverified | DeviceTrust::NoKeys => theme::semantic(t).muted,
        }),
    }
}

fn device_row<'a>(
    model: &'a SessionsModel,
    device: &'a DeviceSummary,
    now_ms: u64,
) -> Element<'a, Message> {
    let id = &device.device_id;
    let mut head = row![text(device_title(device)).size(17)]
        .spacing(8)
        .align_y(iced::Center);
    if device.current {
        head = head.push(text("This device").size(13).style(text::primary));
    }
    head = head.push(space().width(Fill)).push(
        text(trust_label(device.trust))
            .size(13)
            .style(trust_color(device.trust)),
    );
    let mut body = column![
        head,
        muted(device_id_line(id)),
        muted(last_active_text(device.last_seen_ms, now_ms, &Local)),
    ]
    .spacing(4);
    if let Some(edit) = model.rename.as_ref().filter(|e| e.device_id == *id) {
        body = body.push(
            row![
                text_input("Session name", &edit.input)
                    .id(RENAME_INPUT_ID)
                    .on_input(|s| msg(SessionsMessage::RenameInput(s)))
                    .on_submit(msg(SessionsMessage::RenameSubmitted))
                    .padding(6),
                button(text("Save")).on_press(msg(SessionsMessage::RenameSubmitted)),
                button(text("Cancel"))
                    .on_press(msg(SessionsMessage::RenameCancelled))
                    .style(button::secondary),
            ]
            .spacing(8)
            .align_y(iced::Center),
        );
    } else if model.confirm == Some(SignOutAsk::One(id.clone())) {
        body = body.push(confirm_row("Sign out of this session?"));
    } else {
        let mut actions = row![].spacing(8);
        if can_verify(device) {
            actions = actions.push(
                button(text("Verify").size(14)).on_press(msg(SessionsMessage::Verify(id.clone()))),
            );
        }
        actions = actions.push(
            button(text("Rename").size(14))
                .on_press(msg(SessionsMessage::RenameStart(id.clone())))
                .style(button::secondary),
        );
        if !device.current {
            actions = actions.push(
                button(text("Sign out").size(14))
                    .on_press_maybe(
                        model
                            .pending
                            .is_none()
                            .then(|| msg(SessionsMessage::SignOutAsk(id.clone()))),
                    )
                    .style(button::danger),
            );
        }
        body = body.push(actions);
    }
    container(body)
        .padding(10)
        .width(Fill)
        .style(container::bordered_box)
        .into()
}

fn reauth_card(model: &SessionsModel) -> Element<'_, Message> {
    let Some(prompt) = &model.reauth else {
        return space().into();
    };
    let submit = (!prompt.waiting && !prompt.password.expose().is_empty())
        .then_some(msg(SessionsMessage::ReauthSubmitted));
    let mut body = column![text("Enter your password").size(24), text(REAUTH_TEXT)].spacing(12);
    if prompt.failed {
        body = body.push(text(WRONG_PASSWORD).style(text::danger));
    }
    body = body
        .push(
            text_input("Password", prompt.password.expose())
                .id(REAUTH_INPUT_ID)
                .secure(true)
                .on_input_maybe(
                    (!prompt.waiting).then_some(|s: String| {
                        msg(SessionsMessage::ReauthInput(Password::from(s)))
                    }),
                )
                .on_submit_maybe(submit.clone())
                .padding(8),
        )
        .push(wide("Continue", submit));
    if prompt.waiting {
        body = body.push(text("Signing out\u{2026}"));
    } else {
        body = body.push(secondary(
            "Cancel",
            Some(msg(SessionsMessage::ReauthCancelled)),
        ));
    }
    card(body.into())
}

fn logout_card<'a>() -> Element<'a, Message> {
    card(
        column![
            text("Sign out?").size(24),
            text(LOGOUT_WARNING),
            wide(
                "Set up recovery",
                Some(msg(SessionsMessage::LogoutSetupRecovery))
            ),
            button(text("Sign out anyway").center().width(Fill))
                .on_press(msg(SessionsMessage::LogoutAnyway))
                .style(button::danger)
                .width(Fill)
                .padding(8),
            secondary("Cancel", Some(msg(SessionsMessage::LogoutCancelled))),
        ]
        .spacing(12)
        .into(),
    )
}
