//! The Sessions screen's state (plan Task 13): the account's devices, the
//! inline rename and sign-out confirmations, the password prompt of a
//! sign-out and the warning before logging out.
//!
//! Pure: only `App`'s handlers in this file send commands. Device names come
//! from the server and are attacker-controlled; they pass through
//! [`crate::text_safety`] before they are drawn.

use std::collections::HashSet;
use std::fmt;

use chrono::TimeZone;
use iced::Task;
use tessera_core::api::{
    AuthUrl, Command, CoreError, DeviceId, DeviceSummary, DeviceTrust, Password, ReauthAction,
    RecoveryStatus, SecurityStatus,
};

use crate::dates::{day_label, time_label};
use crate::links::{self, LinkKind};
use crate::screens::login::{self, BrowserUrl};
use crate::security::{Busy, ReauthPrompt, SecurityMessage};
use crate::state::{App, Message};
use crate::text_safety::{NAME_PLACEHOLDER, name_or_placeholder, one_line_capped};
use crate::verification::elide_middle;

/// Widget ID of the rename field.
pub const RENAME_INPUT_ID: &str = "sessions-rename-input";
/// Widget ID of the password field.
pub const REAUTH_INPUT_ID: &str = "sessions-reauth-input";

/// The longest device name the core sends, in characters.
pub const MAX_NAME_CHARS: usize = 64;
/// How many characters of a device name or ID a row shows.
pub const MAX_SHOWN_CHARS: usize = 64;
/// How many devices the app keeps from the core's list.
pub const MAX_DEVICES_KEPT: usize = 500;

/// The button of the Recovery line on the Sessions page (plan Task 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// **Change recovery key**.
    ChangeKey,
    /// **Set up recovery**.
    SetUp,
    /// **Enter recovery key**.
    EnterKey,
}

impl RecoveryAction {
    /// The button's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::ChangeKey => "Change recovery key",
            Self::SetUp => "Set up recovery",
            Self::EnterKey => "Enter recovery key",
        }
    }

    /// What pressing the button does.
    pub fn message(self) -> SecurityMessage {
        match self {
            Self::ChangeKey => SecurityMessage::ChangeKeyRequested,
            Self::SetUp => SecurityMessage::SetupRecovery,
            Self::EnterKey => SecurityMessage::OpenRestore,
        }
    }
}

/// The Recovery line at the top of the Sessions page: its text and button,
/// or nothing while the state is not known yet.
pub fn recovery_line(state: RecoveryStatus) -> Option<(&'static str, Option<RecoveryAction>)> {
    match state {
        RecoveryStatus::Enabled => Some(("Recovery is on.", Some(RecoveryAction::ChangeKey))),
        RecoveryStatus::Disabled => Some(("Recovery is off.", Some(RecoveryAction::SetUp))),
        RecoveryStatus::Incomplete => Some((
            "This device isn't connected to your recovery.",
            Some(RecoveryAction::EnterKey),
        )),
        RecoveryStatus::Unknown => None,
    }
}

/// What the logout warning says (plan Q12).
pub const LOGOUT_WARNING: &str = "You'll lose access to your encrypted messages. This is your only session and recovery isn't set up, so nothing can bring them back.";

/// Whether logging out first needs the warning of plan Q12: recovery is not
/// `Enabled` and this is the only device. `other_devices` is the number of
/// other sessions in the last list the core sent, `None` when no list has
/// arrived yet.
///
/// - `Enabled` recovery never warns, and nor does a recovery state the core
///   has not reported yet (`Unknown`: nothing to claim).
/// - Another session means the keys have a second home: no warning.
/// - An unknown list warns, to be safe.
pub fn logout_needs_warning(status: &SecurityStatus, other_devices: Option<usize>) -> bool {
    match status.recovery {
        RecoveryStatus::Enabled | RecoveryStatus::Unknown => false,
        RecoveryStatus::Disabled | RecoveryStatus::Incomplete => {
            other_devices.is_none_or(|n| n == 0)
        }
    }
}

/// What a sign-out would apply to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignOutAsk {
    /// One other session.
    One(DeviceId),
    /// Every other session.
    AllOthers,
}

