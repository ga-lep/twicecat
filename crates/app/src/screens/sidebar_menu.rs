//! The options card of the spaces rail and its name dialog (sidebar plan,
//! Task 6). Both are a [`kit::dialog`]: a header with a close button, a body,
//! and a footer.
//!
//! The layer is always a stack of two (the base first), like the other
//! overlays, so opening it keeps the timeline's scroll offset and the
//! composer's state. The "Move to folder" list is a `pick_list` inside
//! [`kit::menu`], so Ctrl + wheel cannot pick a folder. Space and folder names
//! come from other people and from the user's own typing; they pass through
//! `text_safety` before they are drawn. The views only read the app state;
//! every change goes through `App::update`.

use iced::widget::{Column, column, pick_list, responsive, space, stack, text_input};
use iced::{Element, Fill};

use crate::screens::room_dialogs::error_line;
use crate::screens::spaces_sidebar::space_name;
use crate::sidebar_menu::{
    Entry, FolderChoice, NAME_ID, NameDialog, Purpose, SidebarMenu, SidebarMenuMessage, Target,
    entries, folder_choices, place_of,
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
    }
}

fn card<'a>(
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
    };
    let input = text_input("Folder name", &dialog_state.text)
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
