//! MatrixRTC calls (Phase 6): signalling here, media in `media`.
//!
//! Joining follows Element Call's compatibility mode (see `membership`):
//! 1. find our homeserver's LiveKit service (MSC4143 transport discovery);
//! 2. schedule a delayed empty membership (MSC4140), the crash-safe leave;
//! 3. publish our membership;
//! 4. get a LiveKit token for that service (`jwt`);
//! 5. connect to the SFU, publish the microphone and play the others
//!    (`media`);
//! 6. keep the delayed leave from firing (restart every 4 s), refresh the
//!    membership before it expires, and report the participants;
//! 7. multi-SFU: subscribe on every other member's SFU (their own call
//!    service, e.g. another homeserver's), opened and closed as members come
//!    and go;
//! 8. in an encrypted room, encrypt our media with our own key and send it
//!    to the other members, Olm-encrypted (`e2ee`).
//!
//! Leaving fires the delayed leave at once (or sends the empty membership).

mod cleaning;
mod e2ee;
mod jwt;
mod media;
pub(crate) mod membership;
pub(crate) mod ringing;
pub(crate) mod watcher;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use livekit::e2ee::key_provider::KeyProvider;
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::id::ParticipantIdentity;
use matrix_sdk::deserialized_responses::{EncryptionInfo, RawAnySyncOrStrippedState};
use matrix_sdk::event_handler::EventHandlerHandle;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::UpdateAction;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::unstable_v2 as update_delayed;
use matrix_sdk::ruma::api::client::delayed_events::{DelayParameters, delayed_state_event};
use matrix_sdk::ruma::api::client::rtc::RtcTransport;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::AnyToDeviceEvent;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room, RoomState};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};

use self::cleaning::Cleaning;
pub(crate) use self::cleaning::announcement;
use self::e2ee::{KeyPlanner, Recipient};
pub(crate) use self::media::MediaNotice;
use self::media::{ConnectionShared, Publish, SfuConnection, SharedFlags};
use self::membership::{
    CALL_MEMBER_EVENT, DELAYED_LEAVE, DELAYED_LEAVE_RESTART, MEMBERSHIP_EXPIRES, Membership,
};
use crate::api::{
    AudioSettings, CoreError, CoreEvent, DeviceId, ListDiff, Participant, ParticipantId,
    RemoteCleaning, RoomId, UserId, VoiceCleaning,
};

/// How often participants are re-read even without a state change, so that
/// expired memberships disappear.
const PARTICIPANT_RECHECK: Duration = Duration::from_secs(5);
/// Refresh our membership this long before it would expire.
const REFRESH_MARGIN: Duration = Duration::from_secs(10 * 60);
/// Retry delays for another member's SFU: doubling from the first, capped.
const REMOTE_RETRY_FIRST: Duration = Duration::from_secs(2);
const REMOTE_RETRY_MAX: Duration = Duration::from_secs(60);

/// Call service URL → LiveKit identities expected on its SFU.
type Expected = BTreeMap<String, BTreeSet<String>>;
/// Connected SFUs of other members, by call service URL.
type Remotes = Arc<StdMutex<HashMap<String, SfuConnection>>>;

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Per-user playback volumes, shared with the engine so participants carry
/// the current value.
pub(crate) type UserVolumes = Arc<RwLock<BTreeMap<UserId, u16>>>;

/// The local user's call.
pub(crate) struct CallSession {
    pub room_id: RoomId,
    room: Room,
    state_key: String,
    delay_id: Arc<Mutex<Option<String>>>,
    /// Our SFU, where we publish.
    own: Option<SfuConnection>,
    remotes: Remotes,
    flags: SharedFlags,
    cleaning: Cleaning,
    /// What our SFU connection announces about our microphone's cleaning.
    announce: watch::Sender<Option<&'static str>>,
    own_identity: String,
    /// Media is end-to-end encrypted (the room is).
    pub encrypted: bool,
    /// Receives the other members' call keys, in an encrypted room.
    key_handler: Option<EventHandlerHandle>,
    /// The notification we sent as the first member, to tell its declines.
    pub notification: Option<String>,
    /// SFU connection notices for the engine.
    pub notices: mpsc::Receiver<MediaNotice>,
    tasks: Vec<JoinHandle<()>>,
}

