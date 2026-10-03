//! Ringing (MSC4075 call notifications, MSC4310 declines) in matrix-js-sdk
//! v43's format, as Element Call 0.26.1 sends them:
//!
//! - the first member to join sends `org.matrix.msc4075.rtc.notification`
//!   (`ring` in a DM, `notification` elsewhere), mentioning the room and
//!   referencing its membership event, valid for 90 s;
//! - a callee declines with `org.matrix.msc4310.rtc.decline`, referencing
//!   the notification.
//!
//! Receiving follows js-sdk's `parseCallNotificationContent` rules: not our
//! own, not expired, mentioning us (or the room, if the sender may notify
//! it), and we are not already in the call. One difference: js-sdk also
//! requires an open `org.matrix.msc4143.rtc.slot` state event, which
//! Element Call 0.26.1 never creates for `m.call.member` calls, so a missing
//! slot event counts as open here (a closed one still stops the ring).
//! Sticky events (MSC4354) are not used: notifications go out as regular
//! events, js-sdk's fallback.

use serde_json::{Value, json};

/// The notification event type.
pub(crate) const NOTIFICATION_EVENT: &str = "org.matrix.msc4075.rtc.notification";
/// The decline event type.
pub(crate) const DECLINE_EVENT: &str = "org.matrix.msc4310.rtc.decline";
/// The slot state event type (MSC4143).
pub(crate) const SLOT_EVENT: &str = "org.matrix.msc4143.rtc.slot";
/// The room call's slot ID (`m.call` application, `ROOM` id).
pub(crate) const SLOT_ID: &str = "m.call#ROOM";
/// How long our notification rings (js-sdk `notificationLifetimeMs`).
const LIFETIME_MS: u64 = 90_000;
/// The longest lifetime a receiver honours (MSC4075).
pub(crate) const MAX_LIFETIME_MS: u64 = 120_000;
/// How far `sender_ts` may be ahead of the server's timestamp before the
/// server's counts instead.
const MAX_SENDER_TS_AHEAD_MS: u64 = 20_000;

/// Our notification for a call we started, referencing our membership.
pub(crate) fn notification_content(membership_event_id: &str, ring: bool, sender_ts: u64) -> Value {
    json!({
        "slot_id": SLOT_ID,
        "m.mentions": { "user_ids": [], "room": true },
        "notification_type": if ring { "ring" } else { "notification" },
        "m.relates_to": { "event_id": membership_event_id, "rel_type": "m.reference" },
        "sender_ts": sender_ts,
        "lifetime": LIFETIME_MS,
        "msc4354_sticky_key": SLOT_ID,
        "m.call.intent": "audio",
    })
}

/// Our decline of `notification_event_id`.
pub(crate) fn decline_content(notification_event_id: &str) -> Value {
    json!({
        "m.relates_to": { "event_id": notification_event_id, "rel_type": "m.reference" },
        "msc4354_sticky_key": notification_event_id,
    })
}

/// When a notification stops ringing, ms since the epoch (js-sdk
/// `getCallNotificationExpiry`).
pub(crate) fn expiry_ms(
    sender_ts: u64,
    lifetime_ms: u64,
    origin_server_ts: u64,
    now_ms: u64,
) -> u64 {
    let basis = if sender_ts.saturating_sub(origin_server_ts) > MAX_SENDER_TS_AHEAD_MS {
        origin_server_ts
    } else {
        sender_ts
    };
    basis
        .saturating_add(lifetime_ms.min(MAX_LIFETIME_MS))
        .min(now_ms.saturating_add(MAX_LIFETIME_MS))
}

/// A valid incoming notification.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Incoming {
    /// `ring` rather than a quiet `notification`.
    pub ring: bool,
    /// When it stops ringing, ms since the epoch.
    pub expires_ms: u64,
}

/// What the receiving rules need besides the event.
pub(crate) struct Callee<'a> {
    pub own_user: &'a str,
    pub now_ms: u64,
    /// The sender may notify the whole room (power levels).
    pub sender_may_notify_room: bool,
}

/// Checks a notification against the receiving rules that need no room
/// state beyond the sender's power. Being in the call already, a decline,
/// and the slot are checked by the caller.
pub(crate) fn parse_notification(
    sender: &str,
    origin_server_ts: u64,
    content: &Value,
    rx: &Callee<'_>,
) -> Option<Incoming> {
    if sender == rx.own_user {
        return None;
    }
    // A withdrawn sticky notification is empty but for its key.
    let object = content.as_object()?;
    if object.keys().all(|k| k == "msc4354_sticky_key") {
        return None;
    }
    let slot = content.get("slot_id").and_then(Value::as_str);
    if slot.is_some_and(|s| s != SLOT_ID) {
        return None;
    }
    if slot.is_none() && content.get("m.relates_to").is_none() {
        return None;
    }
    if content
        .get("msc4354_sticky_key")
        .is_some_and(|k| k.as_str() != Some(slot.unwrap_or(SLOT_ID)))
    {
        return None;
    }
    let sender_ts = content.get("sender_ts")?.as_u64()?;
    let lifetime = content.get("lifetime")?.as_u64()?;
    let mentions = content.get("m.mentions")?.as_object()?;
    let room = match mentions.get("room") {
        None => false,
        Some(v) => v.as_bool()?,
    };
    let users: Vec<&str> = match mentions.get("user_ids") {
        None => Vec::new(),
        Some(v) => v
            .as_array()?
            .iter()
            .map(Value::as_str)
            .collect::<Option<Vec<_>>>()?,
    };
    if !room && users.is_empty() {
        return None;
    }
    let mentioned = users.contains(&rx.own_user) || (room && rx.sender_may_notify_room);
    if !mentioned {
        return None;
    }
    let expires_ms = expiry_ms(sender_ts, lifetime, origin_server_ts, rx.now_ms);
    if rx.now_ms >= expires_ms {
        return None;
    }
    Some(Incoming {
        ring: content.get("notification_type").and_then(Value::as_str) == Some("ring"),
        expires_ms,
    })
}

