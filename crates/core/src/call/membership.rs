//! MatrixRTC call memberships in Element Call's "compatibility" format
//! (Element Call 0.26 default; matrix-js-sdk v43 `MembershipManager` for
//! session events): one `org.matrix.msc3401.call.member` state event per
//! device, with a delayed empty event as crash-safe leave.
//!
//! Built and parsed as plain JSON because the content carries
//! `membershipID`, which ruma's `SessionMembershipData` does not model.

use std::time::Duration;

use serde_json::{Value, json};

/// The state event type (MSC3401, unstable prefix, as Element sends it).
pub(crate) const CALL_MEMBER_EVENT: &str = "org.matrix.msc3401.call.member";

/// Delay of the crash-safe leave event, and how often it is restarted
/// (Element Call `delayed_leave` defaults).
pub(crate) const DELAYED_LEAVE: Duration = Duration::from_secs(18);
pub(crate) const DELAYED_LEAVE_RESTART: Duration = Duration::from_secs(4);

/// Validity of a membership, counted from when it was created
/// (js-sdk `DEFAULT_EXPIRE_DURATION`, 4 h); refreshed before it runs out.
pub(crate) const MEMBERSHIP_EXPIRES: Duration = Duration::from_secs(4 * 3600);

/// The state key of `user_id`'s membership on `device_id` for the room call:
/// `_{user}_{device}_m.call`, without the leading `_` in room versions
/// (MSC3757/MSC3779) that let state keys starting with a user ID be owned
/// by that user.
pub(crate) fn state_key(user_id: &str, device_id: &str, room_version: &str) -> String {
    let key = format!("{user_id}_{device_id}_m.call");
    if room_version.starts_with("org.matrix.msc3757")
        || room_version.starts_with("org.matrix.msc3779")
    {
        key
    } else {
        format!("_{key}")
    }
}

/// The member ID (and, for this format, the LiveKit identity) of a device.
pub(crate) fn member_id(user_id: &str, device_id: &str) -> String {
    format!("{user_id}:{device_id}")
}

/// Our join content. `created_ts` is copied from the first join when the
/// membership is refreshed; `expires_ms` grows with each refresh.
pub(crate) fn join_content(
    user_id: &str,
    device_id: &str,
    livekit_service_url: &str,
    room_id: &str,
    created_ts: Option<u64>,
    expires: Duration,
) -> Value {
    let mut content = json!({
        "application": "m.call",
        "call_id": "",
        "scope": "m.room",
        "device_id": device_id,
        "membershipID": member_id(user_id, device_id),
        "expires": u64::try_from(expires.as_millis()).unwrap_or(u64::MAX),
        "m.call.intent": "audio",
        "focus_active": { "type": "livekit", "focus_selection": "multi_sfu" },
        "foci_preferred": [{
            "type": "livekit",
            "livekit_service_url": livekit_service_url,
            "livekit_alias": room_id,
        }],
    });
    if let Some(ts) = created_ts {
        content["created_ts"] = json!(ts);
    }
    content
}

/// One device's membership, as read from the room state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Membership {
    pub user_id: String,
    pub device_id: String,
    /// `membershipID`, or `user:device` when absent; also the LiveKit
    /// identity of this device.
    pub member_id: String,
    /// The SFU this device publishes to (`foci_preferred[0]`), if any.
    pub livekit_service_url: Option<String>,
    /// When the membership started, ms since the epoch.
    pub joined_ms: u64,
    /// Validity from `joined_ms`.
    pub expires_ms: u64,
}

impl Membership {
    /// Still valid at `now_ms`.
    pub(crate) fn is_active(&self, now_ms: u64) -> bool {
        self.joined_ms.saturating_add(self.expires_ms) > now_ms
    }
}

