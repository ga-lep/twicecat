//! The Settings screen (plan Q17, Task 15; as pages in UI/UX rework Task 14):
//! a modal above the main view with a list of pages at its left and one page
//! at its right. Always a stack of two (the base first), so opening it keeps
//! the timeline's scroll offset and the composer's state. Views only read the
//! app state; every change goes through `App::update`.
//!
//! Two levels, chosen by one saved switch in the card's header (**Show
//! advanced settings**, off by default). The simple level has few choices in
//! plain words with good defaults: Profile, Appearance (light or dark,
//! colours, text size), Notifications (group rooms, direct messages, desktop
//! notifications), Voice & audio (microphone, speakers, how you talk, noise
//! reduction, a test), Video (camera, picture quality), Keyboard, Sessions and
//! About. The advanced level adds below a rule, on the same pages: density
//! and the room list, keywords, message content and the tray badge, a button
//! to the card with every audio option, and every tuning of the camera and of
//! screen sharing. Settings opens on Profile, also with Ctrl+,.
//!
//! Tab: this iced only gives text fields the focus (buttons never take it),
//! so Tab walks the text fields of the page on show, in tree order; the list
//! of pages is reached with the pointer.

use iced::widget::text::Wrapping;
use iced::widget::{
    Column, checkbox, column, container, pick_list, radio, row, scrollable, space, stack, text,
    text_input,
};
use iced::{Element, Fill, Length};
use twicecat_core::api::{ThemeChoice, UpdateCheckChoice, UpdateMechanism};

use crate::screens::{audio_settings, video_settings};
use crate::settings::{
    DEFAULT_TEXT_SCALE, DRAG_AND_DROP_NOTE, DefaultChoice, DensityOption, INTER_ATTRIBUTION,
    INTER_NOTICES, KEYWORD_INPUT_ID, MAX_TEXT_SCALE, MIN_TEXT_SCALE, NAME_INPUT_ID, RoomListOption,
    SESSIONS_LINE, SettingsMessage, SettingsPage, ThemeOption, VOICE_AUDIO_LINE, Zoom,
};
use crate::state::{App, Message};
use crate::ui::fonts;
use crate::ui::kit::{self, Severity, banner, modal, muted, primary_button, secondary_button};
use crate::ui::tokens;
use crate::updates::{self, UpdateMessage};

/// Width of the card.
const CARD_WIDTH: f32 = 760.0;
/// Height of the card, with its padding.
const CARD_HEIGHT: f32 = 620.0;
/// The least height of the card's content, in a small window.
const MIN_CARD_HEIGHT: f32 = 240.0;
/// The padding `kit::modal` puts around the card's content.
const CARD_PADDING: f32 = 20.0;
/// Width of the list of pages.
pub const NAV_WIDTH: f32 = 168.0;
/// Width of the keys column of the shortcut list.
const KEYS_WIDTH: f32 = 160.0;
/// Most report lines about the themes folder shown on the page.
const MAX_THEME_REPORT_LINES: usize = 5;
/// How text wraps on this card: a break inside a word when it has none, so a
/// long keyword is wrapped and its end is never cut off.
const WRAP: Wrapping = Wrapping::WordOrGlyph;

fn msg(message: SettingsMessage) -> Message {
    Message::Settings(message)
}

/// Text that wraps on this card's rules.
fn words<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(label).wrapping(WRAP)
}

fn heading<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    words(label).size(tokens::TITLE).into()
}

fn note<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    words(label).size(tokens::BODY).style(muted).into()
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

/// The card, 620 high or as high as the window leaves room for.
fn view(app: &App) -> Element<'_, Message> {
    iced::widget::responsive(move |size| card(app, size.height)).into()
}

