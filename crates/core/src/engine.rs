//! The engine task: owns the SDK client and turns commands into events.
//!
//! Only plain API types cross the public boundary. SDK types stay in here.

use std::sync::Arc;

use futures_util::{Stream, StreamExt as _, pin_mut};
use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
use matrix_sdk::{Client, SessionChange};
use matrix_sdk_ui::eyeball_im::{Vector, VectorDiff};
use matrix_sdk_ui::room_list_service::filters::new_filter_non_left;
use matrix_sdk_ui::sync_service::SyncService;
use matrix_sdk_ui::timeline::{RoomExt as _, Timeline, TimelineItem as SdkTimelineItem};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::api::{
    Command, CoreError, CoreEvent, DeviceId, ListDiff, MessageText, Password, RoomId, UserId,
};
use crate::client::{
    ServerSpec, WRONG_ACCOUNT, build_client, discover_homeserver_url,
    ensure_homeserver_after_login, login_account_matches, map_login_error, same_origin,
    server_name_bound, server_name_of,
};
use crate::paths::DataPaths;
use crate::room_list::{map_diff, summarize};
use crate::secrets::{SecretError, SecretStore};
use crate::session::{
    RECORD_VERSION, SessionAuth, SessionRecord, generate_session_id, generate_store_passphrase,
};
use crate::sync::{SyncControl, supervise};
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
    tasks: Vec<JoinHandle<()>>,
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
}

impl Active {
    /// Stops the timeline forwarder, then the sync forwarders, then the sync
    /// service, so that nothing holds the client or its store afterwards (the
    /// tasks own clones of it).
    async fn stop_tasks(&mut self) {
        if let Some(selected) = self.selected.take() {
            selected.stop().await;
        }
        let Some(sync) = self.sync.take() else {
            return;
        };
        for task in &sync.tasks {
            task.abort();
        }
        for task in sync.tasks {
            // Aborted tasks end with a cancellation error; that is expected.
            let _ = task.await;
        }
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
async fn forward_sync_state(service: Arc<SyncService>, events: mpsc::Sender<CoreEvent>) {
    // Subscribe before reading the current value so that no change is lost.
    let subscriber = service.state();
    let initial = subscriber.get();
    supervise(&ServiceControl(service), initial, subscriber, &events).await;
}

/// Forwards every room list diff batch as one `RoomListChanged` event.
async fn forward_room_list(
    service: Arc<SyncService>,
    page_size: usize,
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
    let (stream, controller) = room_list.entries_with_dynamic_adapters(page_size.max(1));
    // The stream yields nothing until a filter is set.
    controller.set_filter(Box::new(new_filter_non_left()));
    pin_mut!(stream);
    while let Some(diffs) = stream.next().await {
        let batch = diffs
            .into_iter()
            .map(|d| map_diff(d, |item| summarize(&item)))
            .collect();
        if events
            .send(CoreEvent::RoomListChanged(batch))
            .await
            .is_err()
        {
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
) {
    let initial = ListDiff::Reset {
        values: items.iter().map(|i| map_item(i)).collect(),
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
            .map(|d| map_diff(d, |item| map_item(&item)))
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

struct Engine {
    config: CoreConfig,
    secrets: Arc<dyn SecretStore>,
    events: mpsc::Sender<CoreEvent>,
    active: Option<Active>,
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
                change = next_change(&mut self.active), if watch_changes => match change {
                    Change::Got(c) => self.on_change(Some(c)).await,
                    Change::Missed => self.on_change(None).await,
                    Change::Closed => watch_changes = false,
                },
            }
        }
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
                if self.active.is_some() {
                    let e = CoreError::Login("already logged in".into());
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
            Command::LoginOAuth { .. } => {
                let e = CoreError::Internal("OAuth login is not implemented yet".into());
                self.emit(CoreEvent::LoginFailed(e)).await;
            }
            Command::CancelOAuth => {}
            Command::SelectRoom(room_id) => {
                if let Err(e) = self.select_room(room_id).await {
                    self.emit(CoreEvent::Error(e)).await;
                }
            }
            Command::SendMessage { text } => self.send_message(&text).await,
            // Handled in `run`.
            Command::Shutdown => {}
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
        let bound = match returned.as_deref() {
            Some(r) if login_account_matches(username, r) => match server_name_of(r) {
                Some(name) => {
                    server_name_bound(typed_server, &homeserver, name, discover_homeserver_url)
                        .await
                }
                None => false,
            },
            _ => false,
        };
        if !bound {
            return Err(CoreError::Login(WRONG_ACCOUNT.into()));
        }

        match self.persist_new(&client, session_id, passphrase).await {
            Ok((record, event)) => {
                let changes = client.subscribe_to_session_changes();
                Ok((
                    Active {
                        client,
                        record,
                        changes,
                        sync: None,
                        selected: None,
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
        let Some(matrix_sdk::AuthSession::Matrix(session)) = client.session() else {
            return Err(internal());
        };
        let user_id = session.meta.user_id.to_string();
        let device_id = session.meta.device_id.to_string();
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
            auth: SessionAuth::Matrix(session),
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
        if self.active.is_some() {
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
        let SessionAuth::Matrix(session) = record.auth.clone() else {
            return Err(CoreError::Internal(
                "OAuth sessions are not supported yet".into(),
            ));
        };
        let client = build_client(
            &dir,
            &record.store_passphrase,
            ServerSpec::Url(&record.homeserver_url),
        )
        .await?;
        client
            .restore_session(session.clone())
            .await
            .map_err(|_| CoreError::Storage("could not restore the session".into()))?;
        info!("session restored");
        let event = CoreEvent::LoggedIn {
            user_id: UserId::from(session.meta.user_id.to_string()),
            device_id: DeviceId::from(session.meta.device_id.to_string()),
            restored: true,
        };
        let changes = client.subscribe_to_session_changes();
        Ok(Some((
            Active {
                client,
                record,
                changes,
                sync: None,
                selected: None,
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
        let tasks = vec![
            tokio::spawn(forward_sync_state(service.clone(), self.events.clone())),
            tokio::spawn(forward_room_list(
                service.clone(),
                self.config.room_list_page_size,
                self.events.clone(),
            )),
        ];
        service.start().await;
        if let Some(active) = self.active.as_mut() {
            active.sync = Some(Syncing { service, tasks });
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
        // OAuth sessions are persisted by Task 9.
        if let Some(matrix_sdk::AuthSession::Matrix(session)) = active.client.session() {
            active.record.auth = SessionAuth::Matrix(session);
        } else {
            return;
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
        }
    }

    fn record_bytes(user: &str, homeserver: &str, session_id: &str) -> Vec<u8> {
        use matrix_sdk::authentication::matrix::MatrixSession;
        use matrix_sdk::ruma::{OwnedUserId, owned_device_id};
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
        .to_bytes()
        .unwrap()
        .to_vec()
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
}
