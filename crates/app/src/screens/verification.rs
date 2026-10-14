//! The verification dialog: a modal in the fixed layer above the main view
//! and the security screens, with one card per phase of the flow (restyled in
//! UI/UX rework Task 16).
//!
//! The card has a three-step indicator (Request, Compare, Done) that says its
//! state in words as well as colour, the person (or session) being verified
//! in a block of its own, and a body for the phase. The comparison is the
//! point of the whole flow, so it is unmistakable: seven large emoji with
//! their names under each (or three large numbers), **They match** at the
//! right in the main colour and **They don't match** at the left in red, a
//! whole card apart. Buttons that agree wait a moment after the card comes up
//! (see [`crate::verification::ARMING`]); the ones that refuse never do. Esc
//! only ever cancels (`App::on_escape`). The peer's name and ID are other
//! people's text: cleaned and capped by the model, and drawn with a break
//! inside a word when it has none, so the end of an ID is never cut off.

use iced::widget::image::{self, FilterMethod};
use iced::widget::{Row, column, container, responsive, row, space, stack, text};
use iced::{Center, Element, Fill, Length};
use twicecat_core::api::CancelReason;

use crate::screens::security::split_footer;
use crate::state::{App, Message};
use crate::ui::fonts;
use crate::ui::icons::Icon;
use crate::ui::kit::{
    self, Severity, banner, danger_button, dialog, dialog_header, ghost_button, icon_tag,
    outline_tag, primary_button, secondary_button, severity_tag, surface_style, tag, words,
};
use crate::ui::tokens;
use crate::verification::{Phase, VerificationMessage, VerificationModel};

/// Side of the drawn QR code, in logical pixels (the image is larger and is
/// scaled with the nearest-neighbour filter).
pub const QR_SIDE: f32 = 288.0;

/// Width of the verification card: seven emoji across with room for a name
/// under each.
pub const WIDTH: f32 = 600.0;

/// The three steps of the indicator.
pub const STEPS: [&str; 3] = ["Request", "Compare", "Done"];

/// The same with their numbers, as the indicator writes them.
const NUMBERED: [&str; 3] = ["1 Request", "2 Compare", "3 Done"];

/// What the comparison card says under the emoji.
pub const COMPARE_HINT: &str = "Check that these appear in the same order on the other side. If anything differs, choose They don't match: someone may be sitting between you.";

/// What the comparison card says under the numbers.
pub const COMPARE_NUMBERS_HINT: &str = "Check that these numbers are the same on the other side. If anything differs, choose They don't match: someone may be sitting between you.";

/// Said under the buttons while the agreeing one waits.
pub const WAITING_HINT: &str = "Take a moment to look\u{2026}";

/// Said under the buttons once the agreeing one works.
pub const READY_HINT: &str = "Esc cancels the verification.";

fn msg(message: VerificationMessage) -> Message {
    Message::Verification(message)
}

/// The base with the dialog as the layer above it: always a stack of two, so
/// the dialog opening keeps the widget state under it.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let dialog = app
        .verification
        .as_ref()
        .map(|m| responsive(move |size| card(m, size.height)).into());
    stack([base, dialog.unwrap_or_else(|| space().into())]).into()
}

/// The headline of the card.
pub fn title(model: &VerificationModel) -> String {
    let name = &model.name;
    match &model.phase {
        Phase::Requested { incoming: true } if model.own_account => {
            format!("Your other device {name} wants to verify")
        }
        Phase::Requested { incoming: true } => format!("{name} wants to verify"),
        Phase::Requested { incoming: false } => format!("Waiting for {name}\u{2026}"),
        Phase::Ready | Phase::Waiting | Phase::Confirmed => "Verifying\u{2026}".to_owned(),
        Phase::ShowQr if model.own_account => "Scan this code with your other device".to_owned(),
        Phase::ShowQr => format!("Ask {name} to scan this code"),
        Phase::QrScanned if model.own_account => {
            "Does your other device show that the code was scanned successfully?".to_owned()
        }
        Phase::QrScanned => format!("Does {name} see a confirmation?"),
        Phase::Emojis(_) => "Compare the emoji".to_owned(),
        Phase::Decimals(_) => "Compare the numbers".to_owned(),
        Phase::Done => "Verified".to_owned(),
        Phase::Cancelled(_) => "Verification ended".to_owned(),
    }
}

