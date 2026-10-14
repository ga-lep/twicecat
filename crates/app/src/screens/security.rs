//! The security banner and the security screens: Confirm your identity, the
//! recovery key (shown once), the restore field, the reset and change-key
//! warnings and the password prompt (restyled in UI/UX rework Task 16).
//!
//! The screens are a layer of a fixed `stack` over the main view, and the
//! banner sits in a fixed slot at its top, so opening or closing them keeps
//! the widget state under them (timeline scroll, editor focus).
//!
//! Every screen is a [`kit::dialog`]: a header (with a close button that only
//! ever backs out), a body that scrolls in a short window, and a footer with
//! the way back at the left and the main action at the right. A question
//! that cannot be undone (reset the identity, change the recovery key) puts
//! its red button at the far end of the footer, apart from Cancel and from
//! the button that asked, which is somewhere else. The recovery key is hidden
//! until **Show key** and is drawn in the monospace face in groups of four.
//! Esc backs out of a screen or is swallowed; it never confirms (see
//! [`crate::security`]).

use iced::widget::{
    Column, checkbox, column, container, responsive, row, space, stack, text_input,
};
use iced::{Element, Fill};
use twicecat_core::api::{Password, RecoveryKey, RecoveryProgress};

use crate::security::{
    Banner, Busy, Overlay, REAUTH_INPUT_ID, RESTORE_INPUT_ID, RecoverySetup, SecurityMessage,
    SecurityModel, Visible,
};
use crate::state::{App, Message};
use crate::ui::fonts;
use crate::ui::kit::{
    self, Severity, banner, banner_with, danger_button, dialog, dialog_header, ghost_button, modal,
    muted, primary_button, secondary_button, surface_style, words,
};
use crate::ui::tokens;

/// Width of the card of a security screen.
pub const CARD_WIDTH: f32 = 520.0;

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

/// What the key screen says about the clipboard.
pub const CLIPBOARD_NOTE: &str =
    "Copy puts the key on the clipboard; it is cleared after 60 seconds.";

/// What the hidden key shows in its place (the real key is never drawn).
pub const HIDDEN_KEY: &str = "****  ****  ****  ****";

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

/// How serious a banner is. Every banner here needs care, so none is
/// reduced to a neutral note: the words, the icon and the colour say so.
pub fn banner_severity(banner: Banner) -> Severity {
    match banner {
        Banner::DeviceUnverified | Banner::RecoveryDisabled | Banner::RecoveryIncomplete => {
            Severity::Warning
        }
    }
}

/// The label of the banner's one main action.
pub fn banner_action(banner: Banner) -> &'static str {
    match banner {
        Banner::DeviceUnverified => "Verify",
        Banner::RecoveryDisabled => "Set up recovery",
        Banner::RecoveryIncomplete => "Enter recovery key",
    }
}

/// The banner slot at the top of the main view: always a column, empty (and
/// without padding) when there is nothing to say.
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
    let padding = if bars.is_empty() { 0.0 } else { tokens::S };
    Column::from_vec(bars)
        .spacing(tokens::S)
        .padding(padding)
        .width(Fill)
        .into()
}

/// One banner: the words, then (below them, so no word runs under a button)
/// the way to dismiss it where that is allowed, and the one main action last.
fn banner_bar(banner: Banner) -> Element<'static, Message> {
    let primary = match banner {
        Banner::DeviceUnverified => SecurityMessage::OpenConfirmIdentity,
        Banner::RecoveryDisabled => SecurityMessage::SetupRecovery,
        Banner::RecoveryIncomplete => SecurityMessage::OpenRestore,
    };
    let mut actions: Vec<Element<'static, Message>> = Vec::new();
    if banner.dismissible() {
        actions.push(
            ghost_button("Dismiss", Some(msg(SecurityMessage::DismissRecoveryBanner))).into(),
        );
    }
    actions.push(primary_button(banner_action(banner), Some(msg(primary))).into());
    banner_with(banner_severity(banner), banner.text(), actions)
}

/// A failure or outcome, as a Danger banner with a way to dismiss it.
fn notice_bar(notice: &str) -> Element<'_, Message> {
    banner_with(
        Severity::Danger,
        notice,
        vec![ghost_button("Dismiss", Some(msg(SecurityMessage::DismissNotice))).into()],
    )
}

