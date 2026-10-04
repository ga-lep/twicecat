//! The engine task: owns the SDK client and turns commands into events.
//!
//! Only plain API types cross the public boundary. SDK types stay in here.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{Stream, StreamExt as _, pin_mut};
use matrix_sdk::event_cache::PaginationStatus;
use matrix_sdk::room::edit::EditedContent;
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::api::client::receipt::create_receipt::v3::ReceiptType;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::send_queue::SendHandle;
use matrix_sdk::{Client, RoomMemberships, SessionChange};
use matrix_sdk_ui::eyeball_im::{Vector, VectorDiff};
use matrix_sdk_ui::sync_service::SyncService;
use matrix_sdk_ui::timeline::{
    EventSendState, RoomExt as _, Timeline, TimelineItem as SdkTimelineItem,
};
use tokio::sync::{Notify, OnceCell, broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::api::{
    ActionKind, AudioBackend, AudioDevice, AudioDeviceId, AudioDevices, AudioSettings,
    CallEndReason, CallState, Command, CoreError, CoreEvent, DeviceId, Draft, ListDiff, Member,
    MessageText, OutgoingMessage, PaginationState, Password, RoomId, RoomListQuery, TypingUser,
    UserId,
};
use crate::audio_session::{AudioSession, Mode};
use crate::avatars::{Avatars, SdkFetcher, answer_unavailable};
use crate::call::watcher::CallWatch;
use crate::call::{CallAudio, CallSession, MediaNotice, UserVolumes};
use crate::call_settings;
use crate::client::{
    ServerSpec, WRONG_ACCOUNT, account_bound, build_client, discover_homeserver_url,
    ensure_homeserver_after_login, map_login_error, same_origin,
};
use crate::drafts;
use crate::oauth::authorize;
use crate::outgoing::outgoing_content;
use crate::paths::DataPaths;
use crate::room_list::{Registered, build_filter, loaded_event, map_batch, map_diff};
use crate::secrets::{SecretError, SecretStore};
use crate::security::{self, SecurityTasks};
use crate::session::{
    RECORD_VERSION, SessionAuth, SessionRecord, generate_session_id, generate_store_passphrase,
};
use crate::sync::{CancelTrigger, SdkTarget, SyncControl, SyncPause, cancel_pair, run_supervised};
use crate::timeline::{map_item, normalize_reaction_key, parse_room_id, rank_members};
use crate::verification::Target;

/// Events loaded by one back-pagination.
const PAGINATION_BATCH: u16 = 20;

/// Most typing users reported at once (a hostile room could list thousands).
const MAX_TYPING_USERS: usize = 16;

/// Default number of rooms the room list loads per page.
pub const DEFAULT_ROOM_LIST_PAGE_SIZE: usize = 500;

/// How long a password login may take, including the SDK's own retries.
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Configuration of a [`Core`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// Where session stores and the last-account marker live.
    pub data: DataPaths,
    /// Server name or homeserver URL that wins over anything in a login
    /// command (tests, `--homeserver`).
    pub homeserver_override: Option<String>,
    /// Page size of the room list (`DEFAULT_ROOM_LIST_PAGE_SIZE` is 500).
    pub room_list_page_size: usize,
    /// Audio devices for calls and the mic test (`AudioBackend::System`
    /// unless testing).
    pub audio: AudioBackend,
    /// Call service (MatrixRTC authorization service, lk-jwt-service URL)
    /// to publish on instead of the homeserver's advertised one: a
    /// self-hosted SFU, or tests. `https`, or `http` on loopback.
    pub call_service_override: Option<String>,
}

/// The channels to talk to a running engine.
#[derive(Debug)]
pub struct CoreHandle {
    /// Send commands here.
    pub commands: mpsc::Sender<Command>,
    /// Events arrive here; the channel closes after `Shutdown`.
    pub events: mpsc::Receiver<CoreEvent>,
}

/// Entry point of the headless engine.
#[derive(Debug)]
pub struct Core;

impl Core {
    /// Spawns the engine task on the current tokio runtime.
    ///
    /// Must be called inside a tokio runtime.
    pub fn start(config: CoreConfig, secrets: Arc<dyn SecretStore>) -> CoreHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let (ev_tx, ev_rx) = mpsc::channel(256);
        let engine = Engine {
            config,
            secrets,
            events: ev_tx,
            active: None,
            oauth: None,
            room_list_query: watch::channel(RoomListQuery::default()).0,
            audio_settings: None,
            device_watcher: None,
            audio_session: None,
            call: None,
            user_volumes: UserVolumes::default(),
            mic_muted: false,
            deafened: false,
            call_reconnects: 0,
        };
        tokio::spawn(engine.run(cmd_rx));
        CoreHandle {
            commands: cmd_tx,
            events: ev_rx,
        }
    }
}

/// A running sync service and the tasks that forward its output as events.
struct Syncing {
    service: Arc<SyncService>,
    /// Stops the service while an identity operation makes new keys.
    pause: SyncPause,
    /// Asks the supervisor to end between steps (it is never aborted: an
    /// aborted `SyncService::stop` could leave SDK tasks running).
    cancel: CancelTrigger,
    supervisor: JoinHandle<()>,
    room_list: JoinHandle<()>,
    /// Asks the room list forwarder to load one more page.
    more_rooms: mpsc::Sender<()>,
    /// Rings and the calls in every room.
    calls: CallWatch,
}

/// The room whose timeline is open, the task forwarding its diffs and the
/// tasks of its actions (send, retry, discard: every network call runs
/// there, so `handle` never waits on the network).
struct Selected {
    room_id: RoomId,
    timeline: Arc<Timeline>,
    task: JoinHandle<()>,
    tasks: JoinSet<()>,
    /// Set once the full member list has been fetched for this selection.
    members_synced: Arc<OnceCell<()>>,
    /// Set while a back-pagination of this room runs.
    paginating: Arc<AtomicBool>,
}

impl Selected {
    /// Ends the forwarder and the action tasks; the timeline is dropped with
    /// `self`, after the tasks that hold clones of it.
    async fn stop(mut self) {
        self.task.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = self.task.await;
        self.tasks.shutdown().await;
    }

    /// Runs `work` on the timeline in the action tasks. Its error, if any,
    /// is reported as `ActionFailed`.
    fn spawn_action<F, Fut>(
        &mut self,
        action: ActionKind,
        events: &mpsc::Sender<CoreEvent>,
        work: F,
    ) where
        F: FnOnce(Arc<Timeline>) -> Fut,
        Fut: std::future::Future<Output = Result<(), CoreError>> + Send + 'static,
    {
        // Forget finished tasks, so the set does not grow with every action.
        while self.tasks.try_join_next().is_some() {}
        let fut = work(self.timeline.clone());
        let events = events.clone();
        let room_id = self.room_id.clone();
        self.tasks.spawn(async move {
            if let Err(error) = fut.await {
                let _ = events
                    .send(CoreEvent::ActionFailed {
                        room_id,
                        action,
                        error,
                    })
                    .await;
            }
        });
    }
}

struct Active {
    client: Client,
    record: SessionRecord,
    changes: broadcast::Receiver<SessionChange>,
    sync: Option<Syncing>,
    selected: Option<Selected>,
    /// Avatar fetches of this session (in flight and answered).
    avatars: Avatars,
    fetcher: Arc<SdkFetcher>,
    /// The status forwarder and the network actions of the encryption API.
    security: SecurityTasks,
}

impl Active {
    /// Stops the timeline forwarder, then the sync forwarders, then the sync
    /// service, so that nothing holds the client or its store afterwards (the
    /// tasks own clones of it).
    async fn stop_tasks(&mut self) {
        // From here a dropped identity task must not start the sync again.
        if let Some(sync) = &self.sync {
            sync.pause.begin_ending();
        }
        // The security tasks hold clones of the client too, and go first.
        self.security.stop().await;
        // Fetch tasks hold clones of the client, so they go next.
        self.avatars.stop().await;
        if let Some(selected) = self.selected.take() {
            selected.stop().await;
        }
        let Some(sync) = self.sync.take() else {
            return;
        };
        // The supervisor finishes any `stop` or `restart` it is in the middle
        // of, then ends. Only then is the service stopped here, so the store
        // is never deleted while SDK tasks still use it.
        sync.calls.stop().await;
        sync.cancel.cancel();
        let _ = sync.supervisor.await;
        sync.room_list.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = sync.room_list.await;
        sync.service.stop().await;
    }
}

/// Production [`SyncControl`]: restarts the real service.
struct ServiceControl {
    service: Arc<SyncService>,
    /// Closed while an identity operation has the sync paused (`SyncPause`).
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl SyncControl for ServiceControl {
    async fn restart(&self) {
        let _open = self.gate.lock().await;
        self.service.start().await;
    }

    async fn stop(&self) {
        self.service.stop().await;
    }

    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }

    async fn sleep(&self, delay: std::time::Duration) {
        tokio::time::sleep(delay).await;
    }

    fn jitter_unit(&self) -> f64 {
        rand::random::<f64>()
    }
}

/// Forwards sync state changes as events and restarts the service after a
/// failure (see [`supervise`]). When the sync comes back after an outage it
/// also re-enables the send queue, which the SDK disables after a failed
/// send, so that recoverable failures are retried.
async fn forward_sync_state(
    service: Arc<SyncService>,
    client: Client,
    events: mpsc::Sender<CoreEvent>,
    cancel: crate::sync::Cancel,
    gate: Arc<tokio::sync::Mutex<()>>,
) {
    // Subscribe, then mark the current value as seen: the subscriber would
    // otherwise yield it again, which the supervisor reads as a missed
    // transition.
    let mut subscriber = service.state();
    let initial = subscriber.next_now();
    let resumed = Notify::new();
    // Runs in this task, so it ends with the supervisor and holds the client
    // no longer than the supervisor does.
    let reenable = async {
        loop {
            resumed.notified().await;
            client.send_queue().set_enabled(true).await;
        }
    };
    let control = ServiceControl { service, gate };
    let supervised = run_supervised(&control, initial, subscriber, events, cancel, || {
        resumed.notify_one()
    });
    tokio::select! {
        () = supervised => {}
        () = reenable => {}
    }
}

