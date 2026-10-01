//! Mapping of the SDK's room list diffs to the public [`ListDiff`].

use matrix_sdk_ui::eyeball_im::VectorDiff;
use matrix_sdk_ui::room_list_service::RoomListItem;

use crate::api::{ListDiff, RoomId, RoomSummary};

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

/// Builds the summary of a room.
///
/// The name comes from the server and is untrusted display data.
pub(crate) fn summarize(item: &RoomListItem) -> RoomSummary {
    let id = RoomId::from(item.room_id().to_string());
    let name = name_or_id(item.cached_display_name().map(|n| n.to_string()), &id);
    RoomSummary {
        unread_messages: item.num_unread_messages(),
        unread_mentions: item.num_unread_mentions(),
        unread_notifications: item.num_unread_notifications(),
        ..RoomSummary::new(id, name)
    }
}

/// Display name, or the room ID when the room has no cached name.
pub(crate) fn name_or_id(name: Option<String>, id: &RoomId) -> String {
    name.unwrap_or_else(|| id.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use matrix_sdk_ui::eyeball_im::VectorDiff as V;

    use super::*;

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
}