/// The one line that says why a flow ended.
pub fn cancel_text(reason: CancelReason) -> &'static str {
    match reason {
        CancelReason::ByUs => "You cancelled the verification.",
        CancelReason::ByThem => "The other side cancelled the verification.",
        CancelReason::Mismatch => "The codes did not match, so nothing was verified.",
        CancelReason::Timeout => "The verification timed out.",
        CancelReason::AcceptedElsewhere => "Another device accepted the request.",
        CancelReason::Other => "The verification failed.",
    }
}

/// How serious the end of a flow is: a mismatch may be an attack (Danger),
/// a failure or a cancel by the other side needs a look (Warning), and your
/// own cancel is just news (Info). The words say the same.
pub fn cancel_severity(reason: CancelReason) -> Severity {
    match reason {
        CancelReason::Mismatch => Severity::Danger,
        CancelReason::ByThem | CancelReason::Timeout | CancelReason::Other => Severity::Warning,
        CancelReason::ByUs | CancelReason::AcceptedElsewhere => Severity::Info,
    }
}

/// Where the flow is on the three-step indicator: `Some(0)` Request,
/// `Some(1)` Compare, `Some(2)` Done; `None` when it ended without success.
pub fn step(phase: &Phase) -> Option<usize> {
    match phase {
        Phase::Requested { .. } | Phase::Ready | Phase::Waiting => Some(0),
        Phase::ShowQr
        | Phase::QrScanned
        | Phase::Emojis(_)
        | Phase::Decimals(_)
        | Phase::Confirmed => Some(1),
        Phase::Done => Some(2),
        Phase::Cancelled(_) => None,
    }
}

/// The caption under the indicator, which says it in words.
pub fn step_caption(phase: &Phase) -> String {
    match (step(phase), phase) {
        (_, Phase::Done) => "All 3 steps done".to_owned(),
        (Some(n), _) => format!("Step {} of 3: {}", n + 1, STEPS[n]),
        (None, _) => "Stopped".to_owned(),
    }
}

/// The indicator: a tag per step, joined by arrows. Done steps carry a check,
/// the current one is highlighted, the ones to come are outlines; the caption
/// below says the same in words. Always three tags and two arrows.
fn steps(phase: &Phase) -> Element<'static, Message> {
    let at = step(phase);
    let mut line = Row::new().spacing(tokens::S).align_y(Center);
    for (n, &name) in NUMBERED.iter().enumerate() {
        if n > 0 {
            line = line.push(
                text(Icon::ChevronRight.glyph())
                    .size(tokens::CAPTION)
                    .style(kit::muted),
            );
        }
        // The current step is the main colour, never the call colours: red
        // in a security flow means danger.
        let chip: Element<'static, Message> = match at {
            Some(current) if n < current || (n == 2 && current == 2) => {
                icon_tag(Icon::Check, name.to_owned(), false)
            }
            Some(current) if n == current => severity_tag(Severity::Info, name),
            _ => outline_tag(name),
        };
        line = line.push(chip);
    }
    column![
        line,
        words(step_caption(phase))
            .size(tokens::CAPTION)
            .style(kit::muted)
    ]
    .spacing(tokens::XS)
    .into()
}