/// Forwards every room list diff batch as one `RoomListChanged` event.
///
/// Owns the list's controller: the query in `query` (applied when the list
/// starts, then on every change) sets the filter and goes back to the first
/// page, and each message on `more` adds one page.
async fn forward_room_list(
    service: Arc<SyncService>,
    page_size: usize,
    mut query: watch::Receiver<RoomListQuery>,
    mut more: mpsc::Receiver<()>,
    events: mpsc::Sender<CoreEvent>,
) {
    let room_list = match service.room_list_service().all_rooms().await {
        Ok(list) => list,
        Err(_) => {
            let e = CoreError::Internal("could not open the room list".into());
            let _ = events.send(CoreEvent::Error(e)).await;
            return;
        }
    };
    let mut loading = room_list.loading_state();
    let (stream, controller) = room_list.entries_with_dynamic_adapters(page_size.max(1));
    // The stream yields nothing until a filter is set. `borrow_and_update`
    // marks this value seen, so only later queries wake the loop below.
    let first = build_filter(&query.borrow_and_update());
    controller.set_filter(first);
    let mut query_open = true;
    let mut more_open = true;
    pin_mut!(stream);
    // Rooms registered for latest events live and die with this task, so the
    // set starts empty again after sync restarts.
    let mut registered = Registered::default();
    let mut loading_open = true;
    loop {
        let event = tokio::select! {
            diffs = stream.next() => match diffs {
                Some(diffs) => CoreEvent::RoomListChanged(map_batch(diffs, &mut registered).await),
                None => return,
            },
            changed = query.changed(), if query_open => {
                if changed.is_err() {
                    query_open = false;
                    continue;
                }
                let filter = build_filter(&query.borrow_and_update());
                controller.set_filter(filter);
                controller.reset_to_one_page();
                continue;
            }
            request = more.recv(), if more_open => {
                if request.is_none() {
                    more_open = false;
                } else {
                    controller.add_one_page();
                }
                continue;
            }
            state = loading.next(), if loading_open => match state {
                Some(state) => match loaded_event(&state) {
                    Some(event) => event,
                    None => continue,
                },
                None => {
                    loading_open = false;
                    continue;
                }
            },
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

/// Forwards the initial timeline items as one `Reset`, then the room's stored
/// draft (after the items, so a reply draft can find its message), then every
/// diff batch.
async fn forward_timeline(
    room_id: RoomId,
    items: Vector<Arc<SdkTimelineItem>>,
    stream: impl Stream<Item = Vec<VectorDiff<Arc<SdkTimelineItem>>>>,
    events: mpsc::Sender<CoreEvent>,
    own: Option<OwnedUserId>,
    draft: Option<Draft>,
) {
    let initial = ListDiff::Reset {
        values: items.iter().map(|i| map_item(i, own.as_deref())).collect(),
    };
    let first = CoreEvent::TimelineDiff {
        room_id: room_id.clone(),
        diffs: vec![initial],
    };
    if events.send(first).await.is_err() {
        return;
    }
    if let Some(draft) = draft {
        let loaded = CoreEvent::DraftLoaded {
            room_id: room_id.clone(),
            draft,
        };
        if events.send(loaded).await.is_err() {
            return;
        }
    }
    pin_mut!(stream);
    while let Some(diffs) = stream.next().await {
        let diffs = diffs
            .into_iter()
            .map(|d| map_diff(d, |item| map_item(&item, own.as_deref())))
            .collect();
        let event = CoreEvent::TimelineDiff {
            room_id: room_id.clone(),
            diffs,
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

/// How long shutdown waits for a pending OAuth task (and a logout) before
/// giving up on it.
const OAUTH_STOP_TIMEOUT: Duration = Duration::from_secs(5);

struct Engine {
    config: CoreConfig,
    secrets: Arc<dyn SecretStore>,
    events: mpsc::Sender<CoreEvent>,
    active: Option<Active>,
    /// An OAuth login waiting for the browser.
    oauth: Option<PendingOAuth>,
    /// The latest room list query. It outlives sessions, so one set before
    /// login applies when the list starts.
    room_list_query: watch::Sender<RoomListQuery>,
    /// Audio settings, loaded from disk on first use.
    audio_settings: Option<AudioSettings>,
    /// Hot-plug watcher, started by the first `ListAudioDevices`.
    device_watcher: Option<tessera_audio::DeviceWatcher>,
    /// Open devices and the voice engine (mic test for now).
    audio_session: Option<AudioSession>,
    /// The local user's call.
    call: Option<CallSession>,
    /// Per-user volumes, shared with the call's participant reporting.
    user_volumes: UserVolumes,
    /// Microphone mute and deafen requested for the call.
    mic_muted: bool,
    deafened: bool,
    /// Consecutive SFU reconnects of the current call.
    call_reconnects: u32,
}

/// An OAuth login in progress. Its task builds the client, emits `OAuthUrl`,
/// waits for the redirect, and returns the logged-in client; the engine then
/// checks and persists the session (see [`Engine::on_oauth_done`]).
struct PendingOAuth {
    /// Fires the cancellation; taken by `CancelOAuth`.
    cancel: Option<oneshot::Sender<()>>,
    /// A cancel (or shutdown) was requested. A login that still succeeds is
    /// then discarded, not adopted.
    cancelled: bool,
    task: JoinHandle<Result<Client, CoreError>>,
    /// What the user asked for; a bare name binds the returned account.
    server: String,
    session_id: String,
    passphrase: Zeroizing<String>,
}

/// Waits for the pending OAuth task, or forever when there is none. Safe to
/// drop and call again: awaiting a `JoinHandle` by reference loses nothing.
async fn next_oauth(
    pending: &mut Option<PendingOAuth>,
) -> Result<Result<Client, CoreError>, tokio::task::JoinError> {
    match pending {
        Some(p) => (&mut p.task).await,
        None => std::future::pending().await,
    }
}

/// The current call's next SFU notice, or forever without a call (or once
/// its notices end).
async fn next_media_notice(call: &mut Option<CallSession>) -> MediaNotice {
    match call {
        Some(call) => match call.notices.recv().await {
            Some(notice) => notice,
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

/// How often the device watcher re-lists audio devices.
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(2);

fn audio_device(info: tessera_audio::DeviceInfo) -> AudioDevice {
    AudioDevice {
        id: AudioDeviceId::from(info.id),
        name: info.name,
        is_default: info.is_default,
    }
}

fn audio_devices(list: tessera_audio::DeviceList) -> AudioDevices {
    AudioDevices {
        inputs: list.inputs.into_iter().map(audio_device).collect(),
        outputs: list.outputs.into_iter().map(audio_device).collect(),
    }
}

/// Runs blocking work (keychain, file system) off the async threads.
async fn blocking<T, F>(f: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| CoreError::Internal("background task failed".into()))
}

fn secret_err(_: SecretError) -> CoreError {
    CoreError::Storage("the secret store is unavailable".into())
}

fn fs_err(_: impl std::any::Any) -> CoreError {
    CoreError::Storage("could not access the data directory".into())
}

/// Result of waiting on the session-change channel.
enum Change {
    Got(SessionChange),
    Missed,
    Closed,
}

async fn next_change(active: &mut Option<Active>) -> Change {
    let Some(a) = active else {
        return std::future::pending().await;
    };
    match a.changes.recv().await {
        Ok(c) => Change::Got(c),
        Err(broadcast::error::RecvError::Lagged(_)) => Change::Missed,
        Err(broadcast::error::RecvError::Closed) => Change::Closed,
    }
}

/// Finds the timeline item with this `TimelineItem::id`.
async fn find_item(timeline: &Timeline, item_id: &str) -> Result<Arc<SdkTimelineItem>, CoreError> {
    timeline
        .items()
        .await
        .iter()
        .find(|item| item.unique_id().0 == item_id)
        .cloned()
        .ok_or(CoreError::ItemNotFound)
}

/// Queues a message, as a reply when it names the item to reply to.
async fn send_task(timeline: Arc<Timeline>, message: OutgoingMessage) -> Result<(), CoreError> {
    // Resolve the reply target first: a refused reply sends nothing.
    let reply_to = match &message.reply_to {
        None => None,
        Some(item_id) => {
            let item = find_item(&timeline, item_id).await?;
            let event = item.as_event().ok_or(CoreError::NotAllowed)?;
            if !event.can_be_replied_to() {
                return Err(CoreError::NotAllowed);
            }
            Some(event.event_id().ok_or(CoreError::NotAllowed)?.to_owned())
        }
    };
    let content = outgoing_content(&message);
    let queued = match reply_to {
        Some(event_id) => timeline.send_reply(content, event_id).await,
        None => {
            timeline
                .send(RoomMessageEventContent::from(content).into())
                .await
        }
    };
    queued
        .map(drop)
        .map_err(|_| CoreError::Internal("the message could not be queued".into()))
}

/// Replaces the content of one of the user's own messages. The new content
/// goes through the same `outgoing_content` (sanitizer and tag cap) as a send.
async fn edit_task(
    timeline: Arc<Timeline>,
    item_id: String,
    message: OutgoingMessage,
) -> Result<(), CoreError> {
    let item = find_item(&timeline, &item_id).await?;
    let event = item.as_event().ok_or(CoreError::NotAllowed)?;
    if !event.is_editable() {
        return Err(CoreError::NotAllowed);
    }
    let content = EditedContent::RoomMessage(outgoing_content(&message));
    timeline
        .edit(&event.identifier(), content)
        .await
        .map_err(|_| CoreError::Internal("the message could not be edited".into()))
}

/// Redacts a message. The reason is exposed only at the SDK call.
async fn redact_task(
    timeline: Arc<Timeline>,
    item_id: String,
    reason: Option<MessageText>,
) -> Result<(), CoreError> {
    let item = find_item(&timeline, &item_id).await?;
    let event = item.as_event().ok_or(CoreError::NotAllowed)?;
    timeline
        .redact(
            &event.identifier(),
            reason.as_ref().map(MessageText::expose),
        )
        .await
        .map_err(|_| CoreError::Internal("the message could not be redacted".into()))
}

/// Adds the user's reaction to a message, or removes it when present.
async fn react_task(
    timeline: Arc<Timeline>,
    item_id: String,
    key: String,
) -> Result<(), CoreError> {
    let key = normalize_reaction_key(&key)?;
    let item = find_item(&timeline, &item_id).await?;
    let event = item.as_event().ok_or(CoreError::NotAllowed)?;
    timeline
        .toggle_reaction(&event.identifier(), &key)
        .await
        .map(drop)
        .map_err(|_| CoreError::Internal("the reaction could not be sent".into()))
}

/// Answers a member search with the best matches among the joined members.
/// The member list is fetched once per selection (`synced`); the search
/// itself reads only the local store.
async fn members_task(
    timeline: Arc<Timeline>,
    synced: Arc<OnceCell<()>>,
    events: mpsc::Sender<CoreEvent>,
    query: MessageText,
) -> Result<(), CoreError> {
    let room = timeline.room();
    let fail = |_| CoreError::Internal("the members could not be loaded".into());
    synced
        .get_or_try_init(|| async { room.sync_members().await })
        .await
        .map_err(fail)?;
    let joined = room
        .members_no_sync(RoomMemberships::JOIN)
        .await
        .map_err(fail)?;
    let all = joined
        .iter()
        .map(|m| Member {
            user_id: UserId::from(m.user_id().as_str()),
            display_name: m.display_name().map(str::to_owned),
        })
        .collect();
    let members = rank_members(query.expose(), all);
    // A dropped receiver just means nobody listens any more.
    let _ = events
        .send(CoreEvent::Members {
            room_id: RoomId::from(room.room_id().as_str()),
            query,
            members,
        })
        .await;
    Ok(())
}

/// The send handle of the failed local echo `item_id`, and whether the
/// failure is unrecoverable (the request is parked until unwedged).
async fn local_echo(timeline: &Timeline, item_id: &str) -> Result<(SendHandle, bool), CoreError> {
    let item = find_item(timeline, item_id).await?;
    let event = item.as_event().ok_or(CoreError::NotAllowed)?;
    let handle = event
        .local_echo_send_handle()
        .ok_or(CoreError::NotAllowed)?;
    let wedged = matches!(
        event.send_state(),
        Some(EventSendState::SendingFailed {
            is_recoverable: false,
            ..
        })
    );
    Ok((handle, wedged))
}

/// Sends a failed local echo again: unwedges it when needed, then re-enables
/// the room's send queue, which a failure disabled.
async fn retry_task(timeline: Arc<Timeline>, item_id: String) -> Result<(), CoreError> {
    let (handle, wedged) = local_echo(&timeline, &item_id).await?;
    if wedged {
        handle
            .unwedge()
            .await
            .map_err(|_| CoreError::Internal("the message could not be retried".into()))?;
    }
    timeline.room().send_queue().set_enabled(true);
    Ok(())
}

/// Drops a local echo, then re-enables the room's send queue so that later
/// messages go out.
async fn discard_task(timeline: Arc<Timeline>, item_id: String) -> Result<(), CoreError> {
    let (handle, _) = local_echo(&timeline, &item_id).await?;
    let result = handle.abort().await;
    // Enabled even when the abort failed: the queue must not stay stuck.
    timeline.room().send_queue().set_enabled(true);
    result
        .map(drop)
        .map_err(|_| CoreError::Internal("the message could not be discarded".into()))
}

/// Marks a back-pagination as running; at most one runs per selection.
/// Dropping the permit frees the slot.
struct PaginationPermit(Arc<AtomicBool>);

impl PaginationPermit {
    /// Takes the slot, or `None` while another pagination holds it.
    fn try_acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag.clone()))
    }
}

impl Drop for PaginationPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Loads 20 older events, unless a pagination of this room is already
/// running (then the request is dropped: the app asks again when idle).
async fn paginate_task(timeline: Arc<Timeline>, flag: Arc<AtomicBool>) -> Result<(), CoreError> {
    let Some(_permit) = PaginationPermit::try_acquire(&flag) else {
        return Ok(());
    };
    timeline
        .paginate_backwards(PAGINATION_BATCH)
        .await
        .map(drop)
        .map_err(|_| CoreError::Internal("older messages could not be loaded".into()))
}

/// Sends a read receipt for the newest event, and moves the read marker when
/// asked.
async fn mark_read_task(timeline: Arc<Timeline>, fully_read: bool) -> Result<(), CoreError> {
    let fail = |_| CoreError::Internal("the read receipt could not be sent".into());
    timeline
        .mark_as_read(ReceiptType::Read)
        .await
        .map_err(fail)?;
    if fully_read {
        timeline
            .mark_as_read(ReceiptType::FullyRead)
            .await
            .map_err(fail)?;
    }
    Ok(())
}

/// Moves only the private fully-read marker; no public receipt is sent.
async fn move_marker_task(timeline: Arc<Timeline>) -> Result<(), CoreError> {
    timeline
        .mark_as_read(ReceiptType::FullyRead)
        .await
        .map(drop)
        .map_err(|_| CoreError::Internal("the read marker could not be moved".into()))
}

/// Tells the room whether the user is typing.
async fn typing_task(timeline: Arc<Timeline>, typing: bool) -> Result<(), CoreError> {
    timeline
        .room()
        .typing_notice(typing)
        .await
        .map_err(|_| CoreError::Internal("the typing notice could not be sent".into()))
}

/// The API state for an SDK back-pagination status.
fn pagination_state(status: PaginationStatus) -> PaginationState {
    match status {
        PaginationStatus::Idle {
            hit_timeline_start: true,
        } => PaginationState::ReachedStart,
        PaginationStatus::Idle { .. } => PaginationState::Idle,
        PaginationStatus::Paginating => PaginationState::Loading,
    }
}

/// Forwards the back-pagination status of the room as `Pagination` events,
/// the current value first. Ends with the selection (it is aborted) or when
/// the receiver is gone.
async fn forward_pagination(
    room_id: RoomId,
    timeline: Arc<Timeline>,
    events: mpsc::Sender<CoreEvent>,
) {
    let Some((current, stream)) = timeline.live_back_pagination_status().await else {
        return;
    };
    pin_mut!(stream);
    let mut next = Some(current);
    while let Some(status) = next {
        let event = CoreEvent::Pagination {
            room_id: room_id.clone(),
            state: pagination_state(status),
        };
        if events.send(event).await.is_err() {
            return;
        }
        next = stream.next().await;
    }
}

/// The typing users with the names the room's members have, if known.
fn typing_users(ids: Vec<OwnedUserId>, names: &HashMap<OwnedUserId, String>) -> Vec<TypingUser> {
    ids.into_iter()
        .take(MAX_TYPING_USERS)
        .map(|id| TypingUser {
            name: names.get(&id).cloned(),
            user_id: UserId::from(id.as_str()),
        })
        .collect()
}

/// Forwards who else is typing in the room as `Typing` events. Ends with the
/// selection (it is aborted); the SDK event handler goes with the guard.
async fn forward_typing(room_id: RoomId, timeline: Arc<Timeline>, events: mpsc::Sender<CoreEvent>) {
    let room = timeline.room();
    let (_guard, mut receiver) = room.subscribe_to_typing_notifications();
    loop {
        let ids = match receiver.recv().await {
            Ok(ids) => ids,
            // Only the latest list matters; wait for the next one.
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let mut names = HashMap::new();
        for id in ids.iter().take(MAX_TYPING_USERS) {
            // A failed lookup just leaves the name out.
            if let Ok(Some(member)) = room.get_member_no_sync(id).await
                && let Some(name) = member.display_name()
            {
                names.insert(id.clone(), name.to_owned());
            }
        }
        let event = CoreEvent::Typing {
            room_id: room_id.clone(),
            users: typing_users(ids, &names),
        };
        if events.send(event).await.is_err() {
            return;
        }
    }
}

impl Engine {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let mut watch_changes = true;
        loop {
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    None | Some(Command::Shutdown) => break,
                    Some(cmd) => {
                        self.handle(cmd).await;
                        watch_changes = true;
                    }
                },
                joined = next_oauth(&mut self.oauth) => self.on_oauth_done(joined).await,
                notice = next_media_notice(&mut self.call) => self.on_media_notice(notice).await,
                change = next_change(&mut self.active), if watch_changes => match change {
                    Change::Got(c) => self.on_change(Some(c)).await,
                    Change::Missed => self.on_change(None).await,
                    Change::Closed => watch_changes = false,
                },
            }
        }
        self.leave_call().await;
        if let Some(session) = self.audio_session.take() {
            session.stop().await;
        }
        self.cancel_oauth_and_wait().await;
        // Stop sync and drop the client before the events sender so that, once
        // the channel closes, the store is already released.
        if let Some(mut active) = self.active.take() {
            active.stop_tasks().await;
        }
    }

    async fn emit(&self, event: CoreEvent) {
        // A dropped receiver just means nobody listens any more.
        let _ = self.events.send(event).await;
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::RestoreSession => self.restore().await,
            Command::LoginPassword {
                username,
                password,
                server,
            } => {
                if let Some(e) = self.login_busy() {
                    self.emit(CoreEvent::LoginFailed(e)).await;
                    return;
                }
                match self.login_password(&username, &password, server).await {
                    Ok(event) => {
                        self.emit(event).await;
                        self.start_security();
                        self.start_sync().await;
                    }
                    Err(e) => self.emit(CoreEvent::LoginFailed(e)).await,
                }
            }
            Command::Logout => self.logout().await,
            // Awaited here, not in the action set: it is a local store write,
            // and a draft saved just before `Shutdown` must not be cut off.
            Command::SaveDraft { room_id, draft } => {
                self.save_draft(&room_id, draft.as_ref()).await
            }
            Command::LoginOAuth { server } => self.login_oauth(server).await,
            // The task ends with `Cancelled`, which `on_oauth_done` reports.
            Command::CancelOAuth => {
                if let Some(pending) = self.oauth.as_mut() {
                    pending.cancelled = true;
                    if let Some(cancel) = pending.cancel.take() {
                        let _ = cancel.send(());
                    }
                }
            }
            Command::SelectRoom(room_id) => {
                if let Err(e) = self.select_room(room_id).await {
                    self.emit(CoreEvent::Error(e)).await;
                }
            }
            Command::SendMessage { text } => self.send_message(&text).await,
            Command::SetRoomListQuery(query) => {
                // The value is stored even with no forwarder listening; an
                // unchanged query does not wake one.
                self.room_list_query.send_if_modified(|current| {
                    let changed = *current != query;
                    if changed {
                        *current = query;
                    }
                    changed
                });
            }
            Command::LoadMoreRooms => {
                let more = self
                    .active
                    .as_ref()
                    .and_then(|a| a.sync.as_ref())
                    .map(|s| s.more_rooms.clone());
                // A full channel already has requests pending. With no sync
                // there is no list, so this is a no-op.
                if let Some(more) = more {
                    let _ = more.try_send(());
                }
            }
            // Validated, bounded and deduplicated in `avatars`; the fetches run
            // in tasks, so this never waits for the network.
            Command::FetchAvatars(ids) => match self.active.as_mut() {
                Some(active) => active.avatars.request(ids, &active.fetcher, &self.events),
                // No session to fetch with: every ID is unavailable.
                None => answer_unavailable(ids, &self.events).await,
            },
            Command::Send(message) => {
                let room_id = message.room_id.clone();
                self.run_action(room_id, ActionKind::Send, |timeline| {
                    send_task(timeline, message)
                })
                .await;
            }
            Command::RetrySend { room_id, item_id } => {
                self.run_action(room_id, ActionKind::Retry, |timeline| {
                    retry_task(timeline, item_id)
                })
                .await;
            }
            Command::DiscardSend { room_id, item_id } => {
                self.run_action(room_id, ActionKind::Discard, |timeline| {
                    discard_task(timeline, item_id)
                })
                .await;
            }
            Command::Edit { item_id, message } => {
                let room_id = message.room_id.clone();
                self.run_action(room_id, ActionKind::Edit, |timeline| {
                    edit_task(timeline, item_id, message)
                })
                .await;
            }
            Command::Redact {
                room_id,
                item_id,
                reason,
            } => {
                self.run_action(room_id, ActionKind::Redact, |timeline| {
                    redact_task(timeline, item_id, reason)
                })
                .await;
            }
            Command::ToggleReaction {
                room_id,
                item_id,
                key,
            } => {
                self.run_action(room_id, ActionKind::React, |timeline| {
                    react_task(timeline, item_id, key)
                })
                .await;
            }
            Command::SearchMembers { room_id, query } => {
                let query = MessageText::from(query);
                let events = self.events.clone();
                let synced = self
                    .active
                    .as_ref()
                    .and_then(|a| a.selected.as_ref())
                    .map(|s| s.members_synced.clone());
                self.run_action(room_id, ActionKind::Members, |timeline| {
                    // `run_action` only calls this with a selected room, so
                    // `synced` is present then.
                    members_task(timeline, synced.unwrap_or_default(), events, query)
                })
                .await;
            }
            Command::PaginateBackwards { room_id } => {
                let flag = self
                    .active
                    .as_ref()
                    .and_then(|a| a.selected.as_ref())
                    .map(|s| s.paginating.clone())
                    .unwrap_or_default();
                self.run_action(room_id, ActionKind::Paginate, |timeline| {
                    paginate_task(timeline, flag)
                })
                .await;
            }
            Command::MarkRead {
                room_id,
                fully_read,
            } => {
                self.run_action(room_id, ActionKind::Receipt, |timeline| {
                    mark_read_task(timeline, fully_read)
                })
                .await;
            }
            Command::MoveReadMarker { room_id } => {
                self.run_action(room_id, ActionKind::Receipt, move_marker_task)
                    .await;
            }
            Command::SetTyping { room_id, typing } => {
                self.run_action(room_id, ActionKind::Typing, |timeline| {
                    typing_task(timeline, typing)
                })
                .await;
            }
            // Network work runs in the security tasks, so `handle` never
            // waits for it.
            Command::BootstrapCrossSigning { password } => match self.active.as_mut() {
                Some(active) => {
                    let pause = active.sync.as_ref().map(|s| s.pause.clone());
                    active
                        .security
                        .bootstrap(&active.client, pause, password, &self.events);
                }
                None => self.not_logged_in(ActionKind::CrossSigning).await,
            },
            Command::ResetIdentity { password } => match self.active.as_mut() {
                Some(active) => {
                    let pause = active.sync.as_ref().map(|s| s.pause.clone());
                    active
                        .security
                        .reset(&active.client, pause, password, &self.events);
                }
                None => self.not_logged_in(ActionKind::CrossSigning).await,
            },
            Command::CancelIdentityReset => match self.active.as_mut() {
                Some(active) => active.security.cancel_reset(),
                None => self.not_logged_in(ActionKind::CrossSigning).await,
            },
            Command::EnableRecovery => match self.active.as_mut() {
                Some(active) => active
                    .security
                    .enable_recovery(&active.client, &self.events),
                None => self.not_logged_in(ActionKind::Recovery).await,
            },
            Command::Recover { key } => match self.active.as_mut() {
                Some(active) => active.security.recover(&active.client, key, &self.events),
                None => self.not_logged_in(ActionKind::Recovery).await,
            },
            Command::ChangeRecoveryKey => match self.active.as_mut() {
                Some(active) => active
                    .security
                    .change_recovery_key(&active.client, &self.events),
                None => self.not_logged_in(ActionKind::Recovery).await,
            },
            Command::RequestSelfVerification => match self.active.as_mut() {
                Some(active) => {
                    active
                        .security
                        .verification
                        .request(&active.client, &self.events, Target::Own)
                }
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::RequestDeviceVerification { device_id } => match self.active.as_mut() {
                Some(active) => active.security.verification.request(
                    &active.client,
                    &self.events,
                    Target::Device(device_id),
                ),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::RequestUserVerification { user_id } => match self.active.as_mut() {
                Some(active) => active.security.verification.request(
                    &active.client,
                    &self.events,
                    Target::User(user_id),
                ),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::AcceptVerification { id } => match self.active.as_mut() {
                Some(active) => active.security.verification.accept(&self.events, &id),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::StartSas { id } => match self.active.as_mut() {
                Some(active) => active.security.verification.start_sas(&self.events, &id),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::ConfirmQrScanned { id } => match self.active.as_mut() {
                Some(active) => active
                    .security
                    .verification
                    .confirm_qr_scanned(&self.events, &id),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::ConfirmSas { id } => match self.active.as_mut() {
                Some(active) => active.security.verification.confirm_sas(&self.events, &id),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::SasMismatch { id } => match self.active.as_mut() {
                Some(active) => active.security.verification.mismatch(&self.events, &id),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::CancelVerification { id } => match self.active.as_mut() {
                Some(active) => active.security.verification.cancel(&self.events, &id),
                None => self.not_logged_in(ActionKind::Verification).await,
            },
            Command::WatchUserTrust { user_id } => match self.active.as_mut() {
                Some(active) => {
                    active
                        .security
                        .watch_user_trust(&active.client, &user_id, &self.events);
                }
                None => self.not_logged_in(ActionKind::Devices).await,
            },
            Command::ListDevices => match self.active.as_mut() {
                Some(active) => active.security.list_devices(&active.client, &self.events),
                None => self.not_logged_in(ActionKind::Devices).await,
            },
            Command::RenameDevice { device_id, name } => match self.active.as_mut() {
                Some(active) => {
                    active
                        .security
                        .rename_device(&active.client, &device_id, &name, &self.events);
                }
                None => self.not_logged_in(ActionKind::Devices).await,
            },
            Command::SignOutDevices {
                device_ids,
                password,
            } => match self.active.as_mut() {
                Some(active) => active.security.sign_out_devices(
                    &active.client,
                    &device_ids,
                    password,
                    &self.events,
                ),
                None => self.not_logged_in(ActionKind::SignOutDevices).await,
            },
            // Audio settings.
            Command::GetAudioSettings => {
                let settings = self.audio_settings().await.clone();
                self.emit(CoreEvent::AudioSettings(settings)).await;
            }
            Command::ProbeGpuEnhancement => {
                let running = self
                    .audio_session
                    .as_ref()
                    .and_then(|s| s.enhancer_running());
                let events = self.events.clone();
                tokio::spawn(async move {
                    let available = tokio::task::spawn_blocking(|| {
                        tessera_audio::default_model_dir()
                            .map(|dir| crate::gpu_enhancement::probe(&dir))
                            .unwrap_or_default()
                    })
                    .await
                    .unwrap_or_default();
                    let event = crate::api::GpuEnhancement { available, running };
                    let _ = events.send(CoreEvent::GpuEnhancement(event)).await;
                });
            }
            Command::UpdateAudioSettings(settings) => {
                self.store_audio_settings(settings.validated()).await;
            }
            Command::SetUserVolume { user_id, percent } => {
                let mut settings = self.audio_settings().await.clone();
                settings.user_volumes.insert(user_id.clone(), percent);
                let settings = settings.validated();
                let applied = settings.user_volume(&user_id);
                self.store_audio_settings(settings).await;
                if let Some(call) = &self.call {
                    call.set_user_volume(&user_id, applied);
                }
            }
            Command::SetUserCleaning { user_id, cleaning } => {
                let mut settings = self.audio_settings().await.clone();
                settings.user_cleaning.insert(user_id, cleaning);
                // Stored, echoed and applied to the call like any change.
                self.store_audio_settings(settings.validated()).await;
            }
            Command::JoinCall { room_id, intent: _ } => self.join_call(room_id).await,
            Command::LeaveCall => {
                if self.call.is_some() {
                    self.leave_call().await;
                } else {
                    self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
                }
            }
            Command::SetMicMuted(muted) => {
                if self.call.is_some() {
                    self.mic_muted = muted;
                    self.apply_mute();
                } else {
                    self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
                }
            }
            Command::SetDeafened(deafened) => {
                if self.call.is_some() {
                    self.deafened = deafened;
                    self.apply_mute();
                } else {
                    self.emit(CoreEvent::Error(CoreError::NotInCall)).await;
                }
            }
            Command::DeclineCall { room_id } => self.decline_call(room_id).await,
            Command::ListAudioDevices => self.list_audio_devices().await,
            Command::StartMicTest { loopback } => self.start_mic_test(loopback).await,
            Command::StopMicTest => {
                if let Some(session) = self.audio_session.take() {
                    session.stop().await;
                }
            }
            Command::PushToTalk { pressed } => {
                if let Some(session) = &self.audio_session {
                    session.push_to_talk(pressed);
                }
            }
            // Handled in `run`.
            Command::Shutdown => {}
        }
    }

    /// Lists audio devices (blocking enumeration off the async threads) and,
    /// the first time, starts the hot-plug watcher, which sends
    /// `AudioDevices` again whenever the list changes.
    async fn list_audio_devices(&mut self) {
        match blocking(tessera_audio::list_devices).await {
            Ok(Ok(list)) => {
                self.emit(CoreEvent::AudioDevices(audio_devices(list)))
                    .await
            }
            Ok(Err(_)) | Err(_) => {
                self.emit(CoreEvent::Error(CoreError::Audio(
                    "audio devices could not be listed".into(),
                )))
                .await;
            }
        }
        if self.device_watcher.is_none() {
            let events = self.events.clone();
            let watcher = tessera_audio::DeviceWatcher::spawn(DEVICE_POLL_INTERVAL, move |list| {
                // A plain thread, not a runtime one, so blocking is fine; a
                // closed channel means the engine is gone.
                let _ = events.blocking_send(CoreEvent::AudioDevices(audio_devices(list)));
            });
            match watcher {
                Ok(w) => self.device_watcher = Some(w),
                Err(_) => tracing::warn!("audio device watcher could not start"),
            }
        }
    }

    /// The audio settings, loading them from disk on first use. A file that
    /// cannot be used is reported once and replaced by the defaults in memory
    /// (the file itself stays until the next save).
    async fn audio_settings(&mut self) -> &AudioSettings {
        if self.audio_settings.is_none() {
            let paths = self.config.data.clone();
            let loaded = blocking(move || call_settings::load(&paths))
                .await
                .unwrap_or_else(|e| call_settings::Loaded {
                    settings: AudioSettings::default(),
                    problem: Some(e),
                });
            if let Some(problem) = loaded.problem {
                self.emit(CoreEvent::Error(problem)).await;
            }
            self.audio_settings = Some(loaded.settings);
        }
        self.audio_settings.as_ref().expect("loaded above")
    }

    /// Adopts already validated settings, saves them, and echoes them back.
    /// A failed save is reported; the settings still apply for this run.
    async fn store_audio_settings(&mut self, settings: AudioSettings) {
        let paths = self.config.data.clone();
        let to_save = settings.clone();
        let saved = blocking(move || call_settings::save(&paths, &to_save))
            .await
            .and_then(|r| r);
        self.audio_settings = Some(settings.clone());
        if let Ok(mut shared) = self.user_volumes.write() {
            shared.clone_from(&settings.user_volumes);
        }
        if let Some(call) = &self.call {
            call.set_cleaning(settings.remote_cleaning, settings.user_cleaning.clone());
        }
        if let Err(e) = saved {
            self.emit(CoreEvent::Error(e)).await;
        }
        self.emit(CoreEvent::AudioSettings(settings.clone())).await;
        // A running session follows; a device change reopens it.
        if let Some(session) = &self.audio_session
            && !session.apply(&settings)
        {
            match session.mode() {
                Mode::MicTest { loopback } => self.start_mic_test(loopback).await,
                // Rewiring a running call's media to new devices is a
                // follow-up; the new devices are used from the next call.
                Mode::Call => {
                    self.emit(CoreEvent::Error(CoreError::Audio(
                        "the new audio devices are used from the next call".into(),
                    )))
                    .await;
                }
            }
        }
    }

    /// Joins `room_id`'s call, leaving another call first.
    async fn join_call(&mut self, room_id: RoomId) {
        let Some(active) = self.active.as_ref() else {
            self.emit(CoreEvent::Error(CoreError::NotLoggedIn)).await;
            return;
        };
        let client = active.client.clone();
        if let Some(call) = &self.call {
            if call.room_id == room_id {
                let encrypted = call.encrypted;
                self.emit(CoreEvent::CallState(CallState::Connected {
                    room_id,
                    encrypted,
                }))
                .await;
                return;
            }
            self.leave_call().await;
        }
        // Participants carry the per-user volumes.
        let volumes = self.audio_settings().await.user_volumes.clone();
        if let Ok(mut shared) = self.user_volumes.write() {
            *shared = volumes;
        }
        self.mic_muted = false;
        self.deafened = false;
        self.emit(CoreEvent::CallState(CallState::Joining {
            room_id: room_id.clone(),
        }))
        .await;
        let audio = self.start_call_audio().await;
        match CallSession::join(
            &client,
            room_id.clone(),
            audio,
            self.events.clone(),
            self.user_volumes.clone(),
            self.config.call_service_override.as_deref(),
        )
        .await
        {
            Ok(session) => {
                let encrypted = session.encrypted;
                if let Some(calls) = self.calls() {
                    calls.set_sent(&room_id, session.notification.clone());
                    if calls.take_ring(&room_id).is_some() {
                        // Answered here.
                        self.emit(CoreEvent::RingingStopped {
                            room_id: room_id.clone(),
                        })
                        .await;
                    }
                }
                self.call = Some(session);
                self.emit(CoreEvent::CallState(CallState::Connected {
                    room_id,
                    encrypted,
                }))
                .await;
            }
            Err(e) => {
                self.stop_call_audio().await;
                self.emit(CoreEvent::CallState(CallState::Left {
                    room_id,
                    reason: CallEndReason::Failed(e.to_string()),
                }))
                .await;
                self.emit(CoreEvent::Error(e)).await;
            }
        }
    }

    /// Opens the audio devices for a call (replacing a mic test). Without
    /// usable devices the call still joins, silent, and the error is shown.
    async fn start_call_audio(&mut self) -> CallAudio {
        if let Some(session) = self.audio_session.take() {
            session.stop().await;
        }
        let settings = self.audio_settings().await.clone();
        let bitrate_kbps = settings.voice_bitrate_kbps;
        let cleaning = (settings.remote_cleaning, settings.user_cleaning.clone());
        match AudioSession::start(
            &settings,
            Mode::Call,
            &self.config.audio,
            self.events.clone(),
        )
        .await
        {
            Ok((session, outgoing)) => {
                let voice = Some(session.voice());
                self.audio_session = Some(session);
                CallAudio {
                    voice,
                    outgoing,
                    bitrate_kbps,
                    cleaning,
                }
            }
            Err(e) => {
                self.emit(CoreEvent::Error(e)).await;
                CallAudio {
                    voice: None,
                    outgoing: None,
                    bitrate_kbps,
                    cleaning,
                }
            }
        }
    }

    /// Closes the call's audio devices, if they are open.
    async fn stop_call_audio(&mut self) {
        if self
            .audio_session
            .as_ref()
            .is_some_and(|s| s.mode() == Mode::Call)
            && let Some(session) = self.audio_session.take()
        {
            session.stop().await;
        }
    }

    /// Applies mute and deafen (deafen also mutes, as in Discord).
    fn apply_mute(&self) {
        let muted = self.mic_muted || self.deafened;
        if let Some(session) = &self.audio_session {
            session.set_muted(muted);
            session.set_deafened(self.deafened);
        }
        if let Some(call) = &self.call {
            call.set_muted(muted);
        }
    }

    /// Reacts to the SFU connection of the current call.
    async fn on_media_notice(&mut self, notice: MediaNotice) {
        let Some(call) = &self.call else {
            return;
        };
        let room_id = call.room_id.clone();
        match notice {
            MediaNotice::Reconnecting => {
                self.call_reconnects += 1;
                self.emit(CoreEvent::CallState(CallState::Reconnecting {
                    room_id,
                    attempt: self.call_reconnects,
                }))
                .await;
            }
            MediaNotice::Reconnected => {
                self.call_reconnects = 0;
                let encrypted = self.call.as_ref().is_some_and(|c| c.encrypted);
                self.emit(CoreEvent::CallState(CallState::Connected {
                    room_id,
                    encrypted,
                }))
                .await;
            }
            MediaNotice::Disconnected => {
                if let Some(call) = self.call.take() {
                    if let Some(calls) = self.calls() {
                        calls.set_sent(&call.room_id, None);
                    }
                    call.leave().await;
                }
                self.stop_call_audio().await;
                self.emit(CoreEvent::CallState(CallState::Left {
                    room_id,
                    reason: CallEndReason::Failed("the call server connection was lost".into()),
                }))
                .await;
            }
        }
    }

    /// The session's call watcher, while syncing.
    fn calls(&self) -> Option<&CallWatch> {
        self.active.as_ref()?.sync.as_ref().map(|s| &s.calls)
    }

    /// Declines the ring in `room_id`, on every device of ours.
    async fn decline_call(&mut self, room_id: RoomId) {
        let Some(active) = self.active.as_ref() else {
            self.emit(CoreEvent::Error(CoreError::NotLoggedIn)).await;
            return;
        };
        let Some(notification) = self.calls().and_then(|c| c.take_ring(&room_id)) else {
            self.emit(CoreEvent::Error(CoreError::Call(
                "nothing is ringing in this room".into(),
            )))
            .await;
            return;
        };
        let room = parse_room_id(&room_id)
            .ok()
            .and_then(|id| active.client.get_room(&id));
        let sent = match room {
            Some(room) => room
                .send_raw(
                    crate::call::ringing::DECLINE_EVENT,
                    crate::call::ringing::decline_content(&notification),
                )
                .await
                .is_ok(),
            None => false,
        };
        self.emit(CoreEvent::RingingStopped {
            room_id: room_id.clone(),
        })
        .await;
        if !sent {
            self.emit(CoreEvent::Error(CoreError::Call(
                "could not send the decline; the caller keeps ringing".into(),
            )))
            .await;
        }
    }

    /// Leaves the current call, if any.
    async fn leave_call(&mut self) {
        if let Some(call) = self.call.take() {
            let room_id = call.room_id.clone();
            if let Some(calls) = self.calls() {
                calls.set_sent(&room_id, None);
            }
            call.leave().await;
            self.stop_call_audio().await;
            self.emit(CoreEvent::CallState(CallState::Left {
                room_id,
                reason: CallEndReason::Hangup,
            }))
            .await;
        }
    }

    /// Opens the microphone (and, with `loopback`, the speakers) and starts
    /// reporting levels; replaces a running mic test.
    async fn start_mic_test(&mut self, loopback: bool) {
        if let Some(session) = self.audio_session.take() {
            session.stop().await;
        }
        if self.call.is_some() {
            self.emit(CoreEvent::Error(CoreError::Audio(
                "the mic test is not available during a call".into(),
            )))
            .await;
            return;
        }
        let settings = self.audio_settings().await.clone();
        let mode = Mode::MicTest { loopback };
        match AudioSession::start(&settings, mode, &self.config.audio, self.events.clone()).await {
            Ok((session, _)) => self.audio_session = Some(session),
            Err(e) => self.emit(CoreEvent::Error(e)).await,
        }
    }

    /// Answers a Phase 4 action sent without a session. These actions belong
    /// to no room, so `room_id` is empty.
    async fn not_logged_in(&self, action: ActionKind) {
        self.emit(CoreEvent::ActionFailed {
            room_id: RoomId::from(""),
            action,
            error: CoreError::NotLoggedIn,
        })
        .await;
    }

    /// Starts the security status forwarder of the new session.
    fn start_security(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.security.start(&active.client, &self.events);
        }
    }

    /// Why a new login cannot start now, if so.
    fn login_busy(&self) -> Option<CoreError> {
        if self.active.is_some() {
            Some(CoreError::Login("already logged in".into()))
        } else if self.oauth.is_some() {
            Some(CoreError::Login("a login is already in progress".into()))
        } else {
            None
        }
    }

    /// Starts an OAuth login; the outcome arrives through `on_oauth_done`.
    async fn login_oauth(&mut self, server: String) {
        if let Some(e) = self.login_busy() {
            self.emit(CoreEvent::LoginFailed(e)).await;
            return;
        }
        let server = self.config.homeserver_override.clone().unwrap_or(server);
        let session_id = generate_session_id();
        let passphrase = generate_store_passphrase();
        let paths = self.config.data.clone();
        let sid = session_id.clone();
        let dir = match blocking(move || paths.create_session_store_dir(&sid)).await {
            Ok(Ok(dir)) => dir,
            Ok(Err(e)) => {
                self.emit(CoreEvent::LoginFailed(fs_err(e))).await;
                return;
            }
            Err(e) => {
                self.emit(CoreEvent::LoginFailed(e)).await;
                return;
            }
        };
        let (cancel, cancelled) = oneshot::channel();
        let events = self.events.clone();
        let task_server = server.clone();
        let task_passphrase = passphrase.clone();
        let task = tokio::spawn(async move {
            let mut cancelled = cancelled;
            // Dropping the build future on cancel abandons the network call.
            let client = tokio::select! {
                c = build_client(&dir, &task_passphrase, ServerSpec::NameOrUrl(&task_server)) => c?,
                _ = &mut cancelled => return Err(CoreError::Cancelled),
            };
            authorize(&client, &events, cancelled).await?;
            Ok(client)
        });
        self.oauth = Some(PendingOAuth {
            cancel: Some(cancel),
            cancelled: false,
            task,
            server,
            session_id,
            passphrase,
        });
    }

    /// Finishes an OAuth login: the same account binding and persistence as a
    /// password login, then sync. On any failure the store directory goes.
    async fn on_oauth_done(
        &mut self,
        joined: Result<Result<Client, CoreError>, tokio::task::JoinError>,
    ) {
        let Some(pending) = self.oauth.take() else {
            return;
        };
        let result =
            joined.unwrap_or_else(|_| Err(CoreError::Internal("background task failed".into())));
        let outcome = match result {
            // Cancelled after the login had already succeeded: undo it.
            Ok(client) if pending.cancelled => {
                self.discard_client(client).await;
                Err(CoreError::Cancelled)
            }
            Ok(client) => {
                self.adopt_login(
                    client,
                    None,
                    &pending.server,
                    &pending.session_id,
                    pending.passphrase.clone(),
                )
                .await
            }
            Err(e) => Err(e),
        };
        match outcome {
            Ok((active, event)) => {
                self.active = Some(active);
                self.emit(event).await;
                self.start_security();
                self.start_sync().await;
            }
            Err(e) => {
                // The client is already dropped, so the store is released.
                let paths = self.config.data.clone();
                let sid = pending.session_id;
                let _ = blocking(move || paths.remove_session_store_dir(&sid)).await;
                self.emit(CoreEvent::LoginFailed(e)).await;
            }
        }
    }

    /// On shutdown: cancels a pending OAuth login, waits (bounded) for its
    /// task to release the client, and removes its store directory.
    async fn cancel_oauth_and_wait(&mut self) {
        self.cancel_oauth_and_wait_within(OAUTH_STOP_TIMEOUT).await;
    }

    async fn cancel_oauth_and_wait_within(&mut self, limit: Duration) {
        let Some(mut pending) = self.oauth.take() else {
            return;
        };
        pending.cancelled = true;
        if let Some(cancel) = pending.cancel.take() {
            let _ = cancel.send(());
        }
        // The task drops the client when it ends. If it does not end in time
        // (stuck network call or a front end that stopped draining events),
        // abort it and wait for the abort, so the store is released.
        match tokio::time::timeout(limit, &mut pending.task).await {
            Ok(Ok(Ok(client))) => self.discard_client(client).await,
            Ok(_) => {}
            Err(_) => {
                pending.task.abort();
                let _ = (&mut pending.task).await;
            }
        }
        let paths = self.config.data.clone();
        let sid = pending.session_id;
        let _ = blocking(move || paths.remove_session_store_dir(&sid)).await;
    }

    /// Logs out a client whose login was cancelled after it succeeded (best
    /// effort, bounded) and drops it, which releases its store. Nothing is
    /// persisted.
    async fn discard_client(&self, client: Client) {
        match tokio::time::timeout(OAUTH_STOP_TIMEOUT, client.logout()).await {
            Ok(Ok(())) => {}
            _ => warn!("best-effort logout of a cancelled login did not succeed"),
        }
    }

    /// Stores `draft` as the composer draft of `room_id`, or removes it
    /// (`None` or blank). Without a session or a known room, nothing happens.
    async fn save_draft(&self, room_id: &RoomId, draft: Option<&Draft>) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let Some(room) = parse_room_id(room_id)
            .ok()
            .and_then(|id| active.client.get_room(&id))
        else {
            return;
        };
        let result = match draft.and_then(drafts::to_sdk) {
            Some(sdk) => room.save_composer_draft(sdk, None).await,
            None => room.clear_composer_draft(None).await,
        };
        if result.is_err() {
            warn!("the composer draft could not be stored");
        }
    }

    /// Opens the timeline of `room_id`, replacing any previous selection.
    async fn select_room(&mut self, room_id: RoomId) -> Result<(), CoreError> {
        let active = self.active.as_mut().ok_or(CoreError::NotLoggedIn)?;
        let sdk_id = parse_room_id(&room_id)?;
        let room = active
            .client
            .get_room(&sdk_id)
            .ok_or_else(|| CoreError::UnknownRoom(room_id.clone()))?;
        if let Some(previous) = active.selected.take() {
            previous.stop().await;
        }
        let timeline = room
            .timeline()
            .await
            .map_err(|_| CoreError::Internal("could not open the room timeline".into()))?;
        // Typing and receipts of the open room arrive through sliding sync
        // only when the room is subscribed.
        if let Some(sync) = active.sync.as_ref() {
            sync.service
                .room_list_service()
                .set_room_subscriptions(&[&sdk_id])
                .await;
        }
        // A store error loses only the draft, never the room.
        let draft = room
            .load_composer_draft(None)
            .await
            .ok()
            .flatten()
            .and_then(drafts::from_sdk);
        let (items, stream) = timeline.subscribe().await;
        let task = tokio::spawn(forward_timeline(
            room_id.clone(),
            items,
            stream,
            self.events.clone(),
            active.client.user_id().map(ToOwned::to_owned),
            draft,
        ));
        let timeline = Arc::new(timeline);
        // Both forwarders live in the action set, so they end with the
        // selection.
        let mut tasks = JoinSet::new();
        tasks.spawn(forward_pagination(
            room_id.clone(),
            timeline.clone(),
            self.events.clone(),
        ));
        tasks.spawn(forward_typing(
            room_id.clone(),
            timeline.clone(),
            self.events.clone(),
        ));
        active.selected = Some(Selected {
            room_id,
            timeline,
            task,
            tasks,
            members_synced: Arc::new(OnceCell::new()),
            paginating: Arc::new(AtomicBool::new(false)),
        });
        // A DM with exactly one other user shows that user's trust (Q10).
        if let Some(peer) = security::dm_peer_of(&room, active.client.user_id()) {
            active.security.watch_user_trust(
                &active.client,
                &crate::api::UserId::from(peer.as_str()),
                &self.events,
            );
        }
        Ok(())
    }

    /// Runs an action on the selected room's timeline in the background, or
    /// reports `ActionFailed` when there is no session or `room_id` is not the
    /// selected room.
    async fn run_action<F, Fut>(&mut self, room_id: RoomId, action: ActionKind, work: F)
    where
        F: FnOnce(Arc<Timeline>) -> Fut,
        Fut: std::future::Future<Output = Result<(), CoreError>> + Send + 'static,
    {
        let events = self.events.clone();
        let error = match self.active.as_mut() {
            None => CoreError::NotLoggedIn,
            Some(active) => match active.selected.as_mut().filter(|s| s.room_id == room_id) {
                Some(selected) => {
                    selected.spawn_action(action, &events, work);
                    return;
                }
                None => CoreError::NoRoomSelected,
            },
        };
        self.emit(CoreEvent::ActionFailed {
            room_id,
            action,
            error,
        })
        .await;
    }

    /// Sends a text message to the selected room.
    async fn send_message(&self, text: &MessageText) {
        let Some(active) = self.active.as_ref() else {
            self.emit(CoreEvent::Error(CoreError::NotLoggedIn)).await;
            return;
        };
        let Some(selected) = active.selected.as_ref() else {
            self.emit(CoreEvent::Error(CoreError::NoRoomSelected)).await;
            return;
        };
        // The text is exposed only here, where the SDK needs it.
        let content = RoomMessageEventContent::text_plain(text.expose());
        if selected.timeline.send(content.into()).await.is_err() {
            let error = CoreError::Internal("the message could not be queued".into());
            let room_id = selected.room_id.clone();
            self.emit(CoreEvent::SendFailed { room_id, error }).await;
        }
    }

    /// Picks the server: override, then the command's, then the MXID's.
    fn pick_server(&self, username: &str, server: Option<String>) -> Result<String, CoreError> {
        if let Some(s) = self.config.homeserver_override.clone().or(server) {
            return Ok(s);
        }
        match username.strip_prefix('@').and_then(|r| r.split_once(':')) {
            Some((_, name)) if !name.is_empty() => Ok(name.to_owned()),
            _ => Err(CoreError::Login(
                "no homeserver given: use a full user ID or specify a server".into(),
            )),
        }
    }

    async fn login_password(
        &mut self,
        username: &str,
        password: &Password,
        server: Option<String>,
    ) -> Result<CoreEvent, CoreError> {
        let server = self.pick_server(username, server)?;
        let session_id = generate_session_id();
        let passphrase = generate_store_passphrase();
        let paths = self.config.data.clone();
        let sid = session_id.clone();
        let dir = blocking(move || paths.create_session_store_dir(&sid))
            .await?
            .map_err(fs_err)?;

        let result = self
            .try_login(username, password, &server, &dir, &session_id, passphrase)
            .await;
        match result {
            Ok((active, event)) => {
                self.active = Some(active);
                Ok(event)
            }
            Err(e) => {
                let paths = self.config.data.clone();
                let _ = blocking(move || paths.remove_session_store_dir(&session_id)).await;
                Err(e)
            }
        }
    }

    async fn try_login(
        &self,
        username: &str,
        password: &Password,
        server: &str,
        dir: &std::path::Path,
        session_id: &str,
        passphrase: Zeroizing<String>,
    ) -> Result<(Active, CoreEvent), CoreError> {
        let client = build_client(dir, &passphrase, ServerSpec::NameOrUrl(server)).await?;
        // The SDK waits out `retry_after` when the server rate-limits logins,
        // which can take minutes; give the user an answer sooner.
        let login = client
            .matrix_auth()
            .login_username(username, password.expose())
            .initial_device_display_name("Tessera");
        tokio::time::timeout(LOGIN_TIMEOUT, login.send())
            .await
            .map_err(|_| CoreError::Login("login timed out, try again later".into()))?
            .map_err(|e| map_login_error(&e))?;
        self.adopt_login(client, Some(username), server, session_id, passphrase)
            .await
    }

    /// Checks a freshly logged-in client (password or OAuth) and persists its
    /// session. `username` is what the user typed for a password login; OAuth
    /// has none.
    async fn adopt_login(
        &self,
        client: Client,
        username: Option<&str>,
        server: &str,
        session_id: &str,
        passphrase: Zeroizing<String>,
    ) -> Result<(Active, CoreEvent), CoreError> {
        // Never talk to a homeserver that fails the policy, not even to log
        // out: drop the local session only (the caller removes the store dir).
        ensure_homeserver_after_login(&client)?;

        // A homeserver must not choose which account's record we overwrite:
        // check before anything is persisted. No logout is sent to it either.
        // The returned user ID must name the account asked for, and its server
        // name must be served by the homeserver we actually talked to. A bare
        // server name (no `://`) means the client was built by discovery from
        // exactly that name.
        let typed_server = (!server.contains("://")).then_some(server);
        let returned = client.user_id().map(ToString::to_string);
        let homeserver = client.homeserver();
        let bound = account_bound(
            returned.as_deref(),
            username,
            typed_server,
            &homeserver,
            discover_homeserver_url,
        )
        .await;
        if !bound {
            return Err(CoreError::Login(WRONG_ACCOUNT.into()));
        }

        match self.persist_new(&client, session_id, passphrase).await {
            Ok((record, event)) => {
                let changes = client.subscribe_to_session_changes();
                Ok((
                    Active {
                        fetcher: Arc::new(SdkFetcher(client.clone())),
                        client,
                        record,
                        changes,
                        sync: None,
                        selected: None,
                        avatars: Avatars::new(),
                        security: SecurityTasks::new(),
                    },
                    event,
                ))
            }
            // An existing record of another homeserver is left untouched, and
            // that server gets no request.
            Err(e) if matches!(&e, CoreError::Login(m) if m == WRONG_ACCOUNT) => Err(e),
            Err(e) => {
                // Do not leave a device behind that we cannot restore.
                if client.logout().await.is_err() {
                    warn!("best-effort logout after a failed login did not succeed");
                }
                let secrets = self.secrets.clone();
                if let Some(user) = client.user_id().map(ToString::to_string) {
                    let _ = blocking(move || secrets.delete(&user)).await;
                }
                Err(e)
            }
        }
    }

    async fn persist_new(
        &self,
        client: &Client,
        session_id: &str,
        passphrase: Zeroizing<String>,
    ) -> Result<(SessionRecord, CoreEvent), CoreError> {
        let internal = || CoreError::Internal("the login produced no session".into());
        let session = client.session().ok_or_else(internal)?;
        let user_id = session.meta().user_id.to_string();
        let device_id = session.meta().device_id.to_string();
        let auth = SessionAuth::from_auth_session(session).ok_or_else(internal)?;
        // Never overwrite the record of an account on another homeserver.
        let old_session = self
            .existing_record_session(&user_id, &client.homeserver())
            .await?;
        let record = SessionRecord {
            version: RECORD_VERSION,
            user_id: user_id.clone(),
            homeserver_url: client.homeserver().to_string(),
            session_id: session_id.to_owned(),
            store_passphrase: passphrase,
            auth,
        };
        self.save_record(&record).await?;
        let paths = self.config.data.clone();
        let account = user_id.clone();
        blocking(move || paths.write_last_account(&account))
            .await?
            .map_err(fs_err)?;
        // The replaced record's store is now unreachable: do not orphan it.
        if let Some(old) = old_session.filter(|old| old != session_id) {
            let paths = self.config.data.clone();
            let _ = blocking(move || paths.remove_session_store_dir(&old)).await;
        }
        Ok((
            record,
            CoreEvent::LoggedIn {
                user_id: UserId::from(user_id),
                device_id: DeviceId::from(device_id),
                restored: false,
            },
        ))
    }

    /// Looks for a stored record of `user_id`. `Ok(None)` if there is none (or
    /// it is unreadable and so unusable anyway); `Ok(Some(session_id))` if it
    /// is for the same homeserver origin, so the new login may replace it;
    /// `Err(WRONG_ACCOUNT)` if it is for another homeserver.
    async fn existing_record_session(
        &self,
        user_id: &str,
        homeserver: &url::Url,
    ) -> Result<Option<String>, CoreError> {
        let secrets = self.secrets.clone();
        let account = user_id.to_owned();
        let bytes = blocking(move || secrets.get(&account))
            .await?
            .map_err(secret_err)?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let Ok(old) = SessionRecord::from_bytes(&bytes) else {
            return Ok(None);
        };
        match url::Url::parse(&old.homeserver_url) {
            Ok(old_url) if same_origin(&old_url, homeserver) => Ok(Some(old.session_id)),
            _ => Err(CoreError::Login(WRONG_ACCOUNT.into())),
        }
    }

    async fn save_record(&self, record: &SessionRecord) -> Result<(), CoreError> {
        let bytes = record.to_bytes()?;
        let secrets = self.secrets.clone();
        let account = record.user_id.clone();
        blocking(move || secrets.set(&account, &bytes))
            .await?
            .map_err(secret_err)
    }

    async fn restore(&mut self) {
        if self.active.is_some() || self.oauth.is_some() {
            let e = CoreError::Internal("a session is already active".into());
            self.emit(CoreEvent::Error(e)).await;
            return;
        }
        match self.try_restore().await {
            Ok(Some((active, event))) => {
                self.active = Some(active);
                self.emit(event).await;
                self.start_security();
                self.start_sync().await;
            }
            Ok(None) => self.emit(CoreEvent::NoSession).await,
            Err(e) => self.emit(CoreEvent::Error(e)).await,
        }
    }

    async fn try_restore(&self) -> Result<Option<(Active, CoreEvent)>, CoreError> {
        let paths = self.config.data.clone();
        let account = blocking(move || paths.read_last_account())
            .await?
            .map_err(fs_err)?;
        let Some(account) = account else {
            return Ok(None);
        };
        let secrets = self.secrets.clone();
        let key = account.clone();
        let bytes = blocking(move || secrets.get(&key))
            .await?
            .map_err(secret_err)?;
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        let record = SessionRecord::from_bytes(&bytes)?;
        if record.user_id != account {
            return Err(CoreError::Storage(
                "stored session record is inconsistent".into(),
            ));
        }
        let dir = self
            .config
            .data
            .session_store_dir(&record.session_id)
            .map_err(fs_err)?;
        if !dir.is_dir() {
            return Err(CoreError::Storage("the session store is missing".into()));
        }
        let session = record.auth.to_auth_session();
        let user_id = session.meta().user_id.to_string();
        let device_id = session.meta().device_id.to_string();
        let client = build_client(
            &dir,
            &record.store_passphrase,
            ServerSpec::Url(&record.homeserver_url),
        )
        .await?;
        client
            .restore_session(session)
            .await
            .map_err(|_| CoreError::Storage("could not restore the session".into()))?;
        info!("session restored");
        let event = CoreEvent::LoggedIn {
            user_id: UserId::from(user_id),
            device_id: DeviceId::from(device_id),
            restored: true,
        };
        let changes = client.subscribe_to_session_changes();
        Ok(Some((
            Active {
                fetcher: Arc::new(SdkFetcher(client.clone())),
                client,
                record,
                changes,
                sync: None,
                selected: None,
                avatars: Avatars::new(),
                security: SecurityTasks::new(),
            },
            event,
        )))
    }

    /// Builds and starts the sync service and the event forwarders.
    async fn start_sync(&mut self) {
        let Some(active) = self.active.as_ref() else {
            return;
        };
        let service = match SyncService::builder(active.client.clone())
            .with_offline_mode()
            .build()
            .await
        {
            Ok(service) => Arc::new(service),
            Err(_) => {
                let e = CoreError::Internal("could not set up syncing".into());
                self.emit(CoreEvent::Error(e)).await;
                return;
            }
        };
        let (cancel, cancelled) = cancel_pair();
        let pause = SyncPause::new(SdkTarget::new(service.clone(), active.client.clone()));
        let supervisor = tokio::spawn(forward_sync_state(
            service.clone(),
            active.client.clone(),
            self.events.clone(),
            cancelled,
            pause.gate(),
        ));
        let (more_rooms, more_rx) = mpsc::channel(8);
        let room_list = tokio::spawn(forward_room_list(
            service.clone(),
            self.config.room_list_page_size,
            self.room_list_query.subscribe(),
            more_rx,
            self.events.clone(),
        ));
        let calls = CallWatch::start(&active.client, self.events.clone());
        service.start().await;
        if let Some(active) = self.active.as_mut() {
            active.sync = Some(Syncing {
                service,
                pause,
                cancel,
                supervisor,
                room_list,
                more_rooms,
                calls,
            });
        }
    }

    async fn on_change(&mut self, change: Option<SessionChange>) {
        match change {
            Some(SessionChange::UnknownToken(_)) => {
                if let Some(mut active) = self.active.take() {
                    active.stop_tasks().await;
                    let record = active.record.clone();
                    drop(active);
                    self.delete_local(&record).await;
                }
                self.emit(CoreEvent::SessionExpired).await;
            }
            // `None` means we lagged: rewrite the record to be safe.
            Some(SessionChange::TokensRefreshed) | None => self.rewrite_record().await,
        }
    }

    async fn rewrite_record(&mut self) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        // Refreshed tokens (Matrix or OAuth) must reach the keychain, or the
        // next start restores a revoked refresh token.
        match active
            .client
            .session()
            .and_then(SessionAuth::from_auth_session)
        {
            Some(auth) => active.record.auth = auth,
            None => return,
        }
        let record = active.record.clone();
        if let Err(e) = self.save_record(&record).await {
            self.emit(CoreEvent::Error(e)).await;
        }
    }

    /// Deletes the record, the last-account marker, and the store directory.
    async fn delete_local(&self, record: &SessionRecord) {
        let secrets = self.secrets.clone();
        let paths = self.config.data.clone();
        let user = record.user_id.clone();
        let sid = record.session_id.clone();
        let result = blocking(move || {
            let a = secrets.delete(&user).is_ok();
            let b = paths.remove_last_account().is_ok();
            let c = paths.remove_session_store_dir(&sid).is_ok();
            a && b && c
        })
        .await;
        if !matches!(result, Ok(true)) {
            warn!("could not delete all local session data");
        }
    }

    async fn logout(&mut self) {
        // The call needs the session to remove its membership.
        self.leave_call().await;
        let Some(mut active) = self.active.take() else {
            self.emit(CoreEvent::Error(CoreError::NotLoggedIn)).await;
            return;
        };
        active.stop_tasks().await;
        if active.client.logout().await.is_err() {
            warn!("server logout failed; deleting local session anyway");
        }
        let record = active.record.clone();
        // Release the store before deleting its directory.
        drop(active);
        self.delete_local(&record).await;
        self.emit(CoreEvent::LoggedOut).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{AvatarId, RoomListQuery, RoomListTab};
    use crate::secrets::InMemorySecretStore;

    fn engine(secrets: Arc<dyn SecretStore>) -> Engine {
        let (events, _rx) = mpsc::channel(1);
        Engine {
            config: CoreConfig {
                data: DataPaths::at(std::env::temp_dir().join("tessera-unused")),
                homeserver_override: None,
                room_list_page_size: DEFAULT_ROOM_LIST_PAGE_SIZE,
                audio: AudioBackend::default(),
                call_service_override: None,
            },
            secrets,
            events,
            active: None,
            oauth: None,
            room_list_query: watch::channel(RoomListQuery::default()).0,
            audio_settings: None,
            device_watcher: None,
            audio_session: None,
            call: None,
            user_volumes: UserVolumes::default(),
            mic_muted: false,
            deafened: false,
            call_reconnects: 0,
        }
    }

    fn record_bytes(user: &str, homeserver: &str, session_id: &str) -> Vec<u8> {
        record(user, homeserver, session_id)
            .to_bytes()
            .unwrap()
            .to_vec()
    }

    fn record(user: &str, homeserver: &str, session_id: &str) -> SessionRecord {
        use matrix_sdk::authentication::matrix::MatrixSession;
        use matrix_sdk::ruma::owned_device_id;
        use matrix_sdk::{SessionMeta, SessionTokens};
        let user_id = OwnedUserId::try_from(user).unwrap();
        SessionRecord {
            version: RECORD_VERSION,
            user_id: user.to_owned(),
            homeserver_url: homeserver.to_owned(),
            session_id: session_id.to_owned(),
            store_passphrase: Zeroizing::new("p".to_owned()),
            auth: SessionAuth::Matrix(MatrixSession {
                meta: SessionMeta {
                    user_id,
                    device_id: owned_device_id!("DEV"),
                },
                tokens: SessionTokens {
                    access_token: "a".to_owned(),
                    refresh_token: None,
                },
            }),
        }
    }

    fn u(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    #[tokio::test]
    async fn existing_record_for_another_homeserver_is_refused_and_untouched() {
        let secrets = Arc::new(InMemorySecretStore::new());
        let bytes = record_bytes("@alice:matrix.org", "https://matrix.org/", "s1");
        secrets.set("@alice:matrix.org", &bytes).unwrap();
        let e = engine(secrets.clone());
        let err = e
            .existing_record_session("@alice:matrix.org", &u("https://evil.example"))
            .await
            .expect_err("must be refused");
        assert!(matches!(&err, CoreError::Login(m) if m == WRONG_ACCOUNT));
        let kept = secrets.get("@alice:matrix.org").unwrap().unwrap();
        assert_eq!(kept.as_slice(), bytes.as_slice());
    }

    #[tokio::test]
    async fn existing_record_for_same_homeserver_may_be_replaced() {
        let secrets = Arc::new(InMemorySecretStore::new());
        let bytes = record_bytes("@alice:matrix.org", "https://matrix.org/", "s1");
        secrets.set("@alice:matrix.org", &bytes).unwrap();
        let e = engine(secrets);
        let old = e
            .existing_record_session("@alice:matrix.org", &u("https://matrix.org:443/"))
            .await
            .unwrap();
        assert_eq!(old.as_deref(), Some("s1"));
        let none = e
            .existing_record_session("@bob:matrix.org", &u("https://matrix.org"))
            .await
            .unwrap();
        assert_eq!(none, None);
    }

    fn oauth_engine(dir: &std::path::Path) -> (Engine, mpsc::Receiver<CoreEvent>) {
        let (events, rx) = mpsc::channel(8);
        let engine = Engine {
            config: CoreConfig {
                data: DataPaths::at(dir),
                homeserver_override: None,
                room_list_page_size: DEFAULT_ROOM_LIST_PAGE_SIZE,
                audio: AudioBackend::default(),
                call_service_override: None,
            },
            secrets: Arc::new(InMemorySecretStore::new()),
            events,
            active: None,
            oauth: None,
            room_list_query: watch::channel(RoomListQuery::default()).0,
            audio_settings: None,
            device_watcher: None,
            audio_session: None,
            call: None,
            user_volumes: UserVolumes::default(),
            mic_muted: false,
            deafened: false,
            call_reconnects: 0,
        };
        (engine, rx)
    }

    fn pending(
        engine: &mut Engine,
        task: JoinHandle<Result<Client, CoreError>>,
        cancelled: bool,
    ) -> std::path::PathBuf {
        let dir = engine
            .config
            .data
            .create_session_store_dir("s1")
            .expect("create store dir");
        engine.oauth = Some(PendingOAuth {
            cancel: None,
            cancelled,
            task,
            server: "example.org".to_owned(),
            session_id: "s1".to_owned(),
            passphrase: Zeroizing::new("p".to_owned()),
        });
        dir
    }

    #[tokio::test]
    async fn a_cancel_after_success_discards_the_login() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        let task = tokio::spawn(async { std::future::pending().await });
        let dir = pending(&mut engine, task, true);
        engine.on_oauth_done(Ok(Ok(client))).await;
        assert!(matches!(
            rx.try_recv(),
            Ok(CoreEvent::LoginFailed(CoreError::Cancelled))
        ));
        assert!(engine.active.is_none());
        assert!(!dir.exists(), "store dir must be removed");
        assert!(!tmp.path().join("last_account").exists());
    }

    #[tokio::test]
    async fn shutdown_does_not_wait_for_a_stuck_oauth_task() {
        struct Flag(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, _rx) = oauth_engine(tmp.path());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Flag(dropped.clone());
        let task = tokio::spawn(async move {
            let _flag = flag;
            std::future::pending().await
        });
        let dir = pending(&mut engine, task, false);
        tokio::time::timeout(
            Duration::from_secs(5),
            engine.cancel_oauth_and_wait_within(Duration::from_millis(100)),
        )
        .await
        .expect("shutdown must not hang");
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!dir.exists(), "store dir must be removed");
    }

    #[tokio::test]
    async fn room_list_query_is_kept_for_the_list_and_paging_is_a_noop_without_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        let mut watcher = engine.room_list_query.subscribe();
        assert_eq!(*watcher.borrow_and_update(), RoomListQuery::default());
        let query = RoomListQuery {
            tab: RoomListTab::Spaces,
            search: "x".into(),
        };
        engine
            .handle(Command::SetRoomListQuery(query.clone()))
            .await;
        assert!(watcher.has_changed().unwrap());
        assert_eq!(*watcher.borrow_and_update(), query);
        // The same query again does not wake the list.
        engine
            .handle(Command::SetRoomListQuery(query.clone()))
            .await;
        assert!(!watcher.has_changed().unwrap());
        // A list that starts later sees the stored query.
        assert_eq!(*engine.room_list_query.subscribe().borrow(), query);
        engine.handle(Command::LoadMoreRooms).await;
        assert!(rx.try_recv().is_err(), "no event expected");
    }

    #[tokio::test]
    async fn fetch_avatars_answers_each_id_with_none() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        let ids = [AvatarId::from("mxc://x/a"), AvatarId::from("mxc://x/b")];
        engine.handle(Command::FetchAvatars(ids.to_vec())).await;
        for id in ids {
            match rx.try_recv() {
                Ok(CoreEvent::Avatar { id: got, image }) => {
                    assert_eq!(got, id);
                    assert!(image.is_none());
                }
                other => panic!("unexpected: {other:?}"),
            }
        }
        assert!(rx.try_recv().is_err());
    }

    use crate::api::{MessageKind, OutgoingMessage, TextFormat};

    fn rid(n: &str) -> RoomId {
        RoomId::from(format!("!{n}:x"))
    }

    /// One of every new command, each naming `room`, with its action kind.
    fn action_commands(room: &RoomId) -> Vec<(Command, ActionKind)> {
        let message = OutgoingMessage {
            room_id: room.clone(),
            text: MessageText::from("hi"),
            mentions: Vec::new(),
            reply_to: None,
            kind: MessageKind::Text,
            format: TextFormat::Markdown,
        };
        let item_id = || "i".to_owned();
        vec![
            (Command::Send(message.clone()), ActionKind::Send),
            (
                Command::Edit {
                    item_id: item_id(),
                    message,
                },
                ActionKind::Edit,
            ),
            (
                Command::Redact {
                    room_id: room.clone(),
                    item_id: item_id(),
                    reason: None,
                },
                ActionKind::Redact,
            ),
            (
                Command::ToggleReaction {
                    room_id: room.clone(),
                    item_id: item_id(),
                    key: "k".into(),
                },
                ActionKind::React,
            ),
            (
                Command::RetrySend {
                    room_id: room.clone(),
                    item_id: item_id(),
                },
                ActionKind::Retry,
            ),
            (
                Command::DiscardSend {
                    room_id: room.clone(),
                    item_id: item_id(),
                },
                ActionKind::Discard,
            ),
            (
                Command::PaginateBackwards {
                    room_id: room.clone(),
                },
                ActionKind::Paginate,
            ),
            (
                Command::MarkRead {
                    room_id: room.clone(),
                    fully_read: false,
                },
                ActionKind::Receipt,
            ),
            (
                Command::MoveReadMarker {
                    room_id: room.clone(),
                },
                ActionKind::Receipt,
            ),
            (
                Command::SetTyping {
                    room_id: room.clone(),
                    typing: true,
                },
                ActionKind::Typing,
            ),
            (
                Command::SearchMembers {
                    room_id: room.clone(),
                    query: "a".into(),
                },
                ActionKind::Members,
            ),
        ]
    }

    #[test]
    fn pagination_runs_at_most_one_at_a_time() {
        let flag = Arc::new(AtomicBool::new(false));
        let first = PaginationPermit::try_acquire(&flag).expect("free");
        assert!(PaginationPermit::try_acquire(&flag).is_none(), "busy");
        drop(first);
        let again = PaginationPermit::try_acquire(&flag);
        assert!(again.is_some(), "released on drop");
    }

    #[test]
    fn sdk_pagination_status_maps_to_the_api_state() {
        use matrix_sdk::event_cache::PaginationStatus as S;
        assert_eq!(
            pagination_state(S::Idle {
                hit_timeline_start: false
            }),
            PaginationState::Idle
        );
        assert_eq!(
            pagination_state(S::Idle {
                hit_timeline_start: true
            }),
            PaginationState::ReachedStart
        );
        assert_eq!(pagination_state(S::Paginating), PaginationState::Loading);
    }

    #[test]
    fn typing_users_take_names_from_members() {
        let alice: OwnedUserId = "@alice:x".try_into().unwrap();
        let bob: OwnedUserId = "@bob:x".try_into().unwrap();
        let names = HashMap::from([(alice.clone(), "Alice".to_owned())]);
        let users = typing_users(vec![alice, bob], &names);
        assert_eq!(
            users,
            vec![
                TypingUser {
                    user_id: UserId::from("@alice:x"),
                    name: Some("Alice".into())
                },
                TypingUser {
                    user_id: UserId::from("@bob:x"),
                    name: None
                },
            ]
        );
        assert!(typing_users(Vec::new(), &names).is_empty());
    }

    async fn drain(rx: &mut mpsc::Receiver<CoreEvent>) -> Vec<CoreEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// One of every Phase 4 command, with its action kind.
    fn security_commands() -> Vec<(Command, ActionKind)> {
        use crate::api::{RecoveryKey, VerificationId};
        let id = || VerificationId::from("flow");
        let dev = || DeviceId::from("DEV");
        let pw = || Some(Password::from("pw"));
        vec![
            (Command::EnableRecovery, ActionKind::Recovery),
            (
                Command::Recover {
                    key: RecoveryKey::from("k"),
                },
                ActionKind::Recovery,
            ),
            (Command::ChangeRecoveryKey, ActionKind::Recovery),
            (
                Command::BootstrapCrossSigning { password: pw() },
                ActionKind::CrossSigning,
            ),
            (
                Command::ResetIdentity { password: None },
                ActionKind::CrossSigning,
            ),
            (Command::CancelIdentityReset, ActionKind::CrossSigning),
            (Command::RequestSelfVerification, ActionKind::Verification),
            (
                Command::RequestDeviceVerification { device_id: dev() },
                ActionKind::Verification,
            ),
            (
                Command::RequestUserVerification {
                    user_id: UserId::from("@b:x"),
                },
                ActionKind::Verification,
            ),
            (
                Command::AcceptVerification { id: id() },
                ActionKind::Verification,
            ),
            (Command::StartSas { id: id() }, ActionKind::Verification),
            (
                Command::ConfirmQrScanned { id: id() },
                ActionKind::Verification,
            ),
            (Command::ConfirmSas { id: id() }, ActionKind::Verification),
            (Command::SasMismatch { id: id() }, ActionKind::Verification),
            (
                Command::CancelVerification { id: id() },
                ActionKind::Verification,
            ),
            (Command::ListDevices, ActionKind::Devices),
            (
                Command::RenameDevice {
                    device_id: dev(),
                    name: "n".into(),
                },
                ActionKind::Devices,
            ),
            (
                Command::SignOutDevices {
                    device_ids: vec![dev()],
                    password: pw(),
                },
                ActionKind::SignOutDevices,
            ),
            (
                Command::WatchUserTrust {
                    user_id: UserId::from("@b:x"),
                },
                ActionKind::Devices,
            ),
        ]
    }

    #[tokio::test]
    async fn security_commands_without_a_session_fail_with_not_logged_in() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        for (cmd, kind) in security_commands() {
            engine.handle(cmd).await;
            let got = drain(&mut rx).await;
            assert!(
                matches!(
                    got.as_slice(),
                    [CoreEvent::ActionFailed { action, error: CoreError::NotLoggedIn, .. }]
                        if *action == kind
                ),
                "{kind:?}: {got:?}"
            );
        }
    }

    #[tokio::test]
    async fn renaming_a_device_to_nothing_is_refused_before_any_request() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        engine.active = Some(Active {
            fetcher: Arc::new(SdkFetcher(client.clone())),
            client,
            record: record("@alice:x", "http://127.0.0.1:1", "s1"),
            changes: broadcast::channel(1).1,
            sync: None,
            selected: None,
            avatars: Avatars::new(),
            security: SecurityTasks::new(),
        });
        engine
            .handle(Command::RenameDevice {
                device_id: DeviceId::from("DEV"),
                name: "   ".into(),
            })
            .await;
        let got = next_event(&mut rx).await;
        assert!(
            matches!(
                &got,
                CoreEvent::ActionFailed {
                    action: ActionKind::Devices,
                    error: CoreError::NotAllowed,
                    ..
                }
            ),
            "{got:?}"
        );
        if let Some(mut active) = engine.active.take() {
            active.stop_tasks().await;
        }
    }

    #[tokio::test]
    async fn a_sign_out_that_cannot_start_fails_with_its_own_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        // This client never logged in: no user ID, so the sign-out stops at
        // once, and not as a list or rename failure.
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        engine.active = Some(Active {
            fetcher: Arc::new(SdkFetcher(client.clone())),
            client,
            record: record("@alice:x", "http://127.0.0.1:1", "s1"),
            changes: broadcast::channel(1).1,
            sync: None,
            selected: None,
            avatars: Avatars::new(),
            security: SecurityTasks::new(),
        });
        engine
            .handle(Command::SignOutDevices {
                device_ids: vec![DeviceId::from("DEV")],
                password: None,
            })
            .await;
        let got = next_event(&mut rx).await;
        assert!(
            matches!(
                &got,
                CoreEvent::ActionFailed {
                    action: ActionKind::SignOutDevices,
                    error: CoreError::NotLoggedIn,
                    ..
                }
            ),
            "{got:?}"
        );
        if let Some(mut active) = engine.active.take() {
            active.stop_tasks().await;
        }
    }

    #[tokio::test]
    async fn identity_commands_with_a_session_run_in_the_background() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        // This client never logged in, so the task fails at once with no
        // network (bootstrap needs the user ID first).
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        engine.active = Some(Active {
            fetcher: Arc::new(SdkFetcher(client.clone())),
            client,
            record: record("@alice:x", "http://127.0.0.1:1", "s1"),
            changes: broadcast::channel(1).1,
            sync: None,
            selected: None,
            avatars: Avatars::new(),
            security: SecurityTasks::new(),
        });
        engine
            .handle(Command::BootstrapCrossSigning { password: None })
            .await;
        // `handle` returned before the task ended; its answer arrives.
        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("an answer within 5 s")
            .expect("channel open");
        assert!(
            matches!(
                &event,
                CoreEvent::ActionFailed {
                    action: ActionKind::CrossSigning,
                    error: CoreError::NotLoggedIn,
                    ..
                }
            ),
            "{event:?}"
        );
        // Cancelling with nothing running is a no-op.
        engine.handle(Command::CancelIdentityReset).await;
        assert!(drain(&mut rx).await.is_empty());
        if let Some(mut active) = engine.active.take() {
            active.stop_tasks().await;
        }
    }

    #[tokio::test]
    async fn recovery_commands_with_a_session_run_in_the_background() {
        use crate::api::RecoveryKey;
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        // No server and no logged-in user: each task fails on its own, and
        // the answer names the recovery action and carries no secret.
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        engine.active = Some(Active {
            fetcher: Arc::new(SdkFetcher(client.clone())),
            client,
            record: record("@alice:x", "http://127.0.0.1:1", "s1"),
            changes: broadcast::channel(1).1,
            sync: None,
            selected: None,
            avatars: Avatars::new(),
            security: SecurityTasks::new(),
        });
        for cmd in [
            Command::EnableRecovery,
            Command::Recover {
                key: RecoveryKey::from("secret-key-x"),
            },
            Command::ChangeRecoveryKey,
        ] {
            engine.handle(cmd).await;
            let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("an answer within 10 s")
                .expect("channel open");
            match &event {
                CoreEvent::ActionFailed {
                    room_id,
                    action: ActionKind::Recovery,
                    error: CoreError::Internal(_),
                } => assert!(room_id.as_str().is_empty()),
                other => panic!("unexpected {other:?}"),
            }
            assert!(!format!("{event:?}").contains("secret-key-x"));
            assert!(drain(&mut rx).await.is_empty());
        }
        if let Some(mut active) = engine.active.take() {
            active.stop_tasks().await;
        }
    }

    #[tokio::test]
    async fn verification_commands_with_a_session_run_in_the_background() {
        use crate::api::VerificationId;
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        // No server and no logged-in user: a request fails on its own and
        // frees the slot, and an unknown flow is named as such.
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        let events = engine.events.clone();
        let mut security = SecurityTasks::new();
        security.start(&client, &events);
        engine.active = Some(Active {
            fetcher: Arc::new(SdkFetcher(client.clone())),
            client,
            record: record("@alice:x", "http://127.0.0.1:1", "s1"),
            changes: broadcast::channel(1).1,
            sync: None,
            selected: None,
            avatars: Avatars::new(),
            security,
        });
        // Twice: the failed first request must not leave a reservation behind
        // (that would answer `Busy`).
        for _ in 0..2 {
            engine.handle(Command::RequestSelfVerification).await;
            let event = next_event(&mut rx).await;
            assert!(
                matches!(
                    &event,
                    CoreEvent::ActionFailed {
                        action: ActionKind::Verification,
                        error: CoreError::NotLoggedIn,
                        ..
                    }
                ),
                "{event:?}"
            );
        }
        for cmd in [
            Command::AcceptVerification {
                id: VerificationId::from("none"),
            },
            Command::ConfirmSas {
                id: VerificationId::from("none"),
            },
            Command::SasMismatch {
                id: VerificationId::from("none"),
            },
            Command::CancelVerification {
                id: VerificationId::from("none"),
            },
        ] {
            engine.handle(cmd).await;
            let event = next_event(&mut rx).await;
            assert!(
                matches!(
                    &event,
                    CoreEvent::ActionFailed {
                        action: ActionKind::Verification,
                        error: CoreError::Internal(m),
                        ..
                    } if m == "there is no such verification"
                ),
                "{event:?}"
            );
        }
        let rest = drain(&mut rx).await;
        assert!(
            rest.iter().all(|e| matches!(e, CoreEvent::Security(_))),
            "{rest:?}"
        );
        if let Some(mut active) = engine.active.take() {
            active.stop_tasks().await;
        }
    }

    /// The next event that is not the status forwarder's.
    async fn next_event(rx: &mut mpsc::Receiver<CoreEvent>) -> CoreEvent {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("an answer within 10 s")
                .expect("channel open");
            if !matches!(event, CoreEvent::Security(_)) {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn new_commands_without_a_session_fail_with_not_logged_in() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        let room = rid("a");
        for (cmd, kind) in action_commands(&room) {
            engine.handle(cmd).await;
            let got = drain(&mut rx).await;
            assert!(
                matches!(
                    got.as_slice(),
                    [CoreEvent::ActionFailed { room_id, action, error: CoreError::NotLoggedIn }]
                        if *room_id == room && *action == kind
                ),
                "{kind:?}: {got:?}"
            );
        }
    }

    #[tokio::test]
    async fn new_commands_without_a_selected_room_fail_with_no_room_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut engine, mut rx) = oauth_engine(tmp.path());
        let client = Client::builder()
            .homeserver_url("http://127.0.0.1:1")
            .build()
            .await
            .unwrap();
        engine.active = Some(Active {
            fetcher: Arc::new(SdkFetcher(client.clone())),
            client,
            record: record("@alice:x", "http://127.0.0.1:1", "s1"),
            changes: broadcast::channel(1).1,
            sync: None,
            selected: None,
            avatars: Avatars::new(),
            security: SecurityTasks::new(),
        });
        let room = rid("a");
        for (cmd, kind) in action_commands(&room) {
            engine.handle(cmd).await;
            let got = drain(&mut rx).await;
            assert!(
                matches!(
                    got.as_slice(),
                    [CoreEvent::ActionFailed { room_id, action, error: CoreError::NoRoomSelected }]
                        if *room_id == room && *action == kind
                ),
                "{kind:?}: {got:?}"
            );
        }
    }
}
