//! Security state of the app: the banner, the Confirm-your-identity and
//! recovery screens (plan Q5, Q6). Pure: nothing here sends a command.

use std::fmt;
use std::time::Duration;

use iced::widget::operation;
use iced::{Task, clipboard};
use tessera_core::api::{
    ActionKind, AuthUrl, Command, CoreError, CoreEvent, DeviceVerification, Password, ReauthAction,
    RecoveryKey, RecoveryProgress, RecoveryStatus, SecurityStatus,
};

use crate::screens::login;
use crate::state::{App, Message};
use crate::text_safety;

/// Widget ID of the restore field.
pub const RESTORE_INPUT_ID: &str = "security-restore-input";
/// Widget ID of the password field.
pub const REAUTH_INPUT_ID: &str = "security-reauth-input";

/// Said after a reset that failed or was cancelled: the attempt deletes the
/// key backup (plan Q14), whatever came after.
pub const BACKUP_GONE: &str =
    "Your key backup was deleted by the reset attempt. Set up recovery again once you are in.";

/// The core's text for `EnableRecovery` on an account whose backup exists
/// (`security.rs` of the core); matched by this part only.
const BACKUP_EXISTS_PART: &str = "key backup already exists";

/// How long a copied recovery key may stay on the clipboard (plan Q5).
pub const CLIPBOARD_CLEAR_AFTER: Duration = Duration::from_secs(60);

/// A bar at the top of the main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Banner {
    /// This device is not verified. Never dismissible (plan Q6).
    DeviceUnverified,
    /// Recovery is not set up.
    RecoveryDisabled,
    /// Recovery is set up, but this device lacks the secrets.
    RecoveryIncomplete,
}

impl Banner {
    /// Whether the user may dismiss the banner for the session.
    pub fn dismissible(self) -> bool {
        !matches!(self, Self::DeviceUnverified)
    }

    /// The banner's text.
    pub fn text(self) -> &'static str {
        match self {
            Self::DeviceUnverified => {
                "This device isn't verified. Verify it to read your encrypted history and so \
                 others can trust it."
            }
            Self::RecoveryDisabled => {
                "Recovery isn't set up. Without it you can lose your encrypted messages if you \
                 sign out everywhere."
            }
            Self::RecoveryIncomplete => {
                "This device can't use your key backup yet. Enter your recovery key to read \
                 older encrypted messages."
            }
        }
    }
}

/// The banner to show, if any. A device that is `Unverified` always gives the
/// persistent banner, whatever `dismissed_recovery` says; the recovery banners
/// yield to it and to a dismissal; nothing is shown while the state is
/// `Unknown`.
pub fn banner(status: &SecurityStatus, dismissed_recovery: bool) -> Option<Banner> {
    if status.device == DeviceVerification::Unverified {
        return Some(Banner::DeviceUnverified);
    }
    if dismissed_recovery {
        return None;
    }
    match status.recovery {
        RecoveryStatus::Disabled => Some(Banner::RecoveryDisabled),
        RecoveryStatus::Incomplete => Some(Banner::RecoveryIncomplete),
        RecoveryStatus::Unknown | RecoveryStatus::Enabled => None,
    }
}

/// Whether the Confirm-your-identity screen opens by itself (plan Q6): a fresh
/// login (never a restored session) on an account that already has an
/// identity, with this device unverified, and the user has not skipped.
pub fn needs_identity_screen(status: &SecurityStatus, fresh_login: bool, skipped: bool) -> bool {
    fresh_login
        && !skipped
        && status.identity_exists
        && status.device == DeviceVerification::Unverified
}

/// Whether `status` is what a finished reset leaves: this device verified
/// by the new identity and the backup deleted (plan Q14). A status sent
/// before the reset began does not match, so it cannot end the wait early.
pub fn reset_finished(status: &SecurityStatus) -> bool {
    status.device == DeviceVerification::Verified && status.recovery == RecoveryStatus::Disabled
}

/// A new recovery key on the show-once screen.
pub struct RecoverySetup {
    /// The key. Dropped (and zeroized) on **Done**, logout and window close.
    pub key: RecoveryKey,
    /// "I've saved my recovery key" is ticked.
    pub saved_ticked: bool,
}

impl RecoverySetup {
    /// A screen for `key`, not ticked.
    pub fn new(key: RecoveryKey) -> Self {
        Self {
            key,
            saved_ticked: false,
        }
    }
}

