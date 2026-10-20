//! The options cards of the spaces rail, of the section headers and of the
//! room rows of a space's sectioned list, and the name dialog (sidebar plan,
//! Tasks 6 and 9). All are a [`kit::dialog`]: a header with a close button, a
//! body, and a footer.
//!
//! The layer is always a stack of two (the base first), like the other
//! overlays, so opening it keeps the timeline's scroll offset and the
//! composer's state. The "Move to folder" and "Move to section" lists are
//! `pick_list`s inside [`kit::menu`], so Ctrl + wheel cannot pick one. Space and folder names
//! come from other people and from the user's own typing; they pass through
//! `text_safety` before they are drawn. The views only read the app state;
//! every change goes through `App::update`.

use iced::widget::{Column, column, pick_list, responsive, space, stack, text_input};
use iced::{Element, Fill};
use twicecat_core::api::{RoomId, SectionKey};

use crate::room_list::row_name;
use crate::screens::room_dialogs::error_line;
use crate::screens::spaces_sidebar::space_name;
use crate::sections::{Line, drawn_sections, spot_of};
use crate::sidebar_menu::{
    Entry, FolderChoice, NAME_ID, NameDialog, Purpose, SectionChoice, SidebarMenu,
    SidebarMenuMessage, Target, entries, folder_choices, place_of, room_entries, section_choices,
    section_entries,
};
use crate::spaces::RailItem;
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder};
use crate::ui::kit::{
    self, dialog, dialog_footer, dialog_header, field, muted, primary_button, secondary_button,
    words,
};
use crate::ui::tokens;

/// Width of the card and of the dialog.
pub const CARD_WIDTH: f32 = 380.0;

fn msg(message: SidebarMenuMessage) -> Message {
    Message::SidebarMenu(message)
}

/// The base with the card or the dialog on top, or nothing: always a stack of
/// two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    stack([base, top(app).unwrap_or_else(|| space().into())]).into()
}

fn top(app: &App) -> Option<Element<'_, Message>> {
    let menu = app.sidebar_menu.as_ref()?;
    Some(
        responsive(move |size| match menu {
            SidebarMenu::Card { target, error } => card(app, target, *error, size.height),
            SidebarMenu::Name(dialog) => name_dialog(dialog, size.height),
        })
        .into(),
    )
}

/// The title of the card of `target`: the space's name, or the folder's (or
/// "Folder").
fn title_of(app: &App, items: &[RailItem<'_>], target: &Target) -> String {
    match target {
        Target::Space(id) => app
            .spaces
            .spaces()
            .iter()
            .find(|space| space.id == *id)
            .map_or_else(|| "Space".to_owned(), space_name),
        Target::Folder(id) => items
            .iter()
            .find_map(|item| match item {
                RailItem::Folder(folder) if folder.id == *id => Some(folder.name),
                _ => None,
            })
            .flatten()
            .map_or_else(
                || "Folder".to_owned(),
                |name| name_or_placeholder(name, MAX_NAME_CHARS),
            ),
        // These cards have their own (`section_card` and `room_card`).
        Target::Section { .. } | Target::Room { .. } => String::new(),
    }
}

fn card<'a>(
    app: &'a App,
    target: &'a Target,
    error: Option<&'static str>,
    window_height: f32,
) -> Element<'a, Message> {
    match target {
        Target::Space(_) | Target::Folder(_) => rail_card(app, target, error, window_height),
        Target::Section { key, .. } => section_card(app, key, error, window_height),
        Target::Room { space, room } => room_card(app, space, room, error, window_height),
    }
}

/// The frame all cards share: the title with its close button, the body, and
/// a Close button in the footer.
fn frame<'a>(
    title: String,
    body: Column<'a, Message>,
    error: Option<&'static str>,
    window_height: f32,
) -> Element<'a, Message> {
    dialog(
        dialog_header(title, msg(SidebarMenuMessage::Close)),
        body.push(error_line(error)).into(),
        Some(dialog_footer(vec![
            secondary_button("Close", Some(msg(SidebarMenuMessage::Close))).into(),
        ])),
        CARD_WIDTH,
        window_height,
    )
}

/// The button of a card entry: it sends the entry, or is disabled.
fn entry_button<'a>(entry: Entry, enabled: bool) -> Element<'a, Message> {
    secondary_button(
        entry.label(),
        enabled.then(|| msg(SidebarMenuMessage::Entry(entry))),
    )
    .width(Fill)
    .into()
}

/// The card of a section header: its moves, Collapse or Expand, New section,
/// and for the user's own sections Rename and Delete.
fn section_card<'a>(
    app: &'a App,
    key: &SectionKey,
    error: Option<&'static str>,
    window_height: f32,
) -> Element<'a, Message> {
    let lines = app.section_lines().unwrap_or_default();
    let drawn = drawn_sections(&lines);
    let at = drawn.iter().position(|k| k == key);
    let (collapsed, title) = lines
        .iter()
        .find_map(|line| match line {
            Line::Header(header) if header.key == *key => {
                Some((header.collapsed, row_name(header.name)))
            }
            _ => None,
        })
        .unwrap_or((false, "Section".to_owned()));
    let mut body: Column<'a, Message> = Column::new().spacing(tokens::S);
    for entry in section_entries(key, collapsed) {
        let enabled = match entry {
            Entry::MoveUp => at.is_some_and(|n| n > 0),
            Entry::MoveDown => at.is_some_and(|n| n + 1 < drawn.len()),
            _ => true,
        };
        body = body.push(entry_button(entry, enabled));
    }
    frame(title, body, error, window_height)
}

