//! Security status, cross-signing bootstrap, identity reset and
//! re-authentication (plan Q8, Q13, Q14). Private to the core: SDK types stay
//! in here.
//!
//! Nothing here logs or prints a password, a UIA session or an approval URL.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt as _;
use futures_util::stream::{BoxStream, select_all};
use matrix_sdk::encryption::recovery::{
    EnableProgress, IdentityResetHandle, RecoveryError, RecoveryState,
};
use matrix_sdk::encryption::secret_storage::SecretStorageError;
use matrix_sdk::encryption::{CrossSigningResetAuthType, CrossSigningStatus, VerificationState};
use matrix_sdk::ruma::api::client::discovery::get_authorization_server_metadata::v1::{
    AccountManagementActionData, AuthorizationServerMetadata, DeviceDeleteData,
};
use matrix_sdk::ruma::api::client::keys::get_keys;
use matrix_sdk::ruma::api::client::uiaa::{
    AuthData, AuthType, MatrixUserIdentifier, Password as UiaaPassword, UiaaInfo, UserIdentifier,
};
use matrix_sdk::ruma::{DeviceId as SdkDeviceId, OwnedDeviceId, OwnedUserId, UserId};
use matrix_sdk::{AuthApi, Client};
use tokio::sync::{Mutex, MutexGuard, Notify, mpsc, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::api::{
    ActionKind, CoreError, CoreEvent, CrossSigning, DeviceId, DeviceSummary, DeviceTrust,
    DeviceVerification, Password, ReauthAction, RecoveryKey, RecoveryProgress, RecoveryStatus,
    RoomId, SecurityStatus, UserTrust,
};
use crate::oauth::check_auth_url;
use crate::sync::{PauseTarget, SyncPause, run_paused};
use crate::verification::VerificationTasks;

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

const BACKUP_EXISTS: &str = "a key backup already exists; enter your recovery key";
const WRONG_KEY: &str = "that recovery key or passphrase is not correct";
const NOT_SET_UP: &str = "recovery is not set up on this account";
const ALREADY_ON: &str = "recovery is already set up; change the key instead";
const NEEDS_KEY_FIRST: &str = "enter your recovery key before changing it";
const STATE_UNKNOWN: &str = "the recovery state is not known yet; try again in a moment";
const RECOVERY_FAILED: &str = "recovery could not be completed";

/// Like [`map_sdk_error`] for the recovery subsystem's error type. The SDK's
/// own texts can quote parts of the input, so none is forwarded.
fn map_recovery_error(error: &RecoveryError, what: &'static str) -> CoreError {
    match error {
        RecoveryError::BackupExistsOnServer => CoreError::Internal(BACKUP_EXISTS.into()),
        RecoveryError::Sdk(e) => map_sdk_error(e, what),
        // The key does not decode, or fails the secret store's MAC check.
        RecoveryError::SecretStorage(SecretStorageError::SecretStorageKey(_)) => {
            CoreError::Internal(WRONG_KEY.into())
        }
        RecoveryError::SecretStorage(SecretStorageError::MissingKeyInfo { .. }) => {
            CoreError::Internal(NOT_SET_UP.into())
        }
        RecoveryError::SecretStorage(_) => CoreError::Internal(what.into()),
    }
}

/// Progress of `Recovery::enable` as the API reports it. `None` for the final
/// `Done`, whose payload is the new recovery key and must never be forwarded.
fn map_progress(progress: &EnableProgress) -> Option<RecoveryProgress> {
    match progress {
        EnableProgress::Starting => Some(RecoveryProgress::Starting),
        EnableProgress::CreatingBackup => Some(RecoveryProgress::CreatingBackup),
        EnableProgress::CreatingRecoveryKey => Some(RecoveryProgress::CreatingKey),
        EnableProgress::BackingUp(counts) => Some(RecoveryProgress::BackingUp {
            done: counts.backed_up as u64,
            total: counts.total as u64,
        }),
        EnableProgress::RoomKeyUploadError => Some(RecoveryProgress::UploadFailed),
        EnableProgress::Done { .. } => None,
    }
}

/// Whether `EnableRecovery` may start in recovery state `state`. Over an
/// existing secret store `enable` would replace its key with one that lacks
/// the secrets this device is missing, so only "nothing set up" goes on (the
/// SDK still checks the server for a backup this device is not connected to).
fn enable_precheck(state: RecoveryState) -> Result<(), CoreError> {
    match state {
        RecoveryState::Disabled => Ok(()),
        RecoveryState::Incomplete => Err(CoreError::Internal(BACKUP_EXISTS.into())),
        RecoveryState::Enabled => Err(CoreError::Internal(ALREADY_ON.into())),
        RecoveryState::Unknown => Err(CoreError::Internal(STATE_UNKNOWN.into())),
    }
}

/// Whether `ChangeRecoveryKey` may start in recovery state `state`: it
/// re-uploads the secrets this device holds, so it needs all of them.
fn change_key_precheck(state: RecoveryState) -> Result<(), CoreError> {
    match state {
        RecoveryState::Enabled => Ok(()),
        RecoveryState::Disabled => Err(CoreError::Internal(NOT_SET_UP.into())),
        RecoveryState::Incomplete => Err(CoreError::Internal(NEEDS_KEY_FIRST.into())),
        RecoveryState::Unknown => Err(CoreError::Internal(STATE_UNKNOWN.into())),
    }
}

/// What the status says about the own identity. A local identity does not
/// prove that the server has one (a bootstrap saves it locally before the
/// upload, and a keys query that finds no identity keeps it), so with a local
/// identity only the server's answer counts. Without one, "none" counts only
/// after the server was asked (`queried`).
fn identity_known(local: Option<bool>, queried: bool, server: Option<bool>) -> Option<bool> {
    match local {
        Some(true) => server,
        Some(false) => queried.then_some(false),
        None => None,
    }
}

/// A password session must give the password before a reset starts: the SDK
/// deletes the key backup and turns secret storage off before it asks for
/// authentication (matrix-sdk-0.19.1 `recovery/mod.rs:438-443`). OAuth
/// approves in the browser and takes no password.
fn reset_needs_password_first(oauth: bool, password: Option<&Password>) -> bool {
    !oauth && password.is_none()
}

/// How an OAuth-approval reset ended.
#[derive(Debug, PartialEq)]
enum ResetEnd {
    Success,
    Cancelled,
    Failed,
}

/// Classifies the end of `handle.reset(None)`: `ok` is its result,
/// `cancelled` whether a cancel came, `uploaded` whether the server now holds
/// the master key of the local private identity (`None`: could not be checked). After a cancel the SDK's
/// `auth` also returns `Ok` (matrix-sdk-0.19.1 `encryption/mod.rs:386-389`),
/// so only the server tells an approval that raced the cancel from a cancel.
fn reset_end(ok: bool, cancelled: bool, uploaded: Option<bool>) -> ResetEnd {
    match (ok, cancelled) {
        (true, false) => ResetEnd::Success,
        (false, false) => ResetEnd::Failed,
        (true, true) if uploaded == Some(true) => ResetEnd::Success,
        (_, true) => ResetEnd::Cancelled,
    }
}

/// The master key (base64) the homeserver holds for `me`: `None` when the
/// request failed, `Some(None)` when the account has no identity there. Asks
/// the server directly, without feeding the answer to the Olm machine, so it
/// cannot change the local store (`get_keys` `master_keys`, ruma-client-api
/// 0.25.0 `keys/get_keys.rs:67`).
async fn server_master_key(client: &Client, me: &UserId) -> Option<Option<String>> {
    let mut request = get_keys::v3::Request::new();
    request.device_keys.insert(me.to_owned(), Vec::new());
    let response = client.send(request).await.ok()?;
    let Some(raw) = response.master_keys.get(me) else {
        return Some(None);
    };
    // Present but unreadable still counts as present; it matches no local key.
    Some(Some(
        raw.deserialize()
            .ok()
            .and_then(|key| key.keys.values().next().cloned())
            .unwrap_or_default(),
    ))
}

/// Whether the homeserver holds a cross-signing identity for `me`.
async fn server_has_identity(client: &Client, me: &UserId) -> Option<bool> {
    server_master_key(client, me).await.map(|key| key.is_some())
}

/// The master key (base64) of the identity in the local store.
async fn local_master_key(client: &Client, me: &UserId) -> Option<String> {
    let identity = client
        .encryption()
        .get_user_identity(me)
        .await
        .ok()
        .flatten()?;
    identity
        .master_key()
        .get_first_key()
        .map(|key| key.to_base64())
}

/// Reads the current status from the client.
async fn read_status(client: &Client, op: &IdentityOp) -> SecurityStatus {
    let encryption = client.encryption();
    let Some(me) = client.user_id() else {
        return SecurityStatus::default();
    };
    // Also asks the server for the own identity when it is not known yet.
    let devices = encryption.has_devices_to_verify_against().await;
    let local = encryption
        .get_user_identity(me)
        .await
        .ok()
        .map(|identity| identity.is_some());
    // An identity the server has stays there, so that answer is kept and the
    // request is made only until it is confirmed.
    let server = if local == Some(true) {
        if op.server_identity_seen.load(Ordering::Acquire) {
            Some(true)
        } else {
            let server = server_has_identity(client, me).await;
            if server == Some(true) {
                op.server_identity_seen.store(true, Ordering::Release);
            }
            server
        }
    } else {
        None
    };
    let identity_exists = identity_known(local, devices.is_ok(), server);
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
async fn forward_status(
    client: Client,
    events: mpsc::Sender<CoreEvent>,
    refresh: Arc<Notify>,
    op: Arc<IdentityOp>,
) {
    let mut changes = change_stream(&client).await;
    let mut changes_open = true;
    let mut last: Option<SecurityStatus> = None;
    loop {
        if !forward_round(&op, &mut last, &events, read_status(&client, &op)).await {
            return;
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

/// One round of the status forwarder: reads the status and sends it when it
/// changed (or when an operation announced its end). Returns `false` when the
/// receiver is gone.
///
/// While an identity operation has the sync paused, no status goes out: a
/// front end that reacts to it (a message sent right after the reset's
/// status) would run against a stopped sync and send queue. The `paused` lock
/// is held from before the read until the status is sent, so an operation
/// cannot start in between and have its half-done state read (a new account's
/// reset makes its keys before it ends). The counter `op.rounds` is bumped
/// before the lock is released, which is what `announce_end` waits for.
async fn forward_round(
    op: &IdentityOp,
    last: &mut Option<SecurityStatus>,
    events: &mpsc::Sender<CoreEvent>,
    read: impl Future<Output = SecurityStatus>,
) -> bool {
    let paused = op.paused.lock().await;
    let status = {
        let _gate = op.lock_keys(&paused).await;
        read.await
    };
    // After an operation that paused the sync the status goes out even when
    // it equals the last one sent: the paused changes were held back, and the
    // front end waits for the result (a second reset in a row ends in the
    // same status as the first).
    if op.announce.swap(false, Ordering::AcqRel) {
        *last = None;
    }
    let mut open = true;
    if *last != Some(status) {
        *last = Some(status);
        open = events.send(CoreEvent::Security(status)).await.is_ok();
    }
    op.rounds.send_modify(|rounds| *rounds += 1);
    drop(paused);
    open
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

/// The longest `announce_end` waits for the status forwarder (it reads the
/// status, which may ask the server).
const FORWARDER_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Ends the hold on the paused sync of `bootstrap` or `reset` and wakes the
/// status forwarder. After a success or a stop for the password it also
/// waits for the forwarder's round, so that the status goes out before the
/// operation's answer.
///
/// An attempt that stopped for the password changed nothing and goes on once
/// the password is in, so it announces nothing. Its stop is reported after
/// the forwarder's round: the status that waited for the `paused` lock goes
/// out first. A status that came after the front end had sent the password
/// would look like the end of the second attempt when the device is already
/// verified and has no backup (a new account).
///
/// A success announces its end: the forwarder sends the status even when it
/// equals the last one. The reset then answers `IdentityReset`, after this
/// status, so a front end never has to guess the end from a status.
///
/// The order does not rest on the fairness of the lock. The round counter
/// `op.rounds` is read while `paused` is still held, and the forwarder bumps
/// it only while holding `paused` after it sent. So a bump seen after the
/// guard is dropped belongs to a round that read after this operation's work,
/// and the wait ends then. `refresh` makes sure such a round happens. If the
/// forwarder is gone (its receiver closed), the wait ends after
/// `FORWARDER_WAIT` instead of never; a status read that takes longer than
/// that is not waited for.
async fn announce_end(
    op: &IdentityOp,
    refresh: &Notify,
    result: &Result<(), Stop>,
    paused: MutexGuard<'_, ()>,
) {
    let mut rounds = op.rounds.subscribe();
    let announce = announces_end(result);
    if announce {
        op.announce.store(true, Ordering::Release);
    }
    drop(paused);
    refresh.notify_one();
    // A failure is reported at once; its order against the status does not
    // matter, since the front end ends its wait on the failure itself.
    if result.is_ok() || !announce {
        // An error only means that the counter is gone; nothing to wait for.
        let _ = tokio::time::timeout(FORWARDER_WAIT, rounds.changed()).await;
    }
}

/// Whether an operation that ended with `result` announces its end.
fn announces_end(result: &Result<(), Stop>) -> bool {
    !matches!(result, Err(Stop::Reauth(_)))
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
    /// Held by the status forwarder while it reads (reading may query the
    /// server for the own keys) and by an identity operation from the moment
    /// it makes new private keys until the server has the public ones. A
    /// query answered with the old identity in that time makes the SDK drop
    /// the new private keys for good, so no query of ours may run then.
    keys_gate: Mutex<()>,
    /// Held by `bootstrap` and `reset` for as long as they keep the sync
    /// paused, until the sync and the send queue are back. The status
    /// forwarder waits for it before it reads and sends a status, so that the
    /// `Security` event that ends such an operation is sent with the sync
    /// running and the send queue on. Taken before `keys_gate`.
    paused: Mutex<()>,
    /// Set when `bootstrap` or `reset` ends: the next status is sent even if
    /// it equals the last one.
    announce: AtomicBool,
    /// The server was seen to hold an identity for the account (it stays).
    server_identity_seen: AtomicBool,
    /// Counts the rounds of the status forwarder; bumped while it still holds
    /// `paused`. `announce_end` waits for it to move.
    rounds: watch::Sender<u64>,
}

impl IdentityOp {
    /// Takes `keys_gate`. Asking for the `paused` guard makes the order
    /// `paused`, then `keys_gate` the only one that compiles.
    async fn lock_keys(&self, _paused: &MutexGuard<'_, ()>) -> MutexGuard<'_, ()> {
        self.keys_gate.lock().await
    }

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
    /// The watcher of `WatchUserTrust` (one user at a time).
    trust_watch: Option<JoinHandle<()>>,
    /// Re-sends the device list on crypto-store device updates; started by the
    /// first `ListDevices` of the session.
    devices_watch: Option<JoinHandle<()>>,
    /// Verification requests and flows (see `verification.rs`).
    pub(crate) verification: VerificationTasks,
    tasks: JoinSet<()>,
    refresh: Arc<Notify>,
    identity: Arc<IdentityOp>,
}

impl SecurityTasks {
    pub(crate) fn new() -> Self {
        Self {
            forwarder: None,
            trust_watch: None,
            devices_watch: None,
            verification: VerificationTasks::new(),
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
            self.identity.clone(),
        )));
        self.verification.start(client, events);
    }

    /// Ends the forwarder and every action, so that nothing holds the client
    /// afterwards.
    pub(crate) async fn stop(&mut self) {
        if let Some(forwarder) = self.forwarder.take() {
            forwarder.abort();
            // An aborted task ends with a cancellation error; that is expected.
            let _ = forwarder.await;
        }
        if let Some(watch) = self.trust_watch.take() {
            watch.abort();
            // An aborted task ends with a cancellation error; that is expected.
            let _ = watch.await;
        }
        if let Some(watch) = self.devices_watch.take() {
            watch.abort();
            // An aborted task ends with a cancellation error; that is expected.
            let _ = watch.await;
        }
        self.verification.stop().await;
        self.tasks.shutdown().await;
    }

    /// Runs one identity operation in the background. Without a permit the
    /// answer is `Busy`. The permit lives in the spawned task until the work
    /// ends, so the work cannot drop it early by not mentioning it.
    fn spawn_identity<F, Fut>(
        &mut self,
        events: &mpsc::Sender<CoreEvent>,
        action: ActionKind,
        work: F,
    ) where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Option<CoreEvent>, Stop>> + Send + 'static,
    {
        // Forget finished tasks, so the set does not grow with every action.
        while self.tasks.try_join_next().is_some() {}
        let events = events.clone();
        let permit = self.identity.try_start();
        let fut = permit.map(|permit| (permit, work()));
        self.tasks.spawn(async move {
            let result = match fut {
                Some((permit, fut)) => {
                    let result = fut.await;
                    // Free the slot before the answer, so a front end that
                    // reacts to it can start the next operation.
                    drop(permit);
                    result
                }
                None => Err(Stop::Failed(CoreError::Busy)),
            };
            match result {
                // The success answer, if any, goes after the slot is free.
                Ok(Some(event)) => {
                    let _ = events.send(event).await;
                }
                Ok(None) => {}
                Err(stop) => report(&events, action, stop).await,
            }
        });
    }

    /// `BootstrapCrossSigning`: creates the identity when there is none.
    pub(crate) fn bootstrap(
        &mut self,
        client: &Client,
        pause: Option<SyncPause>,
        password: Option<Password>,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        let op = self.identity.clone();
        self.spawn_identity(events, ActionKind::CrossSigning, move || async move {
            let paused = op.paused.lock().await;
            let result = with_sync_paused(&client, pause.as_ref(), async {
                let result = {
                    let _gate = op.lock_keys(&paused).await;
                    bootstrap_task(&client, password.as_ref()).await
                };
                // Also after a failure: a keys query drops local keys when
                // the server turned out to have an identity. It runs while the
                // sync is still paused.
                query_own_identity(&client).await;
                result
            })
            .await;
            announce_end(&op, &refresh, &result, paused).await;
            result.map(|()| None)
        });
    }

    /// `ResetIdentity`: replaces the identity, deleting the backups.
    pub(crate) fn reset(
        &mut self,
        client: &Client,
        pause: Option<SyncPause>,
        password: Option<Password>,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        let op = self.identity.clone();
        let events_for_task = events.clone();
        self.spawn_identity(events, ActionKind::CrossSigning, move || async move {
            let paused = op.paused.lock().await;
            let result = with_sync_paused(&client, pause.as_ref(), async {
                let result = {
                    let _gate = op.lock_keys(&paused).await;
                    reset_task(&client, &op, password.as_ref(), &events_for_task).await
                };
                // Also after a failure: after a reset that made new private
                // keys the server never accepted, the query drops them again.
                // It runs while the sync is still paused.
                query_own_identity(&client).await;
                match result {
                    Ok(()) => reset_complete(client.encryption().cross_signing_status().await),
                    Err(stop) => Err(stop),
                }
            })
            .await;
            announce_end(&op, &refresh, &result, paused).await;
            result.map(|()| Some(CoreEvent::IdentityReset))
        });
    }

    /// `EnableRecovery`: sets recovery up and sends the new key once.
    pub(crate) fn enable_recovery(&mut self, client: &Client, events: &mpsc::Sender<CoreEvent>) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        let events_for_task = events.clone();
        self.spawn_identity(events, ActionKind::Recovery, move || async move {
            let result = enable_task(&client, &events_for_task).await;
            finish(&client, &refresh).await;
            result
        });
    }

    /// `Recover`: restores the secrets with a recovery key or passphrase.
    pub(crate) fn recover(
        &mut self,
        client: &Client,
        key: RecoveryKey,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        let op = self.identity.clone();
        self.spawn_identity(events, ActionKind::Recovery, move || async move {
            let result = {
                // Importing the private keys signs this device; a keys query
                // must not run in between.
                let _gate = op.keys_gate.lock().await;
                recover_task(&client, &key).await
            };
            // Also after a failure: the status is read again.
            finish(&client, &refresh).await;
            result
        });
    }

    /// `ChangeRecoveryKey`: replaces the recovery key and sends the new one.
    pub(crate) fn change_recovery_key(
        &mut self,
        client: &Client,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let client = client.clone();
        let refresh = self.refresh.clone();
        self.spawn_identity(events, ActionKind::Recovery, move || async move {
            let result = change_key_task(&client).await;
            finish(&client, &refresh).await;
            result
        });
    }

    /// `WatchUserTrust`: reports the trust in `user` now and on every change;
    /// replaces the user watched before.
    ///
    /// The old watcher is aborted, not awaited, so one `UserTrust` it had
    /// already queued can still arrive after the new watcher's first one. That
    /// is harmless: every `UserTrust` names its user, and a consumer matches
    /// on `user_id` (the app ignores the trust of a user it does not show).
    /// An aborted watcher sends nothing after its next await point, so no
    /// stale value for the old user is produced later than that.
    pub(crate) fn watch_user_trust(
        &mut self,
        client: &Client,
        user: &crate::api::UserId,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        if let Some(old) = self.trust_watch.take() {
            old.abort();
        }
        let Ok(user) = OwnedUserId::try_from(user.as_str()) else {
            self.spawn_failed(
                events,
                ActionKind::Devices,
                CoreError::Internal("that is not a valid user ID".into()),
            );
            return;
        };
        self.trust_watch = Some(tokio::spawn(watch_user_trust(
            client.clone(),
            user,
            events.clone(),
        )));
    }

    /// Sends `ActionFailed` for a devices action from a task (the handlers do
    /// not wait on the channel).
    fn spawn_failed(
        &mut self,
        events: &mpsc::Sender<CoreEvent>,
        action: ActionKind,
        error: CoreError,
    ) {
        let events = events.clone();
        self.tasks.spawn(async move {
            report(&events, action, Stop::Failed(error)).await;
        });
    }

    /// `ListDevices`: answers with `Devices`, and from the first call on
    /// re-sends the list whenever the crypto store's devices change.
    pub(crate) fn list_devices(&mut self, client: &Client, events: &mpsc::Sender<CoreEvent>) {
        while self.tasks.try_join_next().is_some() {}
        if self
            .devices_watch
            .as_ref()
            .is_some_and(|w| !w.is_finished())
        {
            // Already watching: answer this request on its own.
            let (client, events) = (client.clone(), events.clone());
            self.tasks
                .spawn(async move { send_devices(&client, &events, ActionKind::Devices).await });
        } else {
            self.devices_watch = Some(tokio::spawn(watch_devices(client.clone(), events.clone())));
        }
    }

    /// `RenameDevice`.
    pub(crate) fn rename_device(
        &mut self,
        client: &Client,
        device_id: &DeviceId,
        name: &str,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let name = match clean_device_name(name) {
            Ok(name) => name,
            Err(error) => return self.spawn_failed(events, ActionKind::Devices, error),
        };
        let device = OwnedDeviceId::from(device_id.as_str());
        let (client, events) = (client.clone(), events.clone());
        self.tasks.spawn(async move {
            match client.rename_device(&device, &name).await {
                Ok(_) => send_devices(&client, &events, ActionKind::Devices).await,
                Err(_) => {
                    let error = CoreError::Internal("the device could not be renamed".into());
                    report(&events, ActionKind::Devices, Stop::Failed(error)).await;
                }
            }
        });
    }

    /// `SignOutDevices`: Q12 and Q13. The password is used for the one retry
    /// and dropped (zeroized) with the task.
    pub(crate) fn sign_out_devices(
        &mut self,
        client: &Client,
        device_ids: &[DeviceId],
        password: Option<Password>,
        events: &mpsc::Sender<CoreEvent>,
    ) {
        let (Some(me), Some(current)) = (client.user_id(), client.device_id()) else {
            return self.spawn_failed(events, ActionKind::SignOutDevices, CoreError::NotLoggedIn);
        };
        if let Err(error) = sign_out_precheck(device_ids, current.as_str()) {
            return self.spawn_failed(events, ActionKind::SignOutDevices, error);
        }
        let devices: Vec<OwnedDeviceId> = device_ids
            .iter()
            .map(|id| OwnedDeviceId::from(id.as_str()))
            .collect();
        let (client, events, me) = (client.clone(), events.clone(), me.to_owned());
        let oauth = matches!(client.auth_api(), Some(AuthApi::OAuth(_)));
        self.tasks.spawn(async move {
            if oauth {
                open_account_page(&client, &devices, &events).await;
                return;
            }
            let api = ClientDelete {
                client: &client,
                devices: &devices,
            };
            match delete_flow(&api, &me, password.as_ref()).await {
                Ok(()) => send_devices(&client, &events, ActionKind::SignOutDevices).await,
                Err(stop) => report(&events, ActionKind::SignOutDevices, stop).await,
            }
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
async fn report(events: &mpsc::Sender<CoreEvent>, action: ActionKind, stop: Stop) {
    let event = match stop {
        Stop::Reauth(action) => CoreEvent::ReauthRequired { action },
        Stop::Failed(error) => CoreEvent::ActionFailed {
            // These actions belong to no room.
            room_id: RoomId::from(""),
            action,
            error,
        },
    };
    // A dropped receiver just means nobody listens any more.
    let _ = events.send(event).await;
}

/// Runs `work` with the sync stopped (when there is a sync service): the
/// operations that create new private cross-signing keys must not have a sync
/// `/keys/query` answered with the old identity processed in between, see
/// [`SyncPause`]. The SDK's one-shot setup after the login (it can query keys
/// and bootstrap too) is awaited first.
async fn with_sync_paused<P: PauseTarget, T>(
    client: &Client,
    pause: Option<&SyncPause<P>>,
    work: impl std::future::Future<Output = T>,
) -> T {
    client
        .encryption()
        .wait_for_e2ee_initialization_tasks()
        .await;
    run_paused(pause, work).await
}

/// A reset that reported success must leave all three private keys here: a
/// query answered by another device's newer identity (or any lost race) drops
/// them, and the account would be verified on the server only.
fn reset_complete(status: Option<CrossSigningStatus>) -> Result<(), Stop> {
    if status.is_some_and(|status| status.is_complete()) {
        Ok(())
    } else {
        Err(CoreError::Internal("the identity reset did not complete on this device".into()).into())
    }
}

/// After a change of identity: asks the server for the own keys (so that the
/// device's new signature is seen) and has the status recomputed.
async fn finish(client: &Client, refresh: &Notify) {
    query_own_identity(client).await;
    refresh.notify_one();
}

/// Asks the server for the own keys (best effort: the sync brings the same
/// information a moment later).
async fn query_own_identity(client: &Client) {
    if let Some(me) = client.user_id() {
        let _ = client.encryption().request_user_identity(me).await;
    }
}

/// Why one bootstrap call failed.
#[derive(Debug)]
enum AttemptError {
    /// The homeserver wants user-interactive authentication.
    Uia(Box<UiaaInfo>),
    /// Anything else.
    Failed(CoreError),
}

/// The calls a bootstrap makes, so that the sequence can be tested.
trait BootstrapApi {
    /// Whether the server holds an identity (`None`: could not be checked).
    async fn server_has_identity(&self) -> Option<bool>;
    /// Whether the local store holds an identity.
    async fn local_identity(&self) -> bool;
    /// Fetches the own identity into the local store (best effort).
    async fn sync_identity(&self);
    /// Creates the identity unless the store has one, and uploads it.
    async fn first_attempt(&self) -> Result<(), AttemptError>;
    /// Uploads the identity of the store (it keeps the keys the failed first
    /// attempt made), with `auth` when given.
    async fn upload(&self, auth: Option<AuthData>) -> Result<(), AttemptError>;
}

/// The real calls.
struct ClientBootstrap<'a> {
    client: &'a Client,
    me: &'a UserId,
}

fn attempt_error(error: &matrix_sdk::Error) -> AttemptError {
    match error.as_uiaa_response() {
        Some(info) => AttemptError::Uia(Box::new(info.clone())),
        None => AttemptError::Failed(CoreError::Internal(BOOTSTRAP_FAILED.into())),
    }
}

impl BootstrapApi for ClientBootstrap<'_> {
    async fn server_has_identity(&self) -> Option<bool> {
        server_has_identity(self.client, self.me).await
    }

    async fn local_identity(&self) -> bool {
        self.client
            .encryption()
            .get_user_identity(self.me)
            .await
            .ok()
            .flatten()
            .is_some()
    }

    async fn sync_identity(&self) {
        let _ = self
            .client
            .encryption()
            .request_user_identity(self.me)
            .await;
    }

    async fn first_attempt(&self) -> Result<(), AttemptError> {
        self.client
            .encryption()
            .bootstrap_cross_signing_if_needed(None)
            .await
            .map_err(|e| attempt_error(&e))
    }

    async fn upload(&self, auth: Option<AuthData>) -> Result<(), AttemptError> {
        self.client
            .encryption()
            .bootstrap_cross_signing(auth)
            .await
            .map_err(|e| attempt_error(&e))
    }
}

const BOOTSTRAP_FAILED: &str = "cross-signing could not be set up";

/// What a bootstrap request does. `server`: whether the homeserver holds an
/// identity; `local`: whether the local store does. The SDK saves a new
/// identity locally before it uploads it, and a keys query that finds none on
/// the server leaves it there (matrix-sdk-crypto-0.19.1 `machine/mod.rs:808-835`,
/// `identities/manager.rs:464`, `olm/signing/mod.rs:351`), so a failed first
/// upload leaves "server none, local some", and `bootstrap_cross_signing_if_needed`
/// (matrix-sdk-0.19.1 `encryption/mod.rs:1696-1708`) would then do nothing.
fn bootstrap_plan(server: Option<bool>, local: bool) -> BootstrapPlan {
    match (server, local) {
        (Some(true), _) => BootstrapPlan::AlreadySetUp,
        (Some(false), false) => BootstrapPlan::FirstAttempt,
        (Some(false), true) => BootstrapPlan::UploadLocal,
        (None, _) => BootstrapPlan::Unverifiable,
    }
}

/// The step a bootstrap request starts with.
#[derive(Debug, PartialEq)]
enum BootstrapPlan {
    /// The server has an identity; nothing to upload.
    AlreadySetUp,
    /// Neither side has one: create and upload.
    FirstAttempt,
    /// An earlier attempt made the keys but the server never got them.
    UploadLocal,
    /// The server could not be asked.
    Unverifiable,
}

/// Creates the cross-signing identity when the server has none (Q13).
async fn bootstrap_task(client: &Client, password: Option<&Password>) -> Result<(), Stop> {
    let me = client.user_id().ok_or(CoreError::NotLoggedIn)?;
    bootstrap_flow(&ClientBootstrap { client, me }, me, password).await
}

/// The bootstrap sequence over `api`.
async fn bootstrap_flow<A: BootstrapApi>(
    api: &A,
    me: &UserId,
    password: Option<&Password>,
) -> Result<(), Stop> {
    let first = match bootstrap_plan(api.server_has_identity().await, api.local_identity().await) {
        BootstrapPlan::AlreadySetUp => {
            // Bring a server-side identity this client had not seen into the
            // store; never replace it.
            api.sync_identity().await;
            return Ok(());
        }
        BootstrapPlan::Unverifiable => {
            return Err(CoreError::Internal(
                "the server could not be asked about the identity".into(),
            )
            .into());
        }
        BootstrapPlan::FirstAttempt => api.first_attempt().await,
        BootstrapPlan::UploadLocal => api.upload(None).await,
    };
    let info = match first {
        Ok(()) => return Ok(()),
        Err(AttemptError::Failed(error)) => return Err(error.into()),
        Err(AttemptError::Uia(info)) => info,
    };
    match uia_step(&info, me, password) {
        ResetStep::NeedPassword => Err(Stop::Reauth(ReauthAction::BootstrapCrossSigning)),
        ResetStep::Retry(auth) => {
            // Only a confirmed "none" on the server goes on, or the retry
            // would replace an identity without the reset's intent.
            bootstrap_retry_allowed(api.server_has_identity().await)?;
            // The keys exist locally already (reset is false), so the upload
            // goes on directly with the authentication data.
            api.upload(Some(auth)).await.map_err(|error| match error {
                AttemptError::Uia(info) if auth_rejected(Some(&info)) => {
                    Stop::Failed(CoreError::ReauthFailed)
                }
                AttemptError::Uia(_) => Stop::Failed(CoreError::Internal(BOOTSTRAP_FAILED.into())),
                AttemptError::Failed(error) => Stop::Failed(error),
            })
        }
        ResetStep::Done | ResetStep::Approve(_) | ResetStep::Unsupported => {
            Err(CoreError::Internal(BOOTSTRAP_FAILED.into()).into())
        }
    }
}

/// The success answer of an action that hands out a new recovery key.
fn key_ready(key: String) -> Option<CoreEvent> {
    Some(CoreEvent::RecoveryKeyReady(RecoveryKey::from(key)))
}

/// Sets recovery up (Q5), forwarding progress (never the key it carries).
async fn enable_task(
    client: &Client,
    events: &mpsc::Sender<CoreEvent>,
) -> Result<Option<CoreEvent>, Stop> {
    let recovery = client.encryption().recovery();
    enable_precheck(recovery.state())?;
    let enable = recovery.enable().wait_for_backups_to_upload();
    let progress = enable.subscribe_to_progress();
    let mut progress = std::pin::pin!(progress);
    let mut enable = std::pin::pin!(enable.into_future());
    let result = loop {
        tokio::select! {
            result = &mut enable => break result,
            Some(item) = progress.next() => {
                if let Ok(item) = &item
                    && let Some(progress) = map_progress(item)
                {
                    // A dropped receiver just means nobody listens any more.
                    let _ = events.send(CoreEvent::RecoveryProgress(progress)).await;
                }
            }
        }
    };
    let key = result.map_err(|e| Stop::Failed(map_recovery_error(&e, RECOVERY_FAILED)))?;
    Ok(key_ready(key))
}

/// Restores the secrets from secret storage with `key`.
async fn recover_task(client: &Client, key: &RecoveryKey) -> Result<Option<CoreEvent>, Stop> {
    client
        .encryption()
        .recovery()
        .recover(key.expose())
        .await
        .map_err(|e| Stop::Failed(map_recovery_error(&e, RECOVERY_FAILED)))?;
    Ok(Some(CoreEvent::Recovered))
}

/// Replaces the recovery key.
async fn change_key_task(client: &Client) -> Result<Option<CoreEvent>, Stop> {
    let recovery = client.encryption().recovery();
    change_key_precheck(recovery.state())?;
    let key = recovery
        .reset_key()
        .await
        .map_err(|e| Stop::Failed(map_recovery_error(&e, RECOVERY_FAILED)))?;
    Ok(key_ready(key))
}

/// The approval URL the server gave, when it is safe to hand to the front end.
fn approval_url(url: &url::Url) -> Result<crate::api::AuthUrl, CoreError> {
    // Not `Login`: this is no login, and the check's own message says so.
    check_auth_url(url.as_str())
        .map_err(|_| CoreError::Internal("the identity reset gave an unsafe approval URL".into()))
}

/// Whether a password retry of the first upload may go on, given whether the
/// account has an identity now (`None`: could not be determined). Anything but
/// a confirmed "none" stops, so a retry cannot replace an identity.
fn bootstrap_retry_allowed(identity_exists: Option<bool>) -> Result<(), CoreError> {
    match identity_exists {
        Some(false) => Ok(()),
        Some(true) | None => Err(CoreError::Internal(
            "an identity exists on the server; confirm it or reset it instead".into(),
        )),
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
    // Before anything is touched: `reset_identity` deletes the key backup and
    // turns secret storage off first, then asks for authentication.
    let oauth = matches!(client.auth_api(), Some(AuthApi::OAuth(_)));
    if reset_needs_password_first(oauth, password) {
        return Err(Stop::Reauth(ReauthAction::ResetIdentity));
    }
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
            let url = approval_url(&url)?;
            let handle = Arc::new(handle);
            *op.waiting.lock().await = Some(handle.clone());
            // A cancel that arrived before the handle was stored: the front
            // end never gets the approval URL.
            if op.cancelled.load(Ordering::Acquire) {
                handle.cancel().await;
                *op.waiting.lock().await = None;
                return Err(CoreError::Cancelled.into());
            }
            let _ = events.send(CoreEvent::OpenAccountManagement(url)).await;
            // Polls until the approval arrives, the 2 minutes are up, or the
            // reset is cancelled.
            let result = handle.reset(None).await;
            *op.waiting.lock().await = None;
            let cancelled = op.cancelled.load(Ordering::Acquire);
            let uploaded = if result.is_ok() && cancelled {
                // The private keys must be there too: a public identity alone
                // can be the old one put back by a sync-driven query, which
                // would make a cancelled reset look like a success.
                let complete = client
                    .encryption()
                    .cross_signing_status()
                    .await
                    .is_some_and(|status| status.is_complete());
                match (
                    server_master_key(client, me).await,
                    local_master_key(client, me).await,
                ) {
                    (Some(Some(server)), Some(local)) => Some(complete && server == local),
                    _ => None,
                }
            } else {
                None
            };
            match reset_end(result.is_ok(), cancelled, uploaded) {
                ResetEnd::Success => Ok(()),
                ResetEnd::Cancelled => Err(CoreError::Cancelled.into()),
                ResetEnd::Failed => {
                    result.map_err(|e| Stop::Failed(map_recovery_error(&e, FAILED)))
                }
            }
        }
        (ResetStep::Unsupported, _) => Err(CoreError::Internal(FAILED.into()).into()),
    }
}

/// What the crypto store knows about another user's identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IdentityFacts {
    /// Verified by the logged-in user.
    pub verified: bool,
    /// The identity changed since it was verified.
    pub violation: bool,
}

/// The API trust for `identity` (`None`: the user has no identity).
pub(crate) fn map_user_trust(identity: Option<IdentityFacts>) -> UserTrust {
    match identity {
        None => UserTrust::NoIdentity,
        Some(facts) if facts.violation => UserTrust::IdentityChanged,
        Some(facts) if facts.verified => UserTrust::Verified,
        Some(_) => UserTrust::Unverified,
    }
}

/// The trust in `user` as the local store has it; `None` when the store
/// cannot be read.
async fn read_user_trust(client: &Client, user: &UserId) -> Option<UserTrust> {
    let identity = client
        .encryption()
        .get_user_identity(user)
        .await
        .ok()?
        .map(|identity| IdentityFacts {
            verified: identity.is_verified(),
            violation: identity.has_verification_violation(),
        });
    Some(map_user_trust(identity))
}

/// Sends `UserTrust` for `user` now and whenever it changes.
async fn watch_user_trust(client: Client, user: OwnedUserId, events: mpsc::Sender<CoreEvent>) {
    let mut updates = match client.encryption().user_identities_stream().await {
        Ok(stream) => stream.boxed(),
        Err(_) => {
            let failed = CoreEvent::ActionFailed {
                // These actions belong to no room.
                room_id: RoomId::from(""),
                action: ActionKind::Devices,
                error: CoreError::Internal("the user's trust could not be watched".into()),
            };
            let _ = events.send(failed).await;
            return;
        }
    };
    // Ask the server once: the stored identity can predate a reset.
    let _ = client.encryption().request_user_identity(&user).await;
    let mut last: Option<UserTrust> = None;
    loop {
        if let Some(trust) = read_user_trust(&client, &user).await
            && last != Some(trust)
        {
            last = Some(trust);
            let event = CoreEvent::UserTrust {
                user_id: crate::api::UserId::from(user.as_str()),
                trust,
            };
            if events.send(event).await.is_err() {
                return;
            }
        }
        if updates.next().await.is_none() {
            return;
        }
    }
}

/// What the server lists about one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerDevice {
    pub device_id: String,
    pub display_name: Option<String>,
    pub last_seen_ms: Option<u64>,
}

/// Joins the server's device list with the crypto store's trust (Q12): the
/// current device first, then the rest by last activity, newest first (never
/// seen last, equal times by device ID). A device the store has no keys for is
/// `NoKeys`; an empty name is no name. The last-seen IP is not read at all.
pub(crate) fn merge_devices(
    server: Vec<ServerDevice>,
    crypto_trust: &HashMap<String, DeviceTrust>,
    current: &str,
) -> Vec<DeviceSummary> {
    let mut devices: Vec<DeviceSummary> = server
        .into_iter()
        .map(|device| DeviceSummary {
            trust: crypto_trust
                .get(&device.device_id)
                .copied()
                .unwrap_or(DeviceTrust::NoKeys),
            current: device.device_id == current,
            display_name: device.display_name.filter(|name| !name.trim().is_empty()),
            last_seen_ms: device.last_seen_ms,
            device_id: DeviceId::from(device.device_id),
        })
        .collect();
    devices.sort_by(|a, b| {
        (!a.current, std::cmp::Reverse(a.last_seen_ms), &a.device_id).cmp(&(
            !b.current,
            std::cmp::Reverse(b.last_seen_ms),
            &b.device_id,
        ))
    });
    devices
}

/// The longest device name `RenameDevice` sends, in characters.
const MAX_DEVICE_NAME_CHARS: usize = 64;

/// The name to send for `RenameDevice`: trimmed and capped; an empty one is
/// refused.
pub(crate) fn clean_device_name(name: &str) -> Result<String, CoreError> {
    let capped: String = name.trim().chars().take(MAX_DEVICE_NAME_CHARS).collect();
    let capped = capped.trim_end();
    if capped.is_empty() {
        return Err(CoreError::NotAllowed);
    }
    Ok(capped.to_owned())
}

/// `SignOutDevices` never signs out the device the core runs on (that is
/// Logout), and needs at least one device.
pub(crate) fn sign_out_precheck(device_ids: &[DeviceId], current: &str) -> Result<(), CoreError> {
    if device_ids.is_empty() || device_ids.iter().any(|id| id.as_str() == current) {
        return Err(CoreError::NotAllowed);
    }
    Ok(())
}

/// The account-management page for signing out `device` (OAuth sessions);
/// with no device (several selected) the page that lists the devices. `None`
/// when the server advertises no account page.
pub(crate) fn account_management_target(
    metadata: &AuthorizationServerMetadata,
    device: Option<&SdkDeviceId>,
) -> Option<url::Url> {
    let action = match device {
        Some(device) => AccountManagementActionData::DeviceDelete(DeviceDeleteData::new(device)),
        None => AccountManagementActionData::DevicesList,
    };
    metadata.account_management_url_with_action(action)
}

/// The other user of a DM: the one target that is not `own`. A DM with no or
/// several other users has none (Q10: no badge).
pub(crate) fn dm_peer<'a>(targets: &[&'a UserId], own: Option<&UserId>) -> Option<&'a UserId> {
    let mut others = targets.iter().copied().filter(|user| Some(*user) != own);
    let peer = others.next()?;
    others.next().is_none().then_some(peer)
}

