//! Calls in every room while logged in: incoming rings (until answered,
//! declined, ended or expired), declines of the rings we sent, and how many
//! devices are in each room's call, for the room list.
//!
//! In encrypted rooms a notification or decline can arrive before its room
//! key: the sync service brings room events and to-device keys on separate
//! connections, and the SDK calls event handlers only once, with whatever it
//! could decrypt then. Recent such events are held for a while and tried
//! again, off the sync path, when their keys arrive. Likewise a notification
//! can be handled before the caller's membership reaches our store; it waits
//! (until it expires) for the membership, and rings then.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures_util::StreamExt as _;
use matrix_sdk::event_handler::EventHandlerHandle;
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk::ruma::events::room::encrypted::OriginalSyncRoomEncryptedEvent;
use matrix_sdk::ruma::events::{AnySyncMessageLikeEvent, AnySyncStateEvent, StateEventType};
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room};
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::membership::CALL_MEMBER_EVENT;
use super::ringing::{self, Callee, DECLINE_EVENT, NOTIFICATION_EVENT, SLOT_EVENT, SLOT_ID};
use crate::api::{CoreEvent, RoomId, UserId};

/// How often rings are checked for expiry and calls for stale memberships.
const RECHECK: Duration = Duration::from_secs(2);

/// The type of an event the SDK could not decrypt.
const ENCRYPTED_EVENT: &str = "m.room.encrypted";
/// How many undecrypted events are held for their keys (the oldest go
/// first), and for how long: a notification older than its longest lifetime
/// cannot ring, and a decline only matters while it does, so older events
/// (history in rooms whose keys this device never had) are not held at all.
const HELD_MAX: usize = 64;
const HELD_FOR_MS: u64 = ringing::MAX_LIFETIME_MS;

struct Ring {
    event_id: String,
    expires_ms: u64,
    /// A ring (a DM call), not a quiet notification: it plays the ringtone.
    loud: bool,
    /// `SilenceRinging` stopped its ringtone; it still rings.
    silenced: bool,
}

/// A notification for a call whose membership has not reached us yet.
struct Waiting {
    event_id: String,
    caller: String,
    incoming: ringing::Incoming,
}

/// An event that could not be decrypted at sync.
struct Held {
    room_id: OwnedRoomId,
    /// The Megolm session it needs a key for.
    session_id: String,
    event_id: String,
    raw: Raw<OriginalSyncRoomEncryptedEvent>,
    held_at_ms: u64,
}

#[derive(Default)]
struct WatchState {
    /// Rooms ringing us.
    rings: HashMap<String, Ring>,
    /// Notifications waiting for their call's membership, by room.
    waiting: HashMap<String, Waiting>,
    /// Notifications already handled, until they expire: a sync that
    /// starts over (after a restart or an expired position) sends each
    /// room's latest events again, and a ring answered or declined must not
    /// come back.
    handled: HashMap<String, u64>,
    /// Devices in each room's call, as last reported.
    counts: HashMap<String, u32>,
    /// Our own notifications, by room, to tell their declines.
    sent: HashMap<String, String>,
    /// Events waiting for their room keys, oldest first.
    held: Vec<Held>,
    /// Whether a ring should sound: a loud ring not silenced. Updated by
    /// [`WatchState::sync_loud`] after every change to `rings`.
    loud: watch::Sender<bool>,
}

impl WatchState {
    /// Publishes whether a ring should sound now; only a change wakes the
    /// ringtone.
    fn sync_loud(&self) {
        let loud = self.rings.values().any(|r| r.loud && !r.silenced);
        self.loud.send_if_modified(|now| {
            let changed = *now != loud;
            *now = loud;
            changed
        });
    }

    /// Silences the rings sounding now (`SilenceRinging`).
    fn silence(&mut self) {
        for ring in self.rings.values_mut() {
            ring.silenced = true;
        }
        self.sync_loud();
    }

    /// Removes the ring of `room_id`, if any.
    fn remove_ring(&mut self, room_id: &str) -> Option<Ring> {
        let ring = self.rings.remove(room_id);
        self.sync_loud();
        ring
    }
}

type State = Arc<StdMutex<WatchState>>;

/// Watches calls for one session; stop it before the client goes.
pub(crate) struct CallWatch {
    client: Client,
    handlers: Vec<EventHandlerHandle>,
    tasks: [JoinHandle<()>; 2],
    state: State,
}