/// The rename field of one row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameEdit {
    /// The device being renamed.
    pub device_id: DeviceId,
    /// The text as typed (at most [`MAX_NAME_CHARS`] characters).
    pub input: String,
    /// The text the field started with, so an unchanged Enter sends nothing.
    pub original: String,
}

/// An account-management URL that passed the browser check and waits for the
/// user's go-ahead.
#[derive(Clone, PartialEq, Eq)]
pub struct Approval {
    /// The URL to open.
    pub url: BrowserUrl,
    /// The host the bar shows.
    pub host: String,
}

// Manual impl: the URL carries OAuth state.
impl fmt::Debug for Approval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Approval")
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}

/// The Sessions part of the app state.
#[derive(Default)]
pub struct SessionsModel {
    /// The screen is open.
    pub open: bool,
    /// The last list the core sent (kept after the screen closes).
    pub devices: Option<Vec<DeviceSummary>>,
    /// `ListDevices` was sent and not answered yet.
    pub loading: bool,
    /// The open rename field.
    pub rename: Option<RenameEdit>,
    /// The sign-out waiting for the user's confirmation.
    pub confirm: Option<SignOutAsk>,
    /// The devices of a sign-out that was sent and has not ended. Device IDs
    /// only: the password never stays here.
    pub pending: Option<Vec<DeviceId>>,
    /// The password prompt.
    pub reauth: Option<ReauthPrompt>,
    /// The account page waiting for the go-ahead.
    pub approval: Option<Approval>,
    /// A non-secret line about the last failure.
    pub notice: Option<String>,
    /// The warning before logging out is shown.
    pub logout_warning: bool,
}

impl SessionsModel {
    /// How many sessions other than this one the last list holds.
    pub fn other_devices(&self) -> Option<usize> {
        self.devices
            .as_ref()
            .map(|list| list.iter().filter(|d| !d.current).count())
    }
}

// Manual impl: the prompt's password and the names stay out of `Debug`.
impl fmt::Debug for SessionsModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionsModel")
            .field("open", &self.open)
            .field("devices", &self.devices.as_ref().map(Vec::len))
            .field("pending", &self.pending.as_ref().map(Vec::len))
            .field("reauth", &self.reauth)
            .field("logout_warning", &self.logout_warning)
            .finish_non_exhaustive()
    }
}

/// What the user can do on the Sessions screen and the logout warning.
#[derive(Debug, Clone)]
pub enum SessionsMessage {
    /// **Sessions** in the account area.
    Open,
    /// **Close**.
    Close,
    /// **Refresh**: ask the core again (other clients' renames and
    /// sign-outs are not pushed).
    Refresh,
    /// **Rename** on a row.
    RenameStart(DeviceId),
    /// The rename field changed.
    RenameInput(String),
    /// Enter in the rename field.
    RenameSubmitted,
    /// **Cancel** on the rename field.
    RenameCancelled,
    /// **Verify** on a row.
    Verify(DeviceId),
    /// **Sign out** on a row.
    SignOutAsk(DeviceId),
    /// **Sign out of all other sessions**.
    SignOutAllAsk,
    /// **Sign out** in the inline confirmation.
    SignOutConfirmed,
    /// **Cancel** in the inline confirmation.
    SignOutCancelled,
    /// The password prompt changed; its `Debug` is redacted.
    ReauthInput(Password),
    /// **Continue** on the password prompt.
    ReauthSubmitted,
    /// **Cancel** on the password prompt.
    ReauthCancelled,
    /// **Open** on the account page bar.
    ApprovalConfirmed,
    /// **Cancel** on the account page bar.
    ApprovalDismissed,
    /// The browser launcher finished.
    BrowserOpened(bool),
    /// **Set up recovery** on the logout warning.
    LogoutSetupRecovery,
    /// **Sign out anyway** on the logout warning.
    LogoutAnyway,
    /// **Cancel** on the logout warning.
    LogoutCancelled,
    /// The cross on the notice.
    DismissNotice,
}