impl fmt::Debug for RecoverySetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoverySetup")
            .field("saved_ticked", &self.saved_ticked)
            .finish_non_exhaustive()
    }
}

/// The screen can be left only after the box is ticked.
pub fn can_finish_setup(setup: &RecoverySetup) -> bool {
    setup.saved_ticked
}

/// Whether the clipboard still holds exactly `key` and must be cleared.
pub fn should_clear_clipboard(clipboard: Option<&str>, key: &RecoveryKey) -> bool {
    clipboard.is_some_and(|held| !key.expose().is_empty() && held == key.expose())
}

/// The password prompt of an action that needs re-authentication.
pub struct ReauthPrompt {
    /// The action the core refused until it has the password.
    pub action: ReauthAction,
    /// The password as typed.
    pub password: Password,
    /// The server refused the last password.
    pub failed: bool,
    /// The password was sent; waiting for the answer.
    pub waiting: bool,
}

impl ReauthPrompt {
    /// An empty prompt for `action`.
    pub fn new(action: ReauthAction) -> Self {
        Self {
            action,
            password: Password::from(String::new()),
            failed: false,
            waiting: false,
        }
    }
}

impl fmt::Debug for ReauthPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReauthPrompt")
            .field("action", &self.action)
            .field("failed", &self.failed)
            .field("waiting", &self.waiting)
            .finish_non_exhaustive()
    }
}

/// A full-window screen over the main view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// "Confirm your identity" (plan Q6).
    ConfirmIdentity,
    /// The field for a recovery key or passphrase (plan Q5).
    Restore,
    /// The warning before an identity reset.
    ResetConfirm,
}

/// What the core is busy with for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    /// Setting recovery up.
    SettingUp,
    /// Restoring from a key.
    Restoring,
    /// Resetting the identity. The core pauses sync meanwhile, so no event
    /// arrives for up to about two minutes with OAuth: silence is not "unchanged".
    Resetting,
}

/// The one security screen that is on top right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visible {
    /// None.
    Nothing,
    /// The show-once recovery key.
    Key,
    /// The password prompt.
    Reauth,
    /// An overlay.
    Overlay(Overlay),
}

/// The security part of the app state.
pub struct SecurityModel {
    /// The last status the core sent.
    pub status: SecurityStatus,
    /// Progress of setting recovery up.
    pub progress: Option<RecoveryProgress>,
    /// The show-once key screen.
    pub setup: Option<RecoverySetup>,
    /// The restore field (secure); kept after a failed restore so it can be
    /// corrected, emptied on success, logout and window close.
    pub restore_input: RecoveryKey,
    /// **Skip for now** was pressed on the Confirm-your-identity screen.
    pub identity_screen_skipped: bool,
    /// The recovery banner was dismissed for this session.
    pub dismissed_recovery_banner: bool,
    /// The password prompt.
    pub reauth: Option<ReauthPrompt>,
    /// The overlay, when one is open.
    pub overlay: Option<Overlay>,
    /// What the core is working on.
    pub busy: Option<Busy>,
    /// The login was fresh, not a restored session.
    pub fresh_login: bool,
    /// A non-secret line about the last failure or outcome.
    pub notice: Option<String>,
    /// A reset was started and has not ended: its backup deletion is not undone
    /// by a failure.
    pub reset_started: bool,
    /// The browser was opened for an OAuth approval of the reset.
    pub waiting_approval: bool,
    /// The key last copied, until the clipboard timer has checked it.
    pub copied: Option<RecoveryKey>,
    /// Counts copies, so only the last clipboard timer acts.
    pub copy_seq: u64,
    /// "Verify" was sent to another device and nothing came back yet.
    pub asked_other_device: bool,
}

impl Default for SecurityModel {
    fn default() -> Self {
        Self {
            status: SecurityStatus::default(),
            progress: None,
            setup: None,
            restore_input: RecoveryKey::from(String::new()),
            identity_screen_skipped: false,
            dismissed_recovery_banner: false,
            reauth: None,
            overlay: None,
            busy: None,
            fresh_login: false,
            notice: None,
            reset_started: false,
            waiting_approval: false,
            copied: None,
            copy_seq: 0,
            asked_other_device: false,
        }
    }
}

impl SecurityModel {
    /// The banner to show now.
    pub fn banner(&self) -> Option<Banner> {
        banner(&self.status, self.dismissed_recovery_banner)
    }