/// The security screen on top, when there is one.
pub fn screen(model: &SecurityModel) -> Option<Element<'_, Message>> {
    if model.visible() == Visible::Nothing {
        return None;
    }
    Some(responsive(move |size| content(model, size.height)).into())
}

/// The card of the screen that is on top, for a window `height` high.
fn content(model: &SecurityModel, height: f32) -> Element<'_, Message> {
    let (header, body, footer) = match model.visible() {
        Visible::Nothing => return space().into(),
        Visible::Key => key_screen(model),
        Visible::Reauth => reauth_screen(model),
        Visible::Overlay(Overlay::ConfirmIdentity) => identity_screen(model),
        Visible::Overlay(Overlay::Restore) => restore_screen(model),
        Visible::Overlay(Overlay::ResetConfirm) => reset_screen(model),
        Visible::Overlay(Overlay::ChangeKeyConfirm) => change_key_screen(model),
    };
    dialog(header, body, Some(footer), CARD_WIDTH, height)
}

/// The parts of a screen: header, body, footer.
type Parts<'a> = (
    Element<'a, Message>,
    Element<'a, Message>,
    Element<'a, Message>,
);

/// The title with a close button that sends `close`, or the title alone where
/// there is nothing to back out to (the key, a running job).
fn header<'a>(title: &'a str, close: Option<SecurityMessage>) -> Element<'a, Message> {
    match close {
        Some(close) => dialog_header(title, msg(close)),
        None => words(title).size(tokens::HEADLINE).width(Fill).into(),
    }
}

/// A footer of two slots at the two ends of the card: the way back at the
/// left, the main action at the right, and free room between them. A slot
/// with nothing is an empty space, so the other keeps its place.
pub(crate) fn split_footer<'a>(
    left: Option<Element<'a, Message>>,
    right: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        left.unwrap_or_else(|| space().into()),
        space().width(Fill),
        right.unwrap_or_else(|| space().into()),
    ]
    .spacing(tokens::M)
    .align_y(iced::Center)
    .into()
}

/// The last failure, as a Danger banner in a slot that is always there.
fn notice_slot(model: &SecurityModel) -> Element<'_, Message> {
    let mut slot = Column::new();
    if let Some(notice) = &model.notice {
        slot = slot.push(banner(Severity::Danger, notice.as_str()));
    }
    slot.into()
}

/// A line of progress or waiting, in a slot that is always there.
fn status_slot<'a>(line: Option<&'a str>) -> Element<'a, Message> {
    let mut slot = Column::new();
    if let Some(line) = line {
        slot = slot.push(words(line).style(muted));
    }
    slot.into()
}

/// A full-width choice: its words wrap (never clipped) and are centred.
fn choice<'a>(
    label: &'a str,
    press: Option<Message>,
    style: fn(&iced::Theme, iced::widget::button::Status) -> iced::widget::button::Style,
) -> Element<'a, Message> {
    iced::widget::button(words(label).width(Fill).align_x(iced::Center))
        .padding([tokens::S, tokens::M])
        .width(Fill)
        .on_press_maybe(press)
        .style(style)
        .into()
}

fn identity_screen(model: &SecurityModel) -> Parts<'_> {
    let restoring = model.busy == Some(Busy::Restoring);
    let mut choices = column![].spacing(tokens::M);
    if model.status.has_devices_to_verify_against {
        let label = if model.asked_other_device {
            "Request sent: accept it on your other device"
        } else {
            "Use another device"
        };
        let press =
            (!model.asked_other_device && !restoring).then(|| msg(SecurityMessage::UseOtherDevice));
        choices = choices.push(choice(label, press, kit::primary_style));
    }
    choices = choices.push(choice(
        "Use recovery key",
        (!restoring).then(|| msg(SecurityMessage::OpenRestore)),
        kit::secondary_style,
    ));
    let body = column![
        words("Confirm this device to read your encrypted messages and to let others trust it."),
        notice_slot(model),
        choices,
        status_slot(restoring.then_some("Restoring\u{2026}")),
        kit::divider(),
        // The question that leads to the reset: quiet, at the left of the
        // body. Its red button is on the next screen, at the right of the
        // footer.
        words("Lost your other devices and your recovery key?")
            .size(tokens::CAPTION)
            .style(muted)
            .width(Fill),
        row![
            ghost_button(
                "Can't confirm? Reset your identity",
                (!restoring).then(|| msg(SecurityMessage::ResetRequested)),
            ),
            space().width(Fill),
        ],
    ]
    .spacing(tokens::L);
    let skip = secondary_button(
        "Skip for now",
        (!restoring).then(|| msg(SecurityMessage::SkipIdentity)),
    );
    (
        header(
            "Confirm your identity",
            (!restoring).then_some(SecurityMessage::SkipIdentity),
        ),
        body.into(),
        split_footer(None, Some(skip.into())),
    )
}