impl CallWatch {
    pub(crate) fn start(client: &Client, events: mpsc::Sender<CoreEvent>) -> Self {
        let state = State::default();
        let own = client.user_id().map(|u| u.to_string()).unwrap_or_default();
        // Rooms with newly held events, for one more try in the background:
        // their keys may have come since the sync tried.
        let (newly_held, held_rx) = mpsc::unbounded_channel();
        let (s, e, o) = (state.clone(), events.clone(), own.clone());
        let timeline =
            client.add_event_handler(move |ev: Raw<AnySyncMessageLikeEvent>, room: Room| {
                let (state, events, own) = (s.clone(), e.clone(), o.clone());
                let newly_held = newly_held.clone();
                async move {
                    let Ok(event) = ev.deserialize_as::<Value>() else {
                        return;
                    };
                    if event.get("type").and_then(Value::as_str) == Some(ENCRYPTED_EVENT) {
                        if hold(&room, &event, ev.cast_unchecked(), &state) {
                            let _ = newly_held.send(room.room_id().to_owned());
                        }
                    } else {
                        on_event(&room, &event, &own, &state, &events).await;
                    }
                }
            });
        let (s, e, o) = (state.clone(), events.clone(), own.clone());
        let members = client.add_event_handler(move |ev: Raw<AnySyncStateEvent>, room: Room| {
            let (state, events, own) = (s.clone(), e.clone(), o.clone());
            async move {
                let is_member = ev
                    .get_field::<String>("type")
                    .ok()
                    .flatten()
                    .is_some_and(|t| t == CALL_MEMBER_EVENT);
                if is_member {
                    on_members(&room, &own, &state, &events).await;
                }
            }
        });
        let keys = tokio::spawn(retry_held_events(
            client.clone(),
            own.clone(),
            state.clone(),
            events.clone(),
            held_rx,
        ));
        let recheck = tokio::spawn(recheck(client.clone(), own, state.clone(), events));
        Self {
            client: client.clone(),
            handlers: vec![timeline, members],
            tasks: [keys, recheck],
            state,
        }
    }

    /// Stops ringing in `room_id` (answered or declined here); returns the
    /// notification that rang.
    pub(crate) fn take_ring(&self, room_id: &RoomId) -> Option<String> {
        let mut st = self.state.lock().expect("call watch lock");
        st.remove_ring(room_id.as_str()).map(|r| r.event_id)
    }

    /// Whether a ring should sound now, for the ringtone; `false` once the
    /// watch stops.
    pub(crate) fn loud(&self) -> watch::Receiver<bool> {
        self.state.lock().expect("call watch lock").loud.subscribe()
    }

    /// Stops the ringtone of the rings sounding now; they still ring.
    pub(crate) fn silence(&self) {
        self.state.lock().expect("call watch lock").silence();
    }

    /// Remembers the notification we sent for our call in `room_id`
    /// (`None` when the call ends).
    pub(crate) fn set_sent(&self, room_id: &RoomId, event_id: Option<String>) {
        let mut st = self.state.lock().expect("call watch lock");
        match event_id {
            Some(id) => st.sent.insert(room_id.as_str().to_owned(), id),
            None => st.sent.remove(room_id.as_str()),
        };
    }

    pub(crate) async fn stop(self) {
        // The ringtone ends with the session.
        self.state
            .lock()
            .expect("call watch lock")
            .loud
            .send_replace(false);
        for handle in self.handlers {
            self.client.remove_event_handler(handle);
        }
        for task in self.tasks {
            task.abort();
            let _ = task.await;
        }
    }
}

fn now_ms() -> u64 {
    super::now_ms()
}

async fn on_event(
    room: &Room,
    event: &Value,
    own: &str,
    state: &State,
    events: &mpsc::Sender<CoreEvent>,
) {
    match event.get("type").and_then(Value::as_str) {
        Some(NOTIFICATION_EVENT) => on_notification(room, event, own, state, events).await,
        Some(DECLINE_EVENT) => on_decline(room, event, own, state, events).await,
        _ => {}
    }
}

