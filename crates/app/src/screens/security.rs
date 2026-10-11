//! The security banner and the full-window security screens: Confirm your
//! identity, the recovery key (shown once), the restore field, the reset
//! warning and the password prompt.
//!
//! The screens are a layer of a fixed `stack` over the main view, and the
//! banner sits in a fixed slot at its top, so opening or closing them keeps
//! the widget state under them (timeline scroll, editor focus).

use iced::widget::{
    Column, button, checkbox, column, container, opaque, row, space, stack, text, text_input,
};
use iced::{Element, Fill, Font, Theme};
use twicecat_core::api::{Password, RecoveryKey, RecoveryProgress};

use crate::security::{
    Banner, Busy, Overlay, REAUTH_INPUT_ID, RESTORE_INPUT_ID, SecurityMessage, SecurityModel,
    Visible,
};
use crate::state::{App, Message};

/// Width of the card of a security screen.
const CARD_WIDTH: f32 = 520.0;

/// The key screen's warning (plan Q5).
pub const KEY_WARNING: &str = "Store this key somewhere safe, like a password manager. Without it you can't read your encrypted messages on a new device if you're signed out everywhere.";

/// What the reset warning says.
pub const RESET_WARNING: &str = "Resetting your identity makes your other devices and your contacts see you as a new user, and deletes your key backup: older encrypted messages may become unreadable. Only reset if you can't use another device or your recovery key.";

/// What the warning before changing the recovery key says (plan Task 15).
pub const CHANGE_KEY_WARNING: &str = "Your current recovery key will stop working. Twicecat will show you a new one, once. Other apps and devices that ask for your recovery key will need the new one. Anything another app saved in your secure storage that this device doesn't have will be lost.";

/// Said while the recovery key is being changed.
pub const CHANGE_KEY_WAIT: &str = "Changing your recovery key\u{2026}";

/// Said while a reset runs.
pub const RESET_WAIT: &str = "Resetting\u{2026} Syncing is paused and no messages arrive until this ends. With a browser approval it can take up to two minutes.";

/// A message of the security screens.
fn msg(message: SecurityMessage) -> Message {
    Message::Security(message)
}

/// The progress line of setting recovery up.
pub fn progress_text(progress: Option<RecoveryProgress>) -> String {
    match progress {
        None | Some(RecoveryProgress::Starting) => "Setting up recovery\u{2026}".to_owned(),
        Some(RecoveryProgress::CreatingBackup) => "Creating your key backup\u{2026}".to_owned(),
        Some(RecoveryProgress::CreatingKey) => "Creating your recovery key\u{2026}".to_owned(),
        Some(RecoveryProgress::BackingUp { done, total }) => {
            format!("Backing up your keys ({done} of {total})\u{2026}")
        }
        Some(RecoveryProgress::UploadFailed) => {
            "Some keys could not be uploaded yet; retrying\u{2026}".to_owned()
        }
    }
}

/// The base view with the security screen as the layer above it. Always a
/// stack with the base first, so a screen opening keeps the base's state.
pub fn layer<'a>(
    base: Element<'a, Message>,
    screen: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    stack([base, screen.unwrap_or_else(|| space().into())]).into()
}

/// The banner slot at the top of the main view: always a column, empty when
/// there is nothing to say.
pub fn banner_slot(model: &SecurityModel) -> Element<'_, Message> {
    let mut bars: Vec<Element<'_, Message>> = Vec::new();
    if let Some(banner) = model.banner() {
        bars.push(banner_bar(banner));
    }
    if model.visible() == Visible::Nothing
        && let Some(notice) = &model.notice
    {
        bars.push(notice_bar(notice));
    }
    Column::from_vec(bars).width(Fill).into()
}

fn banner_bar(banner: Banner) -> Element<'static, Message> {
    let mut bar = row![text(banner.text()), space().width(Fill)]
        .spacing(8)
        .align_y(iced::Center);
    bar = match banner {
        Banner::DeviceUnverified => {
            bar.push(button(text("Verify")).on_press(msg(SecurityMessage::OpenConfirmIdentity)))
        }
        Banner::RecoveryDisabled => {
            bar.push(button(text("Set up recovery")).on_press(msg(SecurityMessage::SetupRecovery)))
        }
        Banner::RecoveryIncomplete => {
            bar.push(button(text("Enter recovery key")).on_press(msg(SecurityMessage::OpenRestore)))
        }
    };
    if banner.dismissible() {
        bar = bar.push(
            button(text("\u{2715}"))
                .on_press(msg(SecurityMessage::DismissRecoveryBanner))
                .style(button::secondary),
        );
    }
    container(bar)
        .padding(8)
        .width(Fill)
        .style(|theme: &Theme| {
            let pair = theme.extended_palette().warning.weak;
            container::Style {
                background: Some(pair.color.into()),
                text_color: Some(pair.text),
                ..container::Style::default()
            }
        })
        .into()
}