/// The peer of `room` when it is a DM with exactly one other user.
pub(crate) fn dm_peer_of(room: &matrix_sdk::Room, own: Option<&UserId>) -> Option<OwnedUserId> {
    // A target that is not a user ID (a third-party invite) means no badge.
    let targets: Vec<OwnedUserId> = room
        .direct_targets()
        .iter()
        .map(|target| target.as_user_id().map(ToOwned::to_owned))
        .collect::<Option<_>>()?;
    let refs: Vec<&UserId> = targets.iter().map(|user| &**user).collect();
    dm_peer(&refs, own).map(ToOwned::to_owned)
}

const SIGN_OUT_FAILED: &str = "the devices could not be signed out";
const MANAGE_ON_ACCOUNT_PAGE: &str = "manage your devices on your account page";

/// Why one `delete_devices` call failed.
#[derive(Debug)]
enum DeleteError {
    /// The homeserver wants user-interactive authentication.
    Uia(Box<UiaaInfo>),
    /// Anything else.
    Failed(CoreError),
}

/// The call a sign-out makes, so that the sequence can be tested.
trait DeleteApi {
    /// Deletes the devices, with `auth` when given.
    async fn delete(&self, auth: Option<AuthData>) -> Result<(), DeleteError>;
}

struct ClientDelete<'a> {
    client: &'a Client,
    devices: &'a [OwnedDeviceId],
}

