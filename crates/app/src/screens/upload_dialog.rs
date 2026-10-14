//! The upload confirmation and the "Drop to upload" hint (plan Q9, Task 10;
//! restyled in UI/UX rework Task 15).
//!
//! Both are layers above the main view in a stack that is always two deep,
//! so opening one keeps the timeline's scroll offset and the composer's
//! focus. File names are the user's own but still go through `card_name`.
//!
//! The confirmation names the room the files go to (the dialog's own room,
//! cleaned), shows each file's preview, name, size and type, and has **Cancel**
//! and **Send** as different buttons, `tokens::M` apart. Nothing is sent until
//! **Send** is pressed; there is no text field in it, so Enter and a key
//! repeat do nothing. **Send** is off when the room on screen is not the room
//! the files were meant for. The model has no caption for a file (the core's
//! `OutgoingAttachment` has none), so none is shown.

use iced::widget::{Column, column, container, image, responsive, row, space, stack};
use iced::{Background, Color, ContentFit, Element, Fill, Length, Theme};

use crate::media::{file_title, kind_label, size_label};
use crate::state::{App, Message};
use crate::ui::icons::{self, Icon};
use crate::ui::kit::{
    Severity, banner, dialog, dialog_footer, dialog_header, muted, primary_button,
    secondary_button, tag, words,
};
use crate::ui::tokens;
use crate::upload::{Entry, EntryState, PREVIEW_MAX, Prepared, UploadDialog};
use crate::verification::elide_middle;

/// Width of the confirmation card.
pub const CARD_WIDTH: f32 = 520.0;
/// The note under a JPEG photo.
pub const EXIF_NOTE: &str = "JPEG photos can carry camera details such as the place where they \
                             were taken. The original is sent as it is.";
/// What the card says when the room on screen is not the files' room.
pub const WRONG_ROOM: &str = "These files were picked for a different room than the one on \
                              screen. Cancel and attach them again in the room you mean.";
/// Longest room name or ID shown, in characters; a longer one is cut in the
/// middle, so its end stays.
const ROOM_CHARS: usize = 80;

/// The base with the confirmation, the drop hint or nothing as the layer
/// above it: always a stack of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let top: Element<'a, Message> = match &app.uploads.dialog {
        Some(dialog) => responsive(move |size| confirmation(app, dialog, size.height)).into(),
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

/// The room the files go to, as the card names it: the room list's name for
/// it (cleaned and capped), else its ID cut in the middle. Taken from the
/// dialog's own room, not from the room on screen.
pub fn target_name(app: &App, dialog: &UploadDialog) -> String {
    app.rooms
        .iter()
        .find(|room| room.id == dialog.room_id)
        .map_or_else(
            || elide_middle(dialog.room_id.as_str(), ROOM_CHARS),
            |room| elide_middle(&room.name, ROOM_CHARS),
        )
}

/// Whether the room on screen is the one the files were picked for.
pub fn room_matches(app: &App, dialog: &UploadDialog) -> bool {
    app.selected.as_ref() == Some(&dialog.room_id)
}

/// A hint over the window while files are dragged over it. It takes no
/// clicks: the drag is the window system's.
fn drop_hint<'a>() -> Element<'a, Message> {
    container(words("Drop to upload").size(tokens::DISPLAY))
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

/// The picture box of a file: the real preview size for a lone file, a
/// smaller one in a list.
fn preview_box<'a>(entry: &Entry, alone: bool) -> Element<'a, Message> {
    // The real preview size only for a lone picture; a list, or a file with
    // no picture to show, gets the small box.
    let has_picture = matches!(&entry.state, EntryState::Ready(p) if p.preview.is_some());
    let scale = if alone && has_picture { 1.0 } else { 2.5 };
    let (w, h) = (PREVIEW_MAX.0 as f32 / scale, PREVIEW_MAX.1 as f32 / scale);
    let inner: Element<'a, Message> = match &entry.state {
        EntryState::Ready(prepared) => match &prepared.preview {
            Some(decoded) => image(decoded.handle.clone())
                .width(Length::Fixed(w))
                .height(Length::Fixed(h))
                .content_fit(ContentFit::Contain)
                .into(),
            None => words(kind_label(prepared.kind))
                .size(tokens::CAPTION)
                .style(muted)
                .into(),
        },
        EntryState::Loading => words("\u{2026}").style(muted).into(),
        EntryState::Refused(_) => container(icons::icon(Icon::Warning))
            .style(|theme: &Theme| container::Style {
                text_color: Some(Severity::Danger.color(theme)),
                ..container::Style::default()
            })
            .into(),
    };
    container(inner)
        .center(Length::Fixed(w))
        .width(Length::Fixed(w))
        .height(Length::Fixed(h))
        .into()
}