/// The title of a row: the sanitized name, or the device ID when there is no
/// usable name.
pub fn device_title(device: &DeviceSummary) -> String {
    if let Some(name) = &device.display_name {
        let clean = name_or_placeholder(name, MAX_SHOWN_CHARS);
        if clean != NAME_PLACEHOLDER {
            return clean;
        }
    }
    device_id_line(&device.device_id)
}

/// The device ID as a row shows it: one sanitized line, cut in the middle if
/// at all.
pub fn device_id_line(id: &DeviceId) -> String {
    elide_middle(id.as_str(), MAX_SHOWN_CHARS)
}

/// The badge of a row.
pub fn trust_label(trust: DeviceTrust) -> &'static str {
    match trust {
        DeviceTrust::Verified => "Verified",
        DeviceTrust::Unverified => "Not verified",
        DeviceTrust::NoKeys => "No encryption keys",
    }
}

/// Whether a row offers **Verify**: another device that is unverified.
pub fn can_verify(device: &DeviceSummary) -> bool {
    !device.current && device.trust == DeviceTrust::Unverified
}

/// "Last active ..." for a row, in `tz` as seen from `now_ms`.
pub fn last_active_text<Tz: TimeZone>(last_seen_ms: Option<u64>, now_ms: u64, tz: &Tz) -> String {
    match last_seen_ms {
        Some(ms) => format!(
            "Last active {} {}",
            day_label(ms, now_ms, tz),
            time_label(ms, tz)
        ),
        None => "Last active: unknown".to_owned(),
    }
}