fn restore_screen(model: &SecurityModel) -> Parts<'_> {
    let busy = model.busy == Some(Busy::Restoring);
    let submit = (!busy).then_some(msg(SecurityMessage::RestoreSubmitted));
    let body = column![
        words("Type or paste your recovery key, or your recovery passphrase."),
        notice_slot(model),
        text_input("Recovery key or passphrase", model.restore_input.expose())
            .id(RESTORE_INPUT_ID)
            .secure(true)
            .on_input_maybe(
                (!busy).then_some(|s: String| msg(SecurityMessage::RestoreInput(
                    RecoveryKey::from(s)
                )))
            )
            .on_submit_maybe(submit.clone())
            .padding(tokens::S),
        status_slot(busy.then_some("Restoring\u{2026}")),
    ]
    .spacing(tokens::M);
    (
        header(
            "Enter your recovery key",
            (!busy).then_some(SecurityMessage::Back),
        ),
        body.into(),
        split_footer(
            Some(secondary_button("Back", (!busy).then(|| msg(SecurityMessage::Back))).into()),
            Some(primary_button("Restore", submit).into()),
        ),
    )
}

fn reset_screen(model: &SecurityModel) -> Parts<'_> {
    let resetting = model.busy == Some(Busy::Resetting);
    let mut body =
        column![banner(Severity::Danger, RESET_WARNING), notice_slot(model)].spacing(tokens::M);
    if resetting {
        body = body.push(words(RESET_WAIT));
        if model.waiting_approval {
            body = body.push(banner(
                Severity::Warning,
                "Approve the reset in your browser, then come back here.",
            ));
        }
    }
    // Cancel at the left, the red button alone at the far right; while the
    // reset runs only the way out of an approval is offered.
    let (left, right): (Option<Element<'_, Message>>, Option<Element<'_, Message>>) = if resetting {
        (
            model.waiting_approval.then(|| {
                secondary_button("Cancel", Some(msg(SecurityMessage::CancelApproval))).into()
            }),
            None,
        )
    } else {
        (
            Some(secondary_button("Back", Some(msg(SecurityMessage::Back))).into()),
            Some(
                danger_button("Reset identity", Some(msg(SecurityMessage::ResetConfirmed))).into(),
            ),
        )
    };
    (
        header(
            "Reset your identity?",
            (!resetting).then_some(SecurityMessage::Back),
        ),
        body.into(),
        split_footer(left, right),
    )
}

fn change_key_screen(model: &SecurityModel) -> Parts<'_> {
    let changing = model.busy == Some(Busy::ChangingKey);
    let body = column![
        banner(Severity::Danger, CHANGE_KEY_WARNING),
        status_slot(changing.then_some(CHANGE_KEY_WAIT)),
    ]
    .spacing(tokens::M);
    let (left, right): (Option<Element<'_, Message>>, Option<Element<'_, Message>>) = if changing {
        (None, None)
    } else {
        (
            Some(secondary_button("Cancel", Some(msg(SecurityMessage::ChangeKeyCancelled))).into()),
            Some(
                danger_button("Change key", Some(msg(SecurityMessage::ChangeKeyConfirmed))).into(),
            ),
        )
    };
    (
        header(
            "Change your recovery key?",
            (!changing).then_some(SecurityMessage::ChangeKeyCancelled),
        ),
        body.into(),
        split_footer(left, right),
    )
}