fn card(app: &App, window_height: f32) -> Element<'_, Message> {
    let header = row![
        words("Settings").size(tokens::HEADLINE).width(Fill),
        checkbox(app.ui_settings.advanced_settings)
            .style(kit::checkbox_style)
            .label("Show advanced settings")
            .text_wrapping(WRAP)
            .on_toggle(|on| msg(SettingsMessage::Advanced(on))),
        secondary_button("Close", Some(msg(SettingsMessage::Close))),
    ]
    .spacing(tokens::M)
    .align_y(iced::Center);
    // The line about the last failure: on every page, in a slot that is
    // always there (empty when there is nothing to say).
    let mut notice = Column::new();
    if let Some(error) = app.settings_ui.error {
        notice = notice.push(banner(Severity::Danger, error.text()));
    }
    let labels = SettingsPage::ALL.map(SettingsPage::label);
    let nav = kit::page_nav(&labels, app.settings_ui.page.index(), |index| {
        msg(SettingsMessage::Page(
            SettingsPage::ALL.get(index).copied().unwrap_or_default(),
        ))
    });
    // The scrollbar takes room of its own (it does not float over the page),
    // so a button or a label at the page's right edge is never under it.
    let page = scrollable(page_view(app)).spacing(tokens::S).width(Fill);
    let body = row![
        container(nav).width(Length::Fixed(NAV_WIDTH)),
        kit::vertical_divider(),
        page,
    ]
    .spacing(tokens::L)
    .height(Fill);
    let content = column![column![header, notice].spacing(tokens::S), body].spacing(tokens::L);
    let height = (window_height - 2.0 * CARD_PADDING - 2.0 * tokens::L)
        .clamp(MIN_CARD_HEIGHT, CARD_HEIGHT - 2.0 * CARD_PADDING);
    modal(container(content).height(Length::Fixed(height)), CARD_WIDTH)
}

fn page_view(app: &App) -> Element<'_, Message> {
    match app.settings_ui.page {
        SettingsPage::Profile => profile_page(app),
        SettingsPage::Appearance => appearance_page(app),
        SettingsPage::Notifications => notifications_page(app),
        SettingsPage::VoiceAudio => voice_audio_page(app),
        SettingsPage::Video => video_settings::page(app),
        SettingsPage::Keyboard => keyboard_page(),
        SettingsPage::Sessions => sessions_page(),
        SettingsPage::About => about_page(app),
    }
}

/// A page: its title, then its parts.
fn page_of<'a>(title: &'static str, parts: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    column![
        heading(title),
        Column::from_vec(parts).spacing(tokens::S).width(Fill)
    ]
    .spacing(tokens::M)
    .width(Fill)
    .into()
}

fn profile_page(app: &App) -> Element<'_, Message> {
    let ui = &app.settings_ui;
    let can_save = ui.name_edited && !ui.profile_pending;
    let name = row![
        text_input("Display name", ui.name.expose())
            .style(kit::text_input_style)
            .id(NAME_INPUT_ID)
            .on_input(|s| msg(SettingsMessage::NameInput(s.into())))
            .on_submit_maybe(can_save.then(|| msg(SettingsMessage::SaveName)))
            .padding(tokens::S),
        primary_button("Save", can_save.then(|| msg(SettingsMessage::SaveName))),
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    let has_avatar = ui.profile.as_ref().is_some_and(|p| p.has_avatar);
    let idle = !ui.avatar_busy && !ui.profile_pending;
    // The words on a row of their own, the buttons below them.
    let picture = column![
        words(if has_avatar {
            "Picture: set"
        } else {
            "Picture: none"
        }),
        row![
            secondary_button(
                if ui.avatar_busy {
                    "Working\u{2026}"
                } else {
                    "Change picture\u{2026}"
                },
                idle.then(|| msg(SettingsMessage::ChooseAvatar)),
            ),
            secondary_button(
                "Remove picture",
                (idle && has_avatar).then(|| msg(SettingsMessage::RemoveAvatar)),
            ),
        ]
        .spacing(tokens::M),
    ]
    .spacing(tokens::S);
    page_of("Profile", vec![name.into(), picture.into()])
}