/// Holds a recent event the sync could not decrypt, until its key arrives;
/// returns whether it was held.
fn hold(
    room: &Room,
    event: &Value,
    raw: Raw<OriginalSyncRoomEncryptedEvent>,
    state: &State,
) -> bool {
    let (Some(event_id), Some(ts), Some(session_id)) = (
        event.get("event_id").and_then(Value::as_str),
        event.get("origin_server_ts").and_then(Value::as_u64),
        event.pointer("/content/session_id").and_then(Value::as_str),
    ) else {
        return false;
    };
    let now = now_ms();
    if now.saturating_sub(ts) >= HELD_FOR_MS {
        return false;
    }
    let mut st = state.lock().expect("call watch lock");
    if st.held.iter().any(|h| h.event_id == event_id) {
        return false;
    }
    if st.held.len() >= HELD_MAX {
        st.held.remove(0);
    }
    st.held.push(Held {
        room_id: room.room_id().to_owned(),
        session_id: session_id.to_owned(),
        event_id: event_id.to_owned(),
        raw,
        held_at_ms: now,
    });
    tracing::debug!(room = %room.room_id(), "event held for its room key");
    true
}

/// Tries again the events held in `room` (those of `sessions` only, if
/// given), and handles those that decrypt. Each is handled once, by
/// whichever try takes it out of the list.
async fn retry_held(
    room: &Room,
    sessions: Option<&[&str]>,
    own: &str,
    state: &State,
    events: &mpsc::Sender<CoreEvent>,
) {
    let held: Vec<(String, Raw<OriginalSyncRoomEncryptedEvent>)> = {
        let st = state.lock().expect("call watch lock");
        st.held
            .iter()
            .filter(|h| h.room_id == room.room_id())
            .filter(|h| sessions.is_none_or(|s| s.contains(&h.session_id.as_str())))
            .map(|h| (h.event_id.clone(), h.raw.clone()))
            .collect()
    };
    for (event_id, raw) in held {
        let Ok(decrypted) = room.decrypt_event(&raw, None).await else {
            continue;
        };
        let Ok(event) = decrypted.raw().deserialize_as::<Value>() else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) == Some(ENCRYPTED_EVENT) {
            // Still no key.
            continue;
        }
        let taken = {
            let mut st = state.lock().expect("call watch lock");
            let before = st.held.len();
            st.held.retain(|h| h.event_id != event_id);
            st.held.len() < before
        };
        if taken {
            tracing::debug!(room = %room.room_id(), user = own, "held event decrypted");
            on_event(room, &event, own, state, events).await;
        }
    }
}

/// Tries the held events again: once just after they are held, then
/// whenever keys arrive for their sessions.
async fn retry_held_events(
    client: Client,
    own: String,
    state: State,
    events: mpsc::Sender<CoreEvent>,
    mut newly_held: mpsc::UnboundedReceiver<OwnedRoomId>,
) {
    loop {
        // None until the Olm machine exists; the stream ends when the SDK
        // recreates it.
        let Some(keys) = client.encryption().room_keys_received_stream().await else {
            tokio::time::sleep(RECHECK).await;
            continue;
        };
        let mut keys = std::pin::pin!(keys);
        loop {
            // Rooms to try, with the sessions whose keys came (None: all).
            let tries: Vec<(OwnedRoomId, Option<Vec<String>>)> = tokio::select! {
                room = newly_held.recv() => match room {
                    Some(room) => vec![(room, None)],
                    None => return,
                },
                update = keys.next() => match update {
                    Some(Ok(infos)) => infos
                        .into_iter()
                        .map(|k| (k.room_id, Some(vec![k.session_id])))
                        .collect(),
                    // Some updates were missed: try everything held.
                    Some(Err(_)) => {
                        let st = state.lock().expect("call watch lock");
                        st.held.iter().map(|h| (h.room_id.clone(), None)).collect()
                    }
                    None => break,
                },
            };
            for (room_id, sessions) in tries {
                let Some(room) = client.get_room(&room_id) else {
                    continue;
                };
                let sessions: Option<Vec<&str>> = sessions
                    .as_ref()
                    .map(|s| s.iter().map(String::as_str).collect());
                retry_held(&room, sessions.as_deref(), &own, &state, &events).await;
            }
        }
    }
}

