//! Security status, cross-signing bootstrap, identity reset and
//! re-authentication (plan Q8, Q13, Q14). Private to the core: SDK types stay
//! in here.
//!
//! Nothing here logs or prints a password, a UIA session or an approval URL.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt as _;
use futures_util::stream::{BoxStream, select_all};
use matrix_sdk::Client;
use matrix_sdk::encryption::recovery::{IdentityResetHandle, RecoveryError, RecoveryState};
use matrix_sdk::encryption::{CrossSigningResetAuthType, CrossSigningStatus, VerificationState};
use matrix_sdk::ruma::UserId;
use matrix_sdk::ruma::api::client::uiaa::{
    AuthData, AuthType, MatrixUserIdentifier, Password as UiaaPassword, UiaaInfo, UserIdentifier,
};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::task::{JoinHandle, JoinSet};

use crate::api::{
    ActionKind, CoreError, CoreEvent, CrossSigning, DeviceVerification, Password, ReauthAction,
    RecoveryStatus, RoomId, SecurityStatus,
};
use crate::oauth::check_auth_url;

/// What the status is computed from.
#[derive(Clone, Debug)]
pub(crate) struct StatusInputs {
    /// Secret storage and backup state.
    pub recovery: RecoveryState,
    /// Whether the own device is signed by the own identity.
    pub verification: VerificationState,
    /// Whether the account has a cross-signing identity; `None` when it
    /// could not be determined (not queried yet, or the query failed).
    pub identity_exists: Option<bool>,
    /// The private cross-signing keys held by this device; `None` without an
    /// Olm machine.
    pub private_keys: Option<CrossSigningStatus>,
    /// Another device of the account could verify this one.
    pub has_devices_to_verify_against: bool,
}

/// The API status for `inputs`.
pub(crate) fn compute_status(inputs: &StatusInputs) -> SecurityStatus {
    let recovery = match inputs.recovery {
        RecoveryState::Unknown => RecoveryStatus::Unknown,
        RecoveryState::Enabled => RecoveryStatus::Enabled,
        RecoveryState::Disabled => RecoveryStatus::Disabled,
        RecoveryState::Incomplete => RecoveryStatus::Incomplete,
    };
    let device = match inputs.verification {
        VerificationState::Unknown => DeviceVerification::Unknown,
        VerificationState::Verified => DeviceVerification::Verified,
        VerificationState::Unverified => DeviceVerification::Unverified,
    };
    let cross_signing = match (inputs.identity_exists, &inputs.private_keys) {
        (None, _) => CrossSigning::Unknown,
        (Some(false), _) => CrossSigning::NotSetUp,
        (Some(true), None) => CrossSigning::Unknown,
        (Some(true), Some(keys)) if keys.is_complete() => CrossSigning::Complete,
        (Some(true), Some(_)) => CrossSigning::MissingKeys,
    };
    SecurityStatus {
        recovery,
        device,
        cross_signing,
        has_devices_to_verify_against: inputs.has_devices_to_verify_against,
        identity_exists: inputs.identity_exists == Some(true),
    }
}

/// The next step of an identity reset (or of a first cross-signing upload).
#[derive(Debug)]
pub(crate) enum ResetStep {
    /// Nothing more to do.
    Done,
    /// The homeserver wants the password and none was given.
    NeedPassword,
    /// Retry with this authentication data.
    Retry(AuthData),
    /// The user must approve the reset at this URL; then poll.
    Approve(url::Url),
    /// The homeserver offers no way this core can authenticate with.
    Unsupported,
}

/// UIA data that proves the password of `user`, for the UIA `session`.
pub(crate) fn uia_password(user: &UserId, password: &Password, session: Option<&str>) -> AuthData {
    let identifier = UserIdentifier::Matrix(MatrixUserIdentifier::new(user.to_string()));
    let mut auth = UiaaPassword::new(identifier, password.expose().to_owned());
    auth.session = session.map(str::to_owned);
    AuthData::Password(auth)
}

/// Whether the homeserver offers a flow that is just the password stage.
fn offers_password(info: &UiaaInfo) -> bool {
    info.flows
        .iter()
        .any(|flow| matches!(flow.stages.as_slice(), [AuthType::Password]))
}

