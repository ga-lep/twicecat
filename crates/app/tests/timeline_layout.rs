//! Lays out the timeline pane headlessly (tiny-skia, no window) for a
//! 10,000-item timeline and checks that only the rows around the viewport
//! exist, and that the spacers keep the content height (plan Q15c).

use chrono::Utc;
use iced::advanced::layout::{Limits, Node};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Tree;
use iced::{Font, Pixels, Renderer, Size};
use tessera_app::rich_text::Revealed;
use tessera_app::screens::timeline_pane::{RichEnv, view_in};
use tessera_app::timeline::{MAX_BUILT_ROWS, TimelineView};
use tessera_core::api::{
    ListDiff, MessageDetails, SendStatus, StateChange, TimelineItem, TimelineItemKind, UserId,
};

const NOW: u64 = 1_790_000_000_000;
const WIDTH: f32 = 800.0;
const HEIGHT: f32 = 600.0;

fn renderer() -> Renderer {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a current-thread runtime");
    runtime
        .block_on(Renderer::new(
            Font::DEFAULT,
            Pixels(16.0),
            Some("tiny-skia"),
        ))
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
    let theme = tessera_app::theme::theme_for(iced::theme::Mode::Light);
    let revealed = Revealed::new();
    let actions = tessera_app::actions::ActionState::default();
    let env = RichEnv {
        theme: &theme,
        own_user: None,
        revealed: &revealed,
        actions: &actions,
        rooms: &[],
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