    /// The screen on top: the key, then the password prompt, then an overlay.
    pub fn visible(&self) -> Visible {
        if self.setup.is_some() {
            Visible::Key
        } else if self.reauth.is_some() {
            Visible::Reauth
        } else if let Some(overlay) = self.overlay {
            Visible::Overlay(overlay)
        } else {
            Visible::Nothing
        }
    }

    /// Where **Back** goes: the identity screen while the device is unverified.
    pub fn back_target(&self) -> Option<Overlay> {
        (self.status.device == DeviceVerification::Unverified).then_some(Overlay::ConfirmIdentity)
    }

    /// Drops the key, the inputs and the copy (logout, window close).
    pub fn clear_secrets(&mut self) {
        self.setup = None;
        self.reauth = None;
        self.copied = None;
        self.restore_input = RecoveryKey::from(String::new());
    }
}

// Manual impl: the key, the inputs and the password stay out of `Debug`.
impl fmt::Debug for SecurityModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecurityModel")
            .field("status", &self.status)
            .field("setup", &self.setup)
            .field("reauth", &self.reauth)
            .field("overlay", &self.overlay)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

/// What the user can do on the security screens and the banner.
#[derive(Debug, Clone)]
pub enum SecurityMessage {
    /// **Set up recovery**.
    SetupRecovery,
    /// **Copy** on the key screen.
    CopyKey,
    /// The 60 s clipboard timer of copy number `n` ended.
    ClipboardDue(u64),
    /// The clipboard's contents, read for the check.
    ClipboardRead(Option<RecoveryKey>),
    /// The clipboard's contents (first) read at once because the key (second)
    /// is being dropped; cleared if it still holds that key.
    ClipboardFlushed(Option<RecoveryKey>, RecoveryKey),
    /// "I've saved my recovery key" was toggled.
    SavedToggled(bool),
    /// **Done** on the key screen.
    Done,
    /// The restore field changed; its `Debug` is redacted.
    RestoreInput(RecoveryKey),
    /// **Restore**.
    RestoreSubmitted,
    /// **Enter recovery key** (banner).
    OpenRestore,
    /// **Verify** (banner): reopens the Confirm-your-identity screen.
    OpenConfirmIdentity,
    /// **Use another device**.
    UseOtherDevice,
    /// **Skip for now**.
    SkipIdentity,
    /// **Back**.
    Back,
    /// **Can't confirm? Reset your identity**.
    ResetRequested,
    /// **Reset identity** after the warning.
    ResetConfirmed,
    /// The password prompt changed; its `Debug` is redacted.
    ReauthInput(Password),
    /// **Continue** on the password prompt.
    ReauthSubmitted,
    /// **Cancel** on the password prompt.
    ReauthCancelled,
    /// **Cancel** while waiting for the approval in the browser.
    CancelApproval,
    /// The browser launcher finished.
    BrowserOpened(bool),
    /// The recovery banner's cross.
    DismissRecoveryBanner,
    /// The cross on the security notice.
    DismissNotice,
}

/// Whether `error` from `EnableRecovery` says a backup already exists.
pub fn is_backup_exists(error: &CoreError) -> bool {
    matches!(error, CoreError::Internal(text) if text.contains(BACKUP_EXISTS_PART))
}

/// The one-line text of a core failure. The core's own texts are fixed.
fn failure_line(error: &CoreError) -> String {
    let text = match error {
        CoreError::Internal(text) => text.clone(),
        other => other.to_string(),
    };
    text_safety::one_line_capped(&text, 512)
}