/// What to do about a UIA answer `info`: ask for a password, retry with the
/// one given, or give up when no flow takes a password.
fn uia_step(info: &UiaaInfo, user: &UserId, password: Option<&Password>) -> ResetStep {
    match (offers_password(info), password) {
        (false, _) => ResetStep::Unsupported,
        (true, None) => ResetStep::NeedPassword,
        (true, Some(password)) => {
            ResetStep::Retry(uia_password(user, password, info.session.as_deref()))
        }
    }
}

/// Decides what to do after `reset_identity` returned `auth` (`None`: the
/// reset finished without authentication).
pub(crate) fn reset_outcome(
    auth: Option<&CrossSigningResetAuthType>,
    user: &UserId,
    password: Option<&Password>,
) -> ResetStep {
    match auth {
        None => ResetStep::Done,
        Some(CrossSigningResetAuthType::Uiaa(info)) => uia_step(info, user, password),
        Some(CrossSigningResetAuthType::OAuth(info)) => {
            ResetStep::Approve(info.approval_url.clone())
        }
    }
}

/// Whether `info` says the password (or other credential) was refused, as
/// opposed to the first answer that merely lists the flows.
fn auth_rejected(info: Option<&UiaaInfo>) -> bool {
    info.is_some_and(|info| info.auth_error.is_some())
}

/// The core error for a failed re-authenticated call: `ReauthFailed` when the
/// homeserver refused the password, else a fixed message.
fn map_sdk_error(error: &matrix_sdk::Error, what: &'static str) -> CoreError {
    if auth_rejected(error.as_uiaa_response()) {
        CoreError::ReauthFailed
    } else {
        CoreError::Internal(what.into())
    }
}

/// Like [`map_sdk_error`] for the recovery subsystem's error type.
fn map_recovery_error(error: &RecoveryError, what: &'static str) -> CoreError {
    match error {
        RecoveryError::Sdk(e) => map_sdk_error(e, what),
        _ => CoreError::Internal(what.into()),
    }
}

/// Reads the current status from the client.
async fn read_status(client: &Client) -> SecurityStatus {
    let encryption = client.encryption();
    let Some(me) = client.user_id() else {
        return SecurityStatus::default();
    };
    // Also asks the server for the own identity when it is not known yet.
    let devices = encryption.has_devices_to_verify_against().await;
    let identity = encryption
        .get_user_identity(me)
        .await
        .ok()
        .map(|identity| identity.is_some());
    // "No identity" means something only when the server was asked.
    let identity_exists = match (identity, &devices) {
        (Some(false), Err(_)) => None,
        (known, _) => known,
    };
    compute_status(&StatusInputs {
        recovery: encryption.recovery().state(),
        verification: encryption.verification_state().get(),
        identity_exists,
        private_keys: encryption.cross_signing_status().await,
        has_devices_to_verify_against: devices.unwrap_or(false),
    })
}

/// One stream that yields whenever something that feeds the status changed.
async fn change_stream(client: &Client) -> BoxStream<'static, ()> {
    let encryption = client.encryption();
    let mut streams: Vec<BoxStream<'static, ()>> = vec![
        encryption.recovery().state_stream().map(drop).boxed(),
        encryption.verification_state().map(drop).boxed(),
    ];
    if let Ok(devices) = encryption.devices_stream().await {
        streams.push(devices.map(drop).boxed());
    }
    if let Ok(identities) = encryption.user_identities_stream().await {
        streams.push(identities.map(drop).boxed());
    }
    select_all(streams).boxed()
}

/// Sends `Security` once at the start, then only when the status changed.
/// `refresh` asks for a recomputation (after an action).
async fn forward_status(client: Client, events: mpsc::Sender<CoreEvent>, refresh: Arc<Notify>) {
    let mut changes = change_stream(&client).await;
    let mut changes_open = true;
    let mut last: Option<SecurityStatus> = None;
    loop {
        let status = read_status(&client).await;
        if last != Some(status) {
            last = Some(status);
            if events.send(CoreEvent::Security(status)).await.is_err() {
                return;
            }
        }
        tokio::select! {
            item = changes.next(), if changes_open => {
                if item.is_none() {
                    changes_open = false;
                }
            }
            () = refresh.notified() => {}
        }
    }
}

/// Why a security task stopped before it finished.
#[derive(Debug, PartialEq)]
enum Stop {
    /// The action needs the password.
    Reauth(ReauthAction),
    /// The action failed.
    Failed(CoreError),
}

impl From<CoreError> for Stop {
    fn from(error: CoreError) -> Self {
        Self::Failed(error)
    }
}

