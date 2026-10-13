//! Call media end-to-end encryption, as Element Call does it in encrypted
//! rooms (matrix-js-sdk v43 `RTCEncryptionManager` and
//! `ToDeviceKeyTransport`):
//!
//! - every member encrypts its media with its own random 16-byte key and
//!   sends it to the other members' devices as an Olm-encrypted
//!   `io.element.call.encryption_keys` to-device event;
//! - LiveKit turns each key into the frame key with HKDF-SHA-256 (salt
//!   `LKFrameEncryptionKey`, 128 zero bytes of info, 128-bit AES-GCM), as
//!   livekit-client does for Element Call's `MatrixKeyProvider`
//!   (`ratchetWindowSize: 10`, `keyringSize: 256`);
//! - a new key (index + 1 mod 256) when a member leaves, or when one joins
//!   and our key is older than 10 s; a joiner gets the current key
//!   otherwise. We use a new key 1 s after sending it (js-sdk
//!   `useKeyDelay`), so the others have it first; at once if nobody is left
//!   to send it to. (js-sdk v43 makes the new key in that case but never
//!   uses it, so whoever left could still decrypt us until the next
//!   rotation.)
//!
//! Received keys are taken only from Olm-decrypted events whose claimed
//! device is the one the Olm session belongs to (js-sdk cannot check this
//! yet). Key bytes are never logged, and neither are LiveKit's: see
//! [`SECRET_LOG_TARGETS`].

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use livekit::e2ee::key_provider::{KeyDerivationAlgorithm, KeyProvider, KeyProviderOptions};
use livekit::id::ParticipantIdentity;
use matrix_sdk::Client;
use matrix_sdk::deserialized_responses::EncryptionInfo;
use matrix_sdk::ruma::events::{AnyToDeviceEvent, AnyToDeviceEventContent, ToDeviceEventType};
use matrix_sdk::ruma::serde::{Base64, Raw};
use matrix_sdk::ruma::to_device::DeviceIdOrAllDevices;
use matrix_sdk::ruma::{OwnedDeviceId, OwnedUserId};
use serde_json::{Value, json};
use tokio::sync::watch;
use zeroize::Zeroizing;

use super::{membership, now_ms};

/// Log targets that carry call keys, turned off by [`crate::logging`]
/// whatever the filter asks. libwebrtc's own log reaches `log` as `debug`
/// under `libwebrtc`, whatever its severity (libwebrtc 0.3.50
/// `native/peer_connection_factory.rs:41-43`), and its frame cryptor writes
/// the secret and the derived key there each time a key is set or
/// ratcheted (webrtc-sdk/webrtc#308; the strings are in our prebuilt).
pub(crate) const SECRET_LOG_TARGETS: &[&str] = &["libwebrtc"];

/// The to-device event type carrying call keys.
pub(crate) const KEYS_EVENT: &str = "io.element.call.encryption_keys";
/// Length of our keys (Element Call `generateRandomKey`).
const KEY_LEN: usize = 16;
/// Delay before we encrypt with a key we just sent.
pub(crate) const USE_KEY_DELAY: Duration = Duration::from_secs(1);
/// A joiner gets our current key if it is younger than this.
const ROTATION_GRACE: Duration = Duration::from_secs(10);

/// The LiveKit key provider settings matching Element Call.
pub(crate) fn key_provider_options() -> KeyProviderOptions {
    KeyProviderOptions {
        ratchet_window_size: 10,
        // livekit-client's default for Element Call's provider.
        failure_tolerance: 10,
        key_ring_size: 256,
        key_derivation_algorithm: KeyDerivationAlgorithm::HKDF,
        ..KeyProviderOptions::default()
    }
}

/// A fresh random key.
pub(crate) fn random_key() -> Zeroizing<Vec<u8>> {
    let mut key = Zeroizing::new(vec![0u8; KEY_LEN]);
    // Thread-local ChaCha CSPRNG, seeded from the OS.
    rand::fill(&mut key[..]);
    key
}

/// A member device to share our key with; `since` (its membership's join
/// time) tells a rejoin apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Recipient {
    pub user_id: String,
    pub device_id: String,
    pub since: u64,
}

impl Recipient {
    fn same_device(&self, other: &Self) -> bool {
        self.user_id == other.user_id && self.device_id == other.device_id
    }
}

