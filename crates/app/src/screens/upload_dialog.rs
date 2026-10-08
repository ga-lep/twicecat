//! The upload confirmation and the "Drop to upload" hint (plan Q9, Task 10).
//!
//! Both are layers above the main view in a stack that is always two deep,
//! so opening one keeps the timeline's scroll offset and the composer's
//! focus. File names are the user's own but still go through `card_name`.

use iced::widget::{
    Column, button, column, container, image, opaque, row, scrollable, space, stack, text,
};
use iced::{Background, Color, ContentFit, Element, Fill, Length, Theme};

use crate::media::{card_name, kind_label, size_label};
use crate::state::{App, Message};
use crate::theme;
use crate::upload::{Entry, EntryState, PREVIEW_MAX, Prepared, UploadDialog};

/// Width of the confirmation card.
const CARD_WIDTH: f32 = 520.0;
/// Tallest the list of files grows before it scrolls.
const LIST_MAX_HEIGHT: f32 = 360.0;
/// The note under a JPEG photo.
pub const EXIF_NOTE: &str = "JPEG photos can carry camera details such as the place where they \
                             were taken. The original is sent as it is.";

/// The base with the confirmation, the drop hint or nothing as the layer
/// above it: always a stack of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let top: Element<'a, Message> = match &app.uploads.dialog {
        Some(dialog) => confirmation(dialog),
        None if app.uploads.hovering => drop_hint(),
        None => space().into(),
    };
    stack([base, top]).into()
}

/// "Send 3 files".
pub fn title_text(count: usize) -> String {
    match count {
        1 => "Send 1 file".to_owned(),
        n => format!("Send {n} files"),
    }
}

/// The line under a ready file's name: size and type.
pub fn detail_text(prepared: &Prepared) -> String {
    format!(
        "{} \u{b7} {}",
        size_label(Some(prepared.size)),
        prepared.mime
    )
}

fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme::semantic(theme).muted),
    }
}

fn backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
        ..container::Style::default()
    }
}

/// A hint over the window while files are dragged over it. It takes no
/// clicks: the drag is the window system's.
fn drop_hint<'a>() -> Element<'a, Message> {
    container(text("Drop to upload").size(28))
        .center(Fill)
        .style(|theme: &Theme| container::Style {
            background: Some(Background::Color(Color {
                a: 0.85,
                ..theme.palette().background
            })),
            text_color: Some(theme.palette().text),
            ..container::Style::default()
        })
        .into()
}

fn preview_box<'a>(entry: &Entry) -> Element<'a, Message> {
    let (w, h) = (PREVIEW_MAX.0 as f32 / 2.5, PREVIEW_MAX.1 as f32 / 2.5);
    let inner: Element<'a, Message> = match &entry.state {
        EntryState::Ready(prepared) => match &prepared.preview {
            Some(decoded) => image(decoded.handle.clone())
                .width(Length::Fixed(w))
                .height(Length::Fixed(h))
                .content_fit(ContentFit::Contain)
                .into(),
            None => text(kind_label(prepared.kind)).size(13).style(muted).into(),
        },
        EntryState::Loading => text("\u{2026}").style(muted).into(),
        EntryState::Refused(_) => text("\u{2715}").style(text::danger).into(),
    };
    container(inner)
        .center(Length::Fixed(w.max(h)))
        .width(Length::Fixed(w.max(h)))
        .height(Length::Fixed(h))
        .into()
}

fn entry_row<'a>(entry: &Entry) -> Element<'a, Message> {
    let detail: Element<'a, Message> = match &entry.state {
        EntryState::Loading => text("Reading\u{2026}").size(12).style(muted).into(),
        EntryState::Ready(prepared) => text(detail_text(prepared)).size(12).style(muted).into(),
        EntryState::Refused(why) => text(why.text()).size(12).style(text::danger).into(),
    };
    let label = column![
        text(card_name(&entry.name)).wrapping(text::Wrapping::None),
        detail
    ]
    .width(Fill);
    row![preview_box(entry), label]
        .spacing(8)
        .align_y(iced::Center)
        .into()
}

fn confirmation<'a>(dialog: &'a UploadDialog) -> Element<'a, Message> {
    let mut list = Column::new().spacing(8).width(Fill);
    for entry in &dialog.entries {
        list = list.push(entry_row(entry));
    }
    let exif = dialog.entries.iter().any(
        |entry| matches!(&entry.state, EntryState::Ready(prepared) if prepared.has_exif_note()),
    );
    let mut body = Column::new()
        .spacing(12)
        .push(text(title_text(dialog.entries.len())).size(20))
        .push(scrollable(list).height(Length::Shrink));
    if exif {
        body = body.push(text(EXIF_NOTE).size(12).style(muted));
    }
    let send =
        button(text("Send")).on_press_maybe(dialog.can_send().then_some(Message::UploadConfirmed));
    body = body.push(
        row![
            space().width(Fill),
            button(text("Cancel"))
                .on_press(Message::UploadCancelled)
                .style(button::secondary),
            send,
        ]
        .spacing(8),
    );
    let card = container(body)
        .padding(16)
        .width(Length::Fixed(CARD_WIDTH))
        .max_height(LIST_MAX_HEIGHT + 160.0)
        .style(container::bordered_box);
    opaque(
        container(card)
            .center(Fill)
            .width(Fill)
            .height(Fill)
            .style(backdrop),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upload::{Outcome, prepare_bytes};

    #[test]
    fn titles_count_the_files() {
        assert_eq!(title_text(1), "Send 1 file");
        assert_eq!(title_text(3), "Send 3 files");
    }

    #[test]
    fn the_detail_line_has_size_and_type() {
        let Outcome::Ready(p) = prepare_bytes("a.txt".into(), b"hello".to_vec(), None) else {
            panic!("ready");
        };
        assert_eq!(detail_text(&p), "5 B \u{b7} text/plain");
    }

    #[test]
    fn the_exif_note_says_the_original_is_sent_as_it_is() {
        assert!(EXIF_NOTE.contains("original"));
        assert!(EXIF_NOTE.contains("place"));
    }
}
