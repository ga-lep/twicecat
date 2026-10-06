//! Interactive SAS verification (plan Q8, Q9): incoming and outgoing
//! requests, driven into [`CoreEvent`]s. Private to the core: SDK types stay
//! in here.
//!
//! One flow is open at a time ([`Slot`]). Every flow has one driver task that
//! follows the request's state and then the SAS state, and sends
//! `Verification` events; commands act on the open flow in short tasks. All
//! tasks live in one `JoinSet`, ended by [`VerificationTasks::stop`] before
//! the client is dropped.
//!
//! Nothing here logs or prints the short authentication string, a flow's
//! secrets or a device name.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::StreamExt as _;
use futures_util::stream::BoxStream;
use matrix_sdk::Client;
use matrix_sdk::encryption::verification::{
    CancelInfo, SasState, SasVerification, VerificationRequest, VerificationRequestState,
};
use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::ToDeviceEvent;
use matrix_sdk::ruma::events::key::verification::cancel::CancelCode;
use matrix_sdk::ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEventContent;
use matrix_sdk::ruma::events::room::message::{MessageType, OriginalSyncRoomMessageEvent};
use matrix_sdk::ruma::{OwnedUserId, UserId as SdkUserId};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::api::{
    ActionKind, CancelReason, CoreError, CoreEvent, DeviceId, RoomId, SasEmoji, UserId,
    VerificationId, VerificationPeer, VerificationPhase,
};

const START_FAILED: &str = "the verification could not be started";
const NO_IDENTITY: &str = "there is no identity to verify with";
const NO_DEVICE: &str = "that device is not known";
const BAD_USER: &str = "that is not a valid user ID";
const NO_FLOW: &str = "there is no such verification";
const ACCEPT_FAILED: &str = "the verification request could not be accepted";
const CONFIRM_FAILED: &str = "the verification could not be confirmed";
const NOT_COMPARING: &str = "there is nothing to compare yet";
const MISMATCH_FAILED: &str = "the mismatch could not be reported";
const CANCEL_FAILED: &str = "the verification could not be cancelled";

/// The reason a flow ended, from the cancel code and who cancelled.
pub(crate) fn map_cancel_code(code: &CancelCode, by_us: bool) -> CancelReason {
    match code {
        CancelCode::User if by_us => CancelReason::ByUs,
        CancelCode::User => CancelReason::ByThem,
        CancelCode::MismatchedSas => CancelReason::Mismatch,
        CancelCode::Timeout => CancelReason::Timeout,
        CancelCode::Accepted => CancelReason::AcceptedElsewhere,
        _ => CancelReason::Other,
    }
}

fn cancelled_phase(info: &CancelInfo) -> VerificationPhase {
    VerificationPhase::Cancelled {
        reason: map_cancel_code(info.cancel_code(), info.cancelled_by_us()),
    }
}

/// The phase a request state stands for; `None` when the SAS flow it moved
/// into takes over.
pub(crate) fn map_request_state(state: &VerificationRequestState) -> Option<VerificationPhase> {
    match state {
        VerificationRequestState::Created { .. } => {
            Some(VerificationPhase::Requested { incoming: false })
        }
        VerificationRequestState::Requested { .. } => {
            Some(VerificationPhase::Requested { incoming: true })
        }
        VerificationRequestState::Ready { .. } => Some(VerificationPhase::Ready),
        VerificationRequestState::Transitioned { .. } => None,
        VerificationRequestState::Done => Some(VerificationPhase::Done),
        VerificationRequestState::Cancelled(info) => Some(cancelled_phase(info)),
    }
}

/// The phase a SAS state stands for.
pub(crate) fn map_sas_state(state: &SasState) -> Option<VerificationPhase> {
    match state {
        SasState::Created { .. } | SasState::Started { .. } | SasState::Accepted { .. } => {
            Some(VerificationPhase::Waiting)
        }
        SasState::KeysExchanged {
            emojis: Some(emojis),
            ..
        } => Some(VerificationPhase::Emojis(Box::new(
            emojis.emojis.each_ref().map(|e| SasEmoji {
                symbol: e.symbol.to_owned(),
                description: e.description.to_owned(),
            }),
        ))),
        SasState::KeysExchanged {
            emojis: None,
            decimals: (a, b, c),
        } => Some(VerificationPhase::Decimals([*a, *b, *c])),
        SasState::Confirmed => Some(VerificationPhase::Confirmed),
        SasState::Done { .. } => Some(VerificationPhase::Done),
        SasState::Cancelled(info) => Some(cancelled_phase(info)),
    }
}