/// Who is being verified, prominently: the name, then the whole user ID in the
/// monospace face where the person is another account (the server name at its
/// end is what tells two people apart), or which of your sessions it is.
fn peer_block(model: &VerificationModel) -> Element<'_, Message> {
    let who: Element<'_, Message> = if model.own_account {
        tag("Your other session")
    } else {
        tag("Another person")
    };
    let mut lines = column![
        row![who].spacing(tokens::S),
        words(model.name.as_str())
            .size(tokens::TITLE)
            // Another account's name is its ID: the monospace face keeps
            // look-alike letters apart.
            .font(if model.own_account {
                fonts::BODY_SEMIBOLD
            } else {
                fonts::MONO
            })
            .width(Fill),
    ]
    .spacing(tokens::XS);
    // The name of another account is its ID (cut in the middle only when it is
    // very long): the whole ID is a second line only when it says more.
    if !model.own_account && model.user_id != model.name {
        lines = lines.push(
            words(model.user_id.as_str())
                .font(fonts::MONO)
                .size(tokens::BODY)
                .width(Fill),
        );
    }
    container(lines)
        .padding(tokens::M)
        .width(Fill)
        .style(surface_style_boxed)
        .into()
}

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

/// A cell of the comparison: the large symbol, the name under it, on the
/// surface colour.
fn cell<'a>(symbol: &'a str, name: &'a str, symbol_size: f32) -> Element<'a, Message> {
    container(
        column![
            text(symbol).size(symbol_size),
            words(name)
                .size(tokens::CAPTION)
                .align_x(Center)
                .width(Fill),
        ]
        .align_x(Center)
        .spacing(tokens::XS)
        .width(Fill),
    )
    .padding([tokens::S, tokens::XS])
    .width(Fill)
    .style(surface_style_boxed)
    .into()
}

fn emoji_grid(set: &[twicecat_core::api::SasEmoji; 7]) -> Element<'_, Message> {
    let cells = set
        .iter()
        .map(|e| cell(e.symbol.as_str(), e.description.as_str(), 40.0));
    Row::from_iter(cells).spacing(tokens::XS).width(Fill).into()
}

fn decimals(numbers: [u16; 3]) -> Element<'static, Message> {
    let cells = numbers.iter().map(|n| {
        container(text(n.to_string()).size(tokens::DISPLAY).font(fonts::MONO))
            .padding([tokens::M, tokens::S])
            .center_x(Fill)
            .style(surface_style_boxed)
            .into()
    });
    Row::from_iter(cells).spacing(tokens::M).width(Fill).into()
}

/// A line of words and a quiet **Cancel** at the end, below the comparison:
/// the way out that is neither answer. Text first, so no word runs under it.
fn cancel_line<'a>(hint: &'a str) -> Element<'a, Message> {
    row![
        words(hint)
            .size(tokens::CAPTION)
            .style(kit::muted)
            .width(Fill),
        ghost_button("Cancel", Some(msg(VerificationMessage::Cancel))),
    ]
    .spacing(tokens::M)
    .align_y(Center)
    .into()
}

/// The two answers of a comparison: **They don't match** in red at the left,
/// **They match** in the main colour at the right, a free stretch between.
fn answers(armed: bool) -> Element<'static, Message> {
    split_footer(
        Some(danger_button("They don't match", Some(msg(VerificationMessage::NoMatch))).into()),
        Some(primary_button("They match", armed.then(|| msg(VerificationMessage::Match))).into()),
    )
}

type Parts<'a> = (Element<'a, Message>, Element<'a, Message>);