/// The card of a room row: Star or Unstar, the moves inside a section of the
/// user's own, Back to its default section, and the "Move to section" list.
fn room_card<'a>(
    app: &'a App,
    space: &RoomId,
    room: &RoomId,
    error: Option<&'static str>,
    window_height: f32,
) -> Element<'a, Message> {
    let lines = app.section_lines().unwrap_or_default();
    let spot = spot_of(&lines, room);
    let title = app
        .rooms
        .iter()
        .find(|r| r.id == *room)
        .map_or_else(|| "Room".to_owned(), |r| row_name(&r.name));
    let mut body: Column<'a, Message> = Column::new().spacing(tokens::S);
    let mut current = None;
    if let Some(spot) = &spot {
        if let SectionKey::Custom(id) = spot.section {
            current = Some(id);
        }
        for entry in room_entries(&spot.section) {
            let enabled = match entry {
                Entry::MoveUp => spot.index > 0,
                Entry::MoveDown => spot.index + 1 < spot.rooms.len(),
                _ => true,
            };
            body = body.push(entry_button(entry, enabled));
        }
    }
    let custom = app
        .sidebar
        .layout()
        .and_then(|layout| layout.spaces.iter().find(|s| s.space == *space))
        .map_or(&[][..], |record| record.custom.as_slice());
    body = body.push(field(
        "Move to section",
        kit::menu(
            pick_list(
                section_choices(custom, current),
                None::<SectionChoice>,
                |choice: SectionChoice| msg(SidebarMenuMessage::Section(choice.pick)),
            )
            .placeholder("Choose a section\u{2026}")
            .style(kit::menu_style)
            .width(Fill),
        ),
    ));
    frame(title, body, error, window_height)
}

fn rail_card<'a>(
    app: &'a App,
    target: &'a Target,
    error: Option<&'static str>,
    window_height: f32,
) -> Element<'a, Message> {
    let items = app.spaces.rail_items(app.sidebar.layout());
    let place = place_of(&items, target);
    let can = |entry: Entry| match entry {
        Entry::MoveUp => place.is_some_and(|p| p.can_up),
        Entry::MoveDown => place.is_some_and(|p| p.can_down),
        _ => true,
    };
    let in_folder = place.is_some_and(|p| p.in_folder);
    let mut body: Column<'a, Message> = Column::new().spacing(tokens::S);
    for entry in entries(target, in_folder) {
        body = body.push(
            secondary_button(
                entry.label(),
                can(entry).then(|| msg(SidebarMenuMessage::Entry(entry))),
            )
            .width(Fill),
        );
    }
    if let Target::Space(id) = target {
        let choices = folder_choices(&items, id);
        body = body.push(field(
            "Move to folder",
            kit::menu(
                pick_list(choices, None::<FolderChoice>, |choice: FolderChoice| {
                    msg(SidebarMenuMessage::Folder(choice.pick))
                })
                .placeholder("Choose a folder\u{2026}")
                .style(kit::menu_style)
                .width(Fill),
            ),
        ));
    }
    body = body.push(error_line(error));
    dialog(
        dialog_header(
            title_of(app, &items, target),
            msg(SidebarMenuMessage::Close),
        ),
        body.into(),
        Some(dialog_footer(vec![
            secondary_button("Close", Some(msg(SidebarMenuMessage::Close))).into(),
        ])),
        CARD_WIDTH,
        window_height,
    )
}

fn name_dialog(dialog_state: &NameDialog, window_height: f32) -> Element<'_, Message> {
    let (title, note, submit) = match dialog_state.purpose {
        Purpose::NewFolder(_) => (
            "New folder",
            "Name the folder, or leave the name blank. Up to 32 characters.",
            "Create",
        ),
        Purpose::RenameFolder(_) => (
            "Rename folder",
            "Up to 32 characters. Leave it blank to remove the name.",
            "Save",
        ),
        Purpose::NewSection { .. } => (
            "New section",
            "Name the section. Up to 32 characters.",
            "Create",
        ),
        Purpose::RenameSection { .. } => ("Rename section", "Up to 32 characters.", "Save"),
    };
    let placeholder = if dialog_state.purpose.is_section() {
        "Section name"
    } else {
        "Folder name"
    };
    let input = text_input(placeholder, &dialog_state.text)
        .style(kit::text_input_style)
        .id(NAME_ID)
        .on_input(|text| msg(SidebarMenuMessage::NameInput(text)))
        .on_submit(msg(SidebarMenuMessage::NameSubmit))
        .padding(tokens::S);
    let body = column![
        words(note).size(tokens::BODY).style(muted),
        input,
        error_line(dialog_state.error),
    ]
    .spacing(tokens::M);
    dialog(
        dialog_header(title, msg(SidebarMenuMessage::Close)),
        body.into(),
        Some(dialog_footer(vec![
            secondary_button("Cancel", Some(msg(SidebarMenuMessage::Close))).into(),
            primary_button(submit, Some(msg(SidebarMenuMessage::NameSubmit))).into(),
        ])),
        CARD_WIDTH,
        window_height,
    )
}