impl DeleteApi for ClientDelete<'_> {
    async fn delete(&self, auth: Option<AuthData>) -> Result<(), DeleteError> {
        match self.client.delete_devices(self.devices, auth).await {
            Ok(_) => Ok(()),
            Err(error) => Err(match error.as_uiaa_response() {
                Some(info) => DeleteError::Uia(Box::new(info.clone())),
                // The SDK's text is not forwarded: it can quote the request.
                None => DeleteError::Failed(CoreError::Internal(SIGN_OUT_FAILED.into())),
            }),
        }
    }
}

/// Q13: the first call has no auth; a UIA answer asks for the password, or
/// with one retries once with the answer's session. A refused password is
/// `ReauthFailed`.
async fn delete_flow<A: DeleteApi>(
    api: &A,
    me: &UserId,
    password: Option<&Password>,
) -> Result<(), Stop> {
    let info = match api.delete(None).await {
        Ok(()) => return Ok(()),
        Err(DeleteError::Failed(error)) => return Err(error.into()),
        Err(DeleteError::Uia(info)) => info,
    };
    match uia_step(&info, me, password) {
        ResetStep::NeedPassword => Err(Stop::Reauth(ReauthAction::DeleteDevices)),
        ResetStep::Retry(auth) => api.delete(Some(auth)).await.map_err(|error| match error {
            DeleteError::Uia(info) if auth_rejected(Some(&info)) => {
                Stop::Failed(CoreError::ReauthFailed)
            }
            DeleteError::Uia(_) => Stop::Failed(CoreError::Internal(SIGN_OUT_FAILED.into())),
            DeleteError::Failed(error) => Stop::Failed(error),
        }),
        ResetStep::Done | ResetStep::Approve(_) | ResetStep::Unsupported => {
            Err(CoreError::Internal(SIGN_OUT_FAILED.into()).into())
        }
    }
}