/// Marks an identity operation as running; at most one runs at a time.
struct IdentityPermit(Arc<IdentityOp>);

impl Drop for IdentityPermit {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
        self.0.cancelled.store(false, Ordering::Release);
    }
}

/// State shared by the identity operations of one session.
#[derive(Default)]
struct IdentityOp {
    /// An identity operation is running.
    busy: AtomicBool,
    /// `CancelIdentityReset` was received during this operation.
    cancelled: AtomicBool,
    /// The reset waiting for an OAuth approval, so that it can be cancelled.
    waiting: Mutex<Option<Arc<IdentityResetHandle>>>,
}

impl IdentityOp {
    fn try_start(self: &Arc<Self>) -> Option<IdentityPermit> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| {
                // A cancel that came while nothing ran does not apply.
                self.cancelled.store(false, Ordering::Release);
                IdentityPermit(self.clone())
            })
    }
}

/// The security tasks of one session: the status forwarder, and the network
/// actions (each runs in `tasks`, so `handle` never waits on the network).
pub(crate) struct SecurityTasks {
    forwarder: Option<JoinHandle<()>>,
    tasks: JoinSet<()>,
    refresh: Arc<Notify>,
    identity: Arc<IdentityOp>,
}

impl SecurityTasks {
    pub(crate) fn new() -> Self {
        Self {
            forwarder: None,
            tasks: JoinSet::new(),
            refresh: Arc::new(Notify::new()),
            identity: Arc::new(IdentityOp::default()),
        }
    }

    /// Starts the status forwarder (after login or restore).
    pub(crate) fn start(&mut self, client: &Client, events: &mpsc::Sender<CoreEvent>) {
        if let Some(old) = self.forwarder.take() {
            old.abort();
        }
        self.forwarder = Some(tokio::spawn(forward_status(
            client.clone(),
            events.clone(),
            self.refresh.clone(),
        )));
    }

    /// Ends the forwarder and every action, so that nothing holds the client
    /// afterwards.
    pub(crate) async fn stop(&mut self) {
        if let Some(forwarder) = self.forwarder.take() {
            forwarder.abort();
            // An aborted task ends with a cancellation error; that is expected.
            let _ = forwarder.await;
        }
        self.tasks.shutdown().await;
    }

    /// Runs one identity operation in the background. Without a permit the
    /// answer is `Busy`.
    fn spawn_identity<F, Fut>(&mut self, events: &mpsc::Sender<CoreEvent>, work: F)
    where
        F: FnOnce(IdentityPermit) -> Fut,
        Fut: std::future::Future<Output = Result<(), Stop>> + Send + 'static,
    {
        // Forget finished tasks, so the set does not grow with every action.
        while self.tasks.try_join_next().is_some() {}
        let events = events.clone();
        let permit = self.identity.try_start();
        let fut = permit.map(work);
        self.tasks.spawn(async move {
            let result = match fut {
                Some(fut) => fut.await,
                None => Err(Stop::Failed(CoreError::Busy)),
            };
            if let Err(stop) = result {
                report(&events, stop).await;
            }
        });
    }

    /// `BootstrapCrossSigning`: creates the identity when there is none.
    pub(crate) fn bootstrap(
        &mut self,
        client: &Client,
        password: Option<Password>,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        self.spawn_identity(events, move |_permit| async move {
            bootstrap_task(&client, password.as_ref()).await?;
            finish(&client, &refresh).await;
            Ok(())
        });
    }

    /// `ResetIdentity`: replaces the identity, deleting the backups.
    pub(crate) fn reset(
        &mut self,
        client: &Client,
        password: Option<Password>,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        let op = self.identity.clone();
        let events_for_task = events.clone();
        self.spawn_identity(events, move |_permit| async move {
            let result = reset_task(&client, &op, password.as_ref(), &events_for_task).await;
            // Also after a failure: the attempt made new private keys that
            // the server never accepted, and the query drops them again.
            finish(&client, &refresh).await;
            result
        });
    }

    /// `CancelIdentityReset`: stops a reset that waits for an approval.
    pub(crate) fn cancel_reset(&mut self) {
        let op = self.identity.clone();
        if !op.busy.load(Ordering::Acquire) {
            return;
        }
        op.cancelled.store(true, Ordering::Release);
        self.tasks.spawn(async move {
            let waiting = op.waiting.lock().await.clone();
            if let Some(handle) = waiting {
                handle.cancel().await;
            }
        });
    }
}

