//! The verification dialog's state (plan Task 11): one flow, followed by its
//! ID, and the QR code as an image that lives only while a QR phase does.
//! Pure: only `App`'s handlers in this file send commands.

use std::fmt;
use std::time::Duration;

use iced::Task;
use iced::widget::image::Handle;
use twicecat_core::api::{
    CancelReason, Command, CoreError, QrImage, SasEmoji, VerificationId, VerificationPeer,
    VerificationPhase,
};

use crate::state::{App, Message};
use crate::text_safety::{name_or_placeholder, one_line_capped};

/// Pixels per QR module (plan Task 11).
pub const QR_SCALE: u32 = 6;
/// Quiet zone around the QR code, in modules.
pub const QR_QUIET: u32 = 4;
/// How many characters a peer's name may show in the dialog.
pub const MAX_PEER_NAME_CHARS: usize = 64;
/// How many characters the full user ID line may show (the Matrix limit is
/// 255 bytes); a longer one is cut in the middle.
pub const MAX_PEER_USER_ID_CHARS: usize = 255;
/// How many characters an emoji description or symbol may show.
const MAX_SAS_CHARS: usize = 32;

/// How long a card that just asked something ignores the buttons that agree
/// (**Accept**, **Yes**, **They match**): a click already on its way when the
/// card came up was meant for what was under it. Buttons that refuse
/// (**Decline**, **Cancel**, **They don't match**) never wait.
pub const ARMING: Duration = Duration::from_millis(600);

/// `text` cleaned to one line and, when longer than `max_chars`, cut in the
/// middle (an ellipsis between the start and the end). The end stays visible:
/// the server name of a user ID is the part that tells users apart.
pub fn elide_middle(text: &str, max_chars: usize) -> String {
    let clean = name_or_placeholder(text, usize::MAX);
    let count = clean.chars().count();
    if count <= max_chars {
        return clean;
    }
    let keep = max_chars.saturating_sub(1);
    let tail = keep / 2;
    let head = keep - tail;
    let mut out: String = clean.chars().take(head).collect();
    out.push('\u{2026}');
    out.extend(clean.chars().skip(count - tail));
    out
}

/// The RGBA pixels of a QR code: `(size + 2 * quiet) * scale` pixels square,
/// black modules on a white ground in every theme (a scanner needs the
/// contrast). A grid shorter than `size * size` reads as light.
pub fn qr_rgba(image: &QrImage, scale: u32, quiet: u32) -> (u32, u32, Vec<u8>) {
    let size = u32::from(image.size);
    let side = (size + 2 * quiet) * scale;
    let mut pixels = Vec::with_capacity((side as usize) * (side as usize) * 4);
    for y in 0..side {
        let row = (y / scale).checked_sub(quiet).filter(|r| *r < size);
        for x in 0..side {
            let column = (x / scale).checked_sub(quiet).filter(|c| *c < size);
            let dark = match (row, column) {
                (Some(r), Some(c)) => image
                    .dark
                    .get((r * size + c) as usize)
                    .copied()
                    .unwrap_or(false),
                _ => false,
            };
            let level = if dark { 0 } else { 255 };
            pixels.extend_from_slice(&[level, level, level, 255]);
        }
    }
    (side, side, pixels)
}

/// Where the flow stands, as the dialog needs it: the QR grid is not kept
/// here (it becomes the model's image) and the text is already cleaned.
#[derive(Clone, PartialEq, Eq)]
pub enum Phase {
    /// A request exists.
    Requested {
        /// The other side asked.
        incoming: bool,
    },
    /// Both sides agreed.
    Ready,
    /// Waiting for the other side.
    Waiting,
    /// The QR code is shown.
    ShowQr,
    /// The other device scanned the code.
    QrScanned,
    /// Compare these emoji.
    Emojis(Box<[SasEmoji; 7]>),
    /// Compare these numbers.
    Decimals([u16; 3]),
    /// Confirmed here; waiting for the other side.
    Confirmed,
    /// Verified.
    Done,
    /// Ended without success.
    Cancelled(CancelReason),
}

