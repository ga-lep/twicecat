//! Mapping of the SDK's room list diffs to the public [`ListDiff`].

use std::collections::HashSet;

use matrix_sdk::ruma::OwnedMxcUri;
use matrix_sdk::{Room, RoomState};
use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::room_list_service::filters::{
    BoxedFilterFn, RoomCategory, new_filter_all, new_filter_category, new_filter_non_left,
    new_filter_normalized_match_room_name, new_filter_not, new_filter_space,
};
use matrix_sdk_ui::room_list_service::{RoomListItem, RoomListLoadingState};
use matrix_sdk_ui::timeline::{LatestEventValue, RoomExt, TimelineDetails};
use tracing::warn;

use crate::security;

use crate::api::{
    AvatarId, CoreEvent, ListDiff, MessagePreview, RoomId, RoomListQuery, RoomListTab, RoomSummary,
    UserId,
};

/// Maps one SDK diff, converting each carried item with `f`.
pub(crate) fn map_diff<T: Clone, U>(diff: VectorDiff<T>, mut f: impl FnMut(T) -> U) -> ListDiff<U> {
    match diff {
        VectorDiff::Append { values } => ListDiff::Append {
            values: values.into_iter().map(f).collect(),
        },
        VectorDiff::Clear => ListDiff::Clear,
        VectorDiff::PushFront { value } => ListDiff::PushFront { value: f(value) },
        VectorDiff::PushBack { value } => ListDiff::PushBack { value: f(value) },
        VectorDiff::PopFront => ListDiff::PopFront,
        VectorDiff::PopBack => ListDiff::PopBack,
        VectorDiff::Insert { index, value } => ListDiff::Insert {
            index,
            value: f(value),
        },
        VectorDiff::Set { index, value } => ListDiff::Set {
            index,
            value: f(value),
        },
        VectorDiff::Remove { index } => ListDiff::Remove { index },
        VectorDiff::Truncate { length } => ListDiff::Truncate { length },
        VectorDiff::Reset { values } => ListDiff::Reset {
            values: values.into_iter().map(f).collect(),
        },
    }
}

/// One building block of a room list filter. The filter of a query is the
/// conjunction of its parts, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilterPart {
    /// Rooms that were not left.
    NonLeft,
    /// Direct messages (`RoomCategory::People`).
    People,
    /// Group rooms (`RoomCategory::Group`).
    Group,
    /// Spaces.
    Space,
    /// Anything but spaces.
    NotSpace,
    /// Case- and diacritic-insensitive substring of the room name. Holds the
    /// trimmed, non-empty search text.
    NameMatch(String),
}

/// The filter parts of `query`: its tab, then a name match if the trimmed
/// search is not empty.
pub(crate) fn filter_spec(query: &RoomListQuery) -> Vec<FilterPart> {
    let mut parts = vec![FilterPart::NonLeft];
    match query.tab {
        RoomListTab::All => {}
        RoomListTab::People => parts.extend([FilterPart::People, FilterPart::NotSpace]),
        RoomListTab::Rooms => parts.extend([FilterPart::Group, FilterPart::NotSpace]),
        RoomListTab::Spaces => parts.push(FilterPart::Space),
    }
    let search = query.search.trim();
    if !search.is_empty() {
        parts.push(FilterPart::NameMatch(search.to_owned()));
    }
    parts
}

/// Builds the SDK filter for `query` from the SDK's own filters.
pub(crate) fn build_filter(query: &RoomListQuery) -> BoxedFilterFn {
    let filters = filter_spec(query)
        .into_iter()
        .map(|part| -> BoxedFilterFn {
            match part {
                FilterPart::NonLeft => Box::new(new_filter_non_left()),
                FilterPart::People => Box::new(new_filter_category(RoomCategory::People)),
                FilterPart::Group => Box::new(new_filter_category(RoomCategory::Group)),
                FilterPart::Space => Box::new(new_filter_space()),
                FilterPart::NotSpace => Box::new(new_filter_not(Box::new(new_filter_space()))),
                FilterPart::NameMatch(text) => {
                    Box::new(new_filter_normalized_match_room_name(&text))
                }
            }
        })
        .collect();
    Box::new(new_filter_all(filters))
}

/// Longest preview body, in characters.
const PREVIEW_MAX_CHARS: usize = 120;