/// What to send now.
pub(crate) struct Rollout {
    pub key: Zeroizing<Vec<u8>>,
    pub index: u8,
    pub to: Vec<Recipient>,
    /// A new key: we start using it after [`USE_KEY_DELAY`].
    pub rotated: bool,
}

struct Outbound {
    key: Zeroizing<Vec<u8>>,
    index: u8,
    created: Instant,
    shared_with: Vec<Recipient>,
}

/// Decides when our key is shared and rotated (js-sdk
/// `RTCEncryptionManager.rolloutOutboundKey`).
pub(crate) struct KeyPlanner {
    outbound: Outbound,
}

impl KeyPlanner {
    /// Starts with `key` at index 0, which we use at once.
    pub(crate) fn new(key: Zeroizing<Vec<u8>>, now: Instant) -> Self {
        Self {
            outbound: Outbound {
                key,
                index: 0,
                created: now,
                shared_with: Vec::new(),
            },
        }
    }

    /// The key to send to whom for the current members (us excluded), or
    /// `None` if everyone has ours. `fresh` makes a new key on rotation; a
    /// rotation with nobody to send to still comes back (empty `to`), so we
    /// switch to the new key.
    pub(crate) fn plan(
        &mut self,
        members: &[Recipient],
        now: Instant,
        fresh: impl FnOnce() -> Zeroizing<Vec<u8>>,
    ) -> Option<Rollout> {
        // A device that rejoined (same device, new membership) counts as a
        // joiner, not as someone who left.
        let already: Vec<&Recipient> = self
            .outbound
            .shared_with
            .iter()
            .filter(|x| {
                !members
                    .iter()
                    .any(|o| x.same_device(o) && x.since != o.since)
            })
            .collect();
        let any_left = already.iter().any(|x| !members.contains(x));
        let joined: Vec<Recipient> = members
            .iter()
            .filter(|x| !already.contains(x))
            .cloned()
            .collect();
        let young = now.duration_since(self.outbound.created) < ROTATION_GRACE;
        let (rotated, to) = if any_left || (!joined.is_empty() && !young) {
            (true, members.to_vec())
        } else {
            (false, joined)
        };
        if to.is_empty() && !rotated {
            return None;
        }
        if rotated {
            self.outbound = Outbound {
                key: fresh(),
                index: self.outbound.index.wrapping_add(1),
                created: now,
                shared_with: Vec::new(),
            };
        }
        Some(Rollout {
            key: self.outbound.key.clone(),
            index: self.outbound.index,
            to,
            rotated,
        })
    }

    /// Records that `to` now has our current key.
    pub(crate) fn shared(&mut self, to: &[Recipient]) {
        self.outbound.shared_with.extend_from_slice(to);
    }
}

/// A member's key, received.
pub(crate) struct ReceivedKey {
    pub user_id: String,
    pub device_id: String,
    pub index: u8,
    pub key: Zeroizing<Vec<u8>>,
}

/// Validates a received key event's content. `sender_device` is the device
/// of the Olm session it arrived on: the claimed device must be that one.
pub(crate) fn parse_key(
    content: &Value,
    sender: &str,
    sender_device: Option<&str>,
    room_id: &str,
) -> Option<ReceivedKey> {
    if content.get("room_id")?.as_str()? != room_id {
        return None;
    }
    let keys = content.get("keys")?;
    let index = u8::try_from(keys.get("index")?.as_u64()?).ok()?;
    let key =
        Base64::<matrix_sdk::ruma::serde::base64::Standard>::parse(keys.get("key")?.as_str()?)
            .ok()?;
    if !(16..=32).contains(&key.as_bytes().len()) {
        return None;
    }
    let device_id = content.get("member")?.get("claimed_device_id")?.as_str()?;
    if sender_device.is_some_and(|d| d != device_id) {
        return None;
    }
    Some(ReceivedKey {
        user_id: sender.to_owned(),
        device_id: device_id.to_owned(),
        index,
        key: Zeroizing::new(key.as_bytes().to_vec()),
    })
}

/// Padded standard base64, as js-sdk's `encodeBase64`.
fn padded_base64(bytes: &[u8]) -> Zeroizing<String> {
    let mut s = Zeroizing::new(
        Base64::<matrix_sdk::ruma::serde::base64::Standard, &[u8]>::new(bytes).encode(),
    );
    while !s.len().is_multiple_of(4) {
        s.push('=');
    }
    s
}