/// Whether a slot event's content leaves the call slot open. `None` (no
/// slot event) counts as open, unlike js-sdk; see the module docs.
pub(crate) fn slot_open(content: Option<&Value>) -> bool {
    let Some(content) = content else {
        return true;
    };
    content.get("status").and_then(Value::as_str) == Some("open")
        && content
            .get("application")
            .and_then(|a| a.get("type"))
            .and_then(Value::as_str)
            == Some("m.call")
}

/// The notification a decline answers, if it is one.
pub(crate) fn declined_notification(content: &Value) -> Option<&str> {
    let relation = content.get("m.relates_to")?;
    if relation.get("rel_type")?.as_str()? != "m.reference" {
        return None;
    }
    relation.get("event_id")?.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn rx(may_notify: bool) -> Callee<'static> {
        Callee {
            own_user: "@bob:x",
            now_ms: NOW,
            sender_may_notify_room: may_notify,
        }
    }

    #[test]
    fn our_notification_matches_element_call() {
        let c = notification_content("$member", true, 5);
        assert_eq!(
            c,
            json!({
                "slot_id": "m.call#ROOM",
                "m.mentions": { "user_ids": [], "room": true },
                "notification_type": "ring",
                "m.relates_to": { "event_id": "$member", "rel_type": "m.reference" },
                "sender_ts": 5,
                "lifetime": 90_000,
                "msc4354_sticky_key": "m.call#ROOM",
                "m.call.intent": "audio",
            })
        );
        assert_eq!(
            notification_content("$m", false, 5)["notification_type"],
            "notification"
        );
        assert_eq!(
            decline_content("$n"),
            json!({ "m.relates_to": { "event_id": "$n", "rel_type": "m.reference" }, "msc4354_sticky_key": "$n" })
        );
        assert_eq!(declined_notification(&decline_content("$n")), Some("$n"));
    }

    #[test]
    fn a_fresh_ring_from_someone_who_may_notify_the_room_rings() {
        let c = notification_content("$m", true, NOW - 1_000);
        let got = parse_notification("@alice:x", NOW - 1_000, &c, &rx(true)).unwrap();
        assert!(got.ring);
        assert_eq!(got.expires_ms, NOW - 1_000 + 90_000);
    }

    #[test]
    fn rings_that_must_not_ring() {
        let c = notification_content("$m", true, NOW - 1_000);
        assert!(
            parse_notification("@bob:x", NOW, &c, &rx(true)).is_none(),
            "our own"
        );
        assert!(
            parse_notification("@alice:x", NOW, &c, &rx(false)).is_none(),
            "may not notify the room"
        );
        let old = notification_content("$m", true, NOW - 91_000);
        assert!(
            parse_notification("@alice:x", NOW - 91_000, &old, &rx(true)).is_none(),
            "expired"
        );
        let mut other_slot = c.clone();
        other_slot["slot_id"] = json!("m.call#other");
        assert!(parse_notification("@alice:x", NOW, &other_slot, &rx(true)).is_none());
        let mut nobody = c.clone();
        nobody["m.mentions"] = json!({ "user_ids": [] });
        assert!(parse_notification("@alice:x", NOW, &nobody, &rx(true)).is_none());
        assert!(
            parse_notification(
                "@alice:x",
                NOW,
                &json!({ "msc4354_sticky_key": "m.call#ROOM" }),
                &rx(true)
            )
            .is_none(),
            "withdrawn"
        );
        let mut me = c;
        me["m.mentions"] = json!({ "user_ids": ["@bob:x"] });
        assert!(
            parse_notification("@alice:x", NOW, &me, &rx(false)).is_some(),
            "mentioned by name"
        );
    }

    #[test]
    fn expiry_is_capped_and_ignores_a_sender_clock_far_ahead() {
        assert_eq!(
            expiry_ms(100, 600_000, 100, 100),
            100 + 120_000,
            "lifetime capped at 2 min"
        );
        assert_eq!(
            expiry_ms(100_000, 1_000, 50_000, 0),
            50_000 + 1_000,
            "sender 50 s ahead: server time"
        );
        assert_eq!(
            expiry_ms(60_000, 1_000, 50_000, 0),
            61_000,
            "10 s ahead is fine"
        );
    }

    #[test]
    fn a_missing_slot_is_open_and_a_closed_one_is_not() {
        assert!(slot_open(None));
        assert!(slot_open(Some(
            &json!({ "status": "open", "application": { "type": "m.call" } })
        )));
        assert!(!slot_open(Some(
            &json!({ "status": "closed", "application": { "type": "m.call" } })
        )));
        assert!(!slot_open(Some(&json!({}))));
    }
}
