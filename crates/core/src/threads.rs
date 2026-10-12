//! Threads: the second, thread-focused timeline and the routing of item IDs
//! between it and the room's main timeline.
//!
//! Message bodies are user content: nothing here logs them.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use matrix_sdk::Room;
use matrix_sdk::ruma::{EventId as RumaEventId, OwnedEventId, OwnedUserId};
use matrix_sdk_ui::timeline::{RoomExt as _, Timeline, TimelineFocus};
use tokio::sync::{mpsc, watch};
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

/// The prefix of the item IDs of the thread rooted at `root`:
/// `thread:{root}:`. The SDK appends a counter, so an ID made for one thread
/// can never name an item of another.
pub(crate) fn item_prefix(root: &RumaEventId) -> String {
    format!("{THREAD_ID_PREFIX}{root}:")
}

/// Whether `item_id` is an item ID of the thread rooted at `root`: the
/// prefix of [`item_prefix`] followed by the SDK's numeric counter (the
/// counter check keeps a root that is a prefix of another root's ID, as in
/// room versions 1 and 2, from claiming that root's items).
pub(crate) fn item_in_thread(item_id: &str, root: &RumaEventId) -> bool {
    item_id
        .strip_prefix(item_prefix(root).as_str())
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The open thread of the selected room: its timeline and the task forwarding
/// the timeline's diffs.
pub(crate) struct OpenThread {
    /// The thread's root event.
    pub(crate) root: OwnedEventId,
    pub(crate) timeline: Arc<Timeline>,
    task: JoinHandle<()>,
    /// Dropped or set by [`Self::stop`]: tells the pagination tasks of this
    /// thread to end without reporting.
    cancel: watch::Sender<bool>,
    /// Set while a back-pagination of this thread runs.
    pub(crate) paginating: Arc<AtomicBool>,
}

impl OpenThread {
    /// A receiver that sees `true` once the thread is closed or replaced.
    pub(crate) fn cancelled(&self) -> watch::Receiver<bool> {
        self.cancel.subscribe()
    }

    /// Ends the forwarder and the pagination; the timeline is dropped with `self`, or with the
    /// last action task that still holds it.
    pub(crate) async fn stop(self) {
        self.cancel.send_replace(true);
        self.task.abort();
        // An aborted task ends with a cancellation error; that is expected.
        let _ = self.task.await;
    }
}

/// Builds the thread-focused timeline of `root` in `room` and starts
/// forwarding it as `ThreadDiff`s. Its item IDs start with
/// [`THREAD_ID_PREFIX`], so they never collide with the main timeline's
/// or another thread's.
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
        .with_internal_id_prefix(item_prefix(&root))
        .event_filter(crate::timeline::event_filter)
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
        cancel: watch::channel(false).0,
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
    mut cancelled: watch::Receiver<bool>,
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
    // A closed or replaced thread ends the pagination silently: its root is
    // no longer the app's. (`wait_for` also ends if the sender is dropped.)
    let result = tokio::select! {
        biased;
        _ = cancelled.wait_for(|closed| *closed) => return Ok(()),
        result = timeline.paginate_backwards(PAGINATION_BATCH) => result,
    };
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
    fn item_prefix_binds_the_root() {
        let root = matrix_sdk::ruma::event_id!("$root:x");
        assert_eq!(item_prefix(root), "thread:$root:x:");
    }

    #[test]
    fn item_in_thread_table() {
        let a = matrix_sdk::ruma::event_id!("$a");
        let b = matrix_sdk::ruma::event_id!("$b");
        let legacy = matrix_sdk::ruma::event_id!("$a:x");
        let cases = [
            ("thread:$a:0", a, true),
            ("thread:$a:17", a, true),
            // Another thread's item, same counter.
            ("thread:$b:0", a, false),
            ("thread:$a:0", b, false),
            // The old unbound form is refused.
            ("thread:0", a, false),
            ("0", a, false),
            ("", a, false),
            // No counter, or a counter that is not a number.
            ("thread:$a:", a, false),
            ("thread:$a:x", a, false),
            ("thread:$a:1:2", a, false),
            // A root that is a prefix of another root's ID.
            ("thread:$a:x:3", a, false),
            ("thread:$a:x:3", legacy, true),
            ("thread:$a:3", legacy, false),
        ];
        for (id, root, expected) in cases {
            assert_eq!(item_in_thread(id, root), expected, "{id:?} in {root}");
        }
    }

    #[test]
    fn route_item_table() {
        let cases = [
            ("thread:0", Route::Thread),
            ("thread:17", Route::Thread),
            ("thread:", Route::Thread),
            ("thread:$root:x:3", Route::Thread),
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