/// Highlight count of a room: the larger of the client-side mention count and
/// the server's highlight count. Client-side counts can lag in encrypted
/// rooms and server counts can be imprecise there, so the maximum never hides
/// a mention.
pub(crate) fn highlights_of(mentions: u64, server_highlights: u64) -> u64 {
    mentions.max(server_highlights)
}

/// Whether `c` could reorder or disguise the text around it (bidirectional
/// controls) and so must not reach a one-line preview.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Display form of a message body for the room list: the first line only,
/// with control and bidirectional characters removed, at most
/// [`PREVIEW_MAX_CHARS`] characters (cut on a character boundary).
///
/// The body is untrusted user content: never log the result.
pub(crate) fn preview_body(raw: &str) -> String {
    raw.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(PREVIEW_MAX_CHARS)
        .collect()
}

/// Picks the avatar of a room: its own, else (for a DM with exactly one hero)
/// that hero's. Only valid MXC URIs are kept.
pub(crate) fn pick_avatar(
    room_avatar: Option<&str>,
    is_dm: bool,
    hero_avatars: &[Option<&str>],
) -> Option<AvatarId> {
    let valid = |uri: &str| {
        OwnedMxcUri::from(uri)
            .is_valid()
            .then(|| AvatarId::from(uri))
    };
    if let Some(found) = room_avatar.and_then(valid) {
        return Some(found);
    }
    match hero_avatars {
        [only] if is_dm => only.and_then(valid),
        _ => None,
    }
}

/// Room IDs already registered with the latest-events system.
#[derive(Default)]
pub(crate) struct Registered(HashSet<RoomId>);

impl Registered {
    /// True the first time `id` is seen (the caller should register it).
    pub(crate) fn first_sight(&mut self, id: &RoomId) -> bool {
        self.0.insert(id.clone())
    }

    /// Forgets `id`, so that the next sighting registers it again.
    pub(crate) fn forget(&mut self, id: &RoomId) {
        self.0.remove(id);
    }
}

/// The event for a loading state, if it carries news: only `Loaded` does.
pub(crate) fn loaded_event(state: &RoomListLoadingState) -> Option<CoreEvent> {
    match state {
        RoomListLoadingState::NotLoaded => None,
        RoomListLoadingState::Loaded {
            maximum_number_of_rooms,
        } => Some(CoreEvent::RoomListLoaded {
            total_rooms: *maximum_number_of_rooms,
        }),
    }
}

/// The items a diff carries, mutably.
pub(crate) fn items_mut<T>(diff: &mut ListDiff<T>) -> &mut [T] {
    match diff {
        ListDiff::Append { values } | ListDiff::Reset { values } => values,
        ListDiff::PushFront { value }
        | ListDiff::PushBack { value }
        | ListDiff::Insert { value, .. }
        | ListDiff::Set { value, .. } => std::slice::from_mut(value),
        ListDiff::Clear
        | ListDiff::PopFront
        | ListDiff::PopBack
        | ListDiff::Remove { .. }
        | ListDiff::Truncate { .. } => &mut [],
    }
}

/// Builds the summary of a room, without the last-message preview (that needs
/// an async lookup, see [`map_batch`]).
///
/// The name comes from the server and is untrusted display data.
pub(crate) fn summarize(item: &RoomListItem) -> RoomSummary {
    let id = RoomId::from(item.room_id().to_string());
    let name = name_or_id(item.cached_display_name().map(|n| n.to_string()), &id);
    let is_dm = item.is_dm();
    let room_avatar = item.avatar_url();
    let heroes = item.clone_info();
    let hero_avatars: Vec<Option<&str>> = heroes
        .heroes()
        .iter()
        .map(|h| h.avatar_url.as_deref().map(|u| u.as_str()))
        .collect();
    let mentions = item.num_unread_mentions();
    RoomSummary {
        unread_messages: item.num_unread_messages(),
        unread_mentions: mentions,
        unread_notifications: item.num_unread_notifications(),
        avatar: pick_avatar(
            room_avatar.as_deref().map(|u| u.as_str()),
            is_dm,
            &hero_avatars,
        ),
        is_dm,
        is_encrypted: item.encryption_state().is_encrypted(),
        is_space: item.is_space(),
        is_call_room: item.is_call(),
        is_invite: item.state() == RoomState::Invited,
        highlights: highlights_of(mentions, item.unread_notification_counts().highlight_count),
        marked_unread: item.is_marked_unread(),
        dm_peer: peer_field(security::dm_peer_of(item, item.client().user_id())),
        ..RoomSummary::new(id, name)
    }
}

