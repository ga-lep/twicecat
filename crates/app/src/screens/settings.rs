//! The Settings screen (plan Q17, Task 15): profile, notifications,
//! appearance, keyboard shortcuts, push-to-talk and a link to Sessions.
//!
//! A card over a dimmed window, in a layer that is always a stack of two (the
//! base first), so opening it keeps the timeline's scroll offset and the
//! composer's state. Views only read the app state; every change goes through
//! `App::update`.

use iced::widget::{
    Column, button, checkbox, column, container, pick_list, radio, row, scrollable, space, stack,
    text, text_input,
};
use iced::{Element, Fill};
use tessera_core::api::ThemeChoice;

use crate::screens::room_dialogs::{modal, muted};
use crate::settings::{
    DRAG_AND_DROP_NOTE, DefaultChoice, KEYWORD_INPUT_ID, NAME_INPUT_ID, PUSH_TO_TALK_LINE,
    SHORTCUTS, SettingsMessage,
};
use crate::state::{App, Message};

/// Width of the card.
const CARD_WIDTH: f32 = 640.0;

fn msg(message: SettingsMessage) -> Message {
    Message::Settings(message)
}

/// The base with the Settings screen on top when it is open: always a stack
/// of two.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let top = if app.settings_ui.open {
        view(app)
    } else {
        space().into()
    };
    stack([base, top]).into()
}

fn heading(label: &str) -> Element<'_, Message> {
    text(label).size(18).into()
}

fn note(label: &str) -> Element<'_, Message> {
    text(label).size(13).style(muted).into()
}

fn view(app: &App) -> Element<'_, Message> {
    let header = row![
        text("Settings").size(24),
        space().width(Fill),
        button(text("Close"))
            .on_press(msg(SettingsMessage::Close))
            .style(button::secondary),
    ]
    .align_y(iced::Center);
    let mut body = Column::new().spacing(14).push(header);
    if let Some(error) = app.settings_ui.error {
        body = body.push(text(error.text()).size(13).style(text::danger));
    }
    body = body
        .push(profile_section(app))
        .push(notifications_section(app))
        .push(appearance_section(app))
        .push(shortcuts_section())
        .push(column![heading("Push-to-talk"), note(PUSH_TO_TALK_LINE)].spacing(6))
        .push(
            column![
                heading("Sessions"),
                button(text("Manage sessions\u{2026}"))
                    .on_press(msg(SettingsMessage::OpenSessions))
                    .style(button::secondary),
            ]
            .spacing(6),
        );
    modal(container(scrollable(body)).max_height(620.0), CARD_WIDTH)
}

fn profile_section(app: &App) -> Element<'_, Message> {
    let ui = &app.settings_ui;
    let can_save = ui.name_edited && !ui.profile_pending;
    let name = row![
        text_input("Display name", ui.name.expose())
            .id(NAME_INPUT_ID)
            .on_input(|s| msg(SettingsMessage::NameInput(s.into())))
            .on_submit_maybe(can_save.then(|| msg(SettingsMessage::SaveName)))
            .padding(6),
        button(text("Save")).on_press_maybe(can_save.then(|| msg(SettingsMessage::SaveName))),
    ]
    .spacing(8);
    let has_avatar = ui.profile.as_ref().is_some_and(|p| p.has_avatar);
    let idle = !ui.avatar_busy && !ui.profile_pending;
    let picture = row![
        text(if has_avatar {
            "Picture: set"
        } else {
            "Picture: none"
        })
        .size(14),
        space().width(Fill),
        button(text(if ui.avatar_busy {
            "Working\u{2026}"
        } else {
            "Change picture\u{2026}"
        }))
        .on_press_maybe(idle.then(|| msg(SettingsMessage::ChooseAvatar)))
        .style(button::secondary),
        button(text("Remove picture"))
            .on_press_maybe((idle && has_avatar).then(|| msg(SettingsMessage::RemoveAvatar)))
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center);
    column![heading("Profile"), name, picture].spacing(8).into()
}

fn default_menu<'a>(app: &'a App, label: &'static str, one_to_one: bool) -> Element<'a, Message> {
    let shown = app
        .settings_ui
        .shown_default(app.notification_settings.as_ref(), one_to_one)
        .map(DefaultChoice::from_mode);
    row![
        text(label).width(Fill),
        pick_list(DefaultChoice::ALL, shown, move |choice: DefaultChoice| {
            msg(SettingsMessage::DefaultMode {
                one_to_one,
                mode: choice.mode(),
            })
        })
        .placeholder("Loading\u{2026}"),
    ]
    .spacing(8)
    .align_y(iced::Center)
    .into()
}

fn notifications_section(app: &App) -> Element<'_, Message> {
    let mut keywords = Column::new().spacing(4);
    for (shown, keyword) in app.keyword_rows() {
        keywords = keywords.push(
            row![
                text(shown).width(Fill),
                button(text("Remove"))
                    .on_press(msg(SettingsMessage::RemoveKeyword(keyword)))
                    .style(button::secondary),
            ]
            .spacing(8)
            .align_y(iced::Center),
        );
    }
    let ui = &app.settings_ui;
    let typed = !ui.keyword_input.expose().trim().is_empty();
    let add = row![
        text_input("Add a keyword", ui.keyword_input.expose())
            .id(KEYWORD_INPUT_ID)
            .on_input(|s| msg(SettingsMessage::KeywordInput(s.into())))
            .on_submit(msg(SettingsMessage::AddKeyword))
            .padding(6),
        button(text("Add")).on_press_maybe(typed.then(|| msg(SettingsMessage::AddKeyword))),
    ]
    .spacing(8);
    let local = &app.ui_settings;
    column![
        heading("Notifications"),
        default_menu(app, "Group rooms", false),
        default_menu(app, "Direct messages", true),
        text("Keywords").size(14),
        keywords,
        add,
        note("A message with one of these words notifies you."),
        checkbox(local.desktop_notifications)
            .label("Desktop notifications")
            .on_toggle(|on| msg(SettingsMessage::DesktopNotifications(on))),
        checkbox(local.show_message_content)
            .label("Show message content in notifications")
            .on_toggle(|on| msg(SettingsMessage::ShowMessageContent(on))),
        checkbox(local.tray_badge)
            .label("Show the unread count on the tray icon")
            .on_toggle(|on| msg(SettingsMessage::TrayBadge(on))),
        note("The three options above apply to this device only."),
    ]
    .spacing(8)
    .into()
}

fn appearance_section(app: &App) -> Element<'_, Message> {
    let current = Some(app.ui_settings.theme);
    let choice = |label: &'static str, value: ThemeChoice| {
        radio(label, value, current, |c| msg(SettingsMessage::Theme(c)))
    };
    column![
        heading("Appearance"),
        choice("Follow the system", ThemeChoice::System),
        choice("Light", ThemeChoice::Light),
        choice("Dark", ThemeChoice::Dark),
    ]
    .spacing(6)
    .into()
}

fn shortcuts_section<'a>() -> Element<'a, Message> {
    let mut list = Column::new().spacing(4);
    for shortcut in SHORTCUTS {
        list = list.push(
            row![
                text(shortcut.keys).width(160).font(iced::Font::MONOSPACE),
                text(shortcut.action),
            ]
            .spacing(8),
        );
    }
    column![
        heading("Keyboard shortcuts"),
        list,
        note("On macOS, Ctrl is Cmd."),
        note(DRAG_AND_DROP_NOTE),
    ]
    .spacing(6)
    .into()
}