/// The body and footer of the phase.
fn parts(model: &VerificationModel) -> Parts<'_> {
    let armed = model.armed();
    let cancel = || -> Element<'_, Message> {
        secondary_button("Cancel", Some(msg(VerificationMessage::Cancel))).into()
    };
    let waiting_hint = || -> Element<'_, Message> {
        words("Waiting for the other side\u{2026}")
            .style(kit::muted)
            .into()
    };
    match &model.phase {
        Phase::Requested { incoming: true } => (
            column![
                words("Only accept a request you expected: you will compare a short code with the other side, and if it matches, it is trusted."),
                words(if armed { READY_HINT } else { WAITING_HINT })
                    .size(tokens::CAPTION)
                    .style(kit::muted),
            ]
            .spacing(tokens::M)
            .into(),
            // Decline at the left edge, Accept at the right: no spot is
            // shared, and Accept waits for the card to settle.
            split_footer(
                Some(secondary_button("Decline", Some(msg(VerificationMessage::Cancel))).into()),
                Some(
                    primary_button("Accept", armed.then(|| msg(VerificationMessage::Accept)))
                        .into(),
                ),
            ),
        ),
        Phase::Requested { incoming: false } | Phase::Ready | Phase::Waiting | Phase::Confirmed => (
            waiting_hint(),
            split_footer(Some(cancel()), None),
        ),
        Phase::ShowQr => {
            let mut body = column![].spacing(tokens::M).align_x(Center);
            if let Some(handle) = model.qr_handle() {
                body = body.push(
                    container(
                        image::Image::new(handle.clone())
                            .filter_method(FilterMethod::Nearest)
                            .width(Length::Fixed(QR_SIDE))
                            .height(Length::Fixed(QR_SIDE)),
                    )
                    .center_x(Fill),
                );
            }
            (
                body.into(),
                split_footer(
                    Some(cancel()),
                    Some(
                        secondary_button(
                            "Compare emoji instead",
                            Some(msg(VerificationMessage::CompareEmoji)),
                        )
                        .into(),
                    ),
                ),
            )
        }
        Phase::QrScanned => (
            column![
                words(if armed { READY_HINT } else { WAITING_HINT })
                    .size(tokens::CAPTION)
                    .style(kit::muted),
            ]
            .into(),
            split_footer(
                Some(danger_button("No", Some(msg(VerificationMessage::Cancel))).into()),
                Some(
                    primary_button("Yes", armed.then(|| msg(VerificationMessage::QrConfirmed)))
                        .into(),
                ),
            ),
        ),
        Phase::Emojis(set) => (
            column![
                emoji_grid(set),
                banner(Severity::Info, COMPARE_HINT),
                cancel_line(if armed { READY_HINT } else { WAITING_HINT }),
            ]
            .spacing(tokens::M)
            .into(),
            answers(armed),
        ),
        Phase::Decimals(numbers) => (
            column![
                decimals(*numbers),
                banner(Severity::Info, COMPARE_NUMBERS_HINT),
                cancel_line(if armed { READY_HINT } else { WAITING_HINT }),
            ]
            .spacing(tokens::M)
            .into(),
            answers(armed),
        ),
        Phase::Done => (
            banner(
                Severity::Success,
                if model.own_account {
                    "This session is verified. Your encrypted history can be read here."
                } else {
                    "Verified. You can trust this person's messages are from their own devices."
                },
            ),
            split_footer(
                None,
                Some(primary_button("Done", Some(msg(VerificationMessage::Close))).into()),
            ),
        ),
        Phase::Cancelled(reason) => (
            banner(cancel_severity(*reason), cancel_text(*reason)),
            split_footer(
                None,
                Some(primary_button("Close", Some(msg(VerificationMessage::Close))).into()),
            ),
        ),
    }
}

/// The card in a tall window (what the tests of the phases build).
#[cfg(test)]
fn content(model: &VerificationModel) -> Element<'_, Message> {
    card(model, 2000.0)
}