/// Sends the event that tells the user why `stop` ended an action.
async fn report(events: &mpsc::Sender<CoreEvent>, stop: Stop) {
    let event = match stop {
        Stop::Reauth(action) => CoreEvent::ReauthRequired { action },
        Stop::Failed(error) => CoreEvent::ActionFailed {
            // These actions belong to no room.
            room_id: RoomId::from(""),
            action: ActionKind::CrossSigning,
            error,
        },
    };
    // A dropped receiver just means nobody listens any more.
    let _ = events.send(event).await;
}

/// After a change of identity: asks the server for the own keys (so that the
/// device's new signature is seen) and has the status recomputed.
async fn finish(client: &Client, refresh: &Notify) {
    if let Some(me) = client.user_id() {
        // Best effort: the sync brings the same information a moment later.
        let _ = client.encryption().request_user_identity(me).await;
    }
    refresh.notify_one();
}

/// Creates the cross-signing identity when there is none (Q13).
async fn bootstrap_task(client: &Client, password: Option<&Password>) -> Result<(), Stop> {
    const FAILED: &str = "cross-signing could not be set up";
    let encryption = client.encryption();
    let error = match encryption.bootstrap_cross_signing_if_needed(None).await {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    let Some(info) = error.as_uiaa_response() else {
        return Err(CoreError::Internal(FAILED.into()).into());
    };
    let me = client.user_id().ok_or(CoreError::NotLoggedIn)?;
    match uia_step(info, me, password) {
        ResetStep::NeedPassword => Err(Stop::Reauth(ReauthAction::BootstrapCrossSigning)),
        ResetStep::Retry(auth) => {
            // The failed first attempt already made the keys; the upload
            // goes on directly with the authentication data.
            encryption
                .bootstrap_cross_signing(Some(auth))
                .await
                .map_err(|e| Stop::Failed(map_sdk_error(&e, FAILED)))
        }
        ResetStep::Done | ResetStep::Approve(_) | ResetStep::Unsupported => {
            Err(CoreError::Internal(FAILED.into()).into())
        }
    }
}

/// Replaces the identity (Q13, Q14).
async fn reset_task(
    client: &Client,
    op: &Arc<IdentityOp>,
    password: Option<&Password>,
    events: &mpsc::Sender<CoreEvent>,
) -> Result<(), Stop> {
    const FAILED: &str = "the identity could not be reset";
    let me = client.user_id().ok_or(CoreError::NotLoggedIn)?;
    // A keys query that answers with the old identity after the new keys were
    // made (and before they are uploaded) makes the SDK drop the new private
    // keys. Asking first leaves nothing pending, so the window is one upload.
    let _ = client.encryption().request_user_identity(me).await;
    let handle = client
        .encryption()
        .recovery()
        .reset_identity()
        .await
        .map_err(|e| Stop::Failed(map_recovery_error(&e, FAILED)))?;
    let step = reset_outcome(
        handle.as_ref().map(IdentityResetHandle::auth_type),
        me,
        password,
    );
    match (step, handle) {
        (ResetStep::Done, _) | (_, None) => Ok(()),
        (ResetStep::NeedPassword, _) => Err(Stop::Reauth(ReauthAction::ResetIdentity)),
        (ResetStep::Retry(auth), Some(handle)) => handle
            .reset(Some(auth))
            .await
            .map_err(|e| Stop::Failed(map_recovery_error(&e, FAILED))),
        (ResetStep::Approve(url), Some(handle)) => {
            // The URL comes from the server and the front end opens it.
            let url = check_auth_url(url.as_str())?;
            let handle = Arc::new(handle);
            *op.waiting.lock().await = Some(handle.clone());
            // A cancel that arrived before the handle was stored.
            if op.cancelled.load(Ordering::Acquire) {
                handle.cancel().await;
            }
            let _ = events.send(CoreEvent::OpenAccountManagement(url)).await;
            // Polls until the approval arrives, the 2 minutes are up, or the
            // reset is cancelled.
            let result = handle.reset(None).await;
            *op.waiting.lock().await = None;
            if op.cancelled.load(Ordering::Acquire) {
                return Err(CoreError::Cancelled.into());
            }
            result.map_err(|e| Stop::Failed(map_recovery_error(&e, FAILED)))
        }
        (ResetStep::Unsupported, _) => Err(CoreError::Internal(FAILED.into()).into()),
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::encryption::OAuthCrossSigningResetInfo;
    use matrix_sdk::ruma::api::client::uiaa::{AuthFlow, AuthType, UiaaInfo, UserIdentifier};
    use matrix_sdk::ruma::owned_user_id;

    use super::*;
    use crate::api::{CrossSigning, DeviceVerification, RecoveryStatus};

    fn keys(master: bool, self_signing: bool, user_signing: bool) -> Option<CrossSigningStatus> {
        Some(CrossSigningStatus {
            has_master: master,
            has_self_signing: self_signing,
            has_user_signing: user_signing,
        })
    }

    fn inputs() -> StatusInputs {
        StatusInputs {
            recovery: RecoveryState::Unknown,
            verification: VerificationState::Unknown,
            identity_exists: None,
            private_keys: None,
            has_devices_to_verify_against: false,
        }
    }

    #[test]
    fn status_table() {
        let all = keys(true, true, true);
        let cases = [
            // Nothing known yet.
            (inputs(), SecurityStatus::default()),
            // No identity at all.
            (
                StatusInputs {
                    identity_exists: Some(false),
                    private_keys: keys(false, false, false),
                    ..inputs()
                },
                SecurityStatus {
                    cross_signing: CrossSigning::NotSetUp,
                    ..SecurityStatus::default()
                },
            ),
            // Identity present, this device holds no private key (B0).
            (
                StatusInputs {
                    recovery: RecoveryState::Disabled,
                    verification: VerificationState::Unverified,
                    identity_exists: Some(true),
                    private_keys: keys(false, false, false),
                    has_devices_to_verify_against: false,
                },
                SecurityStatus {
                    recovery: RecoveryStatus::Disabled,
                    device: DeviceVerification::Unverified,
                    cross_signing: CrossSigning::MissingKeys,
                    has_devices_to_verify_against: false,
                    identity_exists: true,
                },
            ),
            // Identity present, only some private keys.
            (
                StatusInputs {
                    identity_exists: Some(true),
                    private_keys: keys(true, true, false),
                    ..inputs()
                },
                SecurityStatus {
                    cross_signing: CrossSigning::MissingKeys,
                    identity_exists: true,
                    ..SecurityStatus::default()
                },
            ),
            // The first device after a reset.
            (
                StatusInputs {
                    recovery: RecoveryState::Disabled,
                    verification: VerificationState::Verified,
                    identity_exists: Some(true),
                    private_keys: all.clone(),
                    ..inputs()
                },
                SecurityStatus {
                    recovery: RecoveryStatus::Disabled,
                    device: DeviceVerification::Verified,
                    cross_signing: CrossSigning::Complete,
                    has_devices_to_verify_against: false,
                    identity_exists: true,
                },
            ),
            // A returning user's new device with recovery set up.
            (
                StatusInputs {
                    recovery: RecoveryState::Incomplete,
                    verification: VerificationState::Unverified,
                    identity_exists: Some(true),
                    private_keys: keys(false, false, false),
                    has_devices_to_verify_against: true,
                },
                SecurityStatus {
                    recovery: RecoveryStatus::Incomplete,
                    device: DeviceVerification::Unverified,
                    cross_signing: CrossSigning::MissingKeys,
                    has_devices_to_verify_against: true,
                    identity_exists: true,
                },
            ),
            // Fully set up.
            (
                StatusInputs {
                    recovery: RecoveryState::Enabled,
                    verification: VerificationState::Verified,
                    identity_exists: Some(true),
                    private_keys: all.clone(),
                    has_devices_to_verify_against: true,
                },
                SecurityStatus {
                    recovery: RecoveryStatus::Enabled,
                    device: DeviceVerification::Verified,
                    cross_signing: CrossSigning::Complete,
                    has_devices_to_verify_against: true,
                    identity_exists: true,
                },
            ),
            // Identity present but no Olm machine to ask about the keys.
            (
                StatusInputs {
                    identity_exists: Some(true),
                    private_keys: None,
                    ..inputs()
                },
                SecurityStatus {
                    identity_exists: true,
                    ..SecurityStatus::default()
                },
            ),
            // Identity unknown stays unknown, whatever the keys say.
            (
                StatusInputs {
                    identity_exists: None,
                    private_keys: all,
                    ..inputs()
                },
                SecurityStatus::default(),
            ),
        ];
        for (i, (input, want)) in cases.iter().enumerate() {
            assert_eq!(compute_status(input), *want, "case {i}: {input:?}");
        }
    }

    #[test]
    fn uia_password_carries_user_password_and_session() {
        let user = owned_user_id!("@alice:example.org");
        let auth = uia_password(&user, &Password::from("secret-pw-x"), Some("sess-1"));
        let AuthData::Password(p) = auth else {
            panic!("not a password auth");
        };
        assert_eq!(p.password, "secret-pw-x");
        assert_eq!(p.session.as_deref(), Some("sess-1"));
        match p.identifier {
            UserIdentifier::Matrix(id) => assert_eq!(id.user, "@alice:example.org"),
            other => panic!("unexpected identifier {other:?}"),
        }
        let none = uia_password(&user, &Password::from("secret-pw-x"), None);
        let AuthData::Password(p) = none else {
            panic!("not a password auth");
        };
        assert_eq!(p.session, None);
    }

    fn uiaa(stages: Vec<Vec<AuthType>>, session: Option<&str>) -> CrossSigningResetAuthType {
        let mut info = UiaaInfo::new(stages.into_iter().map(AuthFlow::new).collect());
        info.session = session.map(str::to_owned);
        CrossSigningResetAuthType::Uiaa(info)
    }

    #[test]
    fn reset_outcome_branches() {
        let user = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");

        // The reset needed no authentication.
        assert!(matches!(reset_outcome(None, &user, None), ResetStep::Done));
        assert!(matches!(
            reset_outcome(None, &user, Some(&pw)),
            ResetStep::Done
        ));

        // UIA with the password stage.
        let auth = uiaa(vec![vec![AuthType::Password]], Some("s1"));
        assert!(matches!(
            reset_outcome(Some(&auth), &user, None),
            ResetStep::NeedPassword
        ));
        match reset_outcome(Some(&auth), &user, Some(&pw)) {
            ResetStep::Retry(AuthData::Password(p)) => {
                assert_eq!(p.session.as_deref(), Some("s1"));
                assert_eq!(p.password, "secret-pw-x");
            }
            other => panic!("unexpected {other:?}"),
        }

        // One acceptable flow among several.
        let auth = uiaa(
            vec![vec![AuthType::Sso], vec![AuthType::Password]],
            Some("s2"),
        );
        assert!(matches!(
            reset_outcome(Some(&auth), &user, Some(&pw)),
            ResetStep::Retry(_)
        ));

        // No flow this core can complete: asking for a password is pointless.
        let auth = uiaa(vec![vec![AuthType::Sso]], Some("s3"));
        assert!(matches!(
            reset_outcome(Some(&auth), &user, Some(&pw)),
            ResetStep::Unsupported
        ));
        assert!(matches!(
            reset_outcome(Some(&auth), &user, None),
            ResetStep::Unsupported
        ));
        let auth = uiaa(vec![vec![AuthType::Password, AuthType::Sso]], None);
        assert!(matches!(
            reset_outcome(Some(&auth), &user, None),
            ResetStep::Unsupported
        ));

        // OAuth: approve in the browser, whatever the password.
        let url = url::Url::parse("https://account.example.org/reset?s=1").unwrap();
        let auth = CrossSigningResetAuthType::OAuth(OAuthCrossSigningResetInfo {
            approval_url: url.clone(),
            session: Some("s4".into()),
        });
        for password in [None, Some(&pw)] {
            match reset_outcome(Some(&auth), &user, password) {
                ResetStep::Approve(got) => assert_eq!(got, url),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn only_one_identity_operation_runs_at_a_time() {
        let op = Arc::new(IdentityOp::default());
        let first = op.try_start().expect("the first operation starts");
        assert!(op.try_start().is_none(), "a second one must be refused");
        op.cancelled.store(true, Ordering::Release);
        drop(first);
        assert!(!op.cancelled.load(Ordering::Acquire));
        let again = op.try_start();
        assert!(again.is_some(), "the slot is free again");
    }

    #[test]
    fn rejection_means_an_auth_error_in_the_uia_answer() {
        use matrix_sdk::ruma::api::error::{ErrorKind, StandardErrorBody};
        assert!(!auth_rejected(None));
        let mut info = UiaaInfo::new(vec![AuthFlow::new(vec![AuthType::Password])]);
        assert!(
            !auth_rejected(Some(&info)),
            "the first answer only lists flows"
        );
        info.auth_error = Some(Box::new(StandardErrorBody::new(
            ErrorKind::Forbidden,
            "Invalid password".into(),
        )));
        assert!(auth_rejected(Some(&info)));
    }
}
