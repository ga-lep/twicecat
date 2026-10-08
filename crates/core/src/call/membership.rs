//! MatrixRTC call memberships in Element Call's "compatibility" format
//! (Element Call 0.26 default; matrix-js-sdk v43 `MembershipManager` for
//! session events): one `org.matrix.msc3401.call.member` state event per
//! device, with a delayed empty event as crash-safe leave.
//!
//! Built and parsed as plain JSON because the content carries
//! `membershipID`, which ruma's `SessionMembershipData` does not model.

use std::collections::{BTreeMap, BTreeSet};
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

/// How a member picks its SFU (`focus_active.focus_selection`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusSelection {
    /// Its own `foci_preferred[0]` (Element Call's default).
    MultiSfu,
    /// The oldest membership's SFU (older Element Call versions).
    OldestMembership,
}

/// One device's membership, as read from the room state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Membership {
    pub user_id: String,
    pub device_id: String,
    /// `membershipID`, or `user:device` when absent.
    pub member_id: String,
    /// The LiveKit identity of this device: `sender:device_id`, as
    /// matrix-js-sdk's `CallMembership.rtcBackendIdentity` for these
    /// (session) memberships, whatever `membershipID` says.
    pub backend_identity: String,
    /// `foci_preferred[0]` when it is a LiveKit focus.
    pub livekit_service_url: Option<String>,
    /// `None` when `focus_active` is missing or not understood: such a
    /// member has no SFU we can find (as in Element Call).
    pub focus: Option<FocusSelection>,
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
    let focus = content
        .get("focus_active")
        .filter(|f| f.get("type").and_then(Value::as_str) == Some("livekit"))
        .and_then(|f| f.get("focus_selection"))
        .and_then(Value::as_str)
        .and_then(|s| match s {
            "multi_sfu" => Some(FocusSelection::MultiSfu),
            "oldest_membership" => Some(FocusSelection::OldestMembership),
            _ => None,
        });
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
        backend_identity: format!("{sender}:{device_id}"),
        livekit_service_url,
        focus,
        joined_ms,
        expires_ms,
    })
}

/// The SFU (call service URL) `member` publishes to, as matrix-js-sdk's
/// `CallMembership.getTransport`: its own `foci_preferred[0]` in multi-SFU
/// mode, the oldest membership's in `oldest_membership` mode. `oldest` is
/// the active membership that joined first.
pub(crate) fn transport_of<'a>(member: &'a Membership, oldest: &'a Membership) -> Option<&'a str> {
    match member.focus? {
        FocusSelection::MultiSfu => member.livekit_service_url.as_deref(),
        FocusSelection::OldestMembership if member == oldest => {
            member.livekit_service_url.as_deref()
        }
        FocusSelection::OldestMembership => transport_of(oldest, oldest),
    }
}

/// For each SFU, the LiveKit identities of the members publishing there,
/// for every active member but `own_identity` (`active` oldest first). Our
/// own service is always present, maybe empty: we publish there.
pub(crate) fn expected_by_sfu(
    active: &[Membership],
    own_identity: &str,
    own_service: &str,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out = BTreeMap::from([(own_service.to_owned(), BTreeSet::new())]);
    let Some(oldest) = active.first() else {
        return out;
    };
    for m in active {
        if m.backend_identity == own_identity {
            continue;
        }
        if let Some(url) = transport_of(m, oldest) {
            out.entry(url.to_owned())
                .or_default()
                .insert(m.backend_identity.clone());
        }
    }
    out
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

    fn member(user: &str, url: &str, focus: Option<FocusSelection>) -> Membership {
        Membership {
            user_id: user.into(),
            device_id: "D".into(),
            member_id: format!("{user}:D"),
            backend_identity: format!("{user}:D"),
            livekit_service_url: Some(url.into()),
            focus,
            joined_ms: 0,
            expires_ms: 1,
        }
    }

    #[test]
    fn transports_follow_the_focus_selection() {
        use FocusSelection::{MultiSfu, OldestMembership};
        let first = member("@a:x", "https://a", Some(OldestMembership));
        let multi = member("@b:y", "https://b", Some(MultiSfu));
        let follower = member("@c:z", "https://c", Some(OldestMembership));
        let unknown = member("@d:w", "https://d", None);
        assert_eq!(transport_of(&first, &first), Some("https://a"));
        assert_eq!(transport_of(&multi, &first), Some("https://b"));
        assert_eq!(transport_of(&follower, &first), Some("https://a"));
        assert_eq!(transport_of(&unknown, &first), None);
        // A follower of a multi-SFU oldest member uses that member's SFU.
        assert_eq!(transport_of(&follower, &multi), Some("https://b"));
    }

    #[test]
    fn members_are_expected_on_their_sfu_but_not_us() {
        use FocusSelection::MultiSfu;
        let us = member("@us:x", "https://own", Some(MultiSfu));
        let near = member("@near:x", "https://own", Some(MultiSfu));
        let far = member("@far:y", "https://far", Some(MultiSfu));
        let lost = member("@lost:z", "https://lost", None);
        let got = expected_by_sfu(&[us, near, far, lost], "@us:x:D", "https://own");
        let ids = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
        assert_eq!(
            got,
            BTreeMap::from([
                ("https://far".to_owned(), ids(&["@far:y:D"])),
                ("https://own".to_owned(), ids(&["@near:x:D"])),
            ])
        );
        assert_eq!(
            expected_by_sfu(&[], "@us:x:D", "https://own"),
            BTreeMap::from([("https://own".to_owned(), BTreeSet::new())])
        );
    }

    #[test]
    fn the_livekit_identity_is_sender_and_device_whatever_membership_id_says() {
        let mut c = join_content(
            ALICE,
            "DEV",
            "https://sfu",
            "!r:x",
            None,
            MEMBERSHIP_EXPIRES,
        );
        c["membershipID"] = json!("a-random-uuid");
        let m = parse(ALICE, &state_key(ALICE, "DEV", "11"), 0, &c).unwrap();
        assert_eq!(m.member_id, "a-random-uuid");
        assert_eq!(m.backend_identity, "@alice:example.org:DEV");
        assert_eq!(m.focus, Some(FocusSelection::MultiSfu));
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