/// The content of our key event (js-sdk `ToDeviceKeyTransport.sendKey`).
fn key_content(
    room_id: &str,
    key: &[u8],
    index: u8,
    device_id: &str,
    member_id: &str,
    sent_ts: u64,
) -> Value {
    json!({
        "keys": { "index": index, "key": padded_base64(key).as_str() },
        "room_id": room_id,
        "member": { "claimed_device_id": device_id, "id": member_id },
        "session": { "call_id": "", "application": "m.call", "scope": "m.room" },
        "sent_ts": sent_ts,
    })
}

/// Sends `rollout`'s key, Olm-encrypted, to its recipients (all their
/// listed devices, as Element Call). Returns the recipients that got it.
pub(crate) async fn send_key(
    client: &Client,
    room_id: &str,
    member_id: &str,
    rollout: &Rollout,
    sent_ts: u64,
) -> Vec<Recipient> {
    let Some(device_id) = client.device_id() else {
        return Vec::new();
    };
    let mut recipients: BTreeMap<OwnedUserId, Vec<DeviceIdOrAllDevices>> = BTreeMap::new();
    for r in &rollout.to {
        if let Ok(user) = OwnedUserId::try_from(r.user_id.as_str()) {
            recipients
                .entry(user)
                .or_default()
                .push(DeviceIdOrAllDevices::DeviceId(OwnedDeviceId::from(
                    r.device_id.as_str(),
                )));
        }
    }
    let content = key_content(
        room_id,
        &rollout.key,
        rollout.index,
        device_id.as_str(),
        member_id,
        sent_ts,
    );
    let Ok(raw) = Raw::<AnyToDeviceEventContent>::from_json_string(content.to_string()) else {
        return Vec::new();
    };
    match client
        .send_encrypted_to_device(&ToDeviceEventType::from(KEYS_EVENT), recipients, raw)
        .await
    {
        Ok(failures) => {
            let failed = |r: &Recipient| {
                failures.iter().any(|(user, devices)| {
                    user.as_str() == r.user_id && devices.iter().any(|d| d.as_str() == r.device_id)
                })
            };
            let missed = rollout.to.iter().filter(|r| failed(r)).count();
            if missed > 0 {
                tracing::warn!(missed, "call key not delivered to some devices; retrying");
            }
            rollout.to.iter().filter(|r| !failed(r)).cloned().collect()
        }
        Err(_) => {
            tracing::warn!("could not send the call key; retrying");
            Vec::new()
        }
    }
}

/// How often keys that did not reach every device are sent again.
const KEY_RETRY: Duration = Duration::from_secs(5);