/// OAuth sessions sign devices out on the account page: emits its URL, or
/// tells the user to go there themselves.
async fn open_account_page(
    client: &Client,
    devices: &[OwnedDeviceId],
    events: &mpsc::Sender<CoreEvent>,
) {
    let target = match client.oauth().server_metadata().await {
        Ok(metadata) => {
            let device = match devices {
                [one] => Some(&**one),
                _ => None,
            };
            account_management_target(&metadata, device)
        }
        Err(_) => None,
    };
    let url = target.and_then(|url| check_auth_url(url.as_str()).ok());
    let event = match url {
        Some(url) => CoreEvent::OpenAccountManagement(url),
        None => CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: ActionKind::SignOutDevices,
            error: CoreError::Internal(MANAGE_ON_ACCOUNT_PAGE.into()),
        },
    };
    let _ = events.send(event).await;
}

/// The account's devices as the user sees them.
async fn read_devices(client: &Client) -> Result<Vec<DeviceSummary>, CoreError> {
    let (Some(me), Some(current)) = (client.user_id(), client.device_id()) else {
        return Err(CoreError::NotLoggedIn);
    };
    let response = client
        .devices()
        .await
        .map_err(|_| CoreError::Network("the devices could not be loaded".into()))?;
    let server = response
        .devices
        .into_iter()
        .map(|device| ServerDevice {
            device_id: device.device_id.to_string(),
            display_name: device.display_name,
            last_seen_ms: device.last_seen_ts.map(|ts| u64::from(ts.0)),
        })
        .collect();
    // No keys query here: a device the store does not know yet is `NoKeys`
    // until the sync's own query brings it (the stream below re-sends then).
    let trust = match client.encryption().get_user_devices(me).await {
        Ok(devices) => devices
            .devices()
            .map(|device| {
                let trust = if device.is_verified() {
                    DeviceTrust::Verified
                } else {
                    DeviceTrust::Unverified
                };
                (device.device_id().to_string(), trust)
            })
            .collect(),
        Err(_) => HashMap::new(),
    };
    Ok(merge_devices(server, &trust, current.as_str()))
}

