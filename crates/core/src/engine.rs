//! The engine task: owns the SDK client and turns commands into events.
//!
//! Only plain API types cross the public boundary. SDK types stay in here.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt as _, pin_mut};
use matrix_sdk::ruma::OwnedUserId;
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::{Client, SessionChange};
use matrix_sdk_ui::eyeball_im::{Vector, VectorDiff};
use matrix_sdk_ui::sync_service::SyncService;
use matrix_sdk_ui::timeline::{RoomExt as _, Timeline, TimelineItem as SdkTimelineItem};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::api::{
    ActionKind, Command, CoreError, CoreEvent, DeviceId, ListDiff, MessageText, Password, RoomId,
    RoomListQuery, UserId,
};
use crate::avatars::{Avatars, SdkFetcher, answer_unavailable};
use crate::client::{
    ServerSpec, WRONG_ACCOUNT, account_bound, build_client, discover_homeserver_url,
    ensure_homeserver_after_login, map_login_error, same_origin,
};
use crate::oauth::authorize;
use crate::paths::DataPaths;
use crate::room_list::{Registered, build_filter, loaded_event, map_batch, map_diff};
use crate::secrets::{SecretError, SecretStore};
use crate::session::{
    RECORD_VERSION, SessionAuth, SessionRecord, generate_session_id, generate_store_passphrase,
};
use crate::sync::{CancelTrigger, SyncControl, cancel_pair, run_supervised};
use crate::timeline::{map_item, parse_room_id};

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
    /// Asks the supervisor to end between steps (it is never aborted: an
    /// aborted `SyncService::stop` could leave SDK tasks running).
    cancel: CancelTrigger,
    supervisor: JoinHandle<()>,
    room_list: JoinHandle<()>,
    /// Asks the room list forwarder to load one more page.
    more_rooms: mpsc::Sender<()>,
}

/// The room whose timeline is open, and the task forwarding its diffs.
struct Selected {
    room_id: RoomId,
    timeline: Timeline,
    task: JoinHandle<()>,
}

impl Selected {
    /// Ends the forwarder; the timeline is dropped with `self`.
    async fn stop(self) {
        self.task.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = self.task.await;
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
}

impl Active {
    /// Stops the timeline forwarder, then the sync forwarders, then the sync
    /// service, so that nothing holds the client or its store afterwards (the
    /// tasks own clones of it).
    async fn stop_tasks(&mut self) {
        // Fetch tasks hold clones of the client, so they go first.
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
        sync.cancel.cancel();
        let _ = sync.supervisor.await;
        sync.room_list.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = sync.room_list.await;
        sync.service.stop().await;
    }
}

/// Production [`SyncControl`]: restarts the real service.
struct ServiceControl(Arc<SyncService>);

impl SyncControl for ServiceControl {
    async fn restart(&self) {
        self.0.start().await;
    }