impl Phase {
    /// A short name for tests and `Debug`: never the emoji or numbers.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Requested { .. } => "requested",
            Self::Ready => "ready",
            Self::Waiting => "waiting",
            Self::ShowQr => "show_qr",
            Self::QrScanned => "qr_scanned",
            Self::Emojis(_) => "emojis",
            Self::Decimals(_) => "decimals",
            Self::Confirmed => "confirmed",
            Self::Done => "done",
            Self::Cancelled(_) => "cancelled",
        }
    }

    /// The flow is over.
    pub fn is_over(&self) -> bool {
        matches!(self, Self::Done | Self::Cancelled(_))
    }

    /// The phases in which the QR image is needed.
    fn needs_qr(&self) -> bool {
        matches!(self, Self::ShowQr | Self::QrScanned)
    }

    /// The phases whose agreeing button waits for [`ARMING`]: the ones where
    /// one click grants trust or starts a flow another person asked for.
    pub fn needs_arming(&self) -> bool {
        matches!(
            self,
            Self::Requested { incoming: true }
                | Self::QrScanned
                | Self::Emojis(_)
                | Self::Decimals(_)
        )
    }
}

/// What the user asked for in the current phase, so a double click sends
/// one command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sent {
    Accept,
    StartSas,
    ConfirmQr,
    ConfirmSas,
    Mismatch,
    Cancel,
}

/// The one verification flow of the dialog.
pub struct VerificationModel {
    /// The flow; events with another ID are ignored.
    pub id: VerificationId,
    /// The peer is another device of this account.
    pub own_account: bool,
    /// The peer's name for the dialog: the device name (own account) or the
    /// user ID, cleaned, one line, capped.
    pub name: String,
    /// The peer's full user ID, cleaned and on one line (cut in the middle
    /// only past [`MAX_PEER_USER_ID_CHARS`]); the dialog shows it in full for
    /// another user, because the capped `name` may hide part of it.
    pub user_id: String,
    /// Where the flow stands.
    pub phase: Phase,
    qr: Option<Handle>,
    pub(crate) sent: Option<Sent>,
    /// The wait of the buttons that agree, while it runs: tells its timer
    /// from an earlier one's. View state only; nothing of it is sent.
    waiting: Option<u64>,
    /// A request that came while this finished card was open; it is shown
    /// after **Close** (a click meant for Close must not land on **Accept**).
    queued: Option<Box<VerificationModel>>,
}

impl VerificationModel {
    /// A model for a new flow with `peer`, in the first phase.
    pub fn new(id: VerificationId, peer: &VerificationPeer) -> Self {
        let raw = if peer.own_account {
            peer.device_name
                .as_deref()
                .or(peer.device_id.as_ref().map(|d| d.as_str()))
        } else {
            None
        };
        // A device name is cut from the end; a user ID never is (its server
        // name is at the end), so it is cut in the middle.
        let name = match raw {
            Some(raw) => name_or_placeholder(raw, MAX_PEER_NAME_CHARS),
            None => elide_middle(peer.user_id.as_str(), MAX_PEER_NAME_CHARS),
        };
        Self {
            id,
            own_account: peer.own_account,
            name,
            user_id: elide_middle(peer.user_id.as_str(), MAX_PEER_USER_ID_CHARS),
            phase: Phase::Requested { incoming: false },
            qr: None,
            sent: None,
            waiting: None,
            queued: None,
        }
    }

    /// Whether the buttons that agree work: the wait after the card (or its
    /// phase) came up is over.
    pub fn armed(&self) -> bool {
        self.waiting.is_none()
    }

    /// The sequence of the wait now running, if one runs.
    pub fn arming_seq(&self) -> Option<u64> {
        self.waiting
    }

    /// The flow is over (**Close** is all that is left).
    pub fn is_over(&self) -> bool {
        self.phase.is_over()
    }

    /// The QR image, while a QR phase lasts.
    pub fn qr_handle(&self) -> Option<&Handle> {
        self.qr.as_ref()
    }

