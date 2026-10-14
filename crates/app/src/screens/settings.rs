//! The Settings screen (plan Q17, Task 15): profile, notifications,
//! appearance, keyboard shortcuts, Voice & audio (with push-to-talk) and a
//! link to Sessions.
//!
//! A card over a dimmed window, in a layer that is always a stack of two (the
//! base first), so opening it keeps the timeline's scroll offset and the
//! composer's state. Views only read the app state; every change goes through
//! `App::update`.

use iced::widget::{
    Column, Row, button, checkbox, column, container, pick_list, radio, row, scrollable, space,
    stack, text, text_input,
};
use iced::{Background, Border, Element, Fill};
use twicecat_core::api::ThemeChoice;

use crate::settings::{
    DEFAULT_TEXT_SCALE, DRAG_AND_DROP_NOTE, DefaultChoice, DensityOption, KEYWORD_INPUT_ID,
    MAX_TEXT_SCALE, MIN_TEXT_SCALE, NAME_INPUT_ID, RoomListOption, SettingsMessage,
    VOICE_AUDIO_LINE, Zoom,
};
use crate::state::{App, Message};
use crate::theme::{self, Family};
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::{self, modal, muted, secondary_button};
use crate::ui::tokens;

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
        .push(
            column![
                heading("Voice & audio"),
                note(VOICE_AUDIO_LINE),
                button(text("Voice & audio\u{2026}"))
                    .on_press(msg(SettingsMessage::OpenVoiceAudio))
                    .style(button::secondary),
            ]
            .spacing(6),
        )
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
        kit::menu(
            pick_list(DefaultChoice::ALL, shown, move |choice: DefaultChoice| {
                msg(SettingsMessage::DefaultMode {
                    one_to_one,
                    mode: choice.mode(),
                })
            })
            .placeholder("Loading\u{2026}")
        ),
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
    let dark = app.current_theme().extended_palette().is_dark;
    let chosen = theme::resolve_family(app.ui_settings.palette);
    let mut swatches = Row::new().spacing(tokens::S);
    for family in Family::ALL {
        swatches = swatches.push(swatch(family, dark, family == chosen));
    }
    let on_default = app.ui_settings.palette.is_none();
    let scale = app.ui_settings.text_scale_percent;
    let zoom = row![
        text("Text size").width(Fill),
        secondary_button(
            "Smaller",
            (scale > MIN_TEXT_SCALE).then(|| msg(SettingsMessage::Zoom(Zoom::Out))),
        ),
        text(format!("{scale}%")).width(48).align_x(iced::Center),
        secondary_button(
            "Larger",
            (scale < MAX_TEXT_SCALE).then(|| msg(SettingsMessage::Zoom(Zoom::In))),
        ),
        secondary_button(
            "Reset",
            (scale != DEFAULT_TEXT_SCALE).then(|| msg(SettingsMessage::Zoom(Zoom::Reset))),
        ),
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    column![
        heading("Appearance"),
        choice("Follow the system", ThemeChoice::System),
        choice("Light", ThemeChoice::Light),
        choice("Dark", ThemeChoice::Dark),
        text("Colours").size(14),
        swatches,
        secondary_button(
            "Use the default",
            (!on_default).then(|| msg(SettingsMessage::Palette(None))),
        ),
        row![
            text("Density").width(Fill),
            kit::menu(pick_list(
                DensityOption::ALL,
                Some(DensityOption(app.ui_settings.density)),
                |o: DensityOption| msg(SettingsMessage::Density(o.0)),
            )),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
        row![
            text("Room list").width(Fill),
            kit::menu(pick_list(
                RoomListOption::ALL,
                Some(RoomListOption(app.ui_settings.room_list)),
                |o: RoomListOption| msg(SettingsMessage::RoomList(o.0)),
            )),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
        zoom,
        note("Ctrl+= and Ctrl+- change the text size, Ctrl+0 resets it."),
    ]
    .spacing(6)
    .into()
}

/// One colour set as a button: samples of its accent, surface and text, the
/// name, and a check and an outline when it is the one in use.
fn swatch<'a>(family: Family, dark: bool, selected: bool) -> Element<'a, Message> {
    let colours = theme::palette(family, dark);
    let sem = theme::semantic_of(family, dark);
    let sample = move |color: iced::Color| {
        container(space().width(18).height(18)).style(move |_: &iced::Theme| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                color: sem.border,
                width: 1.0,
                radius: tokens::SMALL.into(),
            },
            ..container::Style::default()
        })
    };
    let name = if family == theme::DEFAULT_FAMILY {
        format!("{} (default)", family.label())
    } else {
        family.label().to_owned()
    };
    let mut title = Row::new().spacing(tokens::XS).align_y(iced::Center);
    if selected {
        title = title.push(icon::<Message>(Icon::Check));
    }
    title = title.push(text(name).size(tokens::CAPTION));
    let samples = row![
        sample(colours.primary),
        sample(sem.surface),
        sample(colours.text)
    ]
    .spacing(tokens::XS);
    button(column![samples, title].spacing(tokens::XS))
        .padding(tokens::S)
        .on_press(msg(SettingsMessage::Palette(Some(family.choice()))))
        .style(move |theme: &iced::Theme, status| {
            let mut style = button::secondary(theme, status);
            style.background = Some(Background::Color(colours.background));
            style.text_color = colours.text;
            style.border = Border {
                color: if selected {
                    theme.palette().primary
                } else {
                    sem.border
                },
                width: if selected { 3.0 } else { 1.0 },
                radius: tokens::MEDIUM.into(),
            };
            style
        })
        .into()
}

fn shortcuts_section<'a>() -> Element<'a, Message> {
    let mut list = Column::new().spacing(4);
    for shortcut in crate::keys::SHORTCUTS {
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