impl App {
    /// Applies a message of the security screens.
    pub(crate) fn on_security(&mut self, message: SecurityMessage) -> Task<Message> {
        use SecurityMessage as M;
        match message {
            M::SetupRecovery => {
                let model = &mut self.security;
                if model.busy.is_some() || model.setup.is_some() {
                    return Task::none();
                }
                model.notice = None;
                model.progress = None;
                model.busy = Some(Busy::SettingUp);
                self.send(Command::EnableRecovery)
            }
            M::CopyKey => {
                let Some(setup) = &self.security.setup else {
                    return Task::none();
                };
                let key = setup.key.clone();
                let text = key.expose().to_owned();
                self.security.copied = Some(key);
                self.security.copy_seq = self.security.copy_seq.wrapping_add(1);
                let due = Message::Security(M::ClipboardDue(self.security.copy_seq));
                Task::batch([
                    clipboard::write(text),
                    self.timer(CLIPBOARD_CLEAR_AFTER, due),
                ])
            }
            M::ClipboardDue(seq) => {
                if seq != self.security.copy_seq || self.security.copied.is_none() {
                    return Task::none();
                }
                clipboard::read()
                    .map(|held| Message::Security(M::ClipboardRead(held.map(RecoveryKey::from))))
            }
            M::ClipboardRead(held) => {
                let Some(key) = self.security.copied.take() else {
                    return Task::none();
                };
                if should_clear_clipboard(held.as_ref().map(RecoveryKey::expose), &key) {
                    clipboard::write(String::new())
                } else {
                    Task::none()
                }
            }
            M::ClipboardFlushed(held, key) => {
                self.flushes_pending = self.flushes_pending.saturating_sub(1);
                let clear = if should_clear_clipboard(held.as_ref().map(RecoveryKey::expose), &key)
                {
                    clipboard::write(String::new())
                } else {
                    Task::none()
                };
                self.exit_if_flushed(clear)
            }
            M::SavedToggled(ticked) => {
                if let Some(setup) = &mut self.security.setup {
                    setup.saved_ticked = ticked;
                }
                Task::none()
            }
            M::Done => {
                if self.security.setup.as_ref().is_some_and(can_finish_setup) {
                    self.security.setup = None;
                }
                Task::none()
            }
            M::RestoreInput(input) => {
                self.security.restore_input = input;
                Task::none()
            }
            M::RestoreSubmitted => {
                let typed = self.security.restore_input.expose().trim();
                if self.security.busy.is_some() || typed.is_empty() {
                    return Task::none();
                }
                let key = RecoveryKey::from(typed);
                self.security.notice = None;
                self.security.busy = Some(Busy::Restoring);
                // The field keeps the text until the answer: a wrong key can
                // be corrected; success, logout and close empty it.
                self.send(Command::Recover { key })
            }
            M::OpenRestore => {
                self.security.overlay = Some(Overlay::Restore);
                self.security.notice = None;
                operation::focus(RESTORE_INPUT_ID)
            }
            M::OpenConfirmIdentity => {
                self.security.overlay = Some(Overlay::ConfirmIdentity);
                self.security.notice = None;
                self.security.asked_other_device = false;
                Task::none()
            }
            M::UseOtherDevice => {
                if self.security.asked_other_device {
                    return Task::none();
                }
                self.security.asked_other_device = true;
                self.send(Command::RequestSelfVerification)
            }
            M::SkipIdentity => {
                self.security.identity_screen_skipped = true;
                self.security.overlay = None;
                Task::none()
            }
            M::Back => {
                if self.security.busy.is_none() {
                    self.security.overlay = self.security.back_target();
                    self.security.notice = None;
                    self.security.restore_input = RecoveryKey::from(String::new());
                }
                Task::none()
            }
            M::ResetRequested => {
                self.security.overlay = Some(Overlay::ResetConfirm);
                self.security.notice = None;
                Task::none()
            }
            M::ResetConfirmed => {
                let model = &mut self.security;
                if model.busy.is_some() || model.overlay != Some(Overlay::ResetConfirm) {
                    return Task::none();
                }
                model.notice = None;
                model.reset_started = true;
                model.busy = Some(Busy::Resetting);
                self.send(Command::ResetIdentity { password: None })
            }
            M::ReauthInput(password) => {
                if let Some(prompt) = self.security.reauth.as_mut().filter(|p| !p.waiting) {
                    prompt.password = password;
                }
                Task::none()
            }
            M::ReauthSubmitted => self.submit_reauth(),
            M::ReauthCancelled => {
                let model = &mut self.security;
                model.reauth = None;
                model.busy = None;
                if std::mem::take(&mut model.reset_started) {
                    model.notice = Some(format!("Reset cancelled. {BACKUP_GONE}"));
                }
                Task::none()
            }
            M::CancelApproval => {
                let model = &mut self.security;
                if !std::mem::take(&mut model.waiting_approval) {
                    return Task::none();
                }
                model.busy = None;
                model.reset_started = false;
                model.notice = Some(format!("Reset cancelled. {BACKUP_GONE}"));
                self.send(Command::CancelIdentityReset)
            }
            M::BrowserOpened(ok) => {
                if !ok && self.security.waiting_approval {
                    self.security.notice =
                        Some("Your browser could not be started. Cancel and try again.".into());
                }
                Task::none()
            }
            M::DismissRecoveryBanner => {
                if self.security.banner().is_some_and(Banner::dismissible) {
                    self.security.dismissed_recovery_banner = true;
                }
                Task::none()
            }
            M::DismissNotice => {
                self.security.notice = None;
                Task::none()
            }
        }
    }

