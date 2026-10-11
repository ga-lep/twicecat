//! The full-size image viewer and the "Open <file>?" bar (plan Q7, Q8,
//! Task 9).
//!
//! The viewer is a layer above the main view in a stack that is always two
//! deep, so opening it keeps the timeline's scroll offset and the composer's
//! focus. It shows one decoded image at a time; closing drops it.

use iced::widget::{button, column, container, image, opaque, row, space, stack, text};
use iced::{Background, Color, ContentFit, Element, Fill, Theme};

use crate::media::{OpenBar, Viewer, card_name};
use crate::state::{App, Message};

/// The base with the viewer as the layer above it: always a stack of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let viewer = app
        .media
        .viewer
        .as_ref()
        .map(|viewer| overlay(viewer, title(app, viewer)));
    stack([base, viewer.unwrap_or_else(|| space().into())]).into()
}

/// The sanitised file name of the viewed image.
fn title(app: &App, viewer: &Viewer) -> String {
    let (room, item_id) = &viewer.key;
    if app.selected.as_ref() != Some(room) {
        return String::new();
    }
    app.timeline
        .items
        .iter()
        .find(|item| item.id == *item_id)
        .and_then(|item| match &item.kind {
            tessera_core::api::TimelineItemKind::Message { details, .. } => details
                .media
                .as_deref()
                .map(|media| card_name(&media.filename)),
            _ => None,
        })
        .unwrap_or_default()
}

fn backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.88))),
        text_color: Some(Color::WHITE),
        ..container::Style::default()
    }
}

/// The viewer layer: a bar with the name, **Save...** and **Close**, and the
/// image below it, which zooms with the wheel and pans by dragging.
fn overlay<'a>(viewer: &'a Viewer, name: String) -> Element<'a, Message> {
    let item_id = viewer.key.1.clone();
    let bar = row![
        text(name).width(Fill),
        button(text("Save\u{2026}"))
            .on_press(Message::SaveRequested(item_id))
            .style(button::secondary),
        button(text("Close"))
            .on_press(Message::ViewerClosed)
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center);
    let picture: Element<'a, Message> = match viewer.decoded() {
        Some(decoded) => image::viewer(decoded.handle.clone())
            .width(Fill)
            .height(Fill)
            .content_fit(ContentFit::Contain)
            .into(),
        None if viewer.failed() => container(text("The image could not be shown."))
            .center(Fill)
            .into(),
        None => container(text("Loading\u{2026}")).center(Fill).into(),
    };
    opaque(
        container(column![bar, picture].spacing(8).width(Fill).height(Fill))
            .padding(12)
            .width(Fill)
            .height(Fill)
            .style(backdrop),
    )
}

/// The bar above the composer that asks before a saved file is opened.
pub fn open_bar(bar: &OpenBar) -> Element<'_, Message> {
    // The question takes the free width, so a long warning wraps before
    // the buttons.
    let mut question = column![text(bar.question())].spacing(2).width(Fill);
    if let Some(warning) = bar.warning() {
        question = question.push(text(warning).size(13).style(text::danger));
    }
    let bar_row = row![
        question,
        button(text("Open")).on_press(Message::OpenConfirmed),
        button(text("Cancel"))
            .on_press(Message::OpenDismissed)
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center);
    container(bar_row).padding(4).into()
}
