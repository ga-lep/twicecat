//! Who is typing in each room, kept for the whole session.
//!
//! The server sends a room's typing list only when it changes, often before
//! the room is opened (the room list carries it too). A handler registered
//! when the room is selected would miss that list until the next change, so
//! the list of every room is kept here from the first sync on (issue #19).

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use matrix_sdk::event_handler::EventHandlerDropGuard;
use matrix_sdk::ruma::events::typing::SyncTypingEvent;
use matrix_sdk::ruma::{OwnedRoomId, OwnedUserId, RoomId};
use matrix_sdk::{Client, Room};
use tokio::sync::watch;

type Rooms = Arc<StdMutex<HashMap<OwnedRoomId, watch::Sender<Vec<OwnedUserId>>>>>;

/// The latest typing list of each room (other users only); start it before
/// the sync so no list passes by.
pub(super) struct TypingWatch {
    rooms: Rooms,
    /// Keeps the event handler registered; it goes when this does.
    _guard: EventHandlerDropGuard,
}

impl TypingWatch {
    pub(super) fn start(client: &Client) -> Self {
        let rooms = Rooms::default();
        let own = client.user_id().map(ToOwned::to_owned);
        let handler = client.add_event_handler({
            let rooms = rooms.clone();
            move |event: SyncTypingEvent, room: Room| {
                let (rooms, own) = (rooms.clone(), own.clone());
                async move {
                    let ids = event
                        .content
                        .user_ids
                        .into_iter()
                        .filter(|id| Some(id) != own.as_ref())
                        .collect();
                    sender(&rooms, room.room_id()).send_replace(ids);
                }
            }
        });
        Self {
            rooms,
            _guard: client.event_handler_drop_guard(handler),
        }
    }

    /// Who else is typing in `room_id`: the current list, then each change.
    pub(super) fn subscribe(&self, room_id: &RoomId) -> watch::Receiver<Vec<OwnedUserId>> {
        sender(&self.rooms, room_id).subscribe()
    }
}

/// The sender of `room_id`, made empty on first use.
fn sender(rooms: &Rooms, room_id: &RoomId) -> watch::Sender<Vec<OwnedUserId>> {
    rooms
        .lock()
        .expect("typing watch lock")
        .entry(room_id.to_owned())
        .or_insert_with(|| watch::channel(Vec::new()).0)
        .clone()
}