/// Takes a member's call key from a decrypted to-device event. Keys sent
/// in clear, for another room, or claiming another device than the Olm
/// session's are ignored.
pub(super) fn receive_key(
    event: &Raw<AnyToDeviceEvent>,
    info: Option<&EncryptionInfo>,
    room_id: &str,
    provider: &KeyProvider,
) {
    let Ok(event) = event.deserialize_as::<Value>() else {
        return;
    };
    if event.get("type").and_then(Value::as_str) != Some(KEYS_EVENT) {
        return;
    }
    let Some(info) = info else {
        tracing::warn!("call key sent unencrypted; ignored");
        return;
    };
    let Some(key) = event.get("content").and_then(|content| {
        parse_key(
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
pub(super) async fn distribute_keys(
    client: Client,
    room_id: String,
    own_identity: String,
    mut planner: KeyPlanner,
    provider: KeyProvider,
    key_index: watch::Sender<i32>,
    mut members: watch::Receiver<Vec<Recipient>>,
) {
    let own = ParticipantIdentity(own_identity.clone());
    let mut retry = tokio::time::interval(KEY_RETRY);
    loop {
        let now = members.borrow_and_update().clone();
        if let Some(rollout) = planner.plan(&now, Instant::now(), random_key) {
            if !rollout.to.is_empty() {
                let got = send_key(&client, &room_id, &own_identity, &rollout, now_ms()).await;
                planner.shared(&got);
                if rollout.rotated {
                    tokio::time::sleep(USE_KEY_DELAY).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn r(user: &str, since: u64) -> Recipient {
        Recipient {
            user_id: user.into(),
            device_id: "D".into(),
            since,
        }
    }

    fn key(b: u8) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(vec![b; KEY_LEN])
    }

    #[test]
    fn joiners_get_a_young_key_and_an_old_one_rotates() {
        let t0 = Instant::now();
        let mut p = KeyPlanner::new(key(1), t0);
        let first = p.plan(&[r("@b:x", 1)], t0, || key(2)).unwrap();
        assert!(!first.rotated);
        assert_eq!((first.index, first.key[0]), (0, 1));
        assert_eq!(first.to, vec![r("@b:x", 1)]);
        p.shared(&first.to);
        assert!(
            p.plan(&[r("@b:x", 1)], t0, || key(2)).is_none(),
            "nothing new"
        );

        let young = t0 + Duration::from_secs(5);
        let carol = p
            .plan(&[r("@b:x", 1), r("@c:x", 2)], young, || key(2))
            .unwrap();
        assert!(!carol.rotated);
        assert_eq!(carol.to, vec![r("@c:x", 2)], "only the joiner, same key");
        p.shared(&carol.to);

        let old = t0 + Duration::from_secs(11);
        let dave = p
            .plan(&[r("@b:x", 1), r("@c:x", 2), r("@d:x", 3)], old, || key(3))
            .unwrap();
        assert!(dave.rotated);
        assert_eq!((dave.index, dave.key[0]), (1, 3));
        assert_eq!(dave.to.len(), 3, "a new key goes to everyone");
    }

    #[test]
    fn a_leave_rotates_and_a_rejoin_is_a_join() {
        let t0 = Instant::now();
        let mut p = KeyPlanner::new(key(1), t0);
        let both = [r("@b:x", 1), r("@c:x", 2)];
        let first = p.plan(&both, t0, || key(9)).unwrap();
        p.shared(&first.to);

        let left = p.plan(&[r("@b:x", 1)], t0, || key(2)).unwrap();
        assert!(
            left.rotated,
            "carol left: she must not decrypt what follows"
        );
        assert_eq!((left.index, left.to.clone()), (1, vec![r("@b:x", 1)]));
        p.shared(&left.to);

        // Bob rejoins from the same device (new membership) while the key is young.
        let rejoin = p.plan(&[r("@b:x", 5)], t0, || key(3)).unwrap();
        assert!(!rejoin.rotated);
        assert_eq!(rejoin.to, vec![r("@b:x", 5)]);
    }

    #[test]
    fn the_index_wraps_at_256() {
        let t0 = Instant::now();
        let mut p = KeyPlanner::new(key(1), t0);
        p.outbound.index = 255;
        p.outbound.shared_with = vec![r("@b:x", 1)];
        let next = p.plan(&[], t0, || key(2)).unwrap();
        assert!(next.rotated, "bob left: rotate even with nobody to send to");
        assert_eq!((next.index, next.to.len()), (0, 0));
        assert!(
            p.plan(&[], t0, || key(3)).is_none(),
            "nothing changed since"
        );
    }

    #[test]
    fn key_events_round_trip_as_element_call_sends_them() {
        let content = key_content("!r:x", &[7; 16], 3, "DEV", "@a:x:DEV", 42);
        assert_eq!(
            content["keys"]["key"], "BwcHBwcHBwcHBwcHBwcHBw==",
            "padded, as js-sdk"
        );
        assert_eq!(
            content["session"],
            json!({ "call_id": "", "application": "m.call", "scope": "m.room" })
        );
        let got = parse_key(&content, "@a:x", Some("DEV"), "!r:x").expect("valid");
        assert_eq!(
            (got.user_id.as_str(), got.device_id.as_str(), got.index),
            ("@a:x", "DEV", 3)
        );
        assert_eq!(got.key.as_slice(), &[7; 16]);
        // Unpadded keys parse too.
        let mut unpadded = content.clone();
        unpadded["keys"]["key"] = json!("BwcHBwcHBwcHBwcHBwcHBw");
        assert!(parse_key(&unpadded, "@a:x", Some("DEV"), "!r:x").is_some());
    }

    #[test]
    fn foreign_or_spoofed_key_events_are_refused() {
        let content = key_content("!r:x", &[7; 16], 3, "DEV", "@a:x:DEV", 42);
        assert!(
            parse_key(&content, "@a:x", Some("OTHER"), "!r:x").is_none(),
            "device mismatch"
        );
        assert!(
            parse_key(&content, "@a:x", Some("DEV"), "!other:x").is_none(),
            "other room"
        );
        let mut big = content.clone();
        big["keys"]["index"] = json!(256);
        assert!(parse_key(&big, "@a:x", Some("DEV"), "!r:x").is_none());
        let mut short = content;
        short["keys"]["key"] = json!("BwcH");
        assert!(parse_key(&short, "@a:x", Some("DEV"), "!r:x").is_none());
    }
}