/// Sends `Devices`, or the reason it could not be read.
async fn send_devices(client: &Client, events: &mpsc::Sender<CoreEvent>, failure: ActionKind) {
    let event = match read_devices(client).await {
        Ok(devices) => CoreEvent::Devices(devices),
        Err(error) => CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action: failure,
            error,
        },
    };
    let _ = events.send(event).await;
}

/// Sends `Devices` now, and again whenever the crypto store's devices change
/// (a new device, a changed trust) and the list differs from the last one.
async fn watch_devices(client: Client, events: mpsc::Sender<CoreEvent>) {
    // Subscribe first, so no change between the stream and the read is lost.
    let mut updates = client
        .encryption()
        .devices_stream()
        .await
        .ok()
        .map(|stream| stream.boxed());
    let mut last: Option<Vec<DeviceSummary>> = None;
    loop {
        match read_devices(&client).await {
            Ok(devices) => {
                if last.as_ref() != Some(&devices) {
                    last = Some(devices.clone());
                    if events.send(CoreEvent::Devices(devices)).await.is_err() {
                        return;
                    }
                }
            }
            // The first read reports its failure; later ones keep the list.
            Err(error) if last.is_none() => {
                let failed = CoreEvent::ActionFailed {
                    room_id: RoomId::from(""),
                    action: ActionKind::Devices,
                    error,
                };
                if events.send(failed).await.is_err() {
                    return;
                }
                last = Some(Vec::new());
            }
            Err(_) => {}
        }
        let Some(stream) = updates.as_mut() else {
            return;
        };
        if stream.next().await.is_none() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::encryption::OAuthCrossSigningResetInfo;
    use matrix_sdk::ruma::api::client::uiaa::{AuthFlow, AuthType, UiaaInfo, UserIdentifier};
    use matrix_sdk::ruma::owned_user_id;

    use super::*;
    use crate::api::{CrossSigning, DeviceVerification, RecoveryStatus};

    #[test]
    fn a_stop_for_the_password_announces_nothing() {
        assert!(announces_end(&Ok(())));
        assert!(announces_end(&Err(Stop::Failed(CoreError::Busy))));
        assert!(!announces_end(&Err(Stop::Reauth(
            ReauthAction::ResetIdentity
        ))));
    }

    /// A stand-in for `forward_status`: one `forward_round` per wake-up of
    /// `refresh`, after a first one at once when `queued` is set (so that it is
    /// already waiting for `paused` when the operation ends).
    fn fake_forwarder(
        op: &Arc<IdentityOp>,
        refresh: &Arc<Notify>,
        events: &mpsc::Sender<CoreEvent>,
        queued: bool,
    ) -> JoinHandle<()> {
        let (op, refresh, events) = (op.clone(), refresh.clone(), events.clone());
        tokio::spawn(async move {
            let mut last = None;
            if !queued {
                refresh.notified().await;
            }
            loop {
                let read = std::future::ready(SecurityStatus::default());
                if !forward_round(&op, &mut last, &events, read).await {
                    return;
                }
                refresh.notified().await;
            }
        })
    }

    /// Ends an operation that stops for the password and returns whether the
    /// forwarder's status was already sent when that returned, that is, before
    /// the stop would be reported.
    async fn status_is_out_when_the_stop_returns(queued: bool) -> bool {
        let op = Arc::new(IdentityOp::default());
        let refresh = Arc::new(Notify::new());
        let (tx, mut rx) = unlimited_events();
        let paused = op.paused.lock().await;
        let forwarder = fake_forwarder(&op, &refresh, &tx, queued);
        // Let the forwarder run up to the point where it waits.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let stop = Err(Stop::Reauth(ReauthAction::ResetIdentity));
        announce_end(&op, &refresh, &stop, paused).await;
        let out = matches!(rx.try_recv(), Ok(CoreEvent::Security(_)));
        assert!(
            !op.announce.load(Ordering::Acquire),
            "a stop for the password announces nothing"
        );
        forwarder.abort();
        out
    }

    #[tokio::test]
    async fn a_status_waiting_for_the_lock_goes_out_before_the_stop() {
        assert!(status_is_out_when_the_stop_returns(true).await);
    }

    #[tokio::test]
    async fn a_forwarder_not_yet_waiting_sends_before_the_stop_too() {
        assert!(status_is_out_when_the_stop_returns(false).await);
    }

    #[tokio::test]
    async fn a_success_sends_the_status_again_before_it_returns() {
        let op = Arc::new(IdentityOp::default());
        let refresh = Arc::new(Notify::new());
        let (tx, mut rx) = unlimited_events();
        // The forwarder has sent this status already.
        let first = fake_forwarder(&op, &refresh, &tx, true);
        assert!(matches!(next_event(&mut rx).await, CoreEvent::Security(_)));
        let paused = op.paused.lock().await;
        announce_end(&op, &refresh, &Ok(()), paused).await;
        // The same status again, out before the answer would be sent.
        assert!(matches!(rx.try_recv(), Ok(CoreEvent::Security(_))));
        assert!(
            !op.announce.load(Ordering::Acquire),
            "the forwarder took the announcement"
        );
        first.abort();
    }

    #[test]
    fn user_trust_follows_the_identity_facts() {
        let facts = |verified, violation| {
            Some(IdentityFacts {
                verified,
                violation,
            })
        };
        let cases = [
            (None, UserTrust::NoIdentity),
            (facts(true, false), UserTrust::Verified),
            (facts(false, false), UserTrust::Unverified),
            (facts(false, true), UserTrust::IdentityChanged),
            // A violation wins over a stale verified flag.
            (facts(true, true), UserTrust::IdentityChanged),
        ];
        for (identity, want) in cases {
            assert_eq!(map_user_trust(identity), want, "{identity:?}");
        }
    }

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
    fn recovery_errors_map_to_fixed_messages() {
        use matrix_sdk::encryption::secret_storage::SecretStorageError;
        use matrix_sdk_crypto::secret_storage::{DecodeError, MacError};
        const WHAT: &str = "fallback";
        let internal = |m: &str| CoreError::Internal(m.into());
        assert_eq!(
            map_recovery_error(&RecoveryError::BackupExistsOnServer, WHAT),
            internal(BACKUP_EXISTS)
        );
        assert_eq!(
            BACKUP_EXISTS,
            "a key backup already exists; enter your recovery key"
        );
        // A wrong key or passphrase: the key does not decode or fails its MAC
        // check. The SDK's text can quote parts of the input, so it is dropped.
        for decode in [
            DecodeError::Parity(1, 2),
            DecodeError::Prefix([0, 1], [2, 3]),
            DecodeError::KeyLength(32, 3),
            DecodeError::Mac(MacError),
        ] {
            let error = RecoveryError::SecretStorage(SecretStorageError::SecretStorageKey(decode));
            assert_eq!(map_recovery_error(&error, WHAT), internal(WRONG_KEY));
        }
        assert_eq!(WRONG_KEY, "that recovery key or passphrase is not correct");
        // Recovery was never set up on the server.
        let missing =
            RecoveryError::SecretStorage(SecretStorageError::MissingKeyInfo { key_id: None });
        assert_eq!(map_recovery_error(&missing, WHAT), internal(NOT_SET_UP));
        // Anything else keeps the caller's fixed message.
        let other =
            RecoveryError::SecretStorage(SecretStorageError::MissingOrInvalidBackupDecryptionKey);
        assert_eq!(map_recovery_error(&other, WHAT), internal(WHAT));
    }

    #[test]
    fn enable_progress_never_forwards_the_key() {
        use matrix_sdk_crypto::store::types::RoomKeyCounts;
        let counts = RoomKeyCounts {
            total: 7,
            backed_up: 3,
        };
        assert_eq!(
            map_progress(&EnableProgress::Starting),
            Some(RecoveryProgress::Starting)
        );
        assert_eq!(
            map_progress(&EnableProgress::CreatingBackup),
            Some(RecoveryProgress::CreatingBackup)
        );
        assert_eq!(
            map_progress(&EnableProgress::CreatingRecoveryKey),
            Some(RecoveryProgress::CreatingKey)
        );
        assert_eq!(
            map_progress(&EnableProgress::BackingUp(counts)),
            Some(RecoveryProgress::BackingUp { done: 3, total: 7 })
        );
        assert_eq!(
            map_progress(&EnableProgress::RoomKeyUploadError),
            Some(RecoveryProgress::UploadFailed)
        );
        let done = EnableProgress::Done {
            recovery_key: "secret-key-x".into(),
        };
        assert_eq!(map_progress(&done), None);
    }

    #[test]
    fn enabling_and_changing_the_key_are_guarded_by_the_recovery_state() {
        let internal = |m: &str| Err(CoreError::Internal(m.into()));
        // Enable only where nothing is set up: over an existing secret store
        // it would replace the key with one that lacks the secrets this
        // device is missing.
        assert_eq!(enable_precheck(RecoveryState::Disabled), Ok(()));
        assert_eq!(
            enable_precheck(RecoveryState::Incomplete),
            internal(BACKUP_EXISTS)
        );
        assert_eq!(
            enable_precheck(RecoveryState::Enabled),
            internal(ALREADY_ON)
        );
        assert_eq!(
            enable_precheck(RecoveryState::Unknown),
            internal(STATE_UNKNOWN)
        );
        // Changing the key re-uploads the secrets this device holds, so it
        // needs all of them.
        assert_eq!(change_key_precheck(RecoveryState::Enabled), Ok(()));
        assert_eq!(
            change_key_precheck(RecoveryState::Disabled),
            internal(NOT_SET_UP)
        );
        assert_eq!(
            change_key_precheck(RecoveryState::Incomplete),
            internal(NEEDS_KEY_FIRST)
        );
        assert_eq!(
            change_key_precheck(RecoveryState::Unknown),
            internal(STATE_UNKNOWN)
        );
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

    fn unlimited_events() -> (mpsc::Sender<CoreEvent>, mpsc::Receiver<CoreEvent>) {
        mpsc::channel(16)
    }

    async fn next_event(rx: &mut mpsc::Receiver<CoreEvent>) -> CoreEvent {
        tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("an event within 5 s")
            .expect("the channel is open")
    }

    type BoxedWork = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<CoreEvent>, Stop>> + Send>,
    >;

    /// A fake identity operation that runs until `release` is notified and
    /// then ends like `reset_task` does: `Cancelled` when a cancel came.
    fn blocking_work(
        op: Arc<IdentityOp>,
        started: &Arc<Notify>,
        release: &Arc<Notify>,
    ) -> impl FnOnce() -> BoxedWork + use<> {
        let (started, release) = (started.clone(), release.clone());
        move || {
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                if op.cancelled.load(Ordering::Acquire) {
                    return Err(CoreError::Cancelled.into());
                }
                Ok(None)
            })
        }
    }

    #[tokio::test]
    async fn a_second_identity_operation_in_flight_gets_busy() {
        let (tx, mut rx) = unlimited_events();
        let mut tasks = SecurityTasks::new();
        let (started, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        tasks.spawn_identity(
            &tx,
            ActionKind::CrossSigning,
            blocking_work(tasks.identity.clone(), &started, &release),
        );
        started.notified().await;

        tasks.spawn_identity(
            &tx,
            ActionKind::CrossSigning,
            blocking_work(tasks.identity.clone(), &started, &release),
        );
        match next_event(&mut rx).await {
            CoreEvent::ActionFailed { action, error, .. } => {
                assert_eq!(action, ActionKind::CrossSigning);
                assert_eq!(error, CoreError::Busy);
            }
            other => panic!("unexpected {other:?}"),
        }

        // The first one finishes without any event, and frees the slot.
        release.notify_one();
        tasks.tasks.join_next().await;
        tasks.tasks.join_next().await;
        assert!(rx.try_recv().is_err());
        assert!(tasks.identity.try_start().is_some(), "the slot is free");
    }

    #[tokio::test]
    async fn cancel_during_an_identity_operation_ends_it_with_cancelled() {
        let (tx, mut rx) = unlimited_events();
        let mut tasks = SecurityTasks::new();
        let (started, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        tasks.spawn_identity(
            &tx,
            ActionKind::CrossSigning,
            blocking_work(tasks.identity.clone(), &started, &release),
        );
        started.notified().await;

        tasks.cancel_reset();
        assert!(
            tasks.identity.cancelled.load(Ordering::Acquire),
            "the running operation must see the cancel"
        );
        release.notify_one();
        match next_event(&mut rx).await {
            CoreEvent::ActionFailed { action, error, .. } => {
                assert_eq!(action, ActionKind::CrossSigning);
                assert_eq!(error, CoreError::Cancelled);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_cancel_without_an_operation_does_not_hit_the_next_one() {
        let (tx, mut rx) = unlimited_events();
        let mut tasks = SecurityTasks::new();
        tasks.cancel_reset();
        let (started, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        tasks.spawn_identity(
            &tx,
            ActionKind::CrossSigning,
            blocking_work(tasks.identity.clone(), &started, &release),
        );
        started.notified().await;
        release.notify_one();
        tasks.tasks.join_next().await;
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_success_answer_comes_after_the_slot_is_free() {
        let (tx, mut rx) = unlimited_events();
        let mut tasks = SecurityTasks::new();
        tasks.spawn_identity(&tx, ActionKind::Recovery, || async {
            Ok(Some(CoreEvent::Recovered))
        });
        assert!(matches!(next_event(&mut rx).await, CoreEvent::Recovered));
        // A front end that reacts to the answer can start the next operation.
        assert!(
            tasks.identity.try_start().is_some(),
            "the slot is still held"
        );
    }

    #[tokio::test]
    async fn a_failed_recovery_action_is_reported_as_recovery() {
        let (tx, mut rx) = unlimited_events();
        let mut tasks = SecurityTasks::new();
        tasks.spawn_identity(&tx, ActionKind::Recovery, || async {
            Err(Stop::Failed(CoreError::Internal(WRONG_KEY.into())))
        });
        match next_event(&mut rx).await {
            CoreEvent::ActionFailed {
                room_id,
                action,
                error,
            } => {
                assert!(room_id.as_str().is_empty());
                assert_eq!(action, ActionKind::Recovery);
                assert_eq!(error, CoreError::Internal(WRONG_KEY.into()));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_rejected_approval_url_is_an_internal_error_with_a_fixed_message() {
        let bad = url::Url::parse("http://evil.example.org/approve?s=1").unwrap();
        let error = approval_url(&bad).expect_err("plain http to a remote host");
        assert!(
            matches!(&error, CoreError::Internal(m) if !m.contains("evil")),
            "{error:?}"
        );
        let ok = url::Url::parse("https://account.example.org/reset?s=1").unwrap();
        assert!(approval_url(&ok).is_ok());
    }

    #[test]
    fn a_bootstrap_retry_needs_a_confirmed_absence_of_an_identity() {
        assert!(bootstrap_retry_allowed(Some(false)).is_ok());
        for known in [Some(true), None] {
            let error = bootstrap_retry_allowed(known).expect_err("must not upload");
            assert!(matches!(error, CoreError::Internal(_)), "{error:?}");
        }
    }

    #[test]
    fn bootstrap_plan_table() {
        use BootstrapPlan::*;
        let cases = [
            // The server has one: never upload, whatever the local store says.
            (Some(true), false, AlreadySetUp),
            (Some(true), true, AlreadySetUp),
            // Nothing anywhere: the first attempt.
            (Some(false), false, FirstAttempt),
            // A failed first attempt left a local identity the server lacks.
            (Some(false), true, UploadLocal),
            // Cannot tell: do nothing.
            (None, false, Unverifiable),
            (None, true, Unverifiable),
        ];
        for (server, local, want) in cases {
            assert_eq!(bootstrap_plan(server, local), want, "{server:?} {local}");
        }
    }

    #[test]
    fn a_local_identity_never_proves_the_server_has_one() {
        // A pending upload: the store has the identity, the server does not.
        assert_eq!(identity_known(Some(true), true, Some(false)), Some(false));
        let status = compute_status(&StatusInputs {
            identity_exists: identity_known(Some(true), true, Some(false)),
            private_keys: keys(true, true, true),
            ..inputs()
        });
        assert_eq!(status.cross_signing, CrossSigning::NotSetUp);
        assert!(!status.identity_exists);
        // Server unreachable: unknown, not complete.
        assert_eq!(identity_known(Some(true), true, None), None);
        assert_eq!(identity_known(Some(true), false, Some(true)), Some(true));
        // No local identity: "none" only after the server was asked.
        assert_eq!(identity_known(Some(false), true, None), Some(false));
        assert_eq!(identity_known(Some(false), false, None), None);
        assert_eq!(identity_known(None, true, Some(true)), None);
    }

    #[test]
    fn a_password_session_needs_the_password_before_the_reset_starts() {
        let pw = Password::from("secret-pw-x");
        assert!(reset_needs_password_first(false, None));
        assert!(!reset_needs_password_first(false, Some(&pw)));
        // OAuth never takes a password.
        assert!(!reset_needs_password_first(true, None));
        assert!(!reset_needs_password_first(true, Some(&pw)));
    }

    #[test]
    fn a_reset_is_complete_only_with_all_three_private_keys() {
        assert!(reset_complete(keys(true, true, true)).is_ok());
        for status in [
            keys(true, true, false),
            keys(false, true, true),
            keys(false, false, false),
            None,
        ] {
            assert!(reset_complete(status).is_err());
        }
    }

    #[test]
    fn reset_end_table() {
        use ResetEnd::*;
        let cases = [
            (true, false, None, Success),
            (false, false, None, Failed),
            // A cancel that raced an approval that did upload.
            (true, true, Some(true), Success),
            // After a cancel the SDK answers Ok without having uploaded.
            (true, true, Some(false), Cancelled),
            (true, true, None, Cancelled),
            (false, true, None, Cancelled),
            (false, true, Some(false), Cancelled),
        ];
        for (ok, cancelled, uploaded, want) in cases {
            assert_eq!(
                reset_end(ok, cancelled, uploaded),
                want,
                "{ok} {cancelled} {uploaded:?}"
            );
        }
    }

    /// A scripted server and store for the bootstrap sequence.
    struct Fake {
        /// The server holds an identity.
        server: std::sync::Mutex<Option<bool>>,
        /// The local store holds an identity.
        local: AtomicBool,
        /// The server demands UIA for an upload without auth.
        demands_uia: bool,
        /// Calls, in order.
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(server: Option<bool>, local: bool, demands_uia: bool) -> Self {
            Self {
                server: std::sync::Mutex::new(server),
                local: AtomicBool::new(local),
                demands_uia,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn log(&self, call: &str) {
            self.calls.lock().unwrap().push(call.to_owned());
        }

        fn uia() -> AttemptError {
            AttemptError::Uia(Box::new(UiaaInfo::new(vec![AuthFlow::new(vec![
                AuthType::Password,
            ])])))
        }
    }

    impl BootstrapApi for Fake {
        async fn server_has_identity(&self) -> Option<bool> {
            self.log("server");
            *self.server.lock().unwrap()
        }

        async fn local_identity(&self) -> bool {
            self.local.load(Ordering::Acquire)
        }

        async fn sync_identity(&self) {
            self.log("sync");
        }

        async fn first_attempt(&self) -> Result<(), AttemptError> {
            self.log("first");
            // Like the SDK: the identity is saved locally before the upload.
            let had = self.local.swap(true, Ordering::AcqRel);
            if had {
                return Ok(());
            }
            if self.demands_uia {
                return Err(Self::uia());
            }
            *self.server.lock().unwrap() = Some(true);
            Ok(())
        }

        async fn upload(&self, auth: Option<AuthData>) -> Result<(), AttemptError> {
            self.log(if auth.is_some() {
                "upload+auth"
            } else {
                "upload"
            });
            if auth.is_none() && self.demands_uia {
                return Err(Self::uia());
            }
            *self.server.lock().unwrap() = Some(true);
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_failed_first_bootstrap_is_retried_with_the_password() {
        let me = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");
        let fake = Fake::new(Some(false), false, true);

        // Without a password: asked for it, the local identity is left.
        let result = bootstrap_flow(&fake, &me, None).await;
        assert!(
            matches!(
                result,
                Err(Stop::Reauth(ReauthAction::BootstrapCrossSigning))
            ),
            "{result:?}"
        );
        assert!(fake.local.load(Ordering::Acquire));
        assert_eq!(fake.calls(), ["server", "first"]);

        // The next request with the password goes straight to the upload.
        let result = bootstrap_flow(&fake, &me, Some(&pw)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            fake.calls(),
            [
                "server",
                "first",
                "server",
                "upload",
                "server",
                "upload+auth"
            ]
        );
        assert_eq!(*fake.server.lock().unwrap(), Some(true));

        // And now the server has it: nothing more is uploaded.
        let result = bootstrap_flow(&fake, &me, Some(&pw)).await;
        assert!(result.is_ok());
        assert_eq!(fake.calls().last().map(String::as_str), Some("sync"));
        assert!(!fake.calls()[6..].iter().any(|c| c.starts_with("upload")));
    }

    #[tokio::test]
    async fn a_password_on_the_first_request_retries_once_with_the_session() {
        let me = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");
        let fake = Fake::new(Some(false), false, true);
        let result = bootstrap_flow(&fake, &me, Some(&pw)).await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(fake.calls(), ["server", "first", "server", "upload+auth"]);
    }

    #[tokio::test]
    async fn a_server_identity_seen_late_is_never_replaced() {
        let me = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");
        // The first attempt wants UIA, and by the retry the server has one.
        struct Late(Fake);
        impl BootstrapApi for Late {
            async fn server_has_identity(&self) -> Option<bool> {
                let seen = self.0.server_has_identity().await;
                // The first question says "none", later ones "exists".
                *self.0.server.lock().unwrap() = Some(true);
                seen
            }
            async fn local_identity(&self) -> bool {
                self.0.local_identity().await
            }
            async fn sync_identity(&self) {
                self.0.sync_identity().await;
            }
            async fn first_attempt(&self) -> Result<(), AttemptError> {
                self.0.first_attempt().await
            }
            async fn upload(&self, auth: Option<AuthData>) -> Result<(), AttemptError> {
                self.0.upload(auth).await
            }
        }
        let late = Late(Fake::new(Some(false), false, true));
        let result = bootstrap_flow(&late, &me, Some(&pw)).await;
        assert!(matches!(result, Err(Stop::Failed(CoreError::Internal(_)))));
        assert!(!late.0.calls().iter().any(|c| c.starts_with("upload")));
    }

    #[tokio::test]
    async fn an_unreachable_server_uploads_nothing() {
        let me = owned_user_id!("@alice:example.org");
        let fake = Fake::new(None, true, false);
        let result = bootstrap_flow(&fake, &me, None).await;
        assert!(matches!(result, Err(Stop::Failed(CoreError::Internal(_)))));
        assert_eq!(fake.calls(), ["server"]);
    }
}

#[cfg(test)]
mod device_tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use matrix_sdk::ruma::api::client::uiaa::{AuthFlow, AuthType, UiaaInfo};
    use matrix_sdk::ruma::api::error::{ErrorKind, StandardErrorBody};
    use matrix_sdk::ruma::{device_id, owned_user_id, user_id};

    use super::*;
    use crate::api::{DeviceId, DeviceSummary, DeviceTrust};

    fn server(id: &str, name: Option<&str>, seen: Option<u64>) -> ServerDevice {
        ServerDevice {
            device_id: id.to_owned(),
            display_name: name.map(str::to_owned),
            last_seen_ms: seen,
        }
    }

    #[test]
    fn devices_list_the_current_one_first_then_the_latest_seen() {
        let list = vec![
            server("OLD", Some("Old"), Some(10)),
            server("ME", Some("Mine"), Some(5)),
            server("NEW", Some("New"), Some(99)),
            server("NEVER", None, None),
            server("AAA", None, Some(10)),
        ];
        let trust = HashMap::from([
            ("ME".to_owned(), DeviceTrust::Verified),
            ("OLD".to_owned(), DeviceTrust::Unverified),
            ("NEW".to_owned(), DeviceTrust::Verified),
            ("NEVER".to_owned(), DeviceTrust::Unverified),
            ("AAA".to_owned(), DeviceTrust::Unverified),
        ]);
        let merged = merge_devices(list, &trust, "ME");
        let ids: Vec<&str> = merged.iter().map(|d| d.device_id.as_str()).collect();
        // Never-seen devices go last; equal times order by device ID.
        assert_eq!(ids, ["ME", "NEW", "AAA", "OLD", "NEVER"]);
        assert!(merged[0].current);
        assert!(merged[1..].iter().all(|d| !d.current));
    }

    #[test]
    fn a_device_without_crypto_keys_is_no_keys_and_names_may_be_missing() {
        let list = vec![
            server("ME", Some("Mine"), Some(1)),
            server("BARE", None, Some(2)),
            server("BLANK", Some("  "), Some(3)),
        ];
        let trust = HashMap::from([("ME".to_owned(), DeviceTrust::Verified)]);
        let merged = merge_devices(list, &trust, "ME");
        let by_id = |id: &str| -> &DeviceSummary {
            merged.iter().find(|d| d.device_id.as_str() == id).unwrap()
        };
        assert_eq!(by_id("BARE").trust, DeviceTrust::NoKeys);
        assert_eq!(by_id("BARE").display_name, None);
        assert_eq!(by_id("BLANK").display_name, None);
        assert_eq!(by_id("ME").display_name.as_deref(), Some("Mine"));
        assert_eq!(by_id("ME").trust, DeviceTrust::Verified);
        assert_eq!(by_id("BARE").last_seen_ms, Some(2));
    }

    #[test]
    fn an_empty_server_list_gives_an_empty_device_list() {
        let merged = merge_devices(vec![], &HashMap::new(), "ME");
        assert!(merged.is_empty());
    }

    #[test]
    fn device_names_are_trimmed_capped_and_never_empty() {
        assert_eq!(clean_device_name("  Laptop  "), Ok("Laptop".to_owned()));
        assert_eq!(clean_device_name(""), Err(CoreError::NotAllowed));
        assert_eq!(clean_device_name(" \t\n "), Err(CoreError::NotAllowed));
        let long = "é".repeat(100);
        let cleaned = clean_device_name(&long).unwrap();
        assert_eq!(cleaned.chars().count(), 64);
        assert!(long.starts_with(&cleaned));
        let exact = "x".repeat(64);
        assert_eq!(clean_device_name(&exact), Ok(exact.clone()));
    }

    #[test]
    fn signing_out_the_current_device_or_nothing_is_refused() {
        let ids = |v: &[&str]| v.iter().map(|s| DeviceId::from(*s)).collect::<Vec<_>>();
        assert_eq!(sign_out_precheck(&ids(&["A", "B"]), "ME"), Ok(()));
        assert_eq!(
            sign_out_precheck(&ids(&["A", "ME"]), "ME"),
            Err(CoreError::NotAllowed)
        );
        assert_eq!(sign_out_precheck(&[], "ME"), Err(CoreError::NotAllowed));
    }

    fn metadata(extra: serde_json::Value) -> AuthorizationServerMetadata {
        let mut json = serde_json::json!({
            "issuer": "https://server.local/",
            "authorization_endpoint": "https://server.local/authorize",
            "token_endpoint": "https://server.local/token",
            "response_types_supported": ["code"],
            "response_modes_supported": ["query", "fragment"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "revocation_endpoint": "https://server.local/revoke",
            "code_challenge_methods_supported": ["S256"],
        });
        json.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn the_account_page_url_names_the_action_and_the_device() {
        let m = metadata(serde_json::json!({
            "account_management_uri": "https://server.local/account",
            "account_management_actions_supported": ["org.matrix.device_delete"],
        }));
        let url = account_management_target(&m, Some(device_id!("DEV1"))).unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("server.local"));
        let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["action"], "org.matrix.device_delete");
        assert_eq!(pairs["device_id"], "DEV1");

        // Without a device (several selected): the devices list.
        let url = account_management_target(&m, None).unwrap();
        let pairs: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["action"], "org.matrix.devices_list");
        assert!(!pairs.contains_key("device_id"));
    }

    #[test]
    fn no_account_page_means_no_url() {
        let m = metadata(serde_json::json!({}));
        assert!(account_management_target(&m, Some(device_id!("DEV1"))).is_none());
        assert!(account_management_target(&m, None).is_none());
    }

    #[test]
    fn the_dm_peer_is_the_one_other_user() {
        let me = user_id!("@me:example.org");
        let bob = user_id!("@bob:example.org");
        let carol = user_id!("@carol:example.org");
        assert_eq!(dm_peer(&[bob], Some(me)), Some(bob));
        // Own ID in the targets does not count.
        assert_eq!(dm_peer(&[me, bob], Some(me)), Some(bob));
        assert_eq!(dm_peer(&[bob], None), Some(bob));
        assert_eq!(dm_peer(&[bob, carol], Some(me)), None);
        assert_eq!(dm_peer(&[], Some(me)), None);
        assert_eq!(dm_peer(&[me], Some(me)), None);
    }

    /// A fake `delete_devices`.
    struct FakeDelete {
        /// The server asks for UIA when no auth comes.
        demands_uia: bool,
        /// The server refuses the password it gets.
        refuses: bool,
        flows: Vec<Vec<AuthType>>,
        seen: Mutex<Vec<Option<AuthData>>>,
    }

    impl FakeDelete {
        fn new(demands_uia: bool, refuses: bool) -> Self {
            Self {
                demands_uia,
                refuses,
                flows: vec![vec![AuthType::Password]],
                seen: Mutex::new(Vec::new()),
            }
        }

        fn info(&self, rejected: bool) -> Box<UiaaInfo> {
            let flows = self.flows.iter().cloned().map(AuthFlow::new).collect();
            let mut info = UiaaInfo::new(flows);
            info.session = Some("uia-session-1".into());
            if rejected {
                info.auth_error = Some(Box::new(StandardErrorBody::new(
                    ErrorKind::Forbidden,
                    "refused".into(),
                )));
            }
            Box::new(info)
        }
    }

    impl DeleteApi for FakeDelete {
        async fn delete(&self, auth: Option<AuthData>) -> Result<(), DeleteError> {
            self.seen.lock().unwrap().push(auth.clone());
            match auth {
                None if self.demands_uia => Err(DeleteError::Uia(self.info(false))),
                None => Ok(()),
                Some(_) if self.refuses => Err(DeleteError::Uia(self.info(true))),
                Some(_) => Ok(()),
            }
        }
    }

    #[tokio::test]
    async fn sign_out_without_uia_succeeds_at_once() {
        let me = owned_user_id!("@alice:example.org");
        let fake = FakeDelete::new(false, false);
        assert_eq!(delete_flow(&fake, &me, None).await, Ok(()));
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn sign_out_asks_for_the_password_when_the_server_does() {
        let me = owned_user_id!("@alice:example.org");
        let fake = FakeDelete::new(true, false);
        assert_eq!(
            delete_flow(&fake, &me, None).await,
            Err(Stop::Reauth(ReauthAction::DeleteDevices))
        );
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn sign_out_retries_once_with_the_password_and_the_session() {
        let me = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");
        let fake = FakeDelete::new(true, false);
        assert_eq!(delete_flow(&fake, &me, Some(&pw)).await, Ok(()));
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].is_none());
        let Some(AuthData::Password(auth)) = &seen[1] else {
            panic!("the retry carries password auth");
        };
        assert_eq!(auth.session.as_deref(), Some("uia-session-1"));
        assert_eq!(auth.password, "secret-pw-x");
    }

    #[tokio::test]
    async fn a_refused_password_is_reauth_failed_without_the_password_in_the_error() {
        let me = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");
        let fake = FakeDelete::new(true, true);
        let result = delete_flow(&fake, &me, Some(&pw)).await;
        assert_eq!(result, Err(Stop::Failed(CoreError::ReauthFailed)));
        assert!(!format!("{result:?}").contains("secret-pw-x"));
        // One retry only.
        assert_eq!(fake.seen.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_server_that_takes_no_password_cannot_sign_devices_out() {
        let me = owned_user_id!("@alice:example.org");
        let pw = Password::from("secret-pw-x");
        let mut fake = FakeDelete::new(true, false);
        fake.flows = vec![vec![AuthType::Sso]];
        let result = delete_flow(&fake, &me, Some(&pw)).await;
        assert!(matches!(result, Err(Stop::Failed(CoreError::Internal(_)))));
        assert!(!format!("{result:?}").contains("secret-pw-x"));
    }
}