/// The audio side handed to a call.
pub(crate) struct CallAudio {
    pub voice: Option<crate::audio_session::Voice>,
    pub outgoing: Option<tessera_audio::Outgoing>,
    pub bitrate_kbps: u16,
    /// Cleaning other people's voices, and the per-user choices.
    pub cleaning: (RemoteCleaning, BTreeMap<UserId, VoiceCleaning>),
    /// What we announce about our microphone's cleaning ([`announcement`]).
    pub announce: Option<&'static str>,
}

/// Schedules the empty membership that fires if we stop restarting it.
/// `Ok(None)` when the server has no delayed events (MSC4140): the call
/// still works, but a crash leaves a stale membership until it expires.
async fn schedule_delayed_leave(
    client: &Client,
    room: &Room,
    state_key: &str,
) -> Result<Option<String>, CoreError> {
    let empty: Raw<_> = Raw::new(&json!({}))
        .map_err(|_| CoreError::Internal("empty membership".into()))?
        .cast_unchecked();
    let request = delayed_state_event::unstable::Request::new_raw(
        room.room_id().to_owned(),
        state_key.to_owned(),
        StateEventType::from(CALL_MEMBER_EVENT),
        DelayParameters::Timeout {
            timeout: DELAYED_LEAVE,
        },
        empty,
    );
    match client.send(request).await {
        Ok(reply) => Ok(Some(reply.delay_id)),
        Err(e)
            if matches!(
                e.client_api_error_kind(),
                Some(ErrorKind::Unrecognized | ErrorKind::NotFound)
            ) =>
        {
            tracing::warn!("homeserver has no delayed events; crash-safe leave disabled");
            Ok(None)
        }
        Err(_) => Err(CoreError::Call("could not schedule the call leave".into())),
    }
}

/// Sends our membership; returns its event ID.
async fn send_membership(
    room: &Room,
    state_key: &str,
    content: Value,
) -> Result<String, CoreError> {
    room.send_state_event_raw(CALL_MEMBER_EVENT, state_key, content)
        .await
        .map(|r| r.event_id.to_string())
        .map_err(|e| match e.client_api_error_kind() {
            // The room's power levels reserve call memberships (Element
            // creates rooms with `org.matrix.msc3401.call.member` at 0).
            Some(ErrorKind::Forbidden) => {
                CoreError::Call("you are not allowed to join calls in this room".into())
            }
            _ => CoreError::Call("could not publish the call membership".into()),
        })
}

/// Rings the room: `ring` in a DM, a quiet `notification` elsewhere (as
/// Element Call). Returns the notification's event ID; a failure only
/// means nobody is rung.
async fn send_notification(room: &Room, membership_event: &str) -> Option<String> {
    let ring = room.is_direct().await.unwrap_or(false);
    let content = ringing::notification_content(membership_event, ring, now_ms());
    match room.send_raw(ringing::NOTIFICATION_EVENT, content).await {
        Ok(sent) => Some(sent.response.event_id.to_string()),
        Err(_) => {
            tracing::warn!("could not send the call notification");
            None
        }
    }
}

/// Our LiveKit service URL: `service_override`, or our homeserver's
/// (MSC4143 discovery, then `.well-known` `rtc_foci`, as the SDK does).
async fn own_livekit_service(
    client: &Client,
    service_override: Option<&str>,
) -> Result<String, CoreError> {
    if let Some(url) = service_override {
        return Ok(url.to_owned());
    }
    let transports = client
        .discover_rtc_transports()
        .await
        .map_err(|_| CoreError::Call("could not discover the call service".into()))?
        .unwrap_or_default();
    transports
        .into_iter()
        .find_map(|t| match t {
            RtcTransport::LiveKit(lk) => Some(lk.service_url),
            _ => None,
        })
        .ok_or_else(|| CoreError::Call("this homeserver offers no call service".into()))
}

/// Every membership in the room's state, active or not.
async fn read_memberships(room: &Room) -> Vec<Membership> {
    let Ok(events) = room
        .get_state_events(StateEventType::from(CALL_MEMBER_EVENT))
        .await
    else {
        return Vec::new();
    };
    events
        .into_iter()
        .filter_map(|raw| match raw {
            RawAnySyncOrStrippedState::Sync(raw) => raw.deserialize_as::<Value>().ok(),
            RawAnySyncOrStrippedState::Stripped(_) => None,
        })
        .filter_map(|event| {
            let sender = event.get("sender")?.as_str()?;
            let state_key = event.get("state_key")?.as_str()?;
            let ts = event.get("origin_server_ts")?.as_u64()?;
            membership::parse(sender, state_key, ts, event.get("content")?)
        })
        .collect()
}