/// The public form of a DM peer: the one other user the core watches the
/// trust of (see `security::dm_peer_of`).
pub(crate) fn peer_field(peer: Option<matrix_sdk::ruma::OwnedUserId>) -> Option<UserId> {
    peer.map(|peer| UserId::from(peer.as_str()))
}

/// The preview of a room's latest message, if it is a message.
///
/// Invites and non-message content (state events, redactions, ...) give
/// `None`.
async fn last_message(room: &Room) -> Option<MessagePreview> {
    let (timestamp, sender, is_own, profile, content) = match RoomExt::latest_event(room).await {
        LatestEventValue::Remote {
            timestamp,
            sender,
            is_own,
            profile,
            content,
        } => (timestamp, sender, is_own, profile, content),
        LatestEventValue::Local {
            timestamp,
            sender,
            profile,
            content,
            ..
        } => (timestamp, sender, true, profile, content),
        LatestEventValue::None | LatestEventValue::RemoteInvite { .. } => return None,
    };
    let message = content.as_message()?;
    let sender_name = match profile {
        TimelineDetails::Ready(profile) => profile.display_name,
        _ => None,
    };
    Some(MessagePreview {
        sender: UserId::from(sender.to_string()),
        sender_name,
        body: preview_body(message.body()),
        timestamp_ms: u64::from(timestamp.get()),
        is_own,
    })
}

/// Converts one batch of SDK diffs, registering each new room with the
/// latest-events system and attaching last-message previews.
///
/// Latest events are lazy: nothing is computed for a room until it is
/// registered, so a fresh room has no preview yet. Once computed, the
/// room-info update reaches the list as a `Set` diff.
pub(crate) async fn map_batch(
    diffs: Vec<VectorDiff<RoomListItem>>,
    registered: &mut Registered,
) -> Vec<ListDiff<RoomSummary>> {
    let mut out = Vec::with_capacity(diffs.len());
    for diff in diffs {
        let mut rooms = Vec::new();
        let mut mapped = map_diff(diff, |item| {
            let summary = summarize(&item);
            rooms.push(item);
            summary
        });
        for room in &rooms {
            let id = room.room_id();
            if registered.first_sight(&RoomId::from(id.to_string())) {
                let client = room.client();
                let latest = client.latest_events().await;
                if latest.listen_to_room(id).await.is_err() {
                    registered.forget(&RoomId::from(id.to_string()));
                    warn!("could not register a room for latest events");
                }
            }
        }
        for (summary, room) in items_mut(&mut mapped).iter_mut().zip(&rooms) {
            summary.last_message = last_message(room).await;
        }
        out.push(mapped);
    }
    out
}