/// Which flow the core has open (at most one).
#[derive(Debug)]
pub(crate) enum Slot<T> {
    /// No flow.
    Free,
    /// An outgoing request is being created.
    Reserved,
    /// A flow is open.
    Active { id: VerificationId, value: T },
}

/// How an incoming flow fared against the slot.
#[derive(Debug, PartialEq)]
pub(crate) enum Claim<T> {
    /// The flow now owns the slot.
    Taken,
    /// The same flow is already open (a re-delivered event).
    Same,
    /// Another flow is open; the value comes back.
    Busy(T),
}

impl<T> Slot<T> {
    /// Reserves the slot for an outgoing request; `false` when it is taken.
    pub(crate) fn reserve(&mut self) -> bool {
        if matches!(self, Self::Free) {
            *self = Self::Reserved;
            true
        } else {
            false
        }
    }

    /// Turns a reservation into the open flow `id`.
    pub(crate) fn activate(&mut self, id: VerificationId, value: T) {
        *self = Self::Active { id, value };
    }

    /// Takes the free slot for an incoming flow.
    pub(crate) fn claim(&mut self, id: &VerificationId, value: T) -> Claim<T> {
        match self {
            Self::Free => {
                *self = Self::Active {
                    id: id.clone(),
                    value,
                };
                Claim::Taken
            }
            Self::Active { id: open, .. } if open == id => Claim::Same,
            Self::Reserved | Self::Active { .. } => Claim::Busy(value),
        }
    }

    /// Frees the slot when flow `id` holds it.
    pub(crate) fn release(&mut self, id: &VerificationId) {
        if matches!(self, Self::Active { id: open, .. } if open == id) {
            *self = Self::Free;
        }
    }

    /// Gives back a reservation (the request could not be created).
    pub(crate) fn release_reservation(&mut self) {
        if matches!(self, Self::Reserved) {
            *self = Self::Free;
        }
    }

    /// The value of the open flow `id`.
    pub(crate) fn get(&self, id: &VerificationId) -> Option<&T> {
        match self {
            Self::Active { id: open, value } if open == id => Some(value),
            _ => None,
        }
    }
}

/// What state the tasks of one session share.
struct Shared {
    client: Client,
    events: mpsc::Sender<CoreEvent>,
    slot: Mutex<Slot<VerificationRequest>>,
    /// Every driver and command task; `None` once the session is ending.
    tasks: Mutex<Option<JoinSet<()>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded data stays consistent when a holder panics.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = locked(&self.tasks);
        let Some(set) = tasks.as_mut() else {
            return;
        };
        // Forget finished tasks, so the set does not grow with every flow.
        while set.try_join_next().is_some() {}
        set.spawn(task);
    }

    /// Reports a failed action. These actions belong to no room.
    async fn fail(&self, error: CoreError) {
        let _ = self
            .events
            .send(CoreEvent::ActionFailed {
                room_id: RoomId::from(""),
                action: ActionKind::Verification,
                error,
            })
            .await;
    }

    fn flow(&self, id: &VerificationId) -> Option<VerificationRequest> {
        locked(&self.slot).get(id).cloned()
    }
}

/// Who a verification is with, as far as the request state tells.
fn peer_of(request: &VerificationRequest, state: &VerificationRequestState) -> VerificationPeer {
    let device = match state {
        VerificationRequestState::Requested {
            other_device_data, ..
        }
        | VerificationRequestState::Ready {
            other_device_data, ..
        } => Some(other_device_data),
        _ => None,
    };
    VerificationPeer {
        user_id: UserId::from(request.other_user_id().as_str()),
        device_id: device.map(|d| DeviceId::from(d.device_id().as_str())),
        device_name: device.and_then(|d| d.display_name().map(str::to_owned)),
        own_account: request.is_self_verification(),
    }
}