    /// Applies a phase of flow `id`. `false` when it was not applied: another
    /// flow, or this one is over.
    pub fn on_phase(&mut self, id: &VerificationId, phase: VerificationPhase) -> bool {
        if let Some(queued) = self.queued.as_mut().filter(|q| q.id == *id) {
            return queued.on_phase(id, phase);
        }
        if *id != self.id || self.phase.is_over() {
            return false;
        }
        self.phase = match phase {
            VerificationPhase::Requested { incoming } => Phase::Requested { incoming },
            VerificationPhase::Ready => Phase::Ready,
            VerificationPhase::Waiting => Phase::Waiting,
            VerificationPhase::ShowQr(image) => {
                let (w, h, pixels) = qr_rgba(&image, QR_SCALE, QR_QUIET);
                self.qr = Some(Handle::from_rgba(w, h, pixels));
                Phase::ShowQr
            }
            VerificationPhase::QrScanned => Phase::QrScanned,
            VerificationPhase::Emojis(set) => {
                Phase::Emojis(Box::new(set.each_ref().map(|e| SasEmoji {
                    symbol: one_line_capped(&e.symbol, MAX_SAS_CHARS),
                    description: one_line_capped(&e.description, MAX_SAS_CHARS),
                })))
            }
            VerificationPhase::Decimals(numbers) => Phase::Decimals(numbers),
            VerificationPhase::Confirmed => Phase::Confirmed,
            VerificationPhase::Done => Phase::Done,
            VerificationPhase::Cancelled { reason } => Phase::Cancelled(reason),
        };
        // The code is a one-time secret: drop it as soon as no QR phase shows.
        if !self.phase.needs_qr() {
            self.qr = None;
        }
        self.sent = None;
        // A new phase never inherits the wait of the one before.
        self.waiting = None;
        true
    }

    /// The flow `id` (this one or the one queued behind it).
    fn flow_mut(&mut self, id: &VerificationId) -> Option<&mut VerificationModel> {
        if self.id == *id {
            Some(self)
        } else {
            self.queued.as_deref_mut().filter(|q| q.id == *id)
        }
    }
}

// Manual impl: no module data, no emoji or numbers.
impl fmt::Debug for VerificationModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VerificationModel")
            .field("id", &self.id)
            .field("phase", &self.phase.kind())
            .field("has_qr", &self.qr.is_some())
            .finish_non_exhaustive()
    }
}

/// What the user can do in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerificationMessage {
    /// **Accept** an incoming request.
    Accept,
    /// **Decline**, **Cancel** and **No** (the scan was not confirmed).
    Cancel,
    /// **Compare emoji instead**, while the QR code is shown.
    CompareEmoji,
    /// **Yes**: the other device confirms the scan.
    QrConfirmed,
    /// **They match**.
    Match,
    /// **They don't match**.
    NoMatch,
    /// **Close** a finished flow.
    Close,
    /// The wait of the card (its sequence) ended. View state only.
    Armed(u64),
}

impl App {
    /// A flow started or arrived. One dialog at a time: the core holds one
    /// flow too, so a request while another runs is not shown. An ended flow
    /// still on screen stays until **Close**, and the request waits behind it.
    pub(crate) fn on_verification_requested(
        &mut self,
        id: VerificationId,
        peer: &VerificationPeer,
    ) -> Task<Message> {
        // The request is on screen (or queued): the identity screen may ask
        // again later.
        let model = VerificationModel::new(id, peer);
        match &mut self.verification {
            Some(shown) if !shown.is_over() => return Task::none(),
            Some(shown) => shown.queued = Some(Box::new(model)),
            None => self.verification = Some(model),
        }
        self.security.asked_other_device = false;
        Task::none()
    }

    /// A flow moved on; another flow's event changes nothing.
    pub(crate) fn on_verification_phase(
        &mut self,
        id: &VerificationId,
        phase: VerificationPhase,
    ) -> Task<Message> {
        let applied = self
            .verification
            .as_mut()
            .is_some_and(|model| model.on_phase(id, phase));
        if applied {
            return self.start_arming(id);
        }
        Task::none()
    }

    /// Starts the wait of flow `id` when its phase asks for one.
    fn start_arming(&mut self, id: &VerificationId) -> Task<Message> {
        let counter = self.verify_arm_counter.wrapping_add(1);
        let Some(model) = self
            .verification
            .as_mut()
            .and_then(|shown| shown.flow_mut(id))
            .filter(|model| model.phase.needs_arming())
        else {
            return Task::none();
        };
        model.waiting = Some(counter);
        self.verify_arm_counter = counter;
        self.timer(
            ARMING,
            Message::Verification(VerificationMessage::Armed(counter)),
        )
    }

