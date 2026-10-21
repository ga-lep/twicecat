//! The thread panel (plan Q16, Task 13): a column beside the room pane with
//! the thread's timeline, a text field and **Send**.
//!
//! The panel is always one slot of the main row (an empty space when closed)
//! and always three children (header, timeline, composer), so opening or
//! closing it never resets the room timeline's scroll or the room composer's
//! focus.

use chrono::Local;
use iced::widget::{Column, Row, column, container, row, space, text, text_input};
use iced::{Element, Fill, Length};
use twicecat_core::api::MessageText;

use crate::layout::PaneLayout;
use crate::screens::main_view::{self, rich_env};
use crate::screens::timeline_pane::{self, Pane};
use crate::state::{App, Message};
use crate::threads::{PANEL_WIDTH, THREAD_INPUT_ID, ThreadPanel, ThreadsMessage};
use crate::ui::icons::Icon;
use crate::ui::kit;
use crate::ui::tokens;

/// The panel, or an empty slot when no thread is open.
pub fn slot(app: &App, layout: PaneLayout) -> Element<'_, Message> {
    match &app.threads.panel {
        Some(panel) if layout.thread => view(app, panel, layout.panel_fills),
        _ => space().width(0).into(),
    }
}

/// The panel. With `fills` it takes the room pane's width (a narrow window)
/// and its header carries the room's name and lock and the trust tag of a
/// direct message, since the room header is out of sight.
fn view<'a>(app: &'a App, panel: &'a ThreadPanel, fills: bool) -> Element<'a, Message> {
    let mut head: Vec<Element<'a, Message>> = vec![main_view::panel_title(
        app,
        text("Thread").size(tokens::TITLE),
        fills,
    )];
    if let Some(trust) = fills.then(|| main_view::trust_strip(app)).flatten() {
        head.push(trust);
    }
    head.push(kit::icon_button(
        Icon::Close,
        Message::Threads(ThreadsMessage::Close),
    ));
    let header = Row::with_children(head)
        .spacing(tokens::S)
        .align_y(iced::Center);
    let theme = app.current_theme();
    let now = Local::now().timestamp_millis();
    let timeline = container(main_view::timeline_layer(
        timeline_pane::view_pane(
            &panel.timeline,
            u64::try_from(now).unwrap_or(0),
            &Local,
            rich_env(app, &theme),
            Pane::THREAD,
        ),
        main_view::picker_layer(app, true),
    ))
    .height(Fill);
    let body: Column<'a, Message> =
        Column::from_vec(vec![header.into(), timeline.into(), composer(panel)])
            .spacing(8)
            .width(Fill)
            .height(Fill);
    row![
        kit::vertical_divider(),
        container(body)
            .padding(tokens::S)
            .width(Fill)
            .height(Fill)
            .style(kit::surface_style)
    ]
    .width(if fills {
        Length::Fill
    } else {
        Length::Fixed(PANEL_WIDTH)
    })
    .height(Fill)
    .into()
}

/// The text field with **Attach...** and **Send**. Enter sends; the field
/// holds one line (the room's composer is the place for long text).
fn composer(panel: &ThreadPanel) -> Element<'_, Message> {
    let attach = kit::secondary_button(
        "Attach\u{2026}",
        Some(Message::Threads(ThreadsMessage::Attach)),
    );
    let field = text_input("Reply in thread", &panel.draft)
        .style(kit::text_input_style)
        .id(THREAD_INPUT_ID)
        .on_input(|text| Message::Threads(ThreadsMessage::Draft(MessageText::from(text))))
        .on_submit(Message::Threads(ThreadsMessage::Submit));
    let send = kit::primary_button(
        "Send",
        (!panel.draft.trim().is_empty()).then_some(Message::Threads(ThreadsMessage::Submit)),
    );
    column![row![field, send].spacing(6), attach]
        .spacing(6)
        .into()
}