/// The SAS flow a request has moved into.
fn sas_of(request: &VerificationRequest) -> Option<SasVerification> {
    match request.state() {
        VerificationRequestState::Transitioned { verification } => verification.sas(),
        _ => None,
    }
}

/// One flow's driver: follows the request, then the SAS flow.
struct Flow {
    shared: Arc<Shared>,
    request: VerificationRequest,
    id: VerificationId,
    last: Option<VerificationPhase>,
    sas: Option<SasVerification>,
    sas_changes: Option<BoxStream<'static, SasState>>,
    sas_started: bool,
}

impl Flow {
    /// Sends `phase` when it is new. `true` when the flow is over (it ended,
    /// or nobody listens). The slot is freed before the final event, so a
    /// front end that reacts to it can start the next flow.
    async fn emit(&mut self, phase: Option<VerificationPhase>) -> bool {
        let Some(phase) = phase else {
            return false;
        };
        if self.last.as_ref() == Some(&phase) {
            return false;
        }
        let over = matches!(
            phase,
            VerificationPhase::Done | VerificationPhase::Cancelled { .. }
        );
        if over {
            locked(&self.shared.slot).release(&self.id);
        }
        self.last = Some(phase.clone());
        let sent = self
            .shared
            .events
            .send(CoreEvent::Verification {
                id: self.id.clone(),
                phase,
            })
            .await
            .is_ok();
        over || !sent
    }

    async fn on_request(&mut self, state: VerificationRequestState) -> bool {
        let over = self.emit(map_request_state(&state)).await;
        if over {
            return true;
        }
        match state {
            // Tessera starts the SAS flow of a request it made.
            VerificationRequestState::Ready { .. }
                if self.request.we_started() && !self.sas_started =>
            {
                self.sas_started = true;
                if self.request.start_sas().await.is_err() {
                    self.shared
                        .fail(CoreError::Internal(START_FAILED.into()))
                        .await;
                }
                false
            }
            VerificationRequestState::Transitioned { verification } if self.sas.is_none() => {
                let Some(sas) = verification.sas() else {
                    return false;
                };
                // Subscribe before reading, so no change is missed.
                self.sas_changes = Some(sas.changes().boxed());
                let current = sas.state();
                self.sas = Some(sas);
                self.on_sas(current).await
            }
            _ => false,
        }
    }

    async fn on_sas(&mut self, state: SasState) -> bool {
        if matches!(state, SasState::Started { .. })
            && let Some(sas) = &self.sas
            && !sas.we_started()
            && sas.accept().await.is_err()
        {
            self.shared
                .fail(CoreError::Internal(ACCEPT_FAILED.into()))
                .await;
        }
        self.emit(map_sas_state(&state)).await
    }

    async fn run(mut self) {
        // Subscribe before reading the state, so no change is missed.
        let mut changes = self.request.changes().boxed();
        let state = self.request.state();
        let requested = CoreEvent::VerificationRequested {
            id: self.id.clone(),
            peer: peer_of(&self.request, &state),
        };
        if self.shared.events.send(requested).await.is_err() {
            return;
        }
        let mut over = self.on_request(state).await;
        while !over {
            over = tokio::select! {
                Some(state) = changes.next() => self.on_request(state).await,
                Some(state) = next_sas(&mut self.sas_changes) => self.on_sas(state).await,
                else => true,
            };
        }
        // A flow that ended without a final event (a stream that closed) must
        // not keep the slot.
        locked(&self.shared.slot).release(&self.id);
    }
}

async fn next_sas(changes: &mut Option<BoxStream<'static, SasState>>) -> Option<SasState> {
    match changes {
        Some(changes) => changes.next().await,
        None => std::future::pending().await,
    }
}

/// How often and how far apart an incoming request is looked up. The event
/// handler can run a moment before the crypto machine has registered the
/// request, so a first miss is not final.
const LOOKUP_TRIES: usize = 30;
const LOOKUP_DELAY: Duration = Duration::from_millis(100);
/// How long a request refused for being the second one waits to be cancelled.
const BUSY_CANCEL_DELAY: Duration = Duration::from_secs(1);

