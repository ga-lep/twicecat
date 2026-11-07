//! Spaces: the SDK's space filters forwarded as [`CoreEvent::Spaces`], and
//! the descendants of each space for the room list's space filter.
//!
//! Space names are user content: nothing here logs them.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use matrix_sdk::Client;
use matrix_sdk::ruma::OwnedRoomId;
use matrix_sdk_ui::eyeball_im::Vector;
use matrix_sdk_ui::spaces::{SpaceFilter, SpaceService};
use tokio::sync::{mpsc, watch};

use crate::api::{CoreEvent, MAX_SPACE_ROOMS, RoomId, SpaceView};
use crate::notifications::clean_line;
use crate::room_list::pick_avatar;
use crate::rooms::MAX_NAME_CHARS;

/// The room IDs inside each known space, by space ID.
pub(crate) type SpaceChildren = Arc<HashMap<OwnedRoomId, Vec<OwnedRoomId>>>;

/// The first [`MAX_SPACE_ROOMS`] of a space's descendants, as API IDs.
fn room_ids(descendants: &[OwnedRoomId]) -> Vec<RoomId> {
    descendants
        .iter()
        .take(MAX_SPACE_ROOMS)
        .map(|id| RoomId::from(id.as_str()))
        .collect()
}

/// The API form of a space filter. The name is cleaned to one line; the
/// avatar is kept only when it is a valid MXC URI.
pub(crate) fn view_of(filter: &SpaceFilter) -> SpaceView {
    let room = &filter.space_room;
    SpaceView {
        id: RoomId::from(room.room_id.as_str()),
        name: clean_line(&room.display_name, MAX_NAME_CHARS),
        avatar: pick_avatar(room.avatar_url.as_deref().map(|u| u.as_str()), false, &[]),
        level: u32::from(filter.level),
        children: room.children_count,
        rooms: room_ids(&filter.descendants),
    }
}

/// The descendants of every space in `filters`.
pub(crate) fn children_of(filters: &[SpaceFilter]) -> SpaceChildren {
    Arc::new(
        filters
            .iter()
            .map(|f| (f.space_room.room_id.clone(), f.descendants.clone()))
            .collect(),
    )
}

/// Forwards the space filters until the channel closes: one `Spaces` event
/// per change, and the descendants for the room list. Owns the session's
/// `SpaceService`.
pub(crate) async fn run(
    client: Client,
    events: mpsc::Sender<CoreEvent>,
    children: Arc<watch::Sender<SpaceChildren>>,
) {
    let service = SpaceService::new(client).await;
    let (initial, stream) = service.subscribe_to_space_filters().await;
    let mut current: Vector<SpaceFilter> = initial;
    futures_util::pin_mut!(stream);
    loop {
        let list: Vec<SpaceFilter> = current.iter().cloned().collect();
        children.send_replace(children_of(&list));
        let views = list.iter().map(view_of).collect();
        if events.send(CoreEvent::Spaces(views)).await.is_err() {
            return;
        }
        let Some(batch) = stream.next().await else {
            return;
        };
        for diff in batch {
            diff.apply(&mut current);
        }
    }
}

#[cfg(test)]
mod tests {
    use matrix_sdk_ui::spaces::SpaceRoom;

    use crate::api::AvatarId;

    use super::*;

    fn filter(id: &str, name: &str, level: u8, kids: &[&str], avatar: Option<&str>) -> SpaceFilter {
        SpaceFilter {
            space_room: SpaceRoom {
                room_id: id.try_into().unwrap(),
                canonical_alias: None,
                name: Some(name.to_owned()),
                display_name: name.to_owned(),
                topic: None,
                avatar_url: avatar.map(Into::into),
                room_type: None,
                num_joined_members: 1,
                join_rule: None,
                world_readable: None,
                guest_can_join: false,
                is_direct: None,
                children_count: kids.len() as u64,
                state: None,
                heroes: None,
                via: Vec::new(),
                suggested: false,
                is_dm: None,
            },
            level,
            descendants: kids.iter().map(|k| (*k).try_into().unwrap()).collect(),
        }
    }

    #[test]
    fn a_space_filter_becomes_a_view() {
        let view = view_of(&filter(
            "!s:x",
            "Team\n\u{202E}Space",
            1,
            &["!a:x", "!b:x"],
            Some("mxc://x/av"),
        ));
        assert_eq!(view.id.as_str(), "!s:x");
        assert_eq!(view.name, "Team Space");
        assert_eq!(view.level, 1);
        assert_eq!(view.children, 2);
        assert_eq!(
            view.avatar.as_ref().map(AvatarId::as_str),
            Some("mxc://x/av")
        );
        let bad = view_of(&filter("!s:x", "S", 0, &[], Some("https://evil.example/a")));
        assert_eq!(bad.avatar, None);
        assert_eq!(view_of(&filter("!s:x", "S", 0, &[], None)).avatar, None);
    }

    #[test]
    fn descendants_are_kept_by_space() {
        let map = children_of(&[
            filter("!s:x", "S", 0, &["!a:x"], None),
            filter("!t:x", "T", 1, &["!a:x", "!b:x"], None),
        ]);
        let s: OwnedRoomId = "!s:x".try_into().unwrap();
        let t: OwnedRoomId = "!t:x".try_into().unwrap();
        assert_eq!(map[&s].len(), 1);
        assert_eq!(map[&t].len(), 2);
        assert!(children_of(&[]).is_empty());
    }

    #[test]
    fn rooms_are_the_descendants_in_order() {
        let view = view_of(&filter("!s:x", "S", 0, &["!b:x", "!a:x", "!c:x"], None));
        let ids: Vec<&str> = view.rooms.iter().map(RoomId::as_str).collect();
        assert_eq!(ids, ["!b:x", "!a:x", "!c:x"]);
        assert!(view_of(&filter("!s:x", "S", 1, &[], None)).rooms.is_empty());
    }

    #[test]
    fn rooms_are_capped() {
        let ids: Vec<String> = (0..MAX_SPACE_ROOMS + 5)
            .map(|i| format!("!r{i}:x"))
            .collect();
        let kids: Vec<&str> = ids.iter().map(String::as_str).collect();
        let view = view_of(&filter("!s:x", "S", 1, &kids, None));
        assert_eq!(view.rooms.len(), 2000);
        assert_eq!(view.rooms[1999].as_str(), "!r1999:x");
        assert_eq!(view.children, 2005);
        let exact: Vec<&str> = kids[..MAX_SPACE_ROOMS].to_vec();
        assert_eq!(
            view_of(&filter("!s:x", "S", 1, &exact, None)).rooms.len(),
            2000
        );
    }

    #[test]
    fn debug_shows_the_room_count_and_no_id() {
        let view = view_of(&filter(
            "!s:x",
            "S",
            0,
            &["!secretroom:x", "!other:x"],
            None,
        ));
        let text = format!("{view:?}");
        assert!(text.contains("rooms: 2"), "{text}");
        assert!(
            !text.contains("secretroom") && !text.contains("other"),
            "{text}"
        );
    }
}