fn default_menu<'a>(app: &'a App, label: &'static str, one_to_one: bool) -> Element<'a, Message> {
    let shown = app
        .settings_ui
        .shown_default(app.notification_settings.as_ref(), one_to_one)
        .map(DefaultChoice::from_mode);
    row![
        words(label).width(Fill),
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
    .spacing(tokens::S)
    .align_y(iced::Center)
    .into()
}

fn notifications_page(app: &App) -> Element<'_, Message> {
    let mut keywords = Column::new().spacing(tokens::XS);
    for (shown, keyword) in app.keyword_rows() {
        // The keyword wraps inside the room the button leaves: the button is
        // laid out first, so the words never run under it.
        keywords = keywords.push(
            row![
                words(shown).width(Fill),
                secondary_button("Remove", Some(msg(SettingsMessage::RemoveKeyword(keyword)))),
            ]
            .spacing(tokens::S)
            .align_y(iced::Center),
        );
    }
    let ui = &app.settings_ui;
    let typed = !ui.keyword_input.expose().trim().is_empty();
    let add = row![
        text_input("Add a keyword", ui.keyword_input.expose())
            .style(kit::text_input_style)
            .id(KEYWORD_INPUT_ID)
            .on_input(|s| msg(SettingsMessage::KeywordInput(s.into())))
            .on_submit(msg(SettingsMessage::AddKeyword))
            .padding(tokens::S),
        secondary_button("Add", typed.then(|| msg(SettingsMessage::AddKeyword))),
    ]
    .spacing(tokens::S);
    let local = &app.ui_settings;
    let toggle = |label: &'static str, on: bool, change: fn(bool) -> SettingsMessage| {
        checkbox(on)
            .style(kit::checkbox_style)
            .label(label)
            .text_wrapping(WRAP)
            .on_toggle(move |on| msg(change(on)))
    };
    let advanced = app.ui_settings.advanced_settings;
    let mut parts = vec![
        default_menu(app, "Group rooms", false),
        default_menu(app, "Direct messages", true),
        toggle(
            "Desktop notifications",
            local.desktop_notifications,
            SettingsMessage::DesktopNotifications,
        )
        .into(),
        note("This applies to this device only."),
    ];
    if advanced {
        parts.push(kit::divider());
        parts.push(words("Keywords").size(tokens::BODY).into());
        parts.push(keywords.into());
        parts.push(add.into());
        parts.push(note("A message with one of these words notifies you."));
        parts.push(
            toggle(
                "Show message content in notifications",
                local.show_message_content,
                SettingsMessage::ShowMessageContent,
            )
            .into(),
        );
        parts.push(
            toggle(
                "Show the unread count on the tray icon",
                local.tray_badge,
                SettingsMessage::TrayBadge,
            )
            .into(),
        );
        parts.push(note("The options of this part apply to this device only."));
    }
    page_of("Notifications", parts)
}

fn appearance_page(app: &App) -> Element<'_, Message> {
    let current = Some(app.ui_settings.theme);
    let choice = |label: &'static str, value: ThemeChoice| {
        radio(label, value, current, |c| msg(SettingsMessage::Theme(c)))
            .text_wrapping(WRAP)
            .into()
    };
    let scale = app.ui_settings.text_scale_percent;
    let zoom = row![
        words("Text size").width(Fill),
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
    let menu_row = |label: &'static str, menu: Element<'static, Message>| {
        row![words(label).width(Fill), menu]
            .spacing(tokens::S)
            .align_y(iced::Center)
    };
    let mut parts: Vec<Element<'_, Message>> = vec![
        menu_row(
            "Theme",
            kit::menu(pick_list(
                app.theme_options(),
                Some(app.selected_theme_option()),
                |o: ThemeOption| msg(SettingsMessage::ThemePicked(o.0)),
            )),
        )
        .into(),
    ];
    // The choice applies only to a theme that comes as a light and a dark
    // file; a theme with one look is shown as it is, with nothing here.
    if app.selected_is_paired() {
        parts.push(choice("Follow the system", ThemeChoice::System));
        parts.push(choice("Light", ThemeChoice::Light));
        parts.push(choice("Dark", ThemeChoice::Dark));
    }
    parts.push(
        row![
            secondary_button(
                "Reload themes",
                (!app.themes_loading).then(|| msg(SettingsMessage::ReloadThemes)),
            ),
            secondary_button(
                "Open themes folder",
                Some(msg(SettingsMessage::OpenThemesFolder)),
            ),
        ]
        .spacing(tokens::S)
        .into(),
    );
    if app.themes_loading {
        parts.push(note("Reading the themes folder\u{2026}"));
    }
    if let Some(line) = app.theme_fallback() {
        parts.push(banner(Severity::Warning, line));
    }
    if let Some(loaded) = &app.themes {
        parts.extend(loaded.shown(MAX_THEME_REPORT_LINES).into_iter().map(note));
    }
    parts.push(zoom.into());
    parts.push(note(
        "Ctrl+= and Ctrl+- change the text size, Ctrl+0 resets it.",
    ));
    if app.ui_settings.advanced_settings {
        parts.push(kit::divider());
        parts.push(
            menu_row(
                "Density",
                kit::menu(pick_list(
                    DensityOption::ALL,
                    Some(DensityOption(app.ui_settings.density)),
                    |o: DensityOption| msg(SettingsMessage::Density(o.0)),
                )),
            )
            .into(),
        );
        parts.push(
            menu_row(
                "Room list",
                kit::menu(pick_list(
                    RoomListOption::ALL,
                    Some(RoomListOption(app.ui_settings.room_list)),
                    |o: RoomListOption| msg(SettingsMessage::RoomList(o.0)),
                )),
            )
            .into(),
        );
    }
    page_of("Appearance", parts)
}