fn reauth_screen(model: &SecurityModel) -> Parts<'_> {
    let Some(prompt) = &model.reauth else {
        return (space().into(), space().into(), space().into());
    };
    let submit = (!prompt.waiting && !prompt.password.expose().is_empty())
        .then_some(msg(SecurityMessage::ReauthSubmitted));
    let body = column![
        words("Your homeserver needs your account password to go on."),
        notice_slot(model),
        text_input("Password", prompt.password.expose())
            .id(REAUTH_INPUT_ID)
            .secure(true)
            .on_input_maybe(
                (!prompt.waiting)
                    .then_some(|s: String| msg(SecurityMessage::ReauthInput(Password::from(s)))),
            )
            .on_submit_maybe(submit.clone())
            .padding(tokens::S),
        status_slot(prompt.waiting.then_some(RESET_WAIT)),
    ]
    .spacing(tokens::M);
    (
        header(
            "Enter your password",
            (!prompt.waiting).then_some(SecurityMessage::ReauthCancelled),
        ),
        body.into(),
        split_footer(
            Some(
                secondary_button(
                    "Cancel",
                    (!prompt.waiting).then(|| msg(SecurityMessage::ReauthCancelled)),
                )
                .into(),
            ),
            Some(primary_button("Continue", submit).into()),
        ),
    )
}

/// The key as lines of groups of four in the monospace face on the surface
/// colour, or the hidden stand-in. Always a column of three lines, so showing
/// the key moves nothing.
fn key_block(setup: &RecoverySetup) -> Element<'_, Message> {
    let mut lines = Column::new().spacing(tokens::S);
    if setup.revealed {
        // The lines are borrowed from the setup: no copy of the key is made
        // per frame. A key of other lengths still shows whole; the usual one
        // is 3 lines.
        for line in setup.lines() {
            lines = lines.push(words(line.expose()).font(fonts::MONO).size(tokens::TITLE));
        }
    } else {
        for _ in 0..3 {
            lines = lines.push(
                words(HIDDEN_KEY)
                    .font(fonts::MONO)
                    .size(tokens::TITLE)
                    .style(muted),
            );
        }
    }
    container(lines)
        .padding(tokens::L)
        .width(Fill)
        .style(surface_style_boxed)
        .into()
}

/// The surface colour with a border, for a block of text to copy from.
fn surface_style_boxed(theme: &iced::Theme) -> container::Style {
    container::Style {
        border: iced::Border {
            color: crate::theme::semantic(theme).border,
            width: 1.0,
            radius: tokens::MEDIUM.into(),
        },
        ..surface_style(theme)
    }
}

fn key_screen(model: &SecurityModel) -> Parts<'_> {
    let Some(setup) = &model.setup else {
        return (space().into(), space().into(), space().into());
    };
    let done = crate::security::can_finish_setup(setup).then_some(msg(SecurityMessage::Done));
    // Ticking is for a key that has been seen, so the box is off until it
    // was shown (Copy alone does not count: the key must be read once).
    let toggle = setup
        .revealed
        .then_some(|ticked| msg(SecurityMessage::SavedToggled(ticked)));
    let body = column![
        banner(Severity::Warning, KEY_WARNING),
        key_block(setup),
        row![
            secondary_button(
                if setup.revealed {
                    "Hide key"
                } else {
                    "Show key"
                },
                Some(msg(SecurityMessage::RevealToggled(!setup.revealed))),
            ),
            secondary_button("Copy key", Some(msg(SecurityMessage::CopyKey))),
        ]
        .spacing(tokens::M),
        words(CLIPBOARD_NOTE).size(tokens::CAPTION).style(muted),
        checkbox(setup.saved_ticked)
            .label("I've saved my recovery key")
            .text_wrapping(kit::BANNER_WRAPPING)
            .on_toggle_maybe(toggle),
    ]
    .spacing(tokens::M);
    (
        header(crate::security::key_title(setup), None),
        body.into(),
        // Done alone at the far right, a whole card away from the box above
        // it.
        split_footer(None, Some(primary_button("Done", done).into())),
    )
}

/// The text over the whole window while the core sets recovery up: used by
/// the banner's busy state. Kept here so the wording lives in one file.
pub fn setting_up_overlay(model: &SecurityModel) -> Option<Element<'_, Message>> {
    (model.busy == Some(Busy::SettingUp) && model.visible() == Visible::Nothing).then(|| {
        modal(
            column![
                words("Setting up recovery").size(tokens::HEADLINE),
                words(progress_text(model.progress)),
            ]
            .spacing(tokens::M)
            .width(Fill),
            CARD_WIDTH,
        )
    })
}

/// The screen or busy card on top of `app`, if any.
pub fn top_layer(app: &App) -> Option<Element<'_, Message>> {
    screen(&app.security).or_else(|| setting_up_overlay(&app.security))
}
