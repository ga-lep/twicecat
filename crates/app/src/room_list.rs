//! Applying the core's room-list diffs to the app state.
//!
//! Lists are plain `Vec`s changed in place, diff by diff (plan Q5). They are
//! rebuilt only on `Reset` or `Clear`.

use tessera_core::api::{ListDiff, RoomId, RoomSummary};

/// Applies `diffs` to `rooms` in order.
///
/// A diff whose index is out of range is ignored (see [`ListDiff::apply`]).
pub fn apply_room_diffs(rooms: &mut Vec<RoomSummary>, diffs: &[ListDiff<RoomSummary>]) {
    for diff in diffs {
        diff.apply(rooms);
    }
}

/// Whether `rooms` still holds the room `id`.
pub fn contains(rooms: &[RoomSummary], id: &RoomId) -> bool {
    rooms.iter().any(|room| room.id == *id)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use tessera_core::api::{
        AvatarId, Command, CoreEvent, ListDiff, MessagePreview, RoomId, RoomSummary, SendStatus,
        TimelineItem, TimelineItemKind, UserId,
    };
    use tokio::sync::mpsc;

    use crate::bridge::EventsSlot;
    use crate::state::{App, Message, Startup};

    fn app() -> (App, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(64);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        (app, rx)
    }

    fn room(n: &str) -> RoomSummary {
        RoomSummary::new(RoomId::from(format!("!{n}:x")), n.to_owned())
    }

    fn rooms_event(diffs: Vec<ListDiff<RoomSummary>>) -> Message {
        Message::Core(Box::new(CoreEvent::RoomListChanged(diffs)))
    }

    /// An app holding the known list `a, b, c`.
    fn app_abc() -> App {
        let (mut app, _rx) = app();
        let _ = app.update(rooms_event(vec![ListDiff::Reset {
            values: vec![room("a"), room("b"), room("c")],
        }]));
        app
    }

    fn names(app: &App) -> Vec<&str> {
        app.rooms.iter().map(|r| r.name.as_str()).collect()
    }

    fn ids(app: &App) -> Vec<String> {
        app.rooms.iter().map(|r| r.id.to_string()).collect()
    }

    /// Runs one diff on `a, b, c` through `update` and checks ids and names.
    fn check(diff: ListDiff<RoomSummary>, expected: &[&str]) {
        let mut app = app_abc();
        let _ = app.update(rooms_event(vec![diff]));
        assert_eq!(names(&app), expected);
        let want: Vec<String> = expected.iter().map(|n| format!("!{n}:x")).collect();
        assert_eq!(ids(&app), want);
    }

    #[test]
    fn append() {
        check(
            ListDiff::Append {
                values: vec![room("d"), room("e")],
            },
            &["a", "b", "c", "d", "e"],
        );
    }
    #[test]
    fn clear() {
        check(ListDiff::Clear, &[]);
    }
    #[test]
    fn push_front() {
        check(
            ListDiff::PushFront { value: room("d") },
            &["d", "a", "b", "c"],
        );
    }
    #[test]
    fn push_back() {
        check(
            ListDiff::PushBack { value: room("d") },
            &["a", "b", "c", "d"],
        );
    }
    #[test]
    fn pop_front() {
        check(ListDiff::PopFront, &["b", "c"]);
    }
    #[test]
    fn pop_back() {
        check(ListDiff::PopBack, &["a", "b"]);
    }
    #[test]
    fn insert() {
        check(
            ListDiff::Insert {
                index: 1,
                value: room("d"),
            },
            &["a", "d", "b", "c"],
        );
    }
    #[test]
    fn set() {
        check(
            ListDiff::Set {
                index: 1,
                value: room("d"),
            },
            &["a", "d", "c"],
        );
    }
    #[test]
    fn remove() {
        check(ListDiff::Remove { index: 1 }, &["a", "c"]);
    }
    #[test]
    fn truncate() {
        check(ListDiff::Truncate { length: 2 }, &["a", "b"]);
    }
    #[test]
    fn reset() {
        check(
            ListDiff::Reset {
                values: vec![room("x"), room("y")],
            },
            &["x", "y"],
        );
    }

    #[test]
    fn a_batch_applies_in_order() {
        let mut app = app_abc();
        let _ = app.update(rooms_event(vec![
            ListDiff::PushFront { value: room("d") },
            ListDiff::Remove { index: 2 },
            ListDiff::Set {
                index: 0,
                value: room("e"),
            },
            ListDiff::Append {
                values: vec![room("f")],
            },
        ]));
        assert_eq!(names(&app), ["e", "a", "c", "f"]);
    }

    #[test]
    fn out_of_range_diffs_are_ignored() {
        let mut app = app_abc();
        let _ = app.update(rooms_event(vec![
            ListDiff::Insert {
                index: 9,
                value: room("d"),
            },
            ListDiff::Set {
                index: 9,
                value: room("d"),
            },
            ListDiff::Remove { index: 9 },
            ListDiff::Truncate { length: 9 },
        ]));
        assert_eq!(names(&app), ["a", "b", "c"]);
    }

    #[test]
    fn room_list_loaded_stores_the_total() {
        let mut app = app_abc();
        assert_eq!(app.total_rooms, None);
        let _ = app.update(Message::Core(Box::new(CoreEvent::RoomListLoaded {
            total_rooms: Some(7),
        })));
        assert_eq!(app.total_rooms, Some(7));
    }

    fn rid(n: &str) -> RoomId {
        RoomId::from(format!("!{n}:x"))
    }

    fn item(id: &str, body: &str) -> TimelineItem {
        TimelineItem {
            id: id.to_owned(),
            kind: TimelineItemKind::Message {
                sender: UserId::from("@a:x"),
                body: body.to_owned(),
                is_own: false,
                timestamp_ms: 1,
                sent: SendStatus::Sent,
            },
        }
    }

    fn timeline_event(room: &str, diffs: Vec<ListDiff<TimelineItem>>) -> Message {
        Message::Core(Box::new(CoreEvent::TimelineDiff {
            room_id: rid(room),
            diffs,
        }))
    }

    fn timeline_ids(app: &App) -> Vec<&str> {
        app.timeline.iter().map(|i| i.id.as_str()).collect()
    }

    #[test]
    fn selecting_a_room_sends_the_command_and_clears_the_timeline() {
        let (mut app, mut rx) = app();
        let _ = rx.try_recv();
        let _ = app.update(rooms_event(vec![ListDiff::Reset {
            values: vec![room("a"), room("b")],
        }]));
        let _ = app.update(Message::SelectRoom(rid("a")));
        assert_eq!(app.selected, Some(rid("a")));
        assert!(matches!(rx.try_recv(), Ok(Command::SelectRoom(id)) if id == rid("a")));
        let _ = app.update(timeline_event(
            "a",
            vec![ListDiff::Append {
                values: vec![item("1", "x")],
            }],
        ));
        let _ = app.update(Message::SelectRoom(rid("b")));
        assert_eq!(app.selected, Some(rid("b")));
        assert!(app.timeline.is_empty());
    }

    #[test]
    fn selecting_an_unknown_room_is_ignored() {
        let (mut app, mut rx) = app();
        let _ = rx.try_recv();
        let _ = app.update(Message::SelectRoom(rid("zz")));
        assert_eq!(app.selected, None);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn timeline_diffs_apply_only_to_the_selected_room() {
        let mut app = app_abc();
        let _ = app.update(Message::SelectRoom(rid("a")));
        let _ = app.update(timeline_event(
            "a",
            vec![ListDiff::Append {
                values: vec![item("1", "x"), item("2", "y")],
            }],
        ));
        let _ = app.update(timeline_event(
            "b",
            vec![ListDiff::PushBack {
                value: item("9", "other"),
            }],
        ));
        let _ = app.update(timeline_event("a", vec![ListDiff::PopFront]));
        assert_eq!(timeline_ids(&app), ["2"]);
    }

    #[test]
    fn timeline_diffs_without_a_selection_are_ignored() {
        let mut app = app_abc();
        let _ = app.update(timeline_event(
            "a",
            vec![ListDiff::PushBack {
                value: item("1", "x"),
            }],
        ));
        assert!(app.timeline.is_empty());
    }

    #[test]
    fn removing_the_selected_room_clears_the_selection_and_timeline() {
        let mut app = app_abc();
        let _ = app.update(Message::SelectRoom(rid("b")));
        let _ = app.update(timeline_event(
            "b",
            vec![ListDiff::PushBack {
                value: item("1", "x"),
            }],
        ));
        let _ = app.update(rooms_event(vec![ListDiff::Remove { index: 0 }]));
        assert_eq!(app.selected, Some(rid("b")), "another room left");
        assert_eq!(timeline_ids(&app), ["1"]);
        let _ = app.update(rooms_event(vec![ListDiff::Remove { index: 0 }]));
        assert_eq!(app.selected, None);
        assert!(app.timeline.is_empty());
    }

    #[test]
    fn reordering_keeps_the_selection() {
        let mut app = app_abc();
        let _ = app.update(Message::SelectRoom(rid("b")));
        let _ = app.update(timeline_event(
            "b",
            vec![ListDiff::PushBack {
                value: item("1", "x"),
            }],
        ));
        // Remove then insert in one batch: the room is present afterwards.
        let _ = app.update(rooms_event(vec![
            ListDiff::Remove { index: 1 },
            ListDiff::PushFront { value: room("b") },
        ]));
        assert_eq!(app.selected, Some(rid("b")));
        assert_eq!(timeline_ids(&app), ["1"]);
    }

    #[test]
    fn reset_without_the_selected_room_clears_it_and_logout_resets_state() {
        let mut app = app_abc();
        let _ = app.update(Message::SelectRoom(rid("a")));
        let _ = app.update(rooms_event(vec![ListDiff::Reset {
            values: vec![room("x")],
        }]));
        assert_eq!(app.selected, None);
        let _ = app.update(Message::SelectRoom(rid("x")));
        let _ = app.update(Message::Core(Box::new(CoreEvent::RoomListLoaded {
            total_rooms: Some(3),
        })));
        let _ = app.update(Message::Core(Box::new(CoreEvent::LoggedOut)));
        assert_eq!(app.selected, None);
        assert!(app.timeline.is_empty() && app.rooms.is_empty());
        assert_eq!(app.total_rooms, None);
    }

    // ---- Timing (plan Q17) ------------------------------------------------

    const ROOMS: usize = 2_000;
    const RUNS: usize = 15;
    const BUDGET: Duration = Duration::from_millis(16);

    /// Deterministic synthetic room `i`: name, avatar and a 100-char preview.
    fn synthetic(i: usize) -> RoomSummary {
        let mut r = RoomSummary::new(
            RoomId::from(format!("!room{i:05}:example.org")),
            format!("Room number {i:05}"),
        );
        r.avatar = Some(AvatarId::from(format!("mxc://example.org/avatar{i:05}")));
        r.unread_messages = (i % 7) as u64;
        r.highlights = (i % 3) as u64;
        r.is_dm = i.is_multiple_of(5);
        let body: String = format!("message {i:05} ")
            .chars()
            .cycle()
            .take(100)
            .collect();
        r.last_message = Some(MessagePreview {
            sender: UserId::from(format!("@user{}:example.org", i % 50)),
            sender_name: Some(format!("User {}", i % 50)),
            body,
            timestamp_ms: 1_700_000_000_000 + i as u64,
            is_own: i.is_multiple_of(11),
        });
        r
    }

    fn synthetic_range(start: usize, end: usize) -> Vec<RoomSummary> {
        (start..end).map(synthetic).collect()
    }

    fn app_with_rooms(n: usize) -> App {
        let mut app = app().0;
        app.rooms = synthetic_range(0, n);
        app
    }

    /// Batch 3: 100 `Set`, 50 `Remove`+`Insert` reorders, 10 `PushFront`.
    fn mixed_batch() -> Vec<ListDiff<RoomSummary>> {
        let mut diffs = Vec::new();
        for k in 0..100 {
            diffs.push(ListDiff::Set {
                index: k * 19,
                value: synthetic(ROOMS + k),
            });
        }
        for k in 0..50 {
            diffs.push(ListDiff::Remove {
                index: 1_000 + k * 7,
            });
            diffs.push(ListDiff::Insert {
                index: k * 3,
                value: synthetic(k * 13),
            });
        }
        for k in 0..10 {
            diffs.push(ListDiff::PushFront {
                value: synthetic(ROOMS + 100 + k),
            });
        }
        diffs
    }

    /// Times `update` alone on a fresh clone of `start` for each run.
    fn time_shape(start_rooms: usize, diffs: &[ListDiff<RoomSummary>]) -> Vec<Duration> {
        (0..=RUNS)
            .map(|_| {
                let mut app = app_with_rooms(start_rooms);
                let message = rooms_event(diffs.to_vec());
                let began = Instant::now();
                let _ = app.update(message);
                let took = began.elapsed();
                std::hint::black_box(&app);
                took
            })
            .skip(1) // the warm-up
            .collect()
    }

    fn median(timings: &[Duration]) -> Duration {
        let mut sorted = timings.to_vec();
        sorted.sort();
        sorted[sorted.len() / 2]
    }

    #[test]
    fn room_list_batches_under_16ms() {
        let shapes: Vec<(&str, usize, Vec<ListDiff<RoomSummary>>)> = vec![
            (
                "reset 2000",
                0,
                vec![ListDiff::Reset {
                    values: synthetic_range(0, ROOMS),
                }],
            ),
            (
                "4 x append 500",
                0,
                (0..4)
                    .map(|k| ListDiff::Append {
                        values: synthetic_range(k * 500, k * 500 + 500),
                    })
                    .collect(),
            ),
            ("mixed on 2000", ROOMS, mixed_batch()),
            (
                "clear then reset 2000",
                ROOMS,
                vec![
                    ListDiff::Clear,
                    ListDiff::Reset {
                        values: synthetic_range(0, ROOMS),
                    },
                ],
            ),
        ];
        let mut failures = Vec::new();
        for (name, start, diffs) in &shapes {
            let timings = time_shape(*start, diffs);
            let med = median(&timings);
            println!("{name}: median {med:?}, all {timings:?}");
            if med >= BUDGET {
                failures.push(format!(
                    "{name}: median {med:?} >= {BUDGET:?}; all {timings:?}"
                ));
            }
        }
        assert!(failures.is_empty(), "too slow:\n{}", failures.join("\n"));
    }
}