fn entry_row<'a>(entry: &Entry, alone: bool) -> Element<'a, Message> {
    let detail: Element<'a, Message> = match &entry.state {
        EntryState::Loading => words("Reading\u{2026}")
            .size(tokens::CAPTION)
            .style(muted)
            .into(),
        EntryState::Ready(prepared) => words(detail_text(prepared))
            .size(tokens::CAPTION)
            .style(muted)
            .into(),
        EntryState::Refused(why) => words(why.text())
            .size(tokens::CAPTION)
            .style(iced::widget::text::danger)
            .into(),
    };
    // The name takes the width the picture leaves and breaks inside a word,
    // so the end of a long file name (its extension) is always drawn.
    let label = column![words(file_title(&entry.name)), detail]
        .spacing(tokens::XS)
        .width(Fill);
    row![preview_box(entry, alone), label]
        .spacing(tokens::M)
        .align_y(iced::Center)
        .into()
}

fn confirmation<'a>(
    app: &'a App,
    dialog_state: &'a UploadDialog,
    window_height: f32,
) -> Element<'a, Message> {
    let alone = dialog_state.entries.len() == 1;
    let mut list = Column::new().spacing(tokens::M).width(Fill);
    for entry in &dialog_state.entries {
        list = list.push(entry_row(entry, alone));
    }
    let exif = dialog_state.entries.iter().any(
        |entry| matches!(&entry.state, EntryState::Ready(prepared) if prepared.has_exif_note()),
    );
    let matches = room_matches(app, dialog_state);
    let encrypted = app
        .rooms
        .iter()
        .find(|room| room.id == dialog_state.room_id)
        .map(|room| room.is_encrypted);
    // Where the files go: the room's name, cleaned, with whether it is
    // encrypted, above the files.
    let mut target = row![
        words("To").size(tokens::CAPTION).style(muted),
        words(target_name(app, dialog_state))
            .size(tokens::BODY_LARGE)
            .width(Fill),
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    if let Some(encrypted) = encrypted {
        target = target.push(tag(if encrypted {
            Icon::Lock.label()
        } else {
            Icon::LockOpen.label()
        }));
    }
    let mut body = Column::new().spacing(tokens::M).width(Fill).push(target);
    let wrong_room: Element<'a, Message> = if matches {
        space().into()
    } else {
        banner(Severity::Danger, WRONG_ROOM)
    };
    body = body.push(wrong_room);
    body = body.push(list);
    let exif_note: Element<'a, Message> = if exif {
        words(EXIF_NOTE).size(tokens::CAPTION).style(muted).into()
    } else {
        space().into()
    };
    body = body.push(exif_note);
    let send = primary_button(
        "Send",
        (dialog_state.can_send() && matches).then_some(Message::UploadConfirmed),
    );
    let footer = dialog_footer(vec![
        secondary_button("Cancel", Some(Message::UploadCancelled)).into(),
        send.into(),
    ]);
    dialog(
        dialog_header(
            title_text(dialog_state.entries.len()),
            Message::UploadCancelled,
        ),
        body.into(),
        Some(footer),
        CARD_WIDTH,
        window_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::EventsSlot;
    use crate::state::Startup;
    use crate::upload::{Outcome, prepare_bytes};
    use twicecat_core::api::{RoomId, RoomSummary};

    fn app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let (_etx, erx) = tokio::sync::mpsc::channel(1);
        App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(erx),
        })
        .0
    }

    fn dialog_for(room: &str) -> UploadDialog {
        UploadDialog {
            room_id: RoomId::from(room),
            entries: Vec::new(),
        }
    }

    #[test]
    fn the_card_names_the_dialogs_own_room_cleaned_and_cut_in_the_middle() {
        let mut app = app();
        app.rooms
            .push(RoomSummary::new(RoomId::from("!a:x"), "Lounge".to_owned()));
        let long = format!("{}end", "n".repeat(300));
        app.rooms.push(RoomSummary::new(RoomId::from("!b:x"), long));
        app.rooms.push(RoomSummary::new(
            RoomId::from("!c:x"),
            "A\u{202e}B".to_owned(),
        ));
        assert_eq!(target_name(&app, &dialog_for("!a:x")), "Lounge");
        let cut = target_name(&app, &dialog_for("!b:x"));
        assert_eq!(cut.chars().count(), ROOM_CHARS);
        assert!(cut.ends_with("end"), "{cut}");
        assert!(!target_name(&app, &dialog_for("!c:x")).contains('\u{202e}'));
        // A room the list lacks is named by its ID, cut in the middle.
        let unknown = format!("!{}:server.example", "r".repeat(200));
        let shown = target_name(&app, &dialog_for(&unknown));
        assert!(shown.ends_with("server.example") && shown.chars().count() <= ROOM_CHARS);
    }

    #[test]
    fn files_are_only_sendable_while_the_room_on_screen_is_the_dialogs_room() {
        let mut app = app();
        let dialog = dialog_for("!a:x");
        assert!(!room_matches(&app, &dialog), "no room on screen");
        app.selected = Some(RoomId::from("!a:x"));
        assert!(room_matches(&app, &dialog));
        app.selected = Some(RoomId::from("!b:x"));
        assert!(!room_matches(&app, &dialog));
    }

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
