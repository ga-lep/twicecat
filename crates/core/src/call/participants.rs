//! Who is in the call: the active memberships in the room state, reported
//! as `CallParticipants` with what the media layer knows of each (speaking,
//! muted, cleaned), and handed to the SFU manager (who publishes where) and
//! the key distribution (whom to send our key).

use std::sync::Arc;
use std::time::Duration;

use matrix_sdk::Room;
use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use matrix_sdk::ruma::events::StateEventType;
use serde_json::Value;
use tokio::sync::{Notify, mpsc, watch};

use super::e2ee::Recipient;
use super::media::SharedFlags;
use super::membership::{self, CALL_MEMBER_EVENT, Membership};
use super::sfus::Expected;
use super::{UserVolumes, now_ms, user_volume};
use crate::api::{CoreEvent, DeviceId, ListDiff, Participant, ParticipantId, RoomId, UserId};

/// How often participants are re-read even without a state change, so that
/// expired memberships disappear.
const PARTICIPANT_RECHECK: Duration = Duration::from_secs(5);

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
        let volume_percent = user_volume(volumes, &user_id);
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

/// Our side, for the participant watcher.
pub(super) struct Ours {
    pub(super) identity: String,
    pub(super) service: String,
    /// Who to play on which SFU, for `manage_sfus`.
    pub(super) expected: watch::Sender<Expected>,
    /// The other members' devices, for our key (`distribute_keys`).
    pub(super) recipients: watch::Sender<Vec<Recipient>>,
    /// Told when our membership, once seen, is gone (`keep_membership`).
    pub(super) gone: Arc<Notify>,
}

/// Emits `CallParticipants` (a full reset) whenever the active set changes,
/// and tells the SFU manager who publishes where.
pub(super) async fn watch_participants(
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