fn notice_bar(notice: &str) -> Element<'_, Message> {
    container(
        row![
            text(notice).style(text::danger),
            space().width(Fill),
            button(text("\u{2715}"))
                .on_press(msg(SecurityMessage::DismissNotice))
                .style(button::secondary),
        ]
        .spacing(8)
        .align_y(iced::Center),
    )
    .padding(8)
    .width(Fill)
    .into()
}

/// The security screen on top, when there is one.
pub fn screen(model: &SecurityModel) -> Option<Element<'_, Message>> {
    let content = match model.visible() {
        Visible::Nothing => return None,
        Visible::Key => key_screen(model),
        Visible::Reauth => reauth_screen(model),
        Visible::Overlay(Overlay::ConfirmIdentity) => identity_screen(model),
        Visible::Overlay(Overlay::Restore) => restore_screen(model),
        Visible::Overlay(Overlay::ResetConfirm) => reset_screen(model),
        Visible::Overlay(Overlay::ChangeKeyConfirm) => change_key_screen(model),
    };
    Some(card(content))
}

/// An opaque full-window layer with `content` in a centred card.
pub(crate) fn card(content: Element<'_, Message>) -> Element<'_, Message> {
    opaque(
        container(
            container(content)
                .padding(24)
                .max_width(CARD_WIDTH)
                .style(container::bordered_box),
        )
        .center_x(Fill)
        .center_y(Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(theme.extended_palette().background.base.color.into()),
            ..container::Style::default()
        }),
    )
}

fn notice_line(model: &SecurityModel) -> Element<'_, Message> {
    match &model.notice {
        Some(notice) => text(notice.as_str()).style(text::danger).into(),
        None => space().into(),
    }
}

pub(crate) fn wide(label: &str, press: Option<Message>) -> Element<'_, Message> {
    button(text(label).center().width(Fill))
        .on_press_maybe(press)
        .width(Fill)
        .padding(8)
        .into()
}

pub(crate) fn secondary<'a>(label: &'a str, press: Option<Message>) -> Element<'a, Message> {
    button(text(label).center().width(Fill))
        .on_press_maybe(press)
        .style(button::secondary)
        .width(Fill)
        .padding(8)
        .into()
}

fn identity_screen(model: &SecurityModel) -> Element<'_, Message> {
    let mut body = column![
        text("Confirm your identity").size(24),
        text("Confirm this device to read your encrypted messages and to let others trust it."),
        notice_line(model),
    ]
    .spacing(12);
    if model.busy == Some(Busy::Restoring) {
        return body.push(text("Restoring\u{2026}")).into();
    }
    if model.status.has_devices_to_verify_against {
        let label = if model.asked_other_device {
            "Request sent: accept it on your other device"
        } else {
            "Use another device"
        };
        let press = (!model.asked_other_device).then(|| msg(SecurityMessage::UseOtherDevice));
        body = body.push(wide(label, press));
    }
    body.push(wide(
        "Use recovery key",
        Some(msg(SecurityMessage::OpenRestore)),
    ))
    .push(secondary(
        "Can't confirm? Reset your identity",
        Some(msg(SecurityMessage::ResetRequested)),
    ))
    .push(secondary(
        "Skip for now",
        Some(msg(SecurityMessage::SkipIdentity)),
    ))
    .into()
}

fn restore_screen(model: &SecurityModel) -> Element<'_, Message> {
    let busy = model.busy == Some(Busy::Restoring);
    let submit = (!busy).then_some(msg(SecurityMessage::RestoreSubmitted));
    let mut body = column![
        text("Enter your recovery key").size(24),
        text("Type or paste your recovery key, or your recovery passphrase."),
        notice_line(model),
        text_input("Recovery key or passphrase", model.restore_input.expose())
            .id(RESTORE_INPUT_ID)
            .secure(true)
            .on_input_maybe(
                (!busy).then_some(|s: String| msg(SecurityMessage::RestoreInput(
                    RecoveryKey::from(s)
                )))
            )
            .on_submit_maybe(submit.clone())
            .padding(8),
        wide("Restore", submit),
    ]
    .spacing(12);
    if busy {
        body = body.push(text("Restoring\u{2026}"));
    } else {
        body = body.push(secondary("Back", Some(msg(SecurityMessage::Back))));
    }
    body.into()
}