/// Active memberships, oldest first.
pub(crate) async fn active_memberships(room: &Room) -> Vec<Membership> {
    let now = now_ms();
    let mut active: Vec<Membership> = read_memberships(room)
        .await
        .into_iter()
        .filter(|m| m.is_active(now))
        .collect();
    active.sort_by(|a, b| (a.joined_ms, &a.member_id).cmp(&(b.joined_ms, &b.member_id)));
    active
}

/// Active memberships (oldest first) as API participants.
async fn participants(
    room: &Room,
    active: &[Membership],
    volumes: &UserVolumes,
    flags: &SharedFlags,
) -> Vec<Participant> {
    let own_user = room.own_user_id().as_str().to_owned();
    let own_device = room
        .client()
        .device_id()
        .map(|d| d.as_str().to_owned())
        .unwrap_or_default();
    let mut out = Vec::with_capacity(active.len());
    for m in active {
        let display_name = match matrix_sdk::ruma::UserId::parse(&m.user_id) {
            Ok(uid) => room
                .get_member_no_sync(&uid)
                .await
                .ok()
                .flatten()
                .and_then(|member| member.display_name().map(str::to_owned)),
            Err(_) => None,
        };
        let user_id = UserId::from(m.user_id.as_str());
        let volume_percent = volumes
            .read()
            .map(|v| v.get(&user_id).copied().unwrap_or(100))
            .unwrap_or(100);
        let (speaking, muted, cleaning) = flags
            .flags
            .read()
            .map(|f| {
                (
                    f.is_speaking(&m.backend_identity),
                    f.muted.contains(&m.backend_identity),
                    f.cleaning
                        .get(&m.backend_identity)
                        .copied()
                        .unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        out.push(Participant {
            id: ParticipantId::from(m.member_id.as_str()),
            is_local: m.user_id == own_user && m.device_id == own_device,
            user_id,
            device_id: DeviceId::from(m.device_id.as_str()),
            display_name,
            muted,
            speaking,
            volume_percent,
            cleaning,
        });
    }
    out
}

impl CallSession {
    /// Joins the room's call, publishing on `service_override` or our
    /// homeserver's call service. On failure nothing stays published.
    pub(crate) async fn join(
        client: &Client,
        room_id: RoomId,
        audio: CallAudio,
        events: mpsc::Sender<CoreEvent>,
        volumes: UserVolumes,
        service_override: Option<&str>,
    ) -> Result<Self, CoreError> {
        let sdk_id = crate::timeline::parse_room_id(&room_id)?;
        let room = client
            .get_room(&sdk_id)
            .filter(|r| r.state() == RoomState::Joined)
            .ok_or_else(|| CoreError::UnknownRoom(room_id.clone()))?;
        let user_id = client.user_id().ok_or(CoreError::NotLoggedIn)?.to_string();
        let device_id = client
            .device_id()
            .ok_or(CoreError::NotLoggedIn)?
            .to_string();
        let service_url = own_livekit_service(client, service_override).await?;
        let encrypted = room
            .latest_encryption_state()
            .await
            .map_err(|_| CoreError::Call("could not check whether the room is encrypted".into()))?
            .is_encrypted();
        let version = room
            .version()
            .map_or_else(|| "1".to_owned(), |v| v.as_str().to_owned());
        let state_key = membership::state_key(&user_id, &device_id, &version);

        let delay_id = schedule_delayed_leave(client, &room, &state_key).await?;
        let joined_ms = now_ms();
        let content = membership::join_content(
            &user_id,
            &device_id,
            &service_url,
            room_id.as_str(),
            None,
            MEMBERSHIP_EXPIRES,
        );
        let delay_id = Arc::new(Mutex::new(delay_id));
        let flags = SharedFlags::default();
        let (cleaning_settings, cleaning_choices) = audio.cleaning.clone();
        let cleaning = Cleaning::new(
            audio.voice.clone(),
            cleaning_settings,
            cleaning_choices,
            flags.clone(),
        );
        let (announce, announce_rx) = watch::channel(audio.announce);
        let (to_engine, notices) = mpsc::channel(8);
        let (notices_all, from_connections) = mpsc::channel(16);
        let own_identity = membership::member_id(&user_id, &device_id);
        let remotes: Remotes = Arc::default();
        let mut session = Self {
            room_id: room_id.clone(),
            room: room.clone(),
            state_key: state_key.clone(),
            delay_id: delay_id.clone(),
            own: None,
            remotes: remotes.clone(),
            flags: flags.clone(),
            cleaning: cleaning.clone(),
            announce,
            own_identity: own_identity.clone(),
            encrypted,
            key_handler: None,
            notification: None,
            notices,
            tasks: Vec::new(),
        };
        // Kept from now on: joining (the SFU above all) can take longer than
        // the delayed leave, and the server would send our leave meanwhile.
        let own_gone = Arc::new(Notify::new());
        session.tasks.push(tokio::spawn(keep_membership(
            OwnMembership {
                client: client.clone(),
                room: room.clone(),
                state_key: state_key.clone(),
                user_id: user_id.clone(),
                device_id: device_id.clone(),
                service_url: service_url.clone(),
                delay_id,
                gone: own_gone.clone(),
            },
            joined_ms,
        )));
        // In an encrypted room: our first key, used at once, and the others'
        // keys as they arrive (registered before anyone can see us join).
        let keys = encrypted.then(|| KeyProvider::new(e2ee::key_provider_options()));
        let planner = keys.as_ref().map(|provider| {
            let first = e2ee::random_key();
            provider.set_key(
                &ParticipantIdentity(own_identity.clone()),
                0,
                first.to_vec(),
            );
            KeyPlanner::new(first, Instant::now())
        });
        if let Some(provider) = &keys {
            let provider = provider.clone();
            let room_id = room_id.as_str().to_owned();
            session.key_handler = Some(client.add_event_handler(
                move |event: Raw<AnyToDeviceEvent>, info: Option<EncryptionInfo>| {
                    let provider = provider.clone();
                    let room_id = room_id.clone();
                    async move { receive_key(&event, info.as_ref(), &room_id, &provider) }
                },
            ));
        }
        // Whether anyone else is in the call before we join: the first member
        // rings the others (js-sdk `sendCallNotify`).
        let first = active_memberships(&room)
            .await
            .iter()
            .all(|m| m.backend_identity == own_identity);
        let membership_event = match send_membership(&room, &state_key, content).await {
            Ok(id) => id,
            Err(e) => {
                session.leave().await;
                return Err(e);
            }
        };
        if first {
            session.notification = send_notification(&room, &membership_event).await;
        }
        let shared = ConnectionShared {
            encryption: keys.as_ref().map(|provider| E2eeOptions {
                encryption_type: EncryptionType::Gcm,
                key_provider: provider.clone(),
            }),
            voice: audio.voice,
            volumes: volumes.clone(),
            flags: flags.clone(),
            cleaning: cleaning.clone(),
            notices: notices_all,
        };
        let (own_expected, own_expected_rx) = watch::channel(BTreeSet::new());
        let (key_index, key_index_rx) = watch::channel(0);
        let own = match jwt::sfu_access(client, &service_url, room_id.as_str()).await {
            Ok(sfu) => {
                SfuConnection::connect(
                    service_url.clone(),
                    &sfu,
                    Some(Publish {
                        outgoing: audio.outgoing,
                        bitrate_kbps: audio.bitrate_kbps,
                        key_index: encrypted.then_some(key_index_rx),
                        announce: announce_rx,
                    }),
                    own_expected_rx,
                    &shared,
                )
                .await
            }
            Err(e) => Err(e),
        };
        match own {
            Ok(own) => session.own = Some(own),
            Err(e) => {
                session.leave().await;
                return Err(e);
            }
        }
        let (expected_tx, expected_rx) = watch::channel(Expected::new());
        let (recipients, recipients_rx) = watch::channel(Vec::new());
        if let (Some(provider), Some(planner)) = (keys, planner) {
            session.tasks.push(tokio::spawn(distribute_keys(
                client.clone(),
                room_id.as_str().to_owned(),
                own_identity.clone(),
                planner,
                provider,
                key_index,
                recipients_rx,
            )));
        }

        session.tasks.push(tokio::spawn(watch_participants(
            room,
            room_id.clone(),
            events,
            volumes,
            flags,
            Ours {
                identity: own_identity,
                service: service_url.clone(),
                expected: expected_tx,
                recipients,
                gone: own_gone,
            },
        )));
        session.tasks.push(tokio::spawn(manage_sfus(
            SfuManager {
                client: client.clone(),
                room_id: room_id.as_str().to_owned(),
                own_service: service_url,
                shared,
                remotes,
            },
            own_expected,
            expected_rx,
            from_connections,
            to_engine,
        )));
        Ok(session)
    }

    /// Mutes or unmutes our published microphone; others see the change.
    pub(crate) fn set_muted(&self, muted: bool) {
        if let Some(own) = &self.own {
            own.set_muted(muted);
        }
        if let Ok(mut f) = self.flags.flags.write() {
            if muted {
                f.muted.insert(self.own_identity.clone());
            } else {
                f.muted.remove(&self.own_identity);
            }
        }
        self.flags.changed.notify_one();
    }

    /// Applies a user's new playback volume (and re-reports participants).
    pub(crate) fn set_user_volume(&self, user: &UserId, percent: u16) {
        if let Some(own) = &self.own {
            own.set_user_volume(user, percent);
        }
        for remote in self.remotes.lock().expect("remotes lock").values() {
            remote.set_user_volume(user, percent);
        }
        self.flags.changed.notify_one();
    }

    /// Applies new settings for cleaning: other people's voices, and what
    /// we announce about ours.
    pub(crate) fn set_cleaning(&self, settings: &AudioSettings) {
        self.cleaning
            .set_settings(settings.remote_cleaning, settings.user_cleaning.clone());
        self.announce.send_if_modified(|announced| {
            let now = announcement(settings);
            let changed = *announced != now;
            *announced = now;
            changed
        });
    }

    /// Leaves: stops the tasks and removes our membership.
    pub(crate) async fn leave(mut self) {
        if let Some(handle) = self.key_handler.take() {
            self.room.client().remove_event_handler(handle);
        }
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
        }
        if let Some(own) = self.own.take() {
            own.close().await;
        }
        let remotes: Vec<SfuConnection> = self
            .remotes
            .lock()
            .expect("remotes lock")
            .drain()
            .map(|(_, c)| c)
            .collect();
        for remote in remotes {
            remote.close().await;
        }
        let delay_id = self.delay_id.lock().await.take();
        let client = self.room.client();
        let fired = match delay_id {
            Some(id) => client
                .send(update_delayed::Request::new(id, UpdateAction::Send))
                .await
                .is_ok(),
            None => false,
        };
        if !fired
            && send_membership(&self.room, &self.state_key, json!({}))
                .await
                .is_err()
        {
            tracing::warn!("could not remove the call membership; it expires by itself");
        }
    }
}

/// What [`keep_membership`] needs.
struct OwnMembership {
    client: Client,
    room: Room,
    state_key: String,
    user_id: String,
    device_id: String,
    service_url: String,
    delay_id: Arc<Mutex<Option<String>>>,
    /// Woken by `watch_participants` when our membership is gone from the
    /// room state.
    gone: Arc<Notify>,
}

impl OwnMembership {
    /// Restarts the delayed leave so it never fires while we are alive. If
    /// the server lost it (restart, cancelled) or already sent it, schedules
    /// a new one.
    async fn restart_delayed_leave(&self) {
        let mut guard = self.delay_id.lock().await;
        let Some(id) = guard.clone() else {
            // The homeserver has no delayed events.
            return;
        };
        match self
            .client
            .send(update_delayed::Request::new(id, UpdateAction::Restart))
            .await
        {
            Ok(_) => {}
            Err(e) if matches!(e.client_api_error_kind(), Some(ErrorKind::NotFound)) => {
                if let Ok(Some(new)) =
                    schedule_delayed_leave(&self.client, &self.room, &self.state_key).await
                {
                    *guard = Some(new);
                }
            }
            // Network trouble: try again next tick; the server sends the
            // leave if we stay away longer than the delay.
            Err(_) => {}
        }
    }

    /// Sends our membership, valid for `validity` from `created_ts` (`None`:
    /// from now, a fresh membership).
    async fn publish(&self, created_ts: Option<u64>, validity: Duration) -> Result<(), CoreError> {
        let content = membership::join_content(
            &self.user_id,
            &self.device_id,
            &self.service_url,
            self.room.room_id().as_str(),
            created_ts,
            validity,
        );
        send_membership(&self.room, &self.state_key, content)
            .await
            .map(|_| ())
    }

    /// Joins again from scratch, as at the start (js-sdk
    /// `MembershipManager`, "missing own membership"): a new delayed leave,
    /// the old one cancelled first (left alone, it would fire and remove the
    /// new membership), then a fresh membership.
    async fn rejoin(&self) -> Result<(), CoreError> {
        let mut guard = self.delay_id.lock().await;
        if let Some(old) = guard.clone() {
            match self
                .client
                .send(update_delayed::Request::new(old, UpdateAction::Cancel))
                .await
            {
                Ok(_) => {}
                Err(e) if matches!(e.client_api_error_kind(), Some(ErrorKind::NotFound)) => {}
                Err(_) => return Err(CoreError::Call("could not cancel the call leave".into())),
            }
            *guard = None;
        }
        *guard = schedule_delayed_leave(&self.client, &self.room, &self.state_key).await?;
        drop(guard);
        self.publish(None, MEMBERSHIP_EXPIRES).await
    }
}

/// Keeps our membership in the room state: restarts the delayed leave,
/// re-publishes the membership with a longer validity before it expires,
/// and joins again from scratch when it is gone (the server sent our leave
/// during a stall longer than the delay, or it expired).
async fn keep_membership(own: OwnMembership, joined_ms: u64) {
    let refresh_after =
        |validity: Duration| tokio::time::Instant::now() + validity.saturating_sub(REFRESH_MARGIN);
    let mut ticker = tokio::time::interval(DELAYED_LEAVE_RESTART);
    ticker.tick().await;
    let mut created_ts = joined_ms;
    let mut validity = MEMBERSHIP_EXPIRES;
    let mut refresh_at = refresh_after(validity);
    let mut rejoin = false;
    loop {
        tokio::select! {
            _ = ticker.tick() => own.restart_delayed_leave().await,
            () = tokio::time::sleep_until(refresh_at) => {
                validity += MEMBERSHIP_EXPIRES;
                // Counted from the first join, so the next refresh comes
                // a whole validity later.
                refresh_at = refresh_after(MEMBERSHIP_EXPIRES);
                if own.publish(Some(created_ts), validity).await.is_err() {
                    tracing::warn!("could not refresh the call membership");
                }
            }
            () = own.gone.notified() => rejoin = true,
        }
        if rejoin {
            let started = now_ms();
            match own.rejoin().await {
                Ok(()) => {
                    rejoin = false;
                    created_ts = started;
                    validity = MEMBERSHIP_EXPIRES;
                    refresh_at = refresh_after(validity);
                }
                // Tried again on the next tick.
                Err(_) => tracing::warn!("could not join the call again; retrying"),
            }
        }
    }
}

/// Our side, for the participant watcher.
struct Ours {
    identity: String,
    service: String,
    /// Who to play on which SFU, for `manage_sfus`.
    expected: watch::Sender<Expected>,
    /// The other members' devices, for our key (`distribute_keys`).
    recipients: watch::Sender<Vec<Recipient>>,
    /// Told when our membership, once seen, is gone (`keep_membership`).
    gone: Arc<Notify>,
}

/// Emits `CallParticipants` (a full reset) whenever the active set changes,
/// and tells the SFU manager who publishes where.
async fn watch_participants(
    room: Room,
    room_id: RoomId,
    events: mpsc::Sender<CoreEvent>,
    volumes: UserVolumes,
    flags: SharedFlags,
    ours: Ours,
) {
    let mut info = room.subscribe_info();
    let mut ticker = tokio::time::interval(PARTICIPANT_RECHECK);
    let mut last: Option<Vec<Participant>> = None;
    // Our membership reaches the store some time after we send it: only its
    // disappearance after that counts.
    let mut seen_own = false;
    loop {
        let active = active_memberships(&room).await;
        if active.iter().any(|m| m.backend_identity == ours.identity) {
            seen_own = true;
        } else if seen_own {
            seen_own = false;
            tracing::warn!("our call membership is gone; joining again");
            ours.gone.notify_one();
        }
        let expected = membership::expected_by_sfu(&active, &ours.identity, &ours.service);
        ours.expected.send_if_modified(|e| {
            let changed = *e != expected;
            *e = expected;
            changed
        });
        let recipients: Vec<Recipient> = active
            .iter()
            .filter(|m| m.backend_identity != ours.identity)
            .map(|m| Recipient {
                user_id: m.user_id.clone(),
                device_id: m.device_id.clone(),
                since: m.joined_ms,
            })
            .collect();
        ours.recipients.send_if_modified(|r| {
            let changed = *r != recipients;
            *r = recipients;
            changed
        });
        let now = participants(&room, &active, &volumes, &flags).await;
        if last.as_ref() != Some(&now) {
            let event = CoreEvent::CallParticipants {
                room_id: room_id.clone(),
                diffs: vec![ListDiff::Reset {
                    values: now.clone(),
                }],
            };
            if events.send(event).await.is_err() {
                return;
            }
            last = Some(now);
        }
        tokio::select! {
            changed = info.next() => if changed.is_none() { return },
            () = flags.changed.notified() => {}
            _ = ticker.tick() => {}
        }
    }
}

/// What the SFU manager needs.
struct SfuManager {
    client: Client,
    room_id: String,
    own_service: String,
    shared: ConnectionShared,
    remotes: Remotes,
}

/// One other member's SFU we want.
struct Remote {
    /// Who to play there.
    expected: watch::Sender<BTreeSet<String>>,
    /// A connection attempt in flight.
    connecting: Option<AbortHandle>,
    /// After a failure: when to try again, and how many attempts failed.
    retry: Option<(Instant, u32)>,
}

fn retry_delay(failures: u32) -> Duration {
    REMOTE_RETRY_FIRST
        .saturating_mul(1 << failures.min(6))
        .min(REMOTE_RETRY_MAX)
}

/// Keeps one subscribe-only connection per other member's SFU: opens them
/// as members appear, retries failures with backoff, closes the ones nobody
/// publishes to any more, and tells each connection who to play. Notices
/// from our own SFU go to the engine; a lost remote SFU is reconnected.
async fn manage_sfus(
    m: SfuManager,
    own_expected: watch::Sender<BTreeSet<String>>,
    mut expected: watch::Receiver<Expected>,
    mut notices: mpsc::Receiver<(String, MediaNotice)>,
    to_engine: mpsc::Sender<MediaNotice>,
) {
    let mut wanted: HashMap<String, Remote> = HashMap::new();
    let mut attempts: JoinSet<(String, Result<SfuConnection, CoreError>)> = JoinSet::new();
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        let now = expected.borrow_and_update().clone();
        let own = now.get(&m.own_service).cloned().unwrap_or_default();
        own_expected.send_if_modified(|e| {
            let changed = *e != own;
            *e = own;
            changed
        });
        // SFUs nobody publishes to any more.
        let gone: Vec<String> = wanted
            .keys()
            .filter(|url| !now.contains_key(*url))
            .cloned()
            .collect();
        for url in gone {
            if let Some(handle) = wanted.remove(&url).and_then(|r| r.connecting) {
                handle.abort();
            }
            let closed = m.remotes.lock().expect("remotes lock").remove(&url);
            if let Some(connection) = closed {
                connection.close().await;
            }
        }
        for (url, ids) in now.iter().filter(|(url, _)| **url != m.own_service) {
            let remote = wanted.entry(url.clone()).or_insert_with(|| Remote {
                expected: watch::channel(BTreeSet::new()).0,
                connecting: None,
                retry: None,
            });
            remote.expected.send_if_modified(|e| {
                let changed = e != ids;
                e.clone_from(ids);
                changed
            });
            let connected = m.remotes.lock().expect("remotes lock").contains_key(url);
            let due = remote.retry.is_none_or(|(at, _)| at <= Instant::now());
            if !connected && remote.connecting.is_none() && due {
                let client = m.client.clone();
                let room_id = m.room_id.clone();
                let url = url.clone();
                let rx = remote.expected.subscribe();
                let shared = ConnectionShared {
                    encryption: m.shared.encryption.clone(),
                    voice: m.shared.voice.clone(),
                    volumes: m.shared.volumes.clone(),
                    flags: m.shared.flags.clone(),
                    cleaning: m.shared.cleaning.clone(),
                    notices: m.shared.notices.clone(),
                };
                remote.connecting = Some(attempts.spawn(async move {
                    let result = match jwt::subscribe_access(&client, &url, &room_id).await {
                        Ok(sfu) => {
                            SfuConnection::connect(url.clone(), &sfu, None, rx, &shared).await
                        }
                        Err(e) => Err(e),
                    };
                    (url, result)
                }));
            }
        }
        tokio::select! {
            changed = expected.changed() => if changed.is_err() { return },
            Some(done) = attempts.join_next() => {
                // An aborted attempt (its SFU no longer wanted) ends here too.
                if let Ok((url, result)) = done {
                    match (wanted.get_mut(&url), result) {
                        (Some(remote), Ok(connection)) => {
                            remote.connecting = None;
                            remote.retry = None;
                            m.remotes.lock().expect("remotes lock").insert(url, connection);
                        }
                        (Some(remote), Err(_)) => {
                            remote.connecting = None;
                            let failures = remote.retry.map_or(0, |(_, n)| n);
                            remote.retry = Some((Instant::now() + retry_delay(failures), failures + 1));
                            tracing::warn!("could not connect to another member's call server; retrying");
                        }
                        (None, Ok(connection)) => connection.close().await,
                        (None, Err(_)) => {}
                    }
                }
            }
            Some((url, notice)) = notices.recv() => {
                if url == m.own_service {
                    let _ = to_engine.send(notice).await;
                } else if notice == MediaNotice::Disconnected {
                    let lost = m.remotes.lock().expect("remotes lock").remove(&url);
                    if let Some(connection) = lost {
                        connection.close().await;
                    }
                    if let Some(remote) = wanted.get_mut(&url) {
                        remote.retry = Some((Instant::now() + REMOTE_RETRY_FIRST, 1));
                    }
                }
            }
            _ = ticker.tick() => {}
        }
    }
}

/// Takes a member's call key from a decrypted to-device event. Keys sent
/// in clear, for another room, or claiming another device than the Olm
/// session's are ignored.
fn receive_key(
    event: &Raw<AnyToDeviceEvent>,
    info: Option<&EncryptionInfo>,
    room_id: &str,
    provider: &KeyProvider,
) {
    let Ok(event) = event.deserialize_as::<Value>() else {
        return;
    };
    if event.get("type").and_then(Value::as_str) != Some(e2ee::KEYS_EVENT) {
        return;
    }
    let Some(info) = info else {
        tracing::warn!("call key sent unencrypted; ignored");
        return;
    };
    let Some(key) = event.get("content").and_then(|content| {
        e2ee::parse_key(
            content,
            info.sender.as_str(),
            info.sender_device.as_deref().map(|d| d.as_str()),
            room_id,
        )
    }) else {
        return;
    };
    provider.set_key(
        &ParticipantIdentity(membership::member_id(&key.user_id, &key.device_id)),
        i32::from(key.index),
        key.key.to_vec(),
    );
}

/// Shares our key with the members as they come, rotates it as they go,
/// and retries devices it could not reach. A new key is used after
/// `USE_KEY_DELAY`, once the others have it (also when some could not be
/// reached: they get it on the next try), or at once with nobody to send
/// it to.
async fn distribute_keys(
    client: Client,
    room_id: String,
    own_identity: String,
    mut planner: KeyPlanner,
    provider: KeyProvider,
    key_index: watch::Sender<i32>,
    mut members: watch::Receiver<Vec<Recipient>>,
) {
    let own = ParticipantIdentity(own_identity.clone());
    let mut retry = tokio::time::interval(PARTICIPANT_RECHECK);
    loop {
        let now = members.borrow_and_update().clone();
        if let Some(rollout) = planner.plan(&now, Instant::now(), e2ee::random_key) {
            if !rollout.to.is_empty() {
                let got =
                    e2ee::send_key(&client, &room_id, &own_identity, &rollout, now_ms()).await;
                planner.shared(&got);
                if rollout.rotated {
                    tokio::time::sleep(e2ee::USE_KEY_DELAY).await;
                }
            }
            if rollout.rotated {
                provider.set_key(&own, i32::from(rollout.index), rollout.key.to_vec());
                key_index.send_replace(i32::from(rollout.index));
            }
        }
        tokio::select! {
            changed = members.changed() => if changed.is_err() { return },
            _ = retry.tick() => {}
        }
    }
}
