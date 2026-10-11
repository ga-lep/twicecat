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
mod own_membership;
mod participants;
pub(crate) mod ringing;
mod sfus;
pub(crate) mod watcher;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use livekit::e2ee::key_provider::KeyProvider;
use livekit::e2ee::{E2eeOptions, EncryptionType};
use livekit::id::ParticipantIdentity;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::event_handler::EventHandlerHandle;
use matrix_sdk::ruma::api::client::rtc::RtcTransport;
use matrix_sdk::ruma::events::AnyToDeviceEvent;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room, RoomState};
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tokio::task::JoinHandle;

use self::cleaning::Cleaning;
pub(crate) use self::cleaning::announcement;
use self::e2ee::KeyPlanner;
pub(crate) use self::media::MediaNotice;
use self::media::{ConnectionShared, Publish, SfuConnection, SharedFlags};
use self::membership::MEMBERSHIP_EXPIRES;
use self::own_membership::{
    OwnMembership, keep_membership, schedule_delayed_leave, send_membership,
};
pub(crate) use self::participants::active_memberships;
use self::participants::{Ours, watch_participants};
use self::sfus::{Expected, Remotes, SfuManager, manage_sfus};
use crate::api::{
    AudioSettings, CoreError, CoreEvent, RemoteCleaning, RoomId, UserId, VoiceCleaning,
};

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Per-user playback volumes, shared with the engine so participants carry
/// the current value.
pub(crate) type UserVolumes = Arc<RwLock<BTreeMap<UserId, u16>>>;

/// `user`'s playback volume, percent (100 when not set).
pub(crate) fn user_volume(volumes: &UserVolumes, user: &UserId) -> u16 {
    volumes
        .read()
        .ok()
        .and_then(|v| v.get(user).copied())
        .unwrap_or(100)
}

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
    pub outgoing: Option<twicecat_audio::Outgoing>,
    pub bitrate_kbps: u16,
    /// Cleaning other people's voices, and the per-user choices.
    pub cleaning: (RemoteCleaning, BTreeMap<UserId, VoiceCleaning>),
    /// What we announce about our microphone's cleaning ([`announcement`]).
    pub announce: Option<&'static str>,
    /// Where other voices are cleaned, for the audio stats.
    pub cleaning_backend: Option<crate::audio_session::CleaningBackend>,
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
            audio.cleaning_backend.clone(),
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
                    async move { e2ee::receive_key(&event, info.as_ref(), &room_id, &provider) }
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
            session.tasks.push(tokio::spawn(e2ee::distribute_keys(
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
        own_membership::remove(&self.room, &self.state_key, delay_id).await;
    }
}