/// Calls `find` up to `tries` times, `delay` apart, until it finds something.
async fn retry_lookup<T, F, Fut>(tries: usize, delay: Duration, mut find: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    for attempt in 0..tries {
        if attempt > 0 {
            tokio::time::sleep(delay).await;
        }
        if let Some(found) = find().await {
            return Some(found);
        }
    }
    None
}

/// An incoming request event: opens a flow, or cancels the request when one
/// is open already. Runs as a task, so the event handler returns at once.
async fn on_incoming(shared: Arc<Shared>, sender: OwnedUserId, flow_id: String) {
    let client = shared.client.clone();
    let Some(request) = retry_lookup(LOOKUP_TRIES, LOOKUP_DELAY, || {
        let client = client.clone();
        let (sender, flow_id) = (sender.clone(), flow_id.clone());
        async move {
            client
                .encryption()
                .get_verification_request(&sender, &flow_id)
                .await
        }
    })
    .await
    else {
        return;
    };
    // Our own request comes back as an in-room event, and an old request may
    // have ended already.
    if request.we_started()
        || !matches!(request.state(), VerificationRequestState::Requested { .. })
    {
        return;
    }
    let id = VerificationId::from(request.flow_id());
    let claim = locked(&shared.slot).claim(&id, request.clone());
    match claim {
        Claim::Taken => new_flow(shared, request, id).run().await,
        Claim::Same => {}
        // One flow at a time: the other side sees a cancel by the user. An
        // instant cancel can reach the requester before its own send has
        // returned and registered the request's flow ID; it would then not
        // know the cancel and wait for nothing, so the cancel waits a moment.
        Claim::Busy(request) => {
            tokio::time::sleep(BUSY_CANCEL_DELAY).await;
            let _ = request.cancel().await;
        }
    }
}

fn new_flow(shared: Arc<Shared>, request: VerificationRequest, id: VerificationId) -> Flow {
    Flow {
        shared,
        request,
        id,
        last: None,
        sas: None,
        sas_changes: None,
        sas_started: false,
    }
}

/// Whom an outgoing request is for.
#[derive(Debug)]
pub(crate) enum Target {
    /// The account's other devices.
    Own,
    /// One device of the account.
    Device(DeviceId),
    /// Another user.
    User(UserId),
}

/// Creates and sends the request for `target`.
async fn open_request(client: &Client, target: Target) -> Result<VerificationRequest, CoreError> {
    let internal = |text: &str| CoreError::Internal(text.to_owned());
    let encryption = client.encryption();
    let me = client.user_id().ok_or(CoreError::NotLoggedIn)?.to_owned();
    match target {
        Target::Device(device_id) => {
            let device = encryption
                .get_device(&me, device_id.as_str().into())
                .await
                .map_err(|_| internal(START_FAILED))?
                .ok_or_else(|| internal(NO_DEVICE))?;
            device
                .request_verification()
                .await
                .map_err(|_| internal(START_FAILED))
        }
        Target::Own | Target::User(_) => {
            let user = match target {
                Target::User(user) => {
                    SdkUserId::parse(user.as_str()).map_err(|_| internal(BAD_USER))?
                }
                _ => me,
            };
            // Ask the server: the stored identity can be older than a reset
            // of the other side.
            let identity = match encryption.request_user_identity(&user).await {
                Ok(identity) => identity,
                Err(_) => encryption.get_user_identity(&user).await.ok().flatten(),
            }
            .ok_or_else(|| internal(NO_IDENTITY))?;
            identity
                .request_verification()
                .await
                .map_err(|_| internal(START_FAILED))
        }
    }
}

/// The verification handlers and tasks of one session.
pub(crate) struct VerificationTasks {
    shared: Option<Arc<Shared>>,
    /// Keep the event handlers registered; they go when this does.
    guards: Vec<EventHandlerDropGuard>,
}

impl VerificationTasks {
    pub(crate) fn new() -> Self {
        Self {
            shared: None,
            guards: Vec::new(),
        }
    }

