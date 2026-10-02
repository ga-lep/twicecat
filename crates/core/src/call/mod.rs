//! MatrixRTC calls (Phase 6): signalling here, media in later tasks.
//!
//! Joining follows Element Call's compatibility mode (see `membership`):
//! 1. find our homeserver's LiveKit service (MSC4143 transport discovery);
//! 2. schedule a delayed empty membership (MSC4140), the crash-safe leave;
//! 3. publish our membership;
//! 4. get a LiveKit token for that service (`jwt`);
//! 5. keep the delayed leave from firing (restart every 4 s), refresh the
//!    membership before it expires, and report the participants.
//!
//! Leaving fires the delayed leave at once (or sends the empty membership).

mod jwt;
pub(crate) mod membership;

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::UpdateAction;
use matrix_sdk::ruma::api::client::delayed_events::update_delayed_event::unstable_v2 as update_delayed;
use matrix_sdk::ruma::api::client::delayed_events::{DelayParameters, delayed_state_event};
use matrix_sdk::ruma::api::client::rtc::RtcTransport;
use matrix_sdk::ruma::api::error::ErrorKind;
use matrix_sdk::ruma::events::StateEventType;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room, RoomState};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::task::JoinHandle;

use self::jwt::SfuAccess;
use self::membership::{
    CALL_MEMBER_EVENT, DELAYED_LEAVE, DELAYED_LEAVE_RESTART, MEMBERSHIP_EXPIRES, Membership,
};
use crate::api::{
    CoreError, CoreEvent, DeviceId, ListDiff, Participant, ParticipantId, RoomId, UserId,
};

/// How often participants are re-read even without a state change, so that
/// expired memberships disappear.
const PARTICIPANT_RECHECK: Duration = Duration::from_secs(5);
/// Refresh our membership this long before it would expire.
const REFRESH_MARGIN: Duration = Duration::from_secs(10 * 60);

fn now_ms() -> u64 {
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
    /// The SFU we publish to (used by the media task).
    #[allow(dead_code)] // Read once media lands (Phase 6 Task 8).
    pub sfu: SfuAccess,
    tasks: Vec<JoinHandle<()>>,
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

async fn send_membership(room: &Room, state_key: &str, content: Value) -> Result<(), CoreError> {
    room.send_state_event_raw(CALL_MEMBER_EVENT, state_key, content)
        .await
        .map(|_| ())
        .map_err(|e| match e.client_api_error_kind() {
            // The room's power levels reserve call memberships (Element
            // creates rooms with `org.matrix.msc3401.call.member` at 0).
            Some(ErrorKind::Forbidden) => {
                CoreError::Call("you are not allowed to join calls in this room".into())
            }
            _ => CoreError::Call("could not publish the call membership".into()),
        })
}

/// Our homeserver's LiveKit service URL (MSC4143).
async fn own_livekit_service(client: &Client) -> Result<String, CoreError> {
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

/// Active memberships as API participants, oldest first.
async fn participants(room: &Room, volumes: &UserVolumes) -> Vec<Participant> {
    let now = now_ms();
    let own_user = room.own_user_id().as_str().to_owned();
    let own_device = room
        .client()
        .device_id()
        .map(|d| d.as_str().to_owned())
        .unwrap_or_default();
    let mut active: Vec<Membership> = read_memberships(room)
        .await
        .into_iter()
        .filter(|m| m.is_active(now))
        .collect();
    active.sort_by(|a, b| (a.joined_ms, &a.member_id).cmp(&(b.joined_ms, &b.member_id)));
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
        out.push(Participant {
            id: ParticipantId::from(m.member_id.as_str()),
            is_local: m.user_id == own_user && m.device_id == own_device,
            user_id,
            device_id: DeviceId::from(m.device_id.as_str()),
            display_name,
            muted: false,
            speaking: false,
            volume_percent,
        });
    }
    out
}

impl CallSession {
    /// Joins the room's call. On failure nothing stays published.
    pub(crate) async fn join(
        client: &Client,
        room_id: RoomId,
        events: mpsc::Sender<CoreEvent>,
        volumes: UserVolumes,
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
        let service_url = own_livekit_service(client).await?;
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
        let mut session = Self {
            room_id: room_id.clone(),
            room: room.clone(),
            state_key: state_key.clone(),
            delay_id: delay_id.clone(),
            sfu: SfuAccess {
                url: String::new(),
                jwt: Default::default(),
            },
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
        if let Err(e) = send_membership(&room, &state_key, content).await {
            session.leave().await;
            return Err(e);
        }
        match jwt::sfu_access(client, &service_url, room_id.as_str()).await {
            Ok(sfu) => session.sfu = sfu,
            Err(e) => {
                session.leave().await;
                return Err(e);
            }
        }

        session.tasks.push(tokio::spawn(watch_participants(
            room, room_id, events, volumes, own_gone,
        )));
        Ok(session)
    }

    /// Leaves: stops the tasks and removes our membership.
    pub(crate) async fn leave(mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
            let _ = task.await;
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

/// Emits `CallParticipants` (a full reset) whenever the active set changes,
/// and tells `keep_membership` when our membership, once seen, is gone.
async fn watch_participants(
    room: Room,
    room_id: RoomId,
    events: mpsc::Sender<CoreEvent>,
    volumes: UserVolumes,
    gone: Arc<Notify>,
) {
    let mut info = room.subscribe_info();
    let mut ticker = tokio::time::interval(PARTICIPANT_RECHECK);
    let mut last: Option<Vec<Participant>> = None;
    // Our membership reaches the store some time after we send it: only its
    // disappearance after that counts.
    let mut seen_own = false;
    loop {
        let now = participants(&room, &volumes).await;
        if now.iter().any(|p| p.is_local) {
            seen_own = true;
        } else if seen_own {
            seen_own = false;
            tracing::warn!("our call membership is gone; joining again");
            gone.notify_one();
        }
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
            _ = ticker.tick() => {}
        }
    }
}