fn reset_screen(model: &SecurityModel) -> Element<'_, Message> {
    let mut body = column![
        text("Reset your identity?").size(24),
        text(RESET_WARNING),
        notice_line(model),
    ]
    .spacing(12);
    if model.busy == Some(Busy::Resetting) {
        body = body.push(text(RESET_WAIT));
        if model.waiting_approval {
            body = body
                .push(text(
                    "Approve the reset in your browser, then come back here.",
                ))
                .push(secondary(
                    "Cancel",
                    Some(msg(SecurityMessage::CancelApproval)),
                ));
        }
        return body.into();
    }
    body.push(
        button(text("Reset identity").center().width(Fill))
            .on_press(msg(SecurityMessage::ResetConfirmed))
            .style(button::danger)
            .width(Fill)
            .padding(8),
    )
    .push(secondary("Back", Some(msg(SecurityMessage::Back))))
    .into()
}

fn change_key_screen(model: &SecurityModel) -> Element<'_, Message> {
    let body = column![
        text("Change your recovery key?").size(24),
        text(CHANGE_KEY_WARNING),
    ]
    .spacing(12);
    if model.busy == Some(Busy::ChangingKey) {
        return body.push(text(CHANGE_KEY_WAIT)).into();
    }
    body.push(wide(
        "Change key",
        Some(msg(SecurityMessage::ChangeKeyConfirmed)),
    ))
    .push(secondary(
        "Cancel",
        Some(msg(SecurityMessage::ChangeKeyCancelled)),
    ))
    .into()
}

fn reauth_screen(model: &SecurityModel) -> Element<'_, Message> {
    let Some(prompt) = &model.reauth else {
        return space().into();
    };
    let submit = (!prompt.waiting && !prompt.password.expose().is_empty())
        .then_some(msg(SecurityMessage::ReauthSubmitted));
    let mut body = column![
        text("Enter your password").size(24),
        text("Your homeserver needs your account password to go on."),
        notice_line(model),
        text_input("Password", prompt.password.expose())
            .id(REAUTH_INPUT_ID)
            .secure(true)
            .on_input_maybe(
                (!prompt.waiting)
                    .then_some(|s: String| msg(SecurityMessage::ReauthInput(Password::from(s))))
            )
            .on_submit_maybe(submit.clone())
            .padding(8),
        wide("Continue", submit),
    ]
    .spacing(12);
    if prompt.waiting {
        body = body.push(text(RESET_WAIT));
    } else {
        body = body.push(secondary(
            "Cancel",
            Some(msg(SecurityMessage::ReauthCancelled)),
        ));
    }
    body.into()
}

fn key_screen(model: &SecurityModel) -> Element<'_, Message> {
    let Some(setup) = &model.setup else {
        return space().into();
    };
    let done = crate::security::can_finish_setup(setup).then_some(msg(SecurityMessage::Done));
    column![
        text(crate::security::key_title(setup)).size(24),
        container(text(setup.key.expose()).font(Font::MONOSPACE).size(20))
            .padding(12)
            .width(Fill)
            .style(container::bordered_box),
        button(text("Copy"))
            .on_press(msg(SecurityMessage::CopyKey))
            .style(button::secondary),
        text(KEY_WARNING),
        checkbox(setup.saved_ticked)
            .label("I've saved my recovery key")
            .on_toggle(|ticked| msg(SecurityMessage::SavedToggled(ticked))),
        wide("Done", done),
    ]
    .spacing(12)
    .into()
}

/// The text over the whole window while the core sets recovery up: used by
/// the banner's busy state. Kept here so the wording lives in one file.
pub fn setting_up_overlay(model: &SecurityModel) -> Option<Element<'_, Message>> {
    (model.busy == Some(Busy::SettingUp) && model.visible() == Visible::Nothing).then(|| {
        card(
            column![
                text("Setting up recovery").size(24),
                text(progress_text(model.progress)),
            ]
            .spacing(12)
            .into(),
        )
    })
}

/// The screen or busy card on top of `app`, if any.
pub fn top_layer(app: &App) -> Option<Element<'_, Message>> {
    screen(&app.security).or_else(|| setting_up_overlay(&app.security))
}