    /// A verification command failed. The core ends a broken flow with a
    /// `Cancelled` phase and also reports recoverable steps (a QR code that
    /// could not be drawn falls back to SAS), so a flow that runs shows no
    /// failure; its buttons just work again. Without a flow, the request
    /// itself failed.
    pub(crate) fn on_verification_failed(&mut self, error: &CoreError) -> Task<Message> {
        if let Some(model) = self.verification.as_mut().filter(|m| !m.is_over()) {
            model.sent = None;
            return Task::none();
        }
        self.security.asked_other_device = false;
        self.security.notice = Some(one_line_capped(
            &format!("Could not start the verification: {error}"),
            512,
        ));
        Task::none()
    }

    /// Applies a button of the dialog: one command per press, and only the
    /// buttons of the current phase act.
    pub(crate) fn on_verification(&mut self, message: VerificationMessage) -> Task<Message> {
        use VerificationMessage as M;
        let Some(model) = self.verification.as_mut() else {
            return Task::none();
        };
        if let M::Armed(seq) = message {
            // The wait of this card, or of the request queued behind it.
            if model.waiting == Some(seq) {
                model.waiting = None;
            }
            if let Some(queued) = model.queued.as_deref_mut()
                && queued.waiting == Some(seq)
            {
                queued.waiting = None;
            }
            return Task::none();
        }
        if message == M::Close {
            if model.is_over() {
                // The queued request shows next, unless it already ended; its
                // buttons wait again, so a click meant for Close cannot land
                // on its Accept.
                self.verification = model.queued.take().filter(|q| !q.is_over()).map(|q| *q);
                if let Some(next) = self.verification.as_ref().map(|m| m.id.clone()) {
                    return self.start_arming(&next);
                }
            }
            return Task::none();
        }
        // The buttons that agree wait for the card to settle.
        if matches!(message, M::Accept | M::QrConfirmed | M::Match) && !model.armed() {
            return Task::none();
        }
        let id = model.id.clone();
        let (sent, command) = match (message, &model.phase) {
            (M::Accept, Phase::Requested { incoming: true }) => {
                (Sent::Accept, Command::AcceptVerification { id })
            }
            (M::Cancel, phase) if !phase.is_over() => {
                (Sent::Cancel, Command::CancelVerification { id })
            }
            (M::CompareEmoji, Phase::ShowQr) => (Sent::StartSas, Command::StartSas { id }),
            (M::QrConfirmed, Phase::QrScanned) => {
                (Sent::ConfirmQr, Command::ConfirmQrScanned { id })
            }
            (M::Match, Phase::Emojis(_) | Phase::Decimals(_)) => {
                (Sent::ConfirmSas, Command::ConfirmSas { id })
            }
            (M::NoMatch, Phase::Emojis(_) | Phase::Decimals(_)) => {
                (Sent::Mismatch, Command::SasMismatch { id })
            }
            _ => return Task::none(),
        };
        if model.sent == Some(sent) {
            return Task::none();
        }
        model.sent = Some(sent);
        self.send(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twicecat_core::api::{
        CancelReason, DeviceId, QrImage, SasEmoji, UserId, VerificationId, VerificationPeer,
        VerificationPhase as P,
    };

    fn peer(own: bool) -> VerificationPeer {
        VerificationPeer {
            user_id: UserId::from("@bob:x"),
            device_id: Some(DeviceId::from("DEV")),
            device_name: Some("Phone".to_owned()),
            own_account: own,
        }
    }

    fn id(s: &str) -> VerificationId {
        VerificationId::from(s)
    }

    fn model(own: bool) -> VerificationModel {
        VerificationModel::new(id("f1"), &peer(own))
    }

    fn qr(size: u16) -> QrImage {
        let n = usize::from(size);
        let mut dark = vec![false; n * n];
        dark[0] = true;
        QrImage { size, dark }
    }

    fn emojis() -> Box<[SasEmoji; 7]> {
        Box::new(std::array::from_fn(|i| SasEmoji {
            symbol: "\u{1F436}".to_owned(),
            description: format!("Dog {i}"),
        }))
    }

    fn kind_after(model: &mut VerificationModel, phases: &[P]) -> Vec<&'static str> {
        phases
            .iter()
            .map(|p| {
                assert!(model.on_phase(&id("f1"), p.clone()), "{p:?} refused");
                model.phase.kind()
            })
            .collect()
    }

    #[test]
    fn an_incoming_flow_goes_to_done_through_the_emoji() {
        let mut m = model(false);
        let kinds = kind_after(
            &mut m,
            &[
                P::Requested { incoming: true },
                P::Ready,
                P::Emojis(emojis()),
                P::Confirmed,
                P::Done,
            ],
        );
        assert_eq!(kinds, ["requested", "ready", "emojis", "confirmed", "done"]);
    }

    #[test]
    fn a_qr_flow_goes_to_done_through_the_scan() {
        let mut m = model(true);
        let kinds = kind_after(
            &mut m,
            &[
                P::Ready,
                P::ShowQr(qr(21)),
                P::QrScanned,
                P::Confirmed,
                P::Done,
            ],
        );
        assert_eq!(
            kinds,
            ["ready", "show_qr", "qr_scanned", "confirmed", "done"]
        );
    }

    #[test]
    fn a_qr_flow_can_switch_to_the_emoji() {
        let mut m = model(true);
        kind_after(&mut m, &[P::Ready, P::ShowQr(qr(21)), P::Waiting]);
        assert_eq!(kind_after(&mut m, &[P::Emojis(emojis())]), ["emojis"]);
    }

    #[test]
    fn cancelled_ends_the_flow_from_every_phase() {
        let phases = [
            P::Requested { incoming: true },
            P::Ready,
            P::Waiting,
            P::ShowQr(qr(21)),
            P::QrScanned,
            P::Emojis(emojis()),
            P::Decimals([1, 2, 3]),
            P::Confirmed,
        ];
        for phase in phases {
            let mut m = model(false);
            assert!(m.on_phase(&id("f1"), phase.clone()));
            assert!(m.on_phase(
                &id("f1"),
                P::Cancelled {
                    reason: CancelReason::ByThem
                }
            ));
            assert_eq!(m.phase.kind(), "cancelled", "from {phase:?}");
            assert!(m.is_over());
        }
    }

    #[test]
    fn a_stale_id_is_ignored() {
        let mut m = model(false);
        assert!(!m.on_phase(&id("other"), P::Done));
        assert_eq!(m.phase.kind(), "requested");
        assert!(!m.is_over());
    }

    #[test]
    fn an_ended_flow_stays_ended() {
        let mut m = model(false);
        assert!(m.on_phase(&id("f1"), P::Done));
        assert!(!m.on_phase(
            &id("f1"),
            P::Cancelled {
                reason: CancelReason::Other
            }
        ));
        assert_eq!(m.phase.kind(), "done");
    }

    #[test]
    fn qr_rgba_has_the_size_the_quiet_zone_and_black_on_white() {
        let image = qr(5);
        let (w, h, px) = qr_rgba(&image, 3, 2);
        assert_eq!((w, h), ((5 + 2 * 2) * 3, (5 + 2 * 2) * 3));
        assert_eq!(px.len(), (w * h * 4) as usize);
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        let white = [255, 255, 255, 255];
        let black = [0, 0, 0, 255];
        // The whole quiet zone (2 modules = 6 px) is white.
        for y in 0..h {
            for x in 0..w {
                if x < 6 || y < 6 || x >= w - 6 || y >= h - 6 {
                    assert_eq!(at(x, y), white, "({x},{y})");
                }
            }
        }
        // Module (0, 0) is black over its whole 3x3 block, its neighbour white.
        for dy in 0..3 {
            for dx in 0..3 {
                assert_eq!(at(6 + dx, 6 + dy), black);
                assert_eq!(at(9 + dx, 6 + dy), white);
            }
        }
        // Only black and white appear, whatever the theme.
        assert!(px.chunks(4).all(|p| p == white || p == black));
    }

    #[test]
    fn qr_rgba_treats_a_short_grid_as_light() {
        let image = QrImage {
            size: 4,
            dark: vec![true; 3],
        };
        let (w, h, px) = qr_rgba(&image, 1, 0);
        assert_eq!((w, h, px.len()), (4, 4, 64));
    }

    #[test]
    fn the_qr_handle_lives_through_the_qr_phases_only() {
        let mut m = model(true);
        assert!(m.qr_handle().is_none());
        m.on_phase(&id("f1"), P::ShowQr(qr(21)));
        let first = m.qr_handle().expect("built on ShowQr").id();
        // The handle is 6 px per module with a 4 module quiet zone.
        let size = u32::from(21u16 + 8) * 6;
        assert!(matches!(
            m.qr_handle(),
            Some(iced::widget::image::Handle::Rgba { width, height, .. })
                if *width == size && *height == size
        ));
        m.on_phase(&id("f1"), P::QrScanned);
        assert_eq!(m.qr_handle().map(|h| h.id()), Some(first));
        m.on_phase(&id("f1"), P::Confirmed);
        assert!(m.qr_handle().is_none(), "dropped once the QR phases end");
    }

    #[test]
    fn the_qr_handle_is_dropped_on_every_way_out() {
        let outs = [
            P::Waiting,
            P::Emojis(emojis()),
            P::Decimals([1, 2, 3]),
            P::Done,
            P::Cancelled {
                reason: CancelReason::ByUs,
            },
        ];
        for out in outs {
            let mut m = model(true);
            m.on_phase(&id("f1"), P::ShowQr(qr(21)));
            assert!(m.qr_handle().is_some());
            m.on_phase(&id("f1"), out.clone());
            assert!(m.qr_handle().is_none(), "kept after {out:?}");
        }
    }

    #[test]
    fn debug_shows_no_module_data() {
        let mut m = model(true);
        m.on_phase(&id("f1"), P::ShowQr(qr(21)));
        let text = format!("{m:?}");
        assert!(!text.contains("dark") && !text.contains("Rgba"), "{text}");
        assert!(text.contains("show_qr"), "{text}");
    }

    #[test]
    fn names_are_cleaned_and_capped() {
        let hostile = VerificationPeer {
            user_id: UserId::from("@bob:x"),
            device_id: Some(DeviceId::from("DEV")),
            device_name: Some(format!("Ph\u{202E}one\n{}", "x".repeat(500))),
            own_account: true,
        };
        let m = VerificationModel::new(id("f1"), &hostile);
        assert!(!m.name.contains('\u{202E}'), "{}", m.name);
        assert!(!m.name.contains('\n'));
        assert!(m.name.chars().count() <= MAX_PEER_NAME_CHARS);
        // Fallbacks: the device ID, then the user ID.
        let no_name = VerificationPeer {
            device_name: None,
            ..peer(true)
        };
        assert_eq!(VerificationModel::new(id("f1"), &no_name).name, "DEV");
        let other = VerificationPeer {
            user_id: UserId::from("@evil\u{202E}:x"),
            ..peer(false)
        };
        let m = VerificationModel::new(id("f1"), &other);
        assert_eq!(m.name, "@evil\u{FFFD}:x");
    }

    #[test]
    fn a_long_user_id_keeps_the_end_of_the_server_name_in_the_name() {
        let long = format!("@alice:matrix.org.{}evil.example", "a".repeat(200));
        let other = VerificationPeer {
            user_id: UserId::from(long.as_str()),
            ..peer(false)
        };
        let m = VerificationModel::new(id("f1"), &other);
        assert!(m.name.chars().count() <= MAX_PEER_NAME_CHARS, "{}", m.name);
        assert!(m.name.starts_with("@alice:"), "{}", m.name);
        assert!(m.name.ends_with("evil.example"), "{}", m.name);
    }

    #[test]
    fn the_full_user_id_is_kept_on_its_own() {
        let long = format!("@alice:matrix.org.{}evil.example", "a".repeat(200));
        let other = VerificationPeer {
            user_id: UserId::from(long.as_str()),
            ..peer(false)
        };
        assert_eq!(VerificationModel::new(id("f1"), &other).user_id, long);
        let hostile = VerificationPeer {
            user_id: UserId::from("@evil\u{202E}:x"),
            ..peer(false)
        };
        let m = VerificationModel::new(id("f1"), &hostile);
        assert_eq!(m.user_id, "@evil\u{FFFD}:x");
    }

    #[test]
    fn emoji_and_descriptions_are_cleaned_but_keep_the_symbol() {
        let mut m = model(false);
        let mut set = emojis();
        set[0].symbol = "\u{2764}\u{FE0F}".to_owned();
        set[1].description = "Do\u{202E}g\n".to_owned();
        m.on_phase(&id("f1"), P::Emojis(set));
        let Phase::Emojis(shown) = &m.phase else {
            panic!("not emojis");
        };
        assert_eq!(shown[0].symbol, "\u{2764}\u{FE0F}");
        assert!(!shown[1].description.contains('\u{202E}'));
        assert!(!shown[1].description.contains('\n'));
    }
}