async fn on_notification(
    room: &Room,
    event: &Value,
    own: &str,
    state: &State,
    events: &mpsc::Sender<CoreEvent>,
) {
    let (Some(sender), Some(event_id), Some(ts), Some(content)) = (
        event.get("sender").and_then(Value::as_str),
        event.get("event_id").and_then(Value::as_str),
        event.get("origin_server_ts").and_then(Value::as_u64),
        event.get("content"),
    ) else {
        return;
    };
    let may_notify = match matrix_sdk::ruma::UserId::parse(sender) {
        Ok(uid) => room
            .power_levels()
            .await
            .is_ok_and(|pl| pl.user_can_trigger_room_notification(&uid)),
        Err(_) => false,
    };
    let rx = Callee {
        own_user: own,
        now_ms: now_ms(),
        sender_may_notify_room: may_notify,
    };
    let Some(incoming) = ringing::parse_notification(sender, ts, content, &rx) else {
        tracing::debug!(room = %room.room_id(), user = own, "call notification not for us or expired");
        return;
    };
    {
        let mut st = state.lock().expect("call watch lock");
        if st
            .handled
            .insert(event_id.to_owned(), incoming.expires_ms)
            .is_some()
        {
            tracing::debug!(room = %room.room_id(), user = own, "call notification already handled");
            return;
        }
    }
    let slot = match room
        .get_state_event(StateEventType::from(SLOT_EVENT), SLOT_ID)
        .await
    {
        Ok(Some(matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState::Sync(raw))) => raw
            .deserialize_as::<Value>()
            .ok()
            .and_then(|e| e.get("content").cloned()),
        _ => None,
    };
    if !ringing::slot_open(slot.as_ref()) {
        tracing::debug!(room = %room.room_id(), user = own, "call notification for a closed slot");
        return;
    }
    let room_id = room.room_id().as_str();
    let waiting = Waiting {
        event_id: event_id.to_owned(),
        caller: sender.to_owned(),
        incoming,
    };
    let active = super::active_memberships(room).await;
    if active.iter().any(|m| m.user_id == own) {
        // We are already in the call (on any device).
        return;
    }
    if active.is_empty() {
        // Sent after the caller's membership, but handled before it reached
        // our store: `on_members` rings when it does.
        tracing::debug!(room = %room_id, user = own, "call notification waits for the call's membership");
        let mut st = state.lock().expect("call watch lock");
        st.waiting.insert(room_id.to_owned(), waiting);
        return;
    }
    ring(room_id, waiting, own, state, events).await;
}

/// Rings for `waiting` in `room_id`.
async fn ring(
    room_id: &str,
    waiting: Waiting,
    own: &str,
    state: &State,
    events: &mpsc::Sender<CoreEvent>,
) {
    tracing::debug!(room = %room_id, user = own, "call notification rings");
    {
        let mut st = state.lock().expect("call watch lock");
        st.rings.insert(
            room_id.to_owned(),
            Ring {
                event_id: waiting.event_id,
                expires_ms: waiting.incoming.expires_ms,
                loud: waiting.incoming.ring,
                silenced: false,
            },
        );
        st.sync_loud();
    }
    let _ = events
        .send(CoreEvent::IncomingCall {
            room_id: RoomId::from(room_id),
            caller: UserId::from(waiting.caller.as_str()),
            intent: waiting.incoming.intent,
            ring: waiting.incoming.ring,
        })
        .await;
}

async fn on_decline(
    room: &Room,
    event: &Value,
    own: &str,
    state: &State,
    events: &mpsc::Sender<CoreEvent>,
) {
    let (Some(sender), Some(target)) = (
        event.get("sender").and_then(Value::as_str),
        event
            .get("content")
            .and_then(ringing::declined_notification),
    ) else {
        return;
    };
    let room_id = room.room_id().as_str();
    let out = {
        let mut st = state.lock().expect("call watch lock");
        if sender == own {
            // Declined on another of our devices.
            let ours = st.rings.get(room_id).is_some_and(|r| r.event_id == target);
            if ours {
                st.remove_ring(room_id);
            }
            ours.then(|| CoreEvent::RingingStopped {
                room_id: RoomId::from(room_id),
            })
        } else {
            (st.sent.get(room_id).map(String::as_str) == Some(target)).then(|| {
                CoreEvent::CallDeclined {
                    room_id: RoomId::from(room_id),
                    user_id: UserId::from(sender),
                }
            })
        }
    };
    if let Some(out) = out {
        let _ = events.send(out).await;
    }
}