fn voice_audio_page(app: &App) -> Element<'_, Message> {
    // The button to every audio option is at the top, where it is found
    // without scrolling past the basics.
    let more = app.ui_settings.advanced_settings.then(|| {
        row![
            container(note(VOICE_AUDIO_LINE)).width(Fill),
            secondary_button(
                "More audio options\u{2026}",
                Some(msg(SettingsMessage::OpenVoiceAudio)),
            ),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center)
        .into()
    });
    audio_settings::basics(app, more)
}

fn keyboard_page<'a>() -> Element<'a, Message> {
    let mut list = Column::new().spacing(tokens::XS);
    for shortcut in crate::keys::SHORTCUTS {
        list = list.push(
            row![
                text(shortcut.keys)
                    .width(Length::Fixed(KEYS_WIDTH))
                    .font(fonts::MONO),
                words(shortcut.action).width(Fill),
            ]
            .spacing(tokens::S),
        );
    }
    page_of(
        "Keyboard",
        vec![list.into(), note("On macOS, Ctrl is Cmd.")],
    )
}

fn sessions_page<'a>() -> Element<'a, Message> {
    page_of(
        "Sessions",
        vec![
            note(SESSIONS_LINE),
            secondary_button(
                "Manage sessions\u{2026}",
                Some(msg(SettingsMessage::OpenSessions)),
            )
            .into(),
        ],
    )
}

fn about_page(app: &App) -> Element<'_, Message> {
    let updates: Vec<Element<'_, Message>> = match app.updates.mechanism {
        UpdateMechanism::CheckAndLink | UpdateMechanism::AppImage => vec![
            checkbox(app.ui_settings.update_check == UpdateCheckChoice::On)
                .style(kit::checkbox_style)
                .label(updates::TOGGLE)
                .text_wrapping(WRAP)
                .on_toggle(|on| Message::Update(UpdateMessage::Choose(on)))
                .into(),
            note(updates::TOGGLE_NOTE),
        ],
        UpdateMechanism::Flatpak => vec![note(updates::FLATPAK_NOTE)],
        UpdateMechanism::PackageManager => vec![note(updates::PACKAGE_NOTE)],
    };
    let mut parts: Vec<Element<'_, Message>> = vec![
        words(format!("Twicecat {}", env!("CARGO_PKG_VERSION")))
            .size(tokens::BODY_LARGE)
            .into(),
    ];
    parts.extend(updates);
    parts.extend([
        note(DRAG_AND_DROP_NOTE),
        words(INTER_ATTRIBUTION).size(tokens::BODY).into(),
        note(INTER_NOTICES),
    ]);
    page_of("About", parts)
}