    /// Drops the key, the inputs and the copy, and checks the clipboard at
    /// once: a copied key still on it is cleared now, not after 60 s (the
    /// timer would find nothing to compare with). Returns that task.
    pub(crate) fn drop_secrets(&mut self) -> Task<Message> {
        let copied = self.security.copied.take();
        self.security.clear_secrets();
        // The sign-out password prompt: dropping it zeroizes the field.
        self.sessions.reauth = None;
        match copied {
            Some(key) => {
                self.flushes_pending += 1;
                clipboard::read().map(move |held| {
                    Message::Security(SecurityMessage::ClipboardFlushed(
                        held.map(RecoveryKey::from),
                        key.clone(),
                    ))
                })
            }
            None => Task::none(),
        }
    }

    /// Sends the typed password with the action that asked for it.
    fn submit_reauth(&mut self) -> Task<Message> {
        let Some(prompt) = self.security.reauth.as_mut() else {
            return Task::none();
        };
        if prompt.waiting || prompt.password.expose().is_empty() {
            return Task::none();
        }
        let password = std::mem::replace(&mut prompt.password, Password::from(String::new()));
        let command = match prompt.action {
            ReauthAction::ResetIdentity => Command::ResetIdentity {
                password: Some(password),
            },
            ReauthAction::BootstrapCrossSigning => Command::BootstrapCrossSigning {
                password: Some(password),
            },
            // Task 13 (Sessions) asks for its own password.
            ReauthAction::DeleteDevices => return Task::none(),
        };
        prompt.waiting = true;
        prompt.failed = false;
        self.security.busy = Some(Busy::Resetting);
        self.send(command)
    }