/// Re-counts a room's call, stops its ring if the call ended or we joined
/// it on any device, and rings for a notification that waited for the call.
async fn on_members(room: &Room, own: &str, state: &State, events: &mpsc::Sender<CoreEvent>) {
    let active = super::active_memberships(room).await;
    let count = u32::try_from(active.len()).unwrap_or(u32::MAX);
    let room_id = room.room_id().as_str();
    let joined = active.iter().any(|m| m.user_id == own);
    let stop_ring = active.is_empty() || joined;
    let (count_changed, ring_stopped, now_ringing) = {
        let mut st = state.lock().expect("call watch lock");
        let previous = st.counts.insert(room_id.to_owned(), count).unwrap_or(0);
        if count == 0 {
            st.counts.remove(room_id);
        }
        let ring_stopped = stop_ring && st.remove_ring(room_id).is_some();
        // A waiting notification rings once its sender is in the call (not
        // for someone else's call), unless we joined meanwhile.
        let caller_in = |w: &Waiting| active.iter().any(|m| m.user_id == w.caller);
        let settled = joined || st.waiting.get(room_id).is_some_and(caller_in);
        let now_ringing = settled
            .then(|| st.waiting.remove(room_id))
            .flatten()
            .filter(|w| !joined && w.incoming.expires_ms > now_ms());
        (previous != count, ring_stopped, now_ringing)
    };
    if count_changed {
        tracing::debug!(room = %room_id, user = own, count, "call participants");
        let _ = events
            .send(CoreEvent::ActiveCall {
                room_id: RoomId::from(room_id),
                participants: count,
            })
            .await;
    }
    if ring_stopped {
        let _ = events
            .send(CoreEvent::RingingStopped {
                room_id: RoomId::from(room_id),
            })
            .await;
    }
    if let Some(waiting) = now_ringing {
        ring(room_id, waiting, own, state, events).await;
    }
}

/// Ends expired rings, and re-counts the calls we know of, whose
/// memberships can expire without any new event.
async fn recheck(client: Client, own: String, state: State, events: mpsc::Sender<CoreEvent>) {
    let mut ticker = tokio::time::interval(RECHECK);
    loop {
        ticker.tick().await;
        let now = now_ms();
        let (expired, rooms): (Vec<String>, Vec<String>) = {
            let mut st = state.lock().expect("call watch lock");
            let expired: Vec<String> = st
                .rings
                .iter()
                .filter(|(_, r)| r.expires_ms <= now)
                .map(|(room, _)| room.clone())
                .collect();
            for room in &expired {
                st.remove_ring(room);
            }
            st.held
                .retain(|h| now.saturating_sub(h.held_at_ms) < HELD_FOR_MS);
            st.waiting.retain(|_, w| w.incoming.expires_ms > now);
            st.handled.retain(|_, expires_ms| *expires_ms > now);
            (expired, st.counts.keys().cloned().collect())
        };
        for room_id in expired {
            let _ = events
                .send(CoreEvent::RingingStopped {
                    room_id: RoomId::from(room_id.as_str()),
                })
                .await;
        }
        for room_id in rooms {
            let Ok(id) = matrix_sdk::ruma::OwnedRoomId::try_from(room_id.as_str()) else {
                continue;
            };
            if let Some(room) = client.get_room(&id) {
                on_members(&room, &own, &state, &events).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(loud: bool) -> Ring {
        Ring {
            event_id: "$e".into(),
            expires_ms: u64::MAX,
            loud,
            silenced: false,
        }
    }

    fn add(st: &mut WatchState, room: &str, loud: bool) {
        st.rings.insert(room.into(), ring(loud));
        st.sync_loud();
    }

    #[test]
    fn only_a_loud_ring_sounds() {
        let mut st = WatchState::default();
        let rx = st.loud.subscribe();
        assert!(!*rx.borrow());
        add(&mut st, "!group:x", false);
        assert!(!*rx.borrow(), "a quiet notification plays no tone");
        add(&mut st, "!dm:x", true);
        assert!(*rx.borrow());
        st.remove_ring("!group:x");
        assert!(*rx.borrow(), "the DM still rings");
        assert!(st.remove_ring("!dm:x").is_some());
        assert!(!*rx.borrow());
        assert!(st.remove_ring("!dm:x").is_none());
    }

    #[test]
    fn silencing_stops_the_rings_sounding_now_but_not_later_ones() {
        let mut st = WatchState::default();
        let mut rx = st.loud.subscribe();
        add(&mut st, "!a:x", true);
        st.silence();
        assert!(!*rx.borrow_and_update());
        assert!(st.rings.contains_key("!a:x"), "it still rings");
        add(&mut st, "!b:x", true);
        assert!(*rx.borrow_and_update(), "a new ring sounds");
        st.silence();
        assert!(!*rx.borrow_and_update());
    }

    #[test]
    fn only_a_change_wakes_the_ringtone() {
        let mut st = WatchState::default();
        let mut rx = st.loud.subscribe();
        add(&mut st, "!a:x", true);
        assert!(rx.has_changed().expect("open"));
        rx.borrow_and_update();
        add(&mut st, "!b:x", true);
        assert!(!rx.has_changed().expect("open"), "still loud: no wake-up");
    }
}
