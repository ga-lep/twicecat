//! The thread panel (plan Q16, Task 13): a column beside the room pane with
//! the thread's timeline, a text field and **Send**.
//!
//! The panel is always one slot of the main row (an empty space when closed)
//! and always three children (header, timeline, composer), so opening or
//! closing it never resets the room timeline's scroll or the room composer's
//! focus.

use chrono::Local;
use iced::widget::{Column, button, column, container, row, rule, space, text, text_input};
use iced::{Element, Fill};
use tessera_core::api::MessageText;

use crate::screens::main_view::rich_env;
use crate::screens::timeline_pane::{self, Pane};
use crate::state::{App, Message};
use crate::theme;
use crate::threads::{PANEL_WIDTH, THREAD_INPUT_ID, ThreadPanel, ThreadsMessage};

/// The panel, or an empty slot when no thread is open.
pub fn slot(app: &App) -> Element<'_, Message> {
    match &app.threads.panel {
        Some(panel) => view(app, panel),
        None => space().width(0).into(),
    }
}

fn view<'a>(app: &'a App, panel: &'a ThreadPanel) -> Element<'a, Message> {
    let header = row![
        text("Thread").size(18).width(Fill),
        button(text("\u{2715}"))
            .on_press(Message::Threads(ThreadsMessage::Close))
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center);
    let theme = theme::theme_for(app.theme_mode);
    let now = Local::now().timestamp_millis();
    let timeline = container(timeline_pane::view_pane(
        &panel.timeline,
        u64::try_from(now).unwrap_or(0),
        &Local,
        rich_env(app, &theme),
        Pane::THREAD,
    ))
    .height(Fill);
    let body: Column<'a, Message> =
        Column::from_vec(vec![header.into(), timeline.into(), composer(panel)])
            .spacing(8)
            .width(Fill)
            .height(Fill);
    row![
        rule::vertical(1),
        container(body).padding(8).width(Fill).height(Fill)
    ]
    .width(PANEL_WIDTH)
    .height(Fill)
    .into()
}

/// The text field with **Attach...** and **Send**. Enter sends; the field
/// holds one line (the room's composer is the place for long text).
fn composer(panel: &ThreadPanel) -> Element<'_, Message> {
    let attach = button(text("Attach\u{2026}").size(13))
        .padding([4, 8])
        .style(button::secondary)
        .on_press(Message::Threads(ThreadsMessage::Attach));
    let field = text_input("Reply in thread", &panel.draft)
        .id(THREAD_INPUT_ID)
        .on_input(|text| Message::Threads(ThreadsMessage::Draft(MessageText::from(text))))
        .on_submit(Message::Threads(ThreadsMessage::Submit));
    let send = button(text("Send").size(13))
        .padding([4, 8])
        .on_press_maybe(
            (!panel.draft.trim().is_empty()).then_some(Message::Threads(ThreadsMessage::Submit)),
        );
    column![row![field, send].spacing(6), attach]
        .spacing(6)
        .into()
}