    /// Registers the handlers for incoming requests (to-device and in-room)
    /// after login or restore.
    pub(crate) fn start(&mut self, client: &Client, events: &mpsc::Sender<CoreEvent>) {
        self.close_now();
        let shared = Arc::new(Shared {
            client: client.clone(),
            events: events.clone(),
            slot: Mutex::new(Slot::Free),
            tasks: Mutex::new(Some(JoinSet::new())),
        });
        let to_device = client.add_event_handler({
            let shared = shared.clone();
            move |event: ToDeviceEvent<ToDeviceKeyVerificationRequestEventContent>| {
                let shared = shared.clone();
                async move {
                    let flow_id = event.content.transaction_id.to_string();
                    shared.spawn(on_incoming(shared.clone(), event.sender, flow_id));
                }
            }
        });
        let in_room = client.add_event_handler({
            let shared = shared.clone();
            move |event: OriginalSyncRoomMessageEvent| {
                let shared = shared.clone();
                async move {
                    if matches!(event.content.msgtype, MessageType::VerificationRequest(_)) {
                        let flow_id = event.event_id.to_string();
                        shared.spawn(on_incoming(shared.clone(), event.sender, flow_id));
                    }
                }
            }
        });
        self.guards = vec![
            client.event_handler_drop_guard(to_device),
            client.event_handler_drop_guard(in_room),
        ];
        self.shared = Some(shared);
    }

    /// Drops the handlers and aborts the tasks without waiting for them.
    fn close_now(&mut self) {
        self.guards.clear();
        if let Some(shared) = self.shared.take()
            && let Some(mut set) = locked(&shared.tasks).take()
        {
            set.abort_all();
        }
    }

    /// Ends the handlers and every task, so that nothing holds the client
    /// afterwards.
    pub(crate) async fn stop(&mut self) {
        // No new request can arrive from here on.
        self.guards.clear();
        let Some(shared) = self.shared.take() else {
            return;
        };
        let set = locked(&shared.tasks).take();
        if let Some(mut set) = set {
            set.shutdown().await;
        }
    }

    /// The shared state, or the failure to report when there is none yet.
    fn shared_or_fail(&self, events: &mpsc::Sender<CoreEvent>) -> Option<Arc<Shared>> {
        if self.shared.is_none() {
            let events = events.clone();
            tokio::spawn(async move {
                let _ = events
                    .send(CoreEvent::ActionFailed {
                        room_id: RoomId::from(""),
                        action: ActionKind::Verification,
                        error: CoreError::NotLoggedIn,
                    })
                    .await;
            });
        }
        self.shared.clone()
    }

    /// `Request*`: starts an outgoing flow. A flow already open answers
    /// `Busy`.
    pub(crate) fn request(
        &mut self,
        client: &Client,
        events: &mpsc::Sender<CoreEvent>,
        target: Target,
    ) {
        let Some(shared) = self.shared_or_fail(events) else {
            return;
        };
        let reserved = locked(&shared.slot).reserve();
        let client = client.clone();
        let task_shared = shared.clone();
        shared.spawn(async move {
            let shared = task_shared;
            if !reserved {
                shared.fail(CoreError::Busy).await;
                return;
            }
            match open_request(&client, target).await {
                Ok(request) => {
                    let id = VerificationId::from(request.flow_id());
                    locked(&shared.slot).activate(id.clone(), request.clone());
                    new_flow(shared, request, id).run().await;
                }
                Err(error) => {
                    locked(&shared.slot).release_reservation();
                    shared.fail(error).await;
                }
            }
        });
    }

    /// Runs `work` on the open flow `id` in a task; an error text becomes
    /// `ActionFailed`.
    fn act<F, Fut>(&mut self, events: &mpsc::Sender<CoreEvent>, id: &VerificationId, work: F)
    where
        F: FnOnce(VerificationRequest) -> Fut,
        Fut: Future<Output = Result<(), &'static str>> + Send + 'static,
    {
        let Some(shared) = self.shared_or_fail(events) else {
            return;
        };
        let request = shared.flow(id);
        let work = request.map(work);
        let task_shared = shared.clone();
        shared.spawn(async move {
            let error = match work {
                Some(work) => work.await.err(),
                None => Some(NO_FLOW),
            };
            if let Some(text) = error {
                task_shared.fail(CoreError::Internal(text.into())).await;
            }
        });
    }