/// The card for a window `height` high.
fn card(model: &VerificationModel, height: f32) -> Element<'_, Message> {
    let close = if model.is_over() {
        VerificationMessage::Close
    } else {
        VerificationMessage::Cancel
    };
    let (body, footer) = parts(model);
    let header = dialog_header(title(model), msg(close));
    let content = column![steps(&model.phase), peer_block(model), body].spacing(tokens::L);
    dialog(header, content.into(), Some(footer), WIDTH, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_steps_follow_the_phases() {
        assert_eq!(step(&Phase::Waiting), Some(0));
        assert_eq!(step(&Phase::Requested { incoming: true }), Some(0));
        assert_eq!(step(&Phase::QrScanned), Some(1));
        assert_eq!(step(&Phase::Confirmed), Some(1));
        assert_eq!(step(&Phase::Done), Some(2));
        assert_eq!(step(&Phase::Cancelled(CancelReason::ByUs)), None);
        assert_eq!(step_caption(&Phase::Ready), "Step 1 of 3: Request");
        assert_eq!(step_caption(&Phase::Done), "All 3 steps done");
        assert_eq!(
            step_caption(&Phase::Cancelled(CancelReason::Other)),
            "Stopped"
        );
    }

    #[test]
    fn a_mismatch_is_danger_and_every_ending_has_words() {
        assert_eq!(cancel_severity(CancelReason::Mismatch), Severity::Danger);
        assert_eq!(cancel_severity(CancelReason::Timeout), Severity::Warning);
        assert_eq!(cancel_severity(CancelReason::ByUs), Severity::Info);
    }

    use twicecat_core::api::{
        DeviceId, QrImage, SasEmoji, UserId, VerificationId, VerificationPeer,
        VerificationPhase as P,
    };

    fn model(own: bool) -> VerificationModel {
        VerificationModel::new(
            VerificationId::from("f"),
            &VerificationPeer {
                user_id: UserId::from("@bob:x"),
                device_id: Some(DeviceId::from("DEV")),
                device_name: Some("Phone".to_owned()),
                own_account: own,
            },
        )
    }

    fn at(m: &mut VerificationModel, phase: P) {
        assert!(m.on_phase(&VerificationId::from("f"), phase));
    }

    #[test]
    fn the_titles_follow_the_plan() {
        let mut own = model(true);
        at(&mut own, P::Requested { incoming: true });
        assert_eq!(title(&own), "Your other device Phone wants to verify");
        at(
            &mut own,
            P::ShowQr(QrImage {
                size: 21,
                dark: vec![false; 441],
            }),
        );
        assert_eq!(title(&own), "Scan this code with your other device");
        at(&mut own, P::QrScanned);
        assert_eq!(
            title(&own),
            "Does your other device show that the code was scanned successfully?"
        );
        at(&mut own, P::Done);
        assert_eq!(title(&own), "Verified");

        let mut other = model(false);
        at(&mut other, P::Requested { incoming: true });
        assert_eq!(title(&other), "@bob:x wants to verify");
        at(
            &mut other,
            P::ShowQr(QrImage {
                size: 21,
                dark: vec![false; 441],
            }),
        );
        assert_eq!(title(&other), "Ask @bob:x to scan this code");
        at(&mut other, P::QrScanned);
        assert_eq!(title(&other), "Does @bob:x see a confirmation?");
    }

    #[test]
    fn every_cancel_reason_has_one_line() {
        for reason in [
            CancelReason::ByUs,
            CancelReason::ByThem,
            CancelReason::Mismatch,
            CancelReason::Timeout,
            CancelReason::AcceptedElsewhere,
            CancelReason::Other,
        ] {
            assert!(!cancel_text(reason).contains('\n'));
        }
    }

    #[test]
    fn every_phase_builds_a_card() {
        let mut m = model(true);
        let set = Box::new(std::array::from_fn(|_| SasEmoji {
            symbol: "\u{1F436}".to_owned(),
            description: "Dog".to_owned(),
        }));
        for phase in [
            P::Requested { incoming: true },
            P::Ready,
            P::ShowQr(QrImage {
                size: 21,
                dark: vec![true; 441],
            }),
            P::QrScanned,
            P::Emojis(set),
            P::Decimals([1, 2, 3]),
            P::Confirmed,
            P::Cancelled {
                reason: CancelReason::Mismatch,
            },
        ] {
            at(&mut m, phase);
            let _ = content(&m);
        }
    }
}
