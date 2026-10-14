//! Lays out the timeline pane headlessly (tiny-skia, no window) for a
//! 10,000-item timeline and checks that only the rows around the viewport
//! exist, and that the spacers keep the content height (plan Q15c).

use chrono::Utc;
use iced::advanced::graphics::text::font_system;
use iced::advanced::layout::{Limits, Node};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Tree;
use iced::{Pixels, Renderer, Size};
use twicecat_app::rich_text::Revealed;
use twicecat_app::screens::timeline_pane::{RichEnv, view_in};
use twicecat_app::timeline::{MAX_BUILT_ROWS, TimelineView};
use twicecat_app::ui::fonts;
use twicecat_core::api::{
    ListDiff, MessageDetails, SendStatus, StateChange, TimelineItem, TimelineItemKind, UserId,
};

const NOW: u64 = 1_790_000_000_000;
const WIDTH: f32 = 800.0;
const HEIGHT: f32 = 600.0;

/// The headless renderer with the bundled typeface, the one the app draws
/// with: the estimates are compared with real measurements, which must not
/// depend on the fonts a machine happens to have.
fn renderer() -> Renderer {
    use std::borrow::Cow;
    use std::sync::Once;
    static FONTS: Once = Once::new();
    FONTS.call_once(|| {
        let mut system = font_system().write().expect("the font system");
        for face in fonts::ALL {
            system.load_font(Cow::Borrowed(face));
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    runtime
        .block_on(Renderer::new(fonts::BODY, Pixels(16.0), Some("tiny-skia")))
        .expect("the headless tiny-skia renderer is not available")
}

fn timeline(n: usize) -> TimelineView {
    let items = (0..n)
        .map(|i| {
            let kind = if i % 50 == 0 {
                TimelineItemKind::DateDivider {
                    timestamp_ms: NOW - 86_400_000 * (i as u64 / 50 % 400),
                }
            } else if i % 25 == 0 {
                TimelineItemKind::State {
                    sender: UserId::from("@a:x"),
                    sender_name: Some("Alice".into()),
                    timestamp_ms: NOW,
                    change: StateChange::Joined,
                }
            } else {
                TimelineItemKind::Message {
                    sender: UserId::from(if i % 3 == 0 { "@a:x" } else { "@b:x" }),
                    body: format!("message {i}\n{}", "wrapping text ".repeat(i % 20)),
                    is_own: false,
                    timestamp_ms: NOW - (n - i) as u64 * 1000,
                    sent: SendStatus::Sent,
                    details: Box::new(MessageDetails::default()),
                }
            };
            TimelineItem::new(format!("i{i}"), kind)
        })
        .collect();
    let mut view = TimelineView::default();
    view.apply_diffs(vec![ListDiff::Reset { values: items }]);
    view.scrolled(0.0, HEIGHT, WIDTH);
    view
}

/// The layout of the pane: the content column inside the scrollable, which
/// is the first layer of the pane's stack.
fn content_node(view: &TimelineView, renderer: &Renderer) -> Node {
    content_node_with(
        view,
        renderer,
        &twicecat_app::media::MediaModel::default(),
        None,
    )
}

fn content_node_with(
    view: &TimelineView,
    renderer: &Renderer,
    media: &twicecat_app::media::MediaModel,
    room: Option<&twicecat_core::api::RoomId>,
) -> Node {
    let theme = twicecat_app::theme::theme_for(iced::theme::Mode::Light);
    let revealed = Revealed::new();
    let actions = twicecat_app::actions::ActionState::default();
    let env = RichEnv {
        theme: &theme,
        own_user: None,
        revealed: &revealed,
        actions: &actions,
        rooms: &[],
        members: &[],
        media,
        room,
        density: view.density(),
    };
    let mut element = view_in(view, NOW, &Utc, env);
    let mut tree = Tree::new(element.as_widget());
    let root = element.as_widget_mut().layout(
        &mut tree,
        renderer,
        &Limits::new(Size::ZERO, Size::new(WIDTH, HEIGHT)),
    );
    let list = &root.children()[0];
    assert_eq!(list.children().len(), 1, "the scrollable has one child");
    list.children()[0].clone()
}

/// Reports the laid-out height of every built row, as the sensors would,
/// until the window is stable. Returns the final content node.
fn settle(view: &mut TimelineView, renderer: &Renderer) -> Node {
    for _ in 0..20 {
        let node = content_node(view, renderer);
        let window = view.window();
        // Children: top spacer, one per window row, bottom spacer.
        assert_eq!(node.children().len(), window.len() + 2);
        let mut changed = false;
        for (i, row) in window.clone().zip(&node.children()[1..]) {
            let id = view.items[i].id.clone();
            if view.measured.get(&id) != Some(&row.bounds().height) {
                changed = true;
                view.row_measured(&id, row.bounds().height);
            }
        }
        if !changed {
            return node;
        }
    }
    panic!("the window did not settle");
}

fn index_at(prefix: &[f32], y: f32) -> usize {
    (0..prefix.len() - 1)
        .find(|&i| prefix[i] <= y && y < prefix[i + 1])
        .unwrap_or(prefix.len() - 2)
}

#[test]
fn only_the_visible_rows_of_10000_items_are_laid_out() {
    let renderer = renderer();
    let mut view = timeline(10_000);
    let node = content_node(&view, &renderer);
    let children = node.children().len();
    assert!(
        children <= 62,
        "{children} children: more than 60 rows and 2 spacers"
    );
    assert!(children - 2 <= MAX_BUILT_ROWS);
    assert!(children > 2, "some rows are built");

    let node = settle(&mut view, &renderer);
    let total = *view.prefix.last().expect("a prefix total");
    let height = node.bounds().height;
    assert!(
        (height - total).abs() <= 1.0,
        "content {height} against prefix total {total}"
    );
    assert!(node.children().len() <= 62);
}

#[test]
fn a_window_in_the_middle_covers_the_visible_range() {
    let renderer = renderer();
    let mut view = timeline(10_000);
    // Measure the bottom first so the middle has believable heights near it.
    settle(&mut view, &renderer);
    let total = *view.prefix.last().expect("a prefix total");
    view.scrolled(total / 2.0, HEIGHT, WIDTH);
    let node = settle(&mut view, &renderer);
    let total = *view.prefix.last().expect("a prefix total");

    let window = view.window();
    let top = total - view.offset_from_bottom - HEIGHT;
    let first_visible = index_at(&view.prefix, top);
    let last_visible = index_at(&view.prefix, top + HEIGHT - 0.01);
    assert!(first_visible > 100 && last_visible < 9_900, "in the middle");
    assert!(
        window.start <= first_visible && window.end > last_visible,
        "window {window:?} against visible {first_visible}..={last_visible}"
    );
    assert!(window.len() <= 60, "{} rows", window.len());
    // The top spacer holds the rows above the window.
    let spacer = node.children()[0].bounds().height;
    assert!(
        (spacer - view.prefix[window.start]).abs() <= 1.0,
        "{spacer}"
    );
    assert!(
        (node.bounds().height - total).abs() <= 1.0,
        "content {} against prefix total {total}",
        node.bounds().height
    );
}

#[test]
fn an_empty_timeline_has_only_two_empty_spacers() {
    let renderer = renderer();
    let view = TimelineView::default();
    let node = content_node(&view, &renderer);
    assert_eq!(node.children().len(), 2);
    assert!(node.bounds().height < 1.0);
}

fn media_message(id: &str, kind: twicecat_core::api::MediaKind, w: u32, h: u32) -> TimelineItem {
    let mut media = twicecat_core::api::MediaContent::new(kind, "pic.png".to_owned());
    media.width = Some(w);
    media.height = Some(h);
    TimelineItem::new(
        id.to_owned(),
        TimelineItemKind::Message {
            sender: UserId::from("@a:x"),
            body: "pic.png".to_owned(),
            is_own: false,
            timestamp_ms: NOW,
            sent: SendStatus::Sent,
            details: Box::new(MessageDetails {
                media: Some(Box::new(media)),
                ..MessageDetails::default()
            }),
        },
    )
}

/// The heights of the built rows, from the layout.
fn row_heights(node: &Node) -> Vec<f32> {
    let kids = node.children();
    kids[1..kids.len() - 1]
        .iter()
        .map(|row| row.bounds().height)
        .collect()
}

#[test]
fn a_thumbnail_arriving_changes_no_laid_out_row_height() {
    use twicecat_core::api::{MediaKind, RoomId};
    let renderer = renderer();
    let room = RoomId::from("!r:x");
    let mut view = TimelineView::default();
    view.apply_diffs(vec![ListDiff::Reset {
        values: vec![
            media_message("a", MediaKind::Image, 1600, 900),
            media_message("b", MediaKind::Image, 30, 20),
            media_message("c", MediaKind::Image, 0, 0),
            media_message("d", MediaKind::File, 0, 0),
        ],
    }]);
    view.scrolled(0.0, HEIGHT, WIDTH);

    let mut media = twicecat_app::media::MediaModel::default();
    let before = row_heights(&content_node_with(&view, &renderer, &media, Some(&room)));
    assert_eq!(before.len(), 4);

    // The estimate matches what the box lays out to, so the first measure
    // moves nothing either.
    for (i, height) in before.iter().enumerate() {
        let estimate = twicecat_app::timeline::estimate_height(
            &view.items[i],
            twicecat_app::timeline::is_continuation(&view.items, i),
            WIDTH,
            twicecat_app::ui::tokens::Density::Comfortable,
        );
        assert!(
            (estimate - height).abs() <= 8.0,
            "row {i}: estimated {estimate}, laid out {height}"
        );
    }

    // Thumbnails arrive for the images, one of them undecodable.
    for (id, ok) in [("a", true), ("b", true), ("c", false)] {
        let key = (room.clone(), id.to_owned());
        assert!(media.needs_fetch(&key));
        let decoded = ok
            .then(|| {
                twicecat_app::media::decode_thumbnail(&{
                    let img = image::RgbaImage::new(64, 36);
                    let mut out = std::io::Cursor::new(Vec::new());
                    img.write_to(&mut out, image::ImageFormat::Png)
                        .expect("encode");
                    out.into_inner()
                })
            })
            .flatten();
        media.thumb_decoded(&key, decoded);
    }
    let after = row_heights(&content_node_with(&view, &renderer, &media, Some(&room)));
    assert_eq!(after, before);
}

/// A file being uploaded: a local echo with progress.
fn uploading(id: &str, sent: u64) -> TimelineItem {
    let mut media =
        twicecat_core::api::MediaContent::new(twicecat_core::api::MediaKind::File, "f.bin".into());
    media.upload = Some(twicecat_core::api::UploadProgress { sent, total: 100 });
    TimelineItem::new(
        id.to_owned(),
        TimelineItemKind::Message {
            sender: UserId::from("@a:x"),
            body: "f.bin".to_owned(),
            is_own: true,
            timestamp_ms: NOW,
            sent: SendStatus::Sending,
            details: Box::new(MessageDetails {
                media: Some(Box::new(media)),
                ..MessageDetails::default()
            }),
        },
    )
}

#[test]
fn upload_progress_changes_no_laid_out_row_height() {
    let renderer = renderer();
    let heights = |sent| {
        let mut view = TimelineView::default();
        view.apply_diffs(vec![ListDiff::Reset {
            values: vec![uploading("u", sent)],
        }]);
        view.scrolled(0.0, HEIGHT, WIDTH);
        row_heights(&content_node(&view, &renderer))
    };
    let (start, middle, end) = (heights(0), heights(50), heights(100));
    assert_eq!(start.len(), 1);
    assert_eq!(start, middle);
    assert_eq!(middle, end);
    // The bar and **Cancel** add a line under the card.
    let plain = {
        let mut view = TimelineView::default();
        view.apply_diffs(vec![ListDiff::Reset {
            values: vec![media_message(
                "p",
                twicecat_core::api::MediaKind::File,
                0,
                0,
            )],
        }]);
        view.scrolled(0.0, HEIGHT, WIDTH);
        row_heights(&content_node(&view, &renderer))
    };
    assert!(start[0] > plain[0], "{start:?} vs {plain:?}");
}

/// Every row the first window lays out is within 25 % of its estimate, for
/// both densities (Task 9). The estimate is what the scrollbar and the
/// spacers use before a row is measured, so a restyle that changes row
/// heights must move the estimate with it.
#[test]
fn estimates_stay_within_a_quarter_of_the_laid_out_rows_for_both_densities() {
    use twicecat_app::timeline::{estimate_height, is_continuation};
    use twicecat_app::ui::tokens::Density;
    let renderer = renderer();
    for density in [Density::Comfortable, Density::Compact] {
        let mut view = timeline(300);
        view.set_density(density);
        let node = content_node(&view, &renderer);
        let window = view.window();
        let mut checked = 0;
        for (i, row) in window.zip(&node.children()[1..]) {
            let estimate = estimate_height(
                &view.items[i],
                is_continuation(&view.items, i),
                WIDTH,
                density,
            );
            let laid_out = row.bounds().height;
            assert!(
                (estimate - laid_out).abs() <= 0.25 * laid_out,
                "{density:?} row {i}: estimated {estimate}, laid out {laid_out}"
            );
            checked += 1;
        }
        assert!(checked >= 10, "{density:?}: only {checked} rows checked");
    }
}

/// A compact row is shorter than a comfortable one, as laid out.
#[test]
fn a_compact_timeline_lays_out_shorter_rows_on_average() {
    use twicecat_app::ui::tokens::Density;
    let renderer = renderer();
    let average = |density| {
        let mut view = timeline(300);
        view.set_density(density);
        let heights = row_heights(&content_node(&view, &renderer));
        heights.iter().sum::<f32>() / heights.len() as f32
    };
    let (comfortable, compact) = (average(Density::Comfortable), average(Density::Compact));
    assert!(compact < comfortable, "{compact} vs {comfortable}");
}

/// A message with `decorate` applied to its details.
fn decorated(id: &str, decorate: impl FnOnce(&mut MessageDetails)) -> TimelineItem {
    let mut details = MessageDetails::default();
    decorate(&mut details);
    TimelineItem::new(
        id.to_owned(),
        TimelineItemKind::Message {
            sender: UserId::from("@a:x"),
            body: "a short line of text".to_owned(),
            is_own: false,
            timestamp_ms: NOW,
            sent: SendStatus::Sent,
            details: Box::new(details),
        },
    )
}

/// The parts that restyling changed (replies, reaction chips, the edited
/// marker, a thread line, a failed send) keep their estimates within 25 %
/// of the laid out row, in both densities.
#[test]
fn decorated_rows_are_estimated_within_a_quarter_in_both_densities() {
    use twicecat_app::timeline::{estimate_height, is_continuation};
    use twicecat_app::ui::tokens::Density;
    use twicecat_core::api::{EventId, ThreadSummaryView};
    use twicecat_core::api::{ReactionGroup, ReplyPreview, ReplyState, SendFailure};
    let mut items = vec![
        decorated("plain", |_| {}),
        decorated("reply", |d| {
            d.reply_to = Some(ReplyPreview {
                event_id: EventId::from("$o:x"),
                sender: UserId::from("@b:x"),
                sender_name: Some("Bea".into()),
                body: "the quoted line".into(),
                state: ReplyState::Ready,
            });
        }),
        decorated("reactions", |d| {
            d.reactions = vec![ReactionGroup {
                key: "+1".into(),
                count: 2,
                own: true,
                senders: Vec::new(),
            }];
        }),
        decorated("edited", |d| d.edited = true),
        decorated("failed", |d| {
            d.send_failure = Some(SendFailure { recoverable: true });
        }),
        decorated("thread", |d| {
            d.thread = Some(ThreadSummaryView {
                replies: 3,
                latest_sender: None,
                latest_sender_name: None,
                latest_preview: None,
            });
        }),
    ];
    // The failure line shows for a failed send, not for a sent message.
    if let TimelineItemKind::Message { sent, .. } = &mut items[4].kind {
        *sent = SendStatus::Failed;
    }
    // The thread line needs the root's event ID to open the thread.
    items[5].event_id = Some(EventId::from("$t:x"));
    let renderer = renderer();
    for density in [Density::Comfortable, Density::Compact] {
        let mut view = TimelineView::default();
        view.apply_diffs(vec![ListDiff::Reset {
            values: items.clone(),
        }]);
        view.set_density(density);
        view.scrolled(0.0, HEIGHT, WIDTH);
        let node = content_node(&view, &renderer);
        for (i, row) in view.window().zip(&node.children()[1..]) {
            let estimate = estimate_height(
                &view.items[i],
                is_continuation(&view.items, i),
                WIDTH,
                density,
            );
            let laid_out = row.bounds().height;
            assert!(
                (estimate - laid_out).abs() <= 0.25 * laid_out,
                "{density:?} {}: estimated {estimate}, laid out {laid_out}",
                view.items[i].id
            );
        }
    }
}
