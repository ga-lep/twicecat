//! Threads: the second, thread-focused timeline and the routing of item IDs
//! between it and the room's main timeline.
//!
//! Message bodies are user content: nothing here logs them.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use matrix_sdk::Room;
use matrix_sdk::ruma::{OwnedEventId, OwnedUserId};
use matrix_sdk_ui::timeline::{RoomExt as _, Timeline, TimelineFocus};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::api::{CoreError, CoreEvent, EventId, PaginationState, RoomId};
use crate::engine::{PAGINATION_BATCH, PaginationPermit, forward_timeline};

/// Prefix of the item IDs of the thread timeline (set with
/// `with_internal_id_prefix`), which tells the core where an item lives.
pub(crate) const THREAD_ID_PREFIX: &str = "thread:";

/// Which of the selected room's timelines an item ID belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// The room's live timeline.
    Main,
    /// The open thread's timeline.
    Thread,
}

/// Routes an item ID: IDs the thread timeline made start with
/// [`THREAD_ID_PREFIX`], all others are the main timeline's.
pub(crate) fn route_item(item_id: &str) -> Route {
    if item_id.starts_with(THREAD_ID_PREFIX) {
        Route::Thread
    } else {
        Route::Main
    }
}

/// The open thread of the selected room: its timeline and the task forwarding
/// the timeline's diffs.
pub(crate) struct OpenThread {
    /// The thread's root event.
    pub(crate) root: OwnedEventId,
    pub(crate) timeline: Arc<Timeline>,
    task: JoinHandle<()>,
    /// Set while a back-pagination of this thread runs.
    pub(crate) paginating: Arc<AtomicBool>,
}

impl OpenThread {
    /// Ends the forwarder; the timeline is dropped with `self`, or with the
    /// last action task that still holds it.
    pub(crate) async fn stop(self) {
        self.task.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = self.task.await;
    }
}

/// Builds the thread-focused timeline of `root` in `room` and starts
/// forwarding it as `ThreadDiff`s. Its item IDs start with
/// [`THREAD_ID_PREFIX`], so they never collide with the main timeline's.
pub(crate) async fn open(
    room: &Room,
    room_id: &RoomId,
    root: OwnedEventId,
    own: Option<OwnedUserId>,
    events: &mpsc::Sender<CoreEvent>,
) -> Result<OpenThread, CoreError> {
    let timeline = room
        .timeline_builder()
        .with_focus(TimelineFocus::Thread {
            root_event_id: root.clone(),
        })
        .with_internal_id_prefix(THREAD_ID_PREFIX.to_owned())
        .build()
        .await
        .map_err(|_| CoreError::Internal("could not open the thread".into()))?;
    let (items, stream) = timeline.subscribe().await;
    let (room_id, event_id) = (room_id.clone(), EventId::from(root.as_str()));
    // The SDK has no status for a thread's pagination; this starts at Idle
    // and `paginate_task` reports the changes.
    let idle = CoreEvent::ThreadPagination {
        room_id: room_id.clone(),
        root: event_id.clone(),
        state: PaginationState::Idle,
    };
    let task = tokio::spawn(forward_timeline(
        items,
        stream,
        events.clone(),
        own,
        move |diffs| CoreEvent::ThreadDiff {
            room_id: room_id.clone(),
            root: event_id.clone(),
            diffs,
        },
        vec![idle],
    ));
    Ok(OpenThread {
        root,
        timeline: Arc::new(timeline),
        task,
        paginating: Arc::new(AtomicBool::new(false)),
    })
}

/// Loads older events of a thread, unless a pagination of it is already
/// running (then the request is dropped: the app asks again when idle).
/// Reports `Loading`, then `Idle` or `ReachedStart`.
pub(crate) async fn paginate_task(
    timeline: Arc<Timeline>,
    flag: Arc<AtomicBool>,
    events: mpsc::Sender<CoreEvent>,
    room_id: RoomId,
    root: EventId,
) -> Result<(), CoreError> {
    let Some(_permit) = PaginationPermit::try_acquire(&flag) else {
        return Ok(());
    };
    let report = |state| CoreEvent::ThreadPagination {
        room_id: room_id.clone(),
        root: root.clone(),
        state,
    };
    // A gone receiver just means nobody listens any more.
    let _ = events.send(report(PaginationState::Loading)).await;
    let result = timeline.paginate_backwards(PAGINATION_BATCH).await;
    let state = match result {
        Ok(true) => PaginationState::ReachedStart,
        _ => PaginationState::Idle,
    };
    let _ = events.send(report(state)).await;
    result
        .map(drop)
        .map_err(|_| CoreError::Internal("older messages could not be loaded".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_item_table() {
        let cases = [
            ("thread:0", Route::Thread),
            ("thread:17", Route::Thread),
            ("thread:", Route::Thread),
            ("0", Route::Main),
            ("42", Route::Main),
            ("", Route::Main),
            ("__read_marker", Route::Main),
            // The prefix is case-sensitive and must be at the start.
            ("Thread:1", Route::Main),
            (" thread:1", Route::Main),
            ("x thread:1", Route::Main),
            ("thread", Route::Main),
        ];
        for (id, expected) in cases {
            assert_eq!(route_item(id), expected, "{id:?}");
        }
    }
}