/// `ids` without repeats, in order.
pub fn dedup_ids(ids: Vec<DeviceId>) -> Vec<DeviceId> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// The first `max` characters of `text`.
fn capped(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// This device first, the others in the core's order, at most
/// [`MAX_DEVICES_KEPT`].
fn arrange(mut list: Vec<DeviceSummary>) -> Vec<DeviceSummary> {
    list.truncate(MAX_DEVICES_KEPT);
    // Stable: the others keep the core's order.
    list.sort_by_key(|device| !device.current);
    list
}

impl SessionsModel {
    fn find(&self, id: &DeviceId) -> Option<&DeviceSummary> {
        self.devices
            .as_ref()?
            .iter()
            .find(|device| device.device_id == *id)
    }

    /// The devices a sign-out of `ask` would apply to: never this device,
    /// only devices in the list, each once.
    fn sign_out_targets(&self, ask: &SignOutAsk) -> Vec<DeviceId> {
        let Some(list) = &self.devices else {
            return Vec::new();
        };
        let ids: Vec<DeviceId> = match ask {
            SignOutAsk::One(id) => list
                .iter()
                .filter(|d| !d.current && d.device_id == *id)
                .map(|d| d.device_id.clone())
                .collect(),
            SignOutAsk::AllOthers => list
                .iter()
                .filter(|d| !d.current)
                .map(|d| d.device_id.clone())
                .collect(),
        };
        dedup_ids(ids)
    }

    /// Forgets everything about a sign-out in flight.
    fn end_sign_out(&mut self) {
        self.pending = None;
        self.reauth = None;
    }
}

/// The one-line notice for a failed action.
fn failure_line(error: &CoreError) -> String {
    let text = match error {
        CoreError::Internal(text) => text.clone(),
        other => other.to_string(),
    };
    one_line_capped(&text, 512)
}

impl App {
    /// Applies a message of the Sessions screen.
    pub(crate) fn on_sessions(&mut self, message: SessionsMessage) -> Task<Message> {
        use SessionsMessage as M;
        match message {
            M::Open => {
                let model = &mut self.sessions;
                model.open = true;
                model.loading = true;
                model.notice = None;
                self.send(Command::ListDevices)
            }
            M::Close => {
                let model = &mut self.sessions;
                model.open = false;
                model.rename = None;
                model.confirm = None;
                model.approval = None;
                model.notice = None;
                // The prompt (and its password) goes, but a sign-out the core
                // is still running stays in `pending` until the core answers:
                // a reset must not start meanwhile, and the account page it
                // may answer with must not be taken for the reset's.
                model.reauth = None;
                Task::none()
            }
            M::Refresh => {
                self.sessions.loading = true;
                self.send(Command::ListDevices)
            }
            M::RenameStart(id) => {
                let model = &mut self.sessions;
                let Some(device) = model.find(&id) else {
                    return Task::none();
                };
                // The field starts with the sanitized name, never the raw one.
                let original = device
                    .display_name
                    .as_deref()
                    .map(|name| capped(&one_line_capped(name, MAX_NAME_CHARS), MAX_NAME_CHARS))
                    .unwrap_or_default();
                model.rename = Some(RenameEdit {
                    device_id: id,
                    input: original.clone(),
                    original,
                });
                model.confirm = None;
                iced::widget::operation::focus(RENAME_INPUT_ID)
            }
            M::RenameInput(text) => {
                if let Some(edit) = &mut self.sessions.rename {
                    edit.input = capped(&text, MAX_NAME_CHARS);
                }
                Task::none()
            }
            M::RenameSubmitted => {
                let Some(edit) = self.sessions.rename.take() else {
                    return Task::none();
                };
                let name = edit.input.trim();
                if name.is_empty() || edit.input == edit.original {
                    return Task::none();
                }
                self.send(Command::RenameDevice {
                    device_id: edit.device_id,
                    name: name.to_owned(),
                })
            }
            M::RenameCancelled => {
                self.sessions.rename = None;
                Task::none()
            }
            M::Verify(id) => match self.sessions.find(&id) {
                Some(device) if can_verify(device) => {
                    self.send(Command::RequestDeviceVerification { device_id: id })
                }
                _ => Task::none(),
            },
            M::SignOutAsk(id) => {
                if self.refuse_sign_out_during_reset() {
                    return Task::none();
                }
                let model = &mut self.sessions;
                if model.pending.is_none() && model.find(&id).is_some_and(|d| !d.current) {
                    model.confirm = Some(SignOutAsk::One(id));
                    model.rename = None;
                }
                Task::none()
            }
            M::SignOutAllAsk => {
                if self.refuse_sign_out_during_reset() {
                    return Task::none();
                }
                let model = &mut self.sessions;
                if model.pending.is_none() && model.other_devices().is_some_and(|n| n > 0) {
                    model.confirm = Some(SignOutAsk::AllOthers);
                    model.rename = None;
                }
                Task::none()
            }
            M::SignOutConfirmed => {
                if self.refuse_sign_out_during_reset() {
                    self.sessions.confirm = None;
                    return Task::none();
                }
                let model = &mut self.sessions;
                let Some(ask) = model.confirm.take() else {
                    return Task::none();
                };
                let device_ids = model.sign_out_targets(&ask);
                if model.pending.is_some() || device_ids.is_empty() {
                    return Task::none();
                }
                model.notice = None;
                model.pending = Some(device_ids.clone());
                self.send(Command::SignOutDevices {
                    device_ids,
                    password: None,
                })
            }
            M::SignOutCancelled => {
                self.sessions.confirm = None;
                Task::none()
            }
            M::ReauthInput(password) => {
                if let Some(prompt) = self.sessions.reauth.as_mut().filter(|p| !p.waiting) {
                    prompt.password = password;
                }
                Task::none()
            }
            M::ReauthSubmitted => self.submit_sessions_reauth(),
            M::ReauthCancelled => {
                // Dropping the prompt zeroizes the typed password.
                self.sessions.end_sign_out();
                Task::none()
            }
            M::ApprovalConfirmed => match self.sessions.approval.take() {
                Some(approval) => {
                    Task::perform(async move { login::open_in_browser(&approval.url) }, |ok| {
                        Message::Sessions(SessionsMessage::BrowserOpened(ok))
                    })
                }
                None => Task::none(),
            },
            M::ApprovalDismissed => {
                self.sessions.approval = None;
                Task::none()
            }
            M::BrowserOpened(ok) => {
                if !ok {
                    self.sessions.notice = Some("Your browser could not be started.".to_owned());
                }
                Task::none()
            }
            M::LogoutSetupRecovery => {
                if !std::mem::take(&mut self.sessions.logout_warning) {
                    return Task::none();
                }
                self.on_security(SecurityMessage::SetupRecovery)
            }
            M::LogoutAnyway => {
                if !std::mem::take(&mut self.sessions.logout_warning) {
                    return Task::none();
                }
                self.logout()
            }
            M::LogoutCancelled => {
                self.sessions.logout_warning = false;
                Task::none()
            }
            M::DismissNotice => {
                self.sessions.notice = None;
                Task::none()
            }
        }
    }

    /// Sends the typed password with the sign-out that asked for it. The
    /// password moves into the command; the field is left empty.
    fn submit_sessions_reauth(&mut self) -> Task<Message> {
        let model = &mut self.sessions;
        let (Some(prompt), Some(device_ids)) = (model.reauth.as_mut(), model.pending.clone())
        else {
            return Task::none();
        };
        if prompt.waiting || prompt.password.expose().is_empty() {
            return Task::none();
        }
        let password = std::mem::replace(&mut prompt.password, Password::from(String::new()));
        prompt.waiting = true;
        prompt.failed = false;
        self.send(Command::SignOutDevices {
            device_ids,
            password: Some(password),
        })
    }

    /// A sign-out and an identity reset must not overlap: on an OAuth account
    /// both answer with an account-page URL, and the reset's would take the
    /// sign-out's place. While a reset runs this gives a notice and answers
    /// `true`, and the caller starts nothing.
    fn refuse_sign_out_during_reset(&mut self) -> bool {
        let prompt_open = self
            .security
            .reauth
            .as_ref()
            .is_some_and(|p| p.action == ReauthAction::ResetIdentity);
        if self.security.busy != Some(Busy::Resetting) && !prompt_open {
            return false;
        }
        self.sessions.notice =
            Some("Wait for the identity reset to finish before signing out a session.".to_owned());
        true
    }

    /// The core's list of devices.
    pub(crate) fn on_devices_event(&mut self, list: Vec<DeviceSummary>) {
        let model = &mut self.sessions;
        let list = arrange(list);
        model.loading = false;
        // A row that is gone ends its edit and its confirmation.
        let gone = |id: &DeviceId| !list.iter().any(|d| d.device_id == *id);
        if model.rename.as_ref().is_some_and(|e| gone(&e.device_id)) {
            model.rename = None;
        }
        if matches!(&model.confirm, Some(SignOutAsk::One(id)) if gone(id)) {
            model.confirm = None;
        }
        // The sign-out worked once none of its devices is left.
        if model
            .pending
            .as_ref()
            .is_some_and(|ids| ids.iter().all(&gone))
        {
            model.end_sign_out();
        }
        model.devices = Some(list);
    }

    /// `ReauthRequired { DeleteDevices }`: only for a sign-out the user sent.
    ///
    /// With the screen closed the sign-out ends here, quietly: no prompt
    /// appears on its own, and the user can start the sign-out again.
    pub(crate) fn on_sessions_reauth(&mut self) -> Task<Message> {
        if self.sessions.pending.is_none() {
            return Task::none();
        }
        if !self.sessions.open {
            self.sessions.end_sign_out();
            return Task::none();
        }
        self.sessions.reauth = Some(ReauthPrompt::new(ReauthAction::DeleteDevices));
        iced::widget::operation::focus(REAUTH_INPUT_ID)
    }

    /// `ActionFailed { Devices }`: a list, a rename or a trust watch. It never
    /// ends a sign-out; that is [`Self::on_sign_out_failed`]'s.
    pub(crate) fn on_devices_failed(&mut self, error: &CoreError) {
        let model = &mut self.sessions;
        model.loading = false;
        model.notice = Some(failure_line(error));
    }

    /// `ActionFailed { SignOutDevices }`: whatever the error, the sign-out is
    /// over. A refused password with the prompt open keeps the prompt.
    pub(crate) fn on_sign_out_failed(&mut self, error: &CoreError) {
        let model = &mut self.sessions;
        if let (CoreError::ReauthFailed, Some(prompt)) = (error, model.reauth.as_mut()) {
            // The prompt stays, so the password can be typed again.
            prompt.waiting = false;
            prompt.failed = true;
            prompt.password = Password::from(String::new());
            return;
        }
        model.end_sign_out();
        model.notice = Some(failure_line(error));
    }

    /// `OpenAccountManagement` for a sign-out: checked now, opened only after
    /// the user agrees, and only over `https`.
    pub(crate) fn on_sessions_url(&mut self, url: &AuthUrl) -> Task<Message> {
        let model = &mut self.sessions;
        if model.pending.is_none() {
            return Task::none();
        }
        model.end_sign_out();
        let checked = login::check_browser_url(url.expose())
            .ok()
            .and_then(|browser| {
                let link = links::check_link_url(url.expose()).ok()?;
                (link.kind() == LinkKind::Https).then(|| Approval {
                    url: browser,
                    host: link.host().to_owned(),
                })
            });
        match checked {
            Some(approval) => model.approval = Some(approval),
            None => {
                model.approval = None;
                model.notice =
                    Some("The account page was not opened because its address is not safe.".into());
            }
        }
        Task::none()
    }

    /// Sends `Logout` after dropping the secrets.
    pub(crate) fn logout(&mut self) -> Task<Message> {
        let flush = self.drop_secrets();
        Task::batch([flush, self.send(Command::Logout)])
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use tessera_core::api::{CrossSigning, DeviceVerification, ReauthAction};

    use super::*;

    fn dev(id: &str, name: Option<&str>, current: bool, trust: DeviceTrust) -> DeviceSummary {
        DeviceSummary {
            device_id: DeviceId::from(id),
            display_name: name.map(str::to_owned),
            last_seen_ms: Some(0),
            current,
            trust,
        }
    }

    fn status(recovery: RecoveryStatus) -> SecurityStatus {
        SecurityStatus {
            recovery,
            device: DeviceVerification::Verified,
            cross_signing: CrossSigning::Complete,
            has_devices_to_verify_against: false,
            identity_exists: true,
        }
    }

    #[test]
    fn the_logout_warning_table() {
        use RecoveryStatus::{Disabled, Enabled, Incomplete, Unknown};
        // (recovery, other devices, want)
        let cases = [
            (Enabled, Some(0), false),
            (Enabled, None, false),
            (Disabled, Some(0), true),
            (Incomplete, Some(0), true),
            (Disabled, None, true),
            (Incomplete, None, true),
            (Disabled, Some(1), false),
            (Incomplete, Some(3), false),
            (Unknown, Some(0), false),
            (Unknown, None, false),
        ];
        for (recovery, others, want) in cases {
            assert_eq!(
                logout_needs_warning(&status(recovery), others),
                want,
                "{recovery:?} others={others:?}"
            );
        }
    }

    #[test]
    fn a_title_is_the_clean_name_or_the_device_id() {
        let named = dev("ABCDEF", Some("Laptop"), false, DeviceTrust::Verified);
        assert_eq!(device_title(&named), "Laptop");
        let none = dev("ABCDEF", None, false, DeviceTrust::Verified);
        assert_eq!(device_title(&none), "ABCDEF");
        let blank = dev(
            "ABCDEF",
            Some(" \u{200B} \u{00A0}"),
            false,
            DeviceTrust::Verified,
        );
        assert_eq!(device_title(&blank), "ABCDEF");
        // Bidi and control characters are gone, and the name is one line.
        let evil = dev(
            "ABCDEF",
            Some("Phone\u{202E}gnp.exe\nsecond line"),
            false,
            DeviceTrust::Verified,
        );
        let title = device_title(&evil);
        assert!(
            !title.contains('\u{202E}') && !title.contains('\n'),
            "{title}"
        );
        assert!(!title.is_empty());
        // A huge name is capped.
        let huge = dev(
            "ABCDEF",
            Some(&"x".repeat(1000)),
            false,
            DeviceTrust::Verified,
        );
        assert!(device_title(&huge).chars().count() <= MAX_SHOWN_CHARS);
        assert_ne!(device_title(&huge), NAME_PLACEHOLDER);
    }

    #[test]
    fn a_device_id_keeps_its_end_when_long() {
        assert_eq!(device_id_line(&DeviceId::from("ABCDEF")), "ABCDEF");
        let long = DeviceId::from(format!("{}TAIL", "a".repeat(300)));
        let line = device_id_line(&long);
        assert!(line.ends_with("TAIL"), "{line}");
        assert!(line.chars().count() <= MAX_SHOWN_CHARS);
        assert!(!device_id_line(&DeviceId::from("A\nB")).contains('\n'));
    }

    #[test]
    fn the_badge_and_the_verify_offer() {
        assert_eq!(trust_label(DeviceTrust::Verified), "Verified");
        assert_eq!(trust_label(DeviceTrust::Unverified), "Not verified");
        assert_eq!(trust_label(DeviceTrust::NoKeys), "No encryption keys");
        let other = |trust| dev("D", None, false, trust);
        assert!(can_verify(&other(DeviceTrust::Unverified)));
        assert!(!can_verify(&other(DeviceTrust::Verified)));
        assert!(!can_verify(&other(DeviceTrust::NoKeys)));
        // Never for this device.
        assert!(!can_verify(&dev("D", None, true, DeviceTrust::Unverified)));
    }

    #[test]
    fn last_active_uses_the_date_labels() {
        let utc = FixedOffset::east_opt(0).expect("UTC");
        let now = 1_790_000_000_000;
        let text = last_active_text(Some(now), now, &utc);
        assert!(text.starts_with("Last active"), "{text}");
        assert!(text.contains("Today"), "{text}");
        assert!(text.contains(':'), "{text}");
        let unknown = last_active_text(None, now, &utc);
        assert_eq!(unknown, "Last active: unknown");
    }

    #[test]
    fn sign_out_ids_are_deduplicated_in_order() {
        let ids = ["B", "A", "B", "C", "A"].map(DeviceId::from).to_vec();
        assert_eq!(dedup_ids(ids), ["B", "A", "C"].map(DeviceId::from).to_vec());
        assert!(dedup_ids(Vec::new()).is_empty());
    }

    #[test]
    fn debug_hides_the_password_and_the_url() {
        let mut m = SessionsModel::default();
        let mut prompt = ReauthPrompt::new(ReauthAction::DeleteDevices);
        prompt.password = Password::from("secret-pw-x");
        m.reauth = Some(prompt);
        m.approval = Some(Approval {
            url: login::check_browser_url("https://auth.example.org/a?state=secret-state-x")
                .expect("https"),
            host: "auth.example.org".into(),
        });
        let dbg = format!("{m:?} {:?}", m.approval);
        assert!(!dbg.contains("secret-"), "{dbg}");
    }

    #[test]
    fn the_recovery_line_follows_the_recovery_state() {
        assert_eq!(
            recovery_line(RecoveryStatus::Enabled),
            Some(("Recovery is on.", Some(RecoveryAction::ChangeKey)))
        );
        assert_eq!(
            recovery_line(RecoveryStatus::Disabled),
            Some(("Recovery is off.", Some(RecoveryAction::SetUp)))
        );
        assert_eq!(
            recovery_line(RecoveryStatus::Incomplete),
            Some((
                "This device isn't connected to your recovery.",
                Some(RecoveryAction::EnterKey)
            ))
        );
        assert_eq!(recovery_line(RecoveryStatus::Unknown), None);
        assert_eq!(RecoveryAction::ChangeKey.label(), "Change recovery key");
        assert_eq!(RecoveryAction::SetUp.label(), "Set up recovery");
        assert_eq!(RecoveryAction::EnterKey.label(), "Enter recovery key");
        assert!(matches!(
            RecoveryAction::ChangeKey.message(),
            SecurityMessage::ChangeKeyRequested
        ));
        assert!(matches!(
            RecoveryAction::SetUp.message(),
            SecurityMessage::SetupRecovery
        ));
        assert!(matches!(
            RecoveryAction::EnterKey.message(),
            SecurityMessage::OpenRestore
        ));
    }
}
