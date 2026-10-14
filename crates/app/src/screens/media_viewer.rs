//! The full-size image viewer and the "Open <file>?" bar (plan Q7, Q8,
//! Task 9; restyled in UI/UX rework Task 15).
//!
//! The viewer is a layer above the main view in a stack that is always two
//! deep, so opening it keeps the timeline's scroll offset and the composer's
//! focus. It shows one decoded image at a time, centred on a dimmed backdrop
//! that takes every click, with a bar at the bottom: the file's name, its type
//! and size, **Save...** and **Close**. Closing drops the image.
//!
//! What the image widget does with the mouse is what it did before (it zooms
//! with the wheel and pans by dragging); nothing here adds a gesture, and the
//! decode limits are the ones of [`crate::media`]. The name and the type are
//! the sender's: cleaned, capped, and broken inside a word when they have no
//! break, so the end of a file name is never drawn under a button.

use iced::widget::{column, container, image, opaque, row, space, stack};
use iced::{Background, Color, ContentFit, Element, Fill, Theme};
use twicecat_core::api::{MediaContent, TimelineItemKind};

use crate::media::{OpenBar, Viewer, file_title, size_label, type_label};
use crate::state::{App, Message};
use crate::ui::kit::{self, muted, primary_button, secondary_button, words};
use crate::ui::tokens;

/// The base with the viewer as the layer above it: always a stack of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let viewer = app
        .media
        .viewer
        .as_ref()
        .map(|viewer| overlay(viewer, media_of(app, viewer)));
    stack([base, viewer.unwrap_or_else(|| space().into())]).into()
}

/// The media message behind the viewer, if the room on screen still has it.
fn media_of<'a>(app: &'a App, viewer: &Viewer) -> Option<&'a MediaContent> {
    let (room, item_id) = &viewer.key;
    if app.selected.as_ref() != Some(room) {
        return None;
    }
    app.timeline
        .items
        .iter()
        .find(|item| item.id == *item_id)
        .and_then(|item| match &item.kind {
            TimelineItemKind::Message { details, .. } => details.media.as_deref(),
            _ => None,
        })
}

/// The sanitised file name of the viewed image.
pub fn title(app: &App, viewer: &Viewer) -> String {
    media_of(app, viewer)
        .map(|media| file_title(&media.filename))
        .unwrap_or_default()
}

/// The line under the name: type, declared size, and the size in pixels of
/// the image that was decoded.
pub fn detail(media: Option<&MediaContent>, viewer: &Viewer) -> String {
    let mut parts = Vec::new();
    if let Some(media) = media {
        parts.push(type_label(media));
        parts.push(size_label(media.size));
    }
    if let Some(decoded) = viewer.decoded() {
        parts.push(format!("{} \u{d7} {} px", decoded.width, decoded.height));
    }
    parts.join(" \u{b7} ")
}

fn backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.88))),
        ..container::Style::default()
    }
}

/// The viewer layer: the image, which zooms with the wheel and pans by
/// dragging, and the bar below it.
fn overlay<'a>(viewer: &'a Viewer, media: Option<&MediaContent>) -> Element<'a, Message> {
    let item_id = viewer.key.1.clone();
    let name = media
        .map(|media| file_title(&media.filename))
        .unwrap_or_default();
    // The bar is a card, so its words keep the theme's contrast on the dim
    // backdrop in the light colour sets too.
    let bar = container(
        row![
            column![
                words(name).size(tokens::BODY_LARGE),
                words(detail(media, viewer))
                    .size(tokens::CAPTION)
                    .style(muted)
            ]
            .spacing(tokens::XS)
            .width(Fill),
            secondary_button("Save\u{2026}", Some(Message::SaveRequested(item_id))),
            primary_button("Close", Some(Message::ViewerClosed)),
        ]
        .spacing(tokens::M)
        .align_y(iced::Center),
    )
    .padding(tokens::M)
    .width(Fill)
    .style(kit::card_style);
    let picture: Element<'a, Message> = match viewer.decoded() {
        Some(decoded) => image::viewer(decoded.handle.clone())
            .width(Fill)
            .height(Fill)
            .content_fit(ContentFit::Contain)
            .into(),
        None => container(
            container(words(if viewer.failed() {
                "The image could not be shown."
            } else {
                "Loading\u{2026}"
            }))
            .padding(tokens::L)
            .style(kit::card_style),
        )
        .center(Fill)
        .into(),
    };
    opaque(
        container(
            column![picture, bar]
                .spacing(tokens::M)
                .width(Fill)
                .height(Fill),
        )
        .padding(tokens::M)
        .width(Fill)
        .height(Fill)
        .style(backdrop),
    )
}

/// The bar above the composer that asks before a saved file is opened. The
/// question has the whole width; the buttons are on a row of their own,
/// **Cancel** first and **Open** last. A file that may run code gets a red
/// **Open** and a warning, and the buttons are `tokens::M` apart.
pub fn open_bar(bar: &OpenBar) -> Element<'_, Message> {
    let warning = bar.warning();
    let mut lines = column![words(bar.question())]
        .spacing(tokens::XS)
        .width(Fill);
    if let Some(warning) = warning {
        lines = lines.push(
            words(warning)
                .size(tokens::BODY)
                .style(iced::widget::text::danger),
        );
    }
    let open = if warning.is_some() {
        kit::danger_button("Open", Some(Message::OpenConfirmed))
    } else {
        primary_button("Open", Some(Message::OpenConfirmed))
    };
    let severity = if warning.is_some() {
        kit::Severity::Warning
    } else {
        kit::Severity::Info
    };
    container(
        column![
            lines,
            kit::dialog_footer(vec![
                secondary_button("Cancel", Some(Message::OpenDismissed)).into(),
                open.into()
            ])
        ]
        .spacing(tokens::S),
    )
    .padding(tokens::M)
    .width(Fill)
    .style(kit::banner_style(severity))
    .into()
}