    async fn stop(&self) {
        self.0.stop().await;
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
/// failure (see [`supervise`]).
async fn forward_sync_state(
    service: Arc<SyncService>,
    events: mpsc::Sender<CoreEvent>,
    cancel: crate::sync::Cancel,
) {
    // Subscribe, then mark the current value as seen: the subscriber would
    // otherwise yield it again, which the supervisor reads as a missed
    // transition.
    let mut subscriber = service.state();
    let initial = subscriber.next_now();
    run_supervised(
        &ServiceControl(service),
        initial,
        subscriber,
        events,
        cancel,
    )
    .await;
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

/// Forwards the initial timeline items as one `Reset`, then every diff batch.
async fn forward_timeline(
    room_id: RoomId,
    items: Vector<Arc<SdkTimelineItem>>,
    stream: impl Stream<Item = Vec<VectorDiff<Arc<SdkTimelineItem>>>>,
    events: mpsc::Sender<CoreEvent>,
    own: Option<OwnedUserId>,
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

/// The room and action kind of a Phase 3 action command; `None` for any
/// other command.
fn command_action(cmd: &Command) -> Option<(RoomId, ActionKind)> {
    let (room_id, kind) = match cmd {
        Command::Send(m) => (&m.room_id, ActionKind::Send),
        Command::Edit { message, .. } => (&message.room_id, ActionKind::Edit),
        Command::Redact { room_id, .. } => (room_id, ActionKind::Redact),
        Command::ToggleReaction { room_id, .. } => (room_id, ActionKind::React),
        Command::RetrySend { room_id, .. } => (room_id, ActionKind::Retry),
        Command::DiscardSend { room_id, .. } => (room_id, ActionKind::Discard),
        Command::PaginateBackwards { room_id } => (room_id, ActionKind::Paginate),
        Command::MarkRead { room_id, .. } => (room_id, ActionKind::Receipt),
        Command::SetTyping { room_id, .. } => (room_id, ActionKind::Typing),
        Command::SearchMembers { room_id, .. } => (room_id, ActionKind::Members),
        _ => return None,
    };
    Some((room_id.clone(), kind))
}

/// Why an action on `room` cannot run: no session, the room is not the
/// selected one, or (for now) the handler does not exist yet.
fn action_error(logged_in: bool, selected: Option<&RoomId>, room: &RoomId) -> CoreError {
    if !logged_in {
        CoreError::NotLoggedIn
    } else if selected != Some(room) {
        CoreError::NoRoomSelected
    } else {
        CoreError::Internal("not implemented yet".into())
    }
}

/// The answer to an action command whose handler does not exist yet; `None`
/// for a command that is not an action. A member search with a matching room
/// finds nobody.
fn unfinished_action_event(
    logged_in: bool,
    selected: Option<&RoomId>,
    cmd: &Command,
) -> Option<CoreEvent> {
    let (room_id, action) = command_action(cmd)?;
    let error = action_error(logged_in, selected, &room_id);
    if let (Command::SearchMembers { query, .. }, CoreError::Internal(_)) = (cmd, &error) {
        return Some(CoreEvent::Members {
            room_id,
            query: query.clone(),
            members: Vec::new(),
        });
    }
    Some(CoreEvent::ActionFailed {
        room_id,
        action,
        error,
    })
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
                change = next_change(&mut self.active), if watch_changes => match change {
                    Change::Got(c) => self.on_change(Some(c)).await,
                    Change::Missed => self.on_change(None).await,
                    Change::Closed => watch_changes = false,
                },
            }
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
                        self.start_sync().await;
                    }
                    Err(e) => self.emit(CoreEvent::LoginFailed(e)).await,
                }
            }
            Command::Logout => self.logout().await,
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
            // The real handlers land in later tasks; until then each answers
            // without touching the network.
            Command::Send(_)
            | Command::Edit { .. }
            | Command::Redact { .. }
            | Command::ToggleReaction { .. }
            | Command::RetrySend { .. }
            | Command::DiscardSend { .. }
            | Command::PaginateBackwards { .. }
            | Command::MarkRead { .. }
            | Command::SetTyping { .. }
            | Command::SearchMembers { .. } => {
                let selected = self
                    .active
                    .as_ref()
                    .and_then(|a| a.selected.as_ref())
                    .map(|s| &s.room_id);
                if let Some(event) = unfinished_action_event(self.active.is_some(), selected, &cmd)
                {
                    self.emit(event).await;
                }
            }
            // Handled in `run`.
            Command::Shutdown => {}
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
        let (items, stream) = timeline.subscribe().await;
        let task = tokio::spawn(forward_timeline(
            room_id.clone(),
            items,
            stream,
            self.events.clone(),
            active.client.user_id().map(ToOwned::to_owned),
        ));
        active.selected = Some(Selected {
            room_id,
            timeline,
            task,
        });
        Ok(())
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
        let supervisor = tokio::spawn(forward_sync_state(
            service.clone(),
            self.events.clone(),
            cancelled,
        ));
        let (more_rooms, more_rx) = mpsc::channel(8);
        let room_list = tokio::spawn(forward_room_list(
            service.clone(),
            self.config.room_list_page_size,
            self.room_list_query.subscribe(),
            more_rx,
            self.events.clone(),
        ));
        service.start().await;
        if let Some(active) = self.active.as_mut() {
            active.sync = Some(Syncing {
                service,
                cancel,
                supervisor,
                room_list,
                more_rooms,
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
            },
            secrets,
            events,
            active: None,
            oauth: None,
            room_list_query: watch::channel(RoomListQuery::default()).0,
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
            },
            secrets: Arc::new(InMemorySecretStore::new()),
            events,
            active: None,
            oauth: None,
            room_list_query: watch::channel(RoomListQuery::default()).0,
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

    use crate::api::{MessageKind, OutgoingMessage};

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
    fn every_new_command_names_its_room_and_action() {
        let room = rid("a");
        for (cmd, kind) in action_commands(&room) {
            assert_eq!(command_action(&cmd), Some((room.clone(), kind)), "{cmd:?}");
        }
        assert_eq!(command_action(&Command::LoadMoreRooms), None);
        assert_eq!(command_action(&Command::Logout), None);
    }

    #[test]
    fn unfinished_action_errors_follow_session_and_selection() {
        let a = rid("a");
        let b = rid("b");
        assert_eq!(
            action_error(false, Some(&a), &a),
            CoreError::NotLoggedIn,
            "no session wins"
        );
        assert_eq!(action_error(true, None, &a), CoreError::NoRoomSelected);
        assert_eq!(action_error(true, Some(&b), &a), CoreError::NoRoomSelected);
        assert_eq!(
            action_error(true, Some(&a), &a),
            CoreError::Internal("not implemented yet".into())
        );
    }

    #[test]
    fn unfinished_actions_with_a_matching_room_answer_not_implemented() {
        let room = rid("a");
        for (cmd, kind) in action_commands(&room) {
            let ev = unfinished_action_event(true, Some(&room), &cmd);
            match (kind, ev) {
                (
                    ActionKind::Members,
                    Some(CoreEvent::Members {
                        room_id,
                        query,
                        members,
                    }),
                ) => {
                    assert_eq!(room_id, room);
                    assert_eq!(query, "a");
                    assert!(members.is_empty());
                }
                (
                    kind,
                    Some(CoreEvent::ActionFailed {
                        room_id,
                        action,
                        error,
                    }),
                ) => {
                    assert_eq!(room_id, room);
                    assert_eq!(action, kind);
                    assert_eq!(error, CoreError::Internal("not implemented yet".into()));
                }
                (kind, other) => panic!("{kind:?}: {other:?}"),
            }
        }
        assert!(unfinished_action_event(true, Some(&room), &Command::Logout).is_none());
    }

    async fn drain(rx: &mut mpsc::Receiver<CoreEvent>) -> Vec<CoreEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
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