    /// `AcceptVerification`: accepts an incoming request.
    pub(crate) fn accept(&mut self, events: &mpsc::Sender<CoreEvent>, id: &VerificationId) {
        self.act(events, id, |request| async move {
            request.accept().await.map_err(|_| ACCEPT_FAILED)
        });
    }

    /// `ConfirmSas`: the emoji or numbers match.
    pub(crate) fn confirm_sas(&mut self, events: &mpsc::Sender<CoreEvent>, id: &VerificationId) {
        self.act(events, id, |request| async move {
            let sas = sas_of(&request).ok_or(NOT_COMPARING)?;
            sas.confirm().await.map_err(|_| CONFIRM_FAILED)
        });
    }

    /// `SasMismatch`: the emoji or numbers differ.
    pub(crate) fn mismatch(&mut self, events: &mpsc::Sender<CoreEvent>, id: &VerificationId) {
        self.act(events, id, |request| async move {
            let sas = sas_of(&request).ok_or(NOT_COMPARING)?;
            sas.mismatch().await.map_err(|_| MISMATCH_FAILED)
        });
    }

    /// `CancelVerification`: cancels the flow, or declines the request.
    pub(crate) fn cancel(&mut self, events: &mpsc::Sender<CoreEvent>, id: &VerificationId) {
        self.act(events, id, |request| async move {
            match sas_of(&request) {
                Some(sas) => sas.cancel().await.map_err(|_| CANCEL_FAILED),
                None => request.cancel().await.map_err(|_| CANCEL_FAILED),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk::encryption::verification::{
        Emoji, EmojiShortAuthString, SasState, VerificationRequestState,
    };
    use matrix_sdk::ruma::events::key::verification::cancel::CancelCode;
    use matrix_sdk::ruma::events::key::verification::start::{SasV1Content, SasV1ContentInit};

    use super::*;
    use crate::api::SasEmoji;

    #[test]
    fn cancel_codes_map_to_reasons() {
        let cases = [
            (CancelCode::User, true, CancelReason::ByUs),
            (CancelCode::User, false, CancelReason::ByThem),
            (CancelCode::MismatchedSas, true, CancelReason::Mismatch),
            (CancelCode::MismatchedSas, false, CancelReason::Mismatch),
            (CancelCode::Timeout, true, CancelReason::Timeout),
            (CancelCode::Timeout, false, CancelReason::Timeout),
            (CancelCode::Accepted, false, CancelReason::AcceptedElsewhere),
            (CancelCode::KeyMismatch, false, CancelReason::Other),
            (CancelCode::UnexpectedMessage, true, CancelReason::Other),
            (CancelCode::from("m.custom"), false, CancelReason::Other),
        ];
        for (code, by_us, want) in cases {
            assert_eq!(map_cancel_code(&code, by_us), want, "{code:?} {by_us}");
        }
    }

    fn sas_content() -> SasV1Content {
        SasV1ContentInit {
            key_agreement_protocols: Vec::new(),
            hashes: Vec::new(),
            message_authentication_codes: Vec::new(),
            short_authentication_string: Vec::new(),
        }
        .into()
    }

    fn emoji_string() -> EmojiShortAuthString {
        const NAMES: [(&str, &str); 7] = [
            ("\u{1F436}", "Dog"),
            ("\u{1F431}", "Cat"),
            ("\u{1F981}", "Lion"),
            ("\u{1F40E}", "Horse"),
            ("\u{1F984}", "Unicorn"),
            ("\u{1F437}", "Pig"),
            ("\u{1F418}", "Elephant"),
        ];
        let emojis = NAMES.map(|(symbol, description)| Emoji {
            symbol,
            description,
        });
        EmojiShortAuthString {
            indices: [0, 1, 2, 3, 4, 5, 6],
            emojis,
        }
    }

    #[test]
    fn sas_states_map_to_phases() {
        let waiting = Some(VerificationPhase::Waiting);
        assert_eq!(
            map_sas_state(&SasState::Created {
                protocols: sas_content()
            }),
            waiting
        );
        assert_eq!(
            map_sas_state(&SasState::Started {
                protocols: sas_content()
            }),
            waiting
        );
        assert_eq!(
            map_sas_state(&SasState::KeysExchanged {
                emojis: None,
                decimals: (1234, 5678, 90),
            }),
            Some(VerificationPhase::Decimals([1234, 5678, 90]))
        );
        let phase = map_sas_state(&SasState::KeysExchanged {
            emojis: Some(emoji_string()),
            decimals: (1, 2, 3),
        });
        let Some(VerificationPhase::Emojis(emojis)) = phase else {
            panic!("expected emoji, got {phase:?}");
        };
        assert_eq!(emojis[0].symbol, "\u{1F436}");
        assert_eq!(emojis[0].description, "Dog");
        assert_eq!(
            emojis[6],
            SasEmoji {
                symbol: "\u{1F418}".into(),
                description: "Elephant".into()
            }
        );
        assert_eq!(
            map_sas_state(&SasState::Confirmed),
            Some(VerificationPhase::Confirmed)
        );
        assert_eq!(
            map_sas_state(&SasState::Done {
                verified_devices: Vec::new(),
                verified_identities: Vec::new(),
            }),
            Some(VerificationPhase::Done)
        );
    }

    #[test]
    fn request_states_map_to_phases() {
        assert_eq!(
            map_request_state(&VerificationRequestState::Created {
                our_methods: Vec::new()
            }),
            Some(VerificationPhase::Requested { incoming: false })
        );
        assert_eq!(
            map_request_state(&VerificationRequestState::Done),
            Some(VerificationPhase::Done)
        );
    }

    #[tokio::test]
    async fn a_lookup_is_retried_until_the_request_shows_up() {
        use std::cell::Cell;
        use std::time::Duration;
        let calls = Cell::new(0);
        let found_third_time = retry_lookup(5, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            let hit = calls.get() == 3;
            async move { hit.then_some("request") }
        })
        .await;
        assert_eq!(found_third_time, Some("request"));
        assert_eq!(calls.get(), 3, "stops at the first hit");

        calls.set(0);
        let never = retry_lookup(4, Duration::ZERO, || {
            calls.set(calls.get() + 1);
            async { None::<u8> }
        })
        .await;
        assert_eq!(never, None);
        assert_eq!(calls.get(), 4, "gives up after the given number of tries");
    }

    #[test]
    fn the_slot_holds_one_flow() {
        let (a, b) = (VerificationId::from("a"), VerificationId::from("b"));
        let mut slot: Slot<u8> = Slot::Free;
        assert!(slot.get(&a).is_none());
        // An outgoing request reserves, then activates.
        assert!(slot.reserve());
        assert!(!slot.reserve(), "a second outgoing request is busy");
        assert_eq!(slot.claim(&b, 2), Claim::Busy(2), "incoming is refused too");
        slot.activate(a.clone(), 1);
        assert_eq!(slot.get(&a), Some(&1));
        assert!(!slot.reserve());
        assert_eq!(slot.claim(&a, 9), Claim::Same, "a re-delivered request");
        assert_eq!(slot.get(&a), Some(&1), "the open flow keeps its value");
        assert_eq!(slot.claim(&b, 2), Claim::Busy(2));
        // Releasing another flow changes nothing.
        slot.release(&b);
        assert_eq!(slot.get(&a), Some(&1));
        slot.release(&a);
        assert!(slot.get(&a).is_none());
        // An incoming flow takes the free slot.
        assert_eq!(slot.claim(&b, 2), Claim::Taken);
        assert_eq!(slot.get(&b), Some(&2));
        assert!(!slot.reserve());
        slot.release(&b);
        // A failed outgoing request gives its reservation back.
        assert!(slot.reserve());
        slot.release_reservation();
        assert!(slot.reserve());
        // A reservation is not an open flow: releasing by id keeps it.
        slot.release(&a);
        assert!(!slot.reserve());
    }
}