/// Display name, or the room ID when the room has no cached name.
pub(crate) fn name_or_id(name: Option<String>, id: &RoomId) -> String {
    name.unwrap_or_else(|| id.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use matrix_sdk_ui::eyeball_im::VectorDiff as V;

    use super::*;

    use crate::api::{RoomListQuery, RoomListTab};

    #[test]
    fn the_dm_peer_field_carries_the_core_peer() {
        let bob = matrix_sdk::ruma::user_id!("@bob:x").to_owned();
        assert_eq!(peer_field(Some(bob)), Some(UserId::from("@bob:x")));
        assert_eq!(peer_field(None), None);
        assert_eq!(
            RoomSummary::new(RoomId::from("!r:x"), String::new()).dm_peer,
            None
        );
    }

    fn q(tab: RoomListTab, search: &str) -> RoomListQuery {
        RoomListQuery {
            tab,
            search: search.to_owned(),
        }
    }

    #[test]
    fn filter_spec_follows_the_tab_table() {
        use FilterPart::*;
        assert_eq!(filter_spec(&q(RoomListTab::All, "")), vec![NonLeft]);
        assert_eq!(
            filter_spec(&q(RoomListTab::People, "")),
            vec![NonLeft, People, NotSpace]
        );
        assert_eq!(
            filter_spec(&q(RoomListTab::Rooms, "")),
            vec![NonLeft, Group, NotSpace]
        );
        assert_eq!(
            filter_spec(&q(RoomListTab::Spaces, "")),
            vec![NonLeft, Space]
        );
    }

    #[test]
    fn filter_spec_adds_a_trimmed_name_match_last() {
        use FilterPart::*;
        for tab in [
            RoomListTab::All,
            RoomListTab::People,
            RoomListTab::Rooms,
            RoomListTab::Spaces,
        ] {
            let plain = filter_spec(&q(tab, ""));
            let mut expected = plain.clone();
            expected.push(NameMatch("caf\u{e9}".to_owned()));
            assert_eq!(filter_spec(&q(tab, "  caf\u{e9} \t")), expected);
        }
    }

    #[test]
    fn whitespace_only_search_adds_no_name_filter() {
        for search in ["", " ", " \t\n", "\u{a0}"] {
            let spec = filter_spec(&q(RoomListTab::All, search));
            assert!(!spec.iter().any(|p| matches!(p, FilterPart::NameMatch(_))));
        }
    }

    fn vec_of(items: &[u32]) -> matrix_sdk_ui::eyeball_im::Vector<u32> {
        items.iter().copied().collect()
    }

    #[test]
    fn maps_all_eleven_variants() {
        let double = |x: u32| x * 2;
        let cases: Vec<(V<u32>, ListDiff<u32>)> = vec![
            (
                V::Append {
                    values: vec_of(&[1, 2]),
                },
                ListDiff::Append { values: vec![2, 4] },
            ),
            (V::Clear, ListDiff::Clear),
            (V::PushFront { value: 3 }, ListDiff::PushFront { value: 6 }),
            (V::PushBack { value: 3 }, ListDiff::PushBack { value: 6 }),
            (V::PopFront, ListDiff::PopFront),
            (V::PopBack, ListDiff::PopBack),
            (
                V::Insert { index: 1, value: 5 },
                ListDiff::Insert {
                    index: 1,
                    value: 10,
                },
            ),
            (
                V::Set { index: 2, value: 5 },
                ListDiff::Set {
                    index: 2,
                    value: 10,
                },
            ),
            (V::Remove { index: 4 }, ListDiff::Remove { index: 4 }),
            (V::Truncate { length: 7 }, ListDiff::Truncate { length: 7 }),
            (
                V::Reset {
                    values: vec_of(&[8, 9]),
                },
                ListDiff::Reset {
                    values: vec![16, 18],
                },
            ),
        ];
        for (sdk, expected) in cases {
            assert_eq!(map_diff(sdk, double), expected);
        }
    }

    #[test]
    fn mapped_diffs_apply_like_the_originals() {
        let mut sdk_list: matrix_sdk_ui::eyeball_im::Vector<u32> = vec_of(&[]);
        let mut api_list: Vec<u32> = Vec::new();
        let diffs = vec![
            V::Append {
                values: vec_of(&[1, 2, 3]),
            },
            V::Insert { index: 1, value: 9 },
            V::Remove { index: 0 },
            V::PushFront { value: 7 },
            V::Truncate { length: 3 },
        ];
        for d in diffs {
            d.clone().apply(&mut sdk_list);
            map_diff(d, |x| x).apply(&mut api_list);
        }
        assert_eq!(sdk_list.iter().copied().collect::<Vec<_>>(), api_list);
    }

    #[test]
    fn name_falls_back_to_room_id() {
        let id = RoomId::from("!abc:localhost");
        assert_eq!(name_or_id(Some("Lounge".into()), &id), "Lounge");
        assert_eq!(name_or_id(None, &id), "!abc:localhost");
    }

    #[test]
    fn highlights_never_hide_a_mention() {
        assert_eq!(highlights_of(0, 0), 0);
        assert_eq!(highlights_of(3, 1), 3);
        assert_eq!(highlights_of(1, 4), 4);
    }

    #[test]
    fn preview_takes_the_first_line_only() {
        assert_eq!(preview_body("hello\nsecond"), "hello");
        assert_eq!(preview_body("hello\r\nsecond"), "hello");
        assert_eq!(preview_body(""), "");
    }

    #[test]
    fn preview_strips_control_and_bidi_characters() {
        let raw = "a\u{0007}b\u{202E}c\u{2066}d\u{200F}e\u{061C}f\u{0000}g\u{007F}h";
        assert_eq!(preview_body(raw), "abcdefgh");
    }

    #[test]
    fn preview_is_capped_on_a_char_boundary() {
        let long = "x".repeat(500);
        assert_eq!(preview_body(&long).chars().count(), PREVIEW_MAX_CHARS);
        // Multi-byte characters: 3 bytes each, so a byte cap would split one.
        let wide = "\u{20AC}".repeat(500);
        let cut = preview_body(&wide);
        assert_eq!(cut.chars().count(), PREVIEW_MAX_CHARS);
        assert_eq!(cut.len(), PREVIEW_MAX_CHARS * 3);
        // Characters removed before the cap do not count against it.
        let padded = format!("{}{}", "\u{202E}".repeat(10), "y".repeat(200));
        assert_eq!(preview_body(&padded).chars().count(), PREVIEW_MAX_CHARS);
    }

    const MXC_A: &str = "mxc://localhost/aaa";
    const MXC_B: &str = "mxc://localhost/bbb";

    #[test]
    fn room_avatar_wins() {
        let got = pick_avatar(Some(MXC_A), true, &[Some(MXC_B)]);
        assert_eq!(got, Some(AvatarId::from(MXC_A)));
    }

    #[test]
    fn dm_falls_back_to_the_single_hero() {
        assert_eq!(
            pick_avatar(None, true, &[Some(MXC_B)]),
            Some(AvatarId::from(MXC_B))
        );
    }

    #[test]
    fn dm_fallback_needs_exactly_one_hero() {
        assert_eq!(pick_avatar(None, true, &[]), None);
        assert_eq!(pick_avatar(None, true, &[Some(MXC_A), Some(MXC_B)]), None);
        assert_eq!(pick_avatar(None, true, &[None]), None);
    }

    #[test]
    fn non_dm_never_uses_a_hero() {
        assert_eq!(pick_avatar(None, false, &[Some(MXC_B)]), None);
    }

    #[test]
    fn invalid_mxc_uris_are_dropped() {
        assert_eq!(pick_avatar(Some("https://evil/x.png"), false, &[]), None);
        assert_eq!(pick_avatar(Some("mxc://nomedia"), false, &[]), None);
        // An invalid room avatar does not block a valid hero.
        assert_eq!(
            pick_avatar(Some("junk"), true, &[Some(MXC_B)]),
            Some(AvatarId::from(MXC_B))
        );
    }

    #[test]
    fn each_room_is_registered_once() {
        let mut reg = Registered::default();
        let a = RoomId::from("!a:x");
        let b = RoomId::from("!b:x");
        assert!(reg.first_sight(&a));
        assert!(!reg.first_sight(&a));
        assert!(reg.first_sight(&b));
        reg.forget(&a);
        assert!(reg.first_sight(&a), "a failed registration is retried");
    }

    #[test]
    fn loaded_state_maps_to_an_event() {
        use matrix_sdk_ui::room_list_service::RoomListLoadingState as S;
        assert!(loaded_event(&S::NotLoaded).is_none());
        let got = loaded_event(&S::Loaded {
            maximum_number_of_rooms: Some(7),
        });
        assert!(matches!(
            got,
            Some(CoreEvent::RoomListLoaded {
                total_rooms: Some(7)
            })
        ));
        let got = loaded_event(&S::Loaded {
            maximum_number_of_rooms: None,
        });
        assert!(matches!(
            got,
            Some(CoreEvent::RoomListLoaded { total_rooms: None })
        ));
    }

    #[test]
    fn items_mut_reaches_every_carried_item() {
        let mut diffs = vec![
            ListDiff::Append { values: vec![1, 2] },
            ListDiff::Reset { values: vec![3] },
            ListDiff::PushFront { value: 4 },
            ListDiff::PushBack { value: 5 },
            ListDiff::Insert { index: 0, value: 6 },
            ListDiff::Set { index: 0, value: 7 },
            ListDiff::Clear,
            ListDiff::PopFront,
            ListDiff::PopBack,
            ListDiff::Remove { index: 1 },
            ListDiff::Truncate { length: 1 },
        ];
        let mut seen = Vec::new();
        for d in &mut diffs {
            for v in items_mut(d) {
                seen.push(*v);
                *v += 10;
            }
        }
        assert_eq!(seen, [1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(
            diffs[0],
            ListDiff::Append {
                values: vec![11, 12]
            }
        );
    }
}