/// Parses one state event's fields. `None` for left (empty) memberships,
/// other applications or calls, malformed content, and state keys not owned
/// by `sender` (another user cannot announce a membership for someone else).
pub(crate) fn parse(
    sender: &str,
    state_key: &str,
    origin_server_ts: u64,
    content: &Value,
) -> Option<Membership> {
    let key = state_key.strip_prefix('_').unwrap_or(state_key);
    let rest = key.strip_prefix(sender)?.strip_prefix('_')?;
    let device_id = content.get("device_id")?.as_str()?;
    if rest != format!("{device_id}_m.call") {
        return None;
    }
    if content.get("application")?.as_str()? != "m.call"
        || !content.get("call_id")?.as_str()?.is_empty()
        || content
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("m.room")
            != "m.room"
    {
        return None;
    }
    let expires_ms = content.get("expires")?.as_u64()?;
    let created = content.get("created_ts").and_then(Value::as_u64);
    let joined_ms = created.map_or(origin_server_ts, |c| c.min(origin_server_ts));
    let member_id = content
        .get("membershipID")
        .and_then(Value::as_str)
        .map_or_else(|| member_id(sender, device_id), str::to_owned);
    let livekit_service_url = content
        .get("foci_preferred")
        .and_then(Value::as_array)
        .and_then(|foci| foci.first())
        .filter(|f| f.get("type").and_then(Value::as_str) == Some("livekit"))
        .and_then(|f| f.get("livekit_service_url"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(Membership {
        user_id: sender.to_owned(),
        device_id: device_id.to_owned(),
        member_id,
        livekit_service_url,
        joined_ms,
        expires_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "@alice:example.org";

    #[test]
    fn state_keys_follow_the_room_version() {
        assert_eq!(
            state_key(ALICE, "DEV", "11"),
            "_@alice:example.org_DEV_m.call"
        );
        assert_eq!(
            state_key(ALICE, "DEV", "org.matrix.msc3757.11"),
            "@alice:example.org_DEV_m.call"
        );
    }

    #[test]
    fn join_content_matches_element_call_compatibility_mode() {
        let c = join_content(
            ALICE,
            "DEV",
            "https://rtc.example.org/livekit/jwt",
            "!room:example.org",
            None,
            MEMBERSHIP_EXPIRES,
        );
        assert_eq!(
            c,
            json!({
                "application": "m.call",
                "call_id": "",
                "scope": "m.room",
                "device_id": "DEV",
                "membershipID": "@alice:example.org:DEV",
                "expires": 14_400_000,
                "m.call.intent": "audio",
                "focus_active": { "type": "livekit", "focus_selection": "multi_sfu" },
                "foci_preferred": [{
                    "type": "livekit",
                    "livekit_service_url": "https://rtc.example.org/livekit/jwt",
                    "livekit_alias": "!room:example.org",
                }],
            })
        );
        let refreshed = join_content(ALICE, "DEV", "u", "!r:x", Some(5), MEMBERSHIP_EXPIRES * 2);
        assert_eq!(refreshed["created_ts"], 5);
        assert_eq!(refreshed["expires"], 28_800_000);
    }

    #[test]
    fn our_own_content_parses_back() {
        let c = join_content(
            ALICE,
            "DEV",
            "https://sfu",
            "!r:x",
            None,
            MEMBERSHIP_EXPIRES,
        );
        let key = state_key(ALICE, "DEV", "11");
        let m = parse(ALICE, &key, 1_000, &c).expect("valid");
        assert_eq!(m.member_id, "@alice:example.org:DEV");
        assert_eq!(m.livekit_service_url.as_deref(), Some("https://sfu"));
        assert_eq!(m.joined_ms, 1_000);
        assert!(m.is_active(1_000 + 14_399_999));
        assert!(!m.is_active(1_000 + 14_400_000));
    }

    #[test]
    fn element_call_membership_without_membership_id_uses_user_and_device() {
        // Shape of an older Element Call membership (no membershipID,
        // oldest_membership focus selection), created before it was sent.
        let c = json!({
            "application": "m.call", "call_id": "", "scope": "m.room",
            "device_id": "EC", "expires": 14_400_000, "created_ts": 500,
            "focus_active": { "type": "livekit", "focus_selection": "oldest_membership" },
            "foci_preferred": [{ "type": "livekit", "livekit_service_url": "https://a", "livekit_alias": "!r:x" }],
        });
        let m = parse(ALICE, "_@alice:example.org_EC_m.call", 900, &c).unwrap();
        assert_eq!(m.member_id, "@alice:example.org:EC");
        assert_eq!(m.joined_ms, 500, "min(created_ts, origin_server_ts)");
    }

    #[test]
    fn left_foreign_or_spoofed_memberships_are_ignored() {
        let good = join_content(ALICE, "DEV", "u", "!r:x", None, MEMBERSHIP_EXPIRES);
        let key = state_key(ALICE, "DEV", "11");
        assert!(parse(ALICE, &key, 0, &json!({})).is_none(), "left");
        assert!(
            parse("@mallory:x", &key, 0, &good).is_none(),
            "key owned by someone else"
        );
        assert!(
            parse(ALICE, "_@alice:example.org_OTHER_m.call", 0, &good).is_none(),
            "device does not match the key"
        );
        let mut other_call = good.clone();
        other_call["call_id"] = json!("abc");
        assert!(parse(ALICE, &key, 0, &other_call).is_none());
        let mut other_app = good.clone();
        other_app["application"] = json!("io.example.whiteboard");
        assert!(parse(ALICE, &key, 0, &other_app).is_none());
        let mut no_expiry = good;
        no_expiry.as_object_mut().unwrap().remove("expires");
        assert!(parse(ALICE, &key, 0, &no_expiry).is_none());
    }
}