    /// Handles the security events of the core; any other event changes
    /// nothing.
    pub(crate) fn on_security_event(&mut self, event: CoreEvent) -> Task<Message> {
        match event {
            CoreEvent::Security(status) => self.on_status(status),
            CoreEvent::RecoveryProgress(progress) => {
                if self.security.busy == Some(Busy::SettingUp) {
                    self.security.progress = Some(progress);
                }
                Task::none()
            }
            CoreEvent::RecoveryKeyReady(key) => {
                let model = &mut self.security;
                model.setup = Some(RecoverySetup::new(key));
                model.busy = None;
                model.progress = None;
                model.notice = None;
                Task::none()
            }
            CoreEvent::Recovered => {
                let model = &mut self.security;
                model.restore_input = RecoveryKey::from(String::new());
                model.busy = None;
                model.notice = None;
                if model.overlay == Some(Overlay::Restore) {
                    model.overlay = None;
                }
                Task::none()
            }
            CoreEvent::ReauthRequired {
                action: action @ (ReauthAction::ResetIdentity | ReauthAction::BootstrapCrossSigning),
            } => {
                self.security.busy = None;
                self.security.reauth = Some(ReauthPrompt::new(action));
                operation::focus(REAUTH_INPUT_ID)
            }
            CoreEvent::OpenAccountManagement(url) => self.on_approval_url(&url),
            CoreEvent::ActionFailed {
                action: ActionKind::Recovery,
                error,
                ..
            } => self.on_recovery_failed(&error),
            CoreEvent::ActionFailed {
                action: ActionKind::CrossSigning,
                error,
                ..
            } => {
                self.on_cross_signing_failed(&error);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn on_status(&mut self, status: SecurityStatus) -> Task<Message> {
        let model = &mut self.security;
        let was_restore = model.overlay == Some(Overlay::Restore);
        model.status = status;
        if model.busy == Some(Busy::Resetting) && reset_finished(&status) {
            // The core holds the reset's own event until the reset is over;
            // an earlier status must not end the wait.
            model.busy = None;
            model.reset_started = false;
            model.waiting_approval = false;
            model.reauth = None;
            model.notice = None;
            if model.overlay == Some(Overlay::ResetConfirm) {
                model.overlay = None;
            }
        }
        if status.recovery == RecoveryStatus::Enabled {
            model.progress = None;
        }
        if status.device == DeviceVerification::Verified
            && model.overlay == Some(Overlay::ConfirmIdentity)
        {
            model.overlay = None;
        }
        if model.overlay.is_none()
            && model.setup.is_none()
            && needs_identity_screen(&status, model.fresh_login, model.identity_screen_skipped)
        {
            model.overlay = Some(Overlay::ConfirmIdentity);
        }
        if was_restore && model.overlay != Some(Overlay::Restore) {
            model.restore_input = RecoveryKey::from(String::new());
        }
        Task::none()
    }

    fn on_recovery_failed(&mut self, error: &CoreError) -> Task<Message> {
        let model = &mut self.security;
        model.busy = None;
        model.progress = None;
        model.notice = Some(failure_line(error));
        if is_backup_exists(error) {
            model.overlay = Some(Overlay::Restore);
            return operation::focus(RESTORE_INPUT_ID);
        }
        Task::none()
    }

    fn on_cross_signing_failed(&mut self, error: &CoreError) {
        let model = &mut self.security;
        let reset = std::mem::take(&mut model.reset_started);
        model.busy = None;
        model.waiting_approval = false;
        let line = match (error, model.reauth.as_mut()) {
            (CoreError::ReauthFailed, Some(prompt)) => {
                // The prompt stays, so the password can be typed again.
                prompt.waiting = false;
                prompt.failed = true;
                prompt.password = Password::from(String::new());
                model.reset_started = reset;
                "That password was not accepted.".to_owned()
            }
            _ => {
                model.reauth = None;
                if model.overlay == Some(Overlay::ResetConfirm) {
                    model.overlay = model.back_target();
                }
                failure_line(error)
            }
        };
        model.notice = Some(if reset {
            format!("{line} {BACKUP_GONE}")
        } else {
            line
        });
    }

    /// Opens the OAuth approval page of a reset, only from the discovered
    /// `https` endpoint; otherwise cancels the reset.
    fn on_approval_url(&mut self, url: &AuthUrl) -> Task<Message> {
        if self.security.busy != Some(Busy::Resetting) {
            // Not ours (Task 13 handles the Sessions screen).
            return Task::none();
        }
        match login::check_browser_url(url.expose()) {
            Ok(link) if link.as_str().starts_with("https://") => {
                self.security.waiting_approval = true;
                Task::perform(async move { login::open_in_browser(&link) }, |ok| {
                    Message::Security(SecurityMessage::BrowserOpened(ok))
                })
            }
            _ => {
                let model = &mut self.security;
                model.busy = None;
                model.reset_started = false;
                model.notice = Some(format!(
                    "The approval page was not opened because its address is not safe. {BACKUP_GONE}"
                ));
                self.send(Command::CancelIdentityReset)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tessera_core::api::{
        CrossSigning, DeviceVerification, RecoveryKey, RecoveryStatus, SecurityStatus,
    };

    use super::*;

    fn status(device: DeviceVerification, recovery: RecoveryStatus) -> SecurityStatus {
        SecurityStatus {
            recovery,
            device,
            cross_signing: CrossSigning::Complete,
            has_devices_to_verify_against: true,
            identity_exists: true,
        }
    }

    #[test]
    fn the_banner_table() {
        use DeviceVerification::{Unknown as DUnknown, Unverified, Verified};
        use RecoveryStatus::{Disabled, Enabled, Incomplete, Unknown};
        let cases = [
            (Unverified, Enabled, false, Some(Banner::DeviceUnverified)),
            // Q6: the unverified banner cannot be dismissed.
            (Unverified, Disabled, true, Some(Banner::DeviceUnverified)),
            (Unverified, Enabled, true, Some(Banner::DeviceUnverified)),
            (Verified, Disabled, false, Some(Banner::RecoveryDisabled)),
            (
                Verified,
                Incomplete,
                false,
                Some(Banner::RecoveryIncomplete),
            ),
            (Verified, Disabled, true, None),
            (Verified, Incomplete, true, None),
            (Verified, Enabled, false, None),
            (Verified, Unknown, false, None),
            (DUnknown, Disabled, false, Some(Banner::RecoveryDisabled)),
            (DUnknown, Unknown, false, None),
        ];
        for (device, recovery, dismissed, want) in cases {
            assert_eq!(
                banner(&status(device, recovery), dismissed),
                want,
                "{device:?} {recovery:?} dismissed={dismissed}"
            );
        }
        assert_eq!(banner(&SecurityStatus::default(), false), None);
        assert!(!Banner::DeviceUnverified.dismissible());
        assert!(Banner::RecoveryDisabled.dismissible());
        assert!(Banner::RecoveryIncomplete.dismissible());
    }

    #[test]
    fn the_identity_screen_table() {
        use DeviceVerification::{Unknown, Unverified, Verified};
        let mut s = status(Unverified, RecoveryStatus::Enabled);
        // (device, identity exists, fresh login, skipped, want)
        let cases = [
            (Unverified, true, true, false, true),
            (Unverified, true, true, true, false),
            (Unverified, true, false, false, false),
            (Unverified, false, true, false, false),
            (Verified, true, true, false, false),
            (Unknown, true, true, false, false),
        ];
        for (device, identity, fresh, skipped, want) in cases {
            s.device = device;
            s.identity_exists = identity;
            assert_eq!(
                needs_identity_screen(&s, fresh, skipped),
                want,
                "{device:?} identity={identity} fresh={fresh} skipped={skipped}"
            );
        }
    }

    #[test]
    fn setup_finishes_only_when_ticked() {
        let mut setup = RecoverySetup::new(RecoveryKey::from("secret-key-x"));
        assert!(!can_finish_setup(&setup));
        setup.saved_ticked = true;
        assert!(can_finish_setup(&setup));
    }

    #[test]
    fn the_clipboard_is_cleared_only_while_it_still_holds_the_key() {
        let key = RecoveryKey::from("EsTc aaaa bbbb");
        assert!(should_clear_clipboard(Some("EsTc aaaa bbbb"), &key));
        assert!(!should_clear_clipboard(Some("something else"), &key));
        assert!(!should_clear_clipboard(Some("EsTc aaaa bbbb "), &key));
        assert!(!should_clear_clipboard(Some(""), &key));
        assert!(!should_clear_clipboard(None, &key));
    }

    #[test]
    fn the_visible_screen_follows_its_priority() {
        let mut m = SecurityModel::default();
        assert_eq!(m.visible(), Visible::Nothing);
        m.overlay = Some(Overlay::ConfirmIdentity);
        assert_eq!(m.visible(), Visible::Overlay(Overlay::ConfirmIdentity));
        m.reauth = Some(ReauthPrompt::new(
            tessera_core::api::ReauthAction::ResetIdentity,
        ));
        assert_eq!(m.visible(), Visible::Reauth);
        m.setup = Some(RecoverySetup::new(RecoveryKey::from("secret-key-x")));
        assert_eq!(m.visible(), Visible::Key);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)] // the model has no const-friendly constructor with these fields
    fn debug_prints_no_key_input_or_password() {
        let mut m = SecurityModel::default();
        m.setup = Some(RecoverySetup::new(RecoveryKey::from("secret-key-x")));
        m.restore_input = RecoveryKey::from("secret-input-x");
        m.copied = Some(RecoveryKey::from("secret-copied-x"));
        let mut prompt = ReauthPrompt::new(tessera_core::api::ReauthAction::ResetIdentity);
        prompt.password = tessera_core::api::Password::from("secret-pw-x");
        m.reauth = Some(prompt);
        let dbg = format!("{m:?}");
        for secret in [
            "secret-key-x",
            "secret-input-x",
            "secret-copied-x",
            "secret-pw-x",
        ] {
            assert!(!dbg.contains(secret), "{dbg}");
        }
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)] // as above: sets secret fields one by one
    fn clearing_secrets_drops_the_key_the_inputs_and_the_copy() {
        let mut m = SecurityModel::default();
        m.setup = Some(RecoverySetup::new(RecoveryKey::from("secret-key-x")));
        m.restore_input = RecoveryKey::from("secret-input-x");
        m.copied = Some(RecoveryKey::from("secret-copied-x"));
        m.reauth = Some(ReauthPrompt::new(
            tessera_core::api::ReauthAction::ResetIdentity,
        ));
        m.clear_secrets();
        assert!(m.setup.is_none() && m.reauth.is_none() && m.copied.is_none());
        assert_eq!(m.restore_input.expose(), "");
    }
}
