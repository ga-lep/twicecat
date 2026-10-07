//! The verification dialog: a modal in the fixed layer above the main view
//! and the security screens, with one card per phase of the flow.

use iced::widget::image::{self, FilterMethod};
use iced::widget::{Row, column, container, space, stack, text};
use iced::{Center, Element, Fill, Length};
use tessera_core::api::CancelReason;

use crate::screens::security::{card, secondary, wide};
use crate::state::{App, Message};
use crate::verification::{Phase, VerificationMessage, VerificationModel};

/// Side of the drawn QR code, in logical pixels (the image is larger and is
/// scaled with the nearest-neighbour filter).
pub const QR_SIDE: f32 = 288.0;

fn msg(message: VerificationMessage) -> Message {
    Message::Verification(message)
}

/// The base with the dialog as the layer above it: always a stack of two, so
/// the dialog opening keeps the widget state under it.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let dialog = app.verification.as_ref().map(|m| card(content(m)));
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

fn cancel_button() -> Element<'static, Message> {
    secondary("Cancel", Some(msg(VerificationMessage::Cancel)))
}

fn emoji_grid(set: &[tessera_core::api::SasEmoji; 7]) -> Element<'_, Message> {
    let cells = set.iter().map(|e| {
        column![
            text(e.symbol.as_str()).size(36),
            text(e.description.as_str()).size(12)
        ]
        .align_x(Center)
        .width(Fill)
        .into()
    });
    Row::from_iter(cells).spacing(4).width(Fill).into()
}

fn content(model: &VerificationModel) -> Element<'_, Message> {
    let mut body = column![text(title(model)).size(22)].spacing(12);
    // The title may cut a long user ID in the middle: show it whole, wrapped.
    if !model.own_account && !model.is_over() {
        body = body.push(text(model.user_id.as_str()).size(14));
    }
    match &model.phase {
        Phase::Requested { incoming: true } => {
            body = body
                .push(wide("Accept", Some(msg(VerificationMessage::Accept))))
                .push(secondary("Decline", Some(msg(VerificationMessage::Cancel))));
        }
        Phase::Requested { incoming: false } | Phase::Ready | Phase::Waiting | Phase::Confirmed => {
            body = body
                .push(text("Waiting for the other side\u{2026}"))
                .push(cancel_button());
        }
        Phase::ShowQr => {
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
            body = body
                .push(secondary(
                    "Compare emoji instead",
                    Some(msg(VerificationMessage::CompareEmoji)),
                ))
                .push(cancel_button());
        }
        Phase::QrScanned => {
            body = body
                .push(wide("Yes", Some(msg(VerificationMessage::QrConfirmed))))
                .push(secondary("No", Some(msg(VerificationMessage::Cancel))));
        }
        Phase::Emojis(set) => {
            body = body
                .push(emoji_grid(set))
                .push(text(
                    "Check that these appear in the same order on the other side.",
                ))
                .push(wide("They match", Some(msg(VerificationMessage::Match))))
                .push(secondary(
                    "They don't match",
                    Some(msg(VerificationMessage::NoMatch)),
                ));
        }
        Phase::Decimals([a, b, c]) => {
            body = body
                .push(text(format!("{a}  {b}  {c}")).size(32))
                .push(text(
                    "Check that these numbers are the same on the other side.",
                ))
                .push(wide("They match", Some(msg(VerificationMessage::Match))))
                .push(secondary(
                    "They don't match",
                    Some(msg(VerificationMessage::NoMatch)),
                ));
        }
        Phase::Done => {
            body = body.push(wide("Close", Some(msg(VerificationMessage::Close))));
        }
        Phase::Cancelled(reason) => {
            body = body
                .push(text(cancel_text(*reason)))
                .push(wide("Close", Some(msg(VerificationMessage::Close))));
        }
    }
    body.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::api::{
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
