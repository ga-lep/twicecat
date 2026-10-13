//! The two-pane main layout: the room list pane on the left and the room view
//! (timeline pane and composer) on the right.

use std::fmt;

use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, button, column, container, opaque, pick_list, row, space, stack, text,
    text_editor, text_input,
};
use iced::{Element, Fill};
use twicecat_core::api::{MessageText, RoomId, SyncState, UserId, UserTrust};

use crate::actions::{self, Picker};
use crate::audio_settings::AudioMessage;
use crate::calls::{CallButton, CallMessage};
use crate::composer::{self, EditorAction, Mode, Suggestions};
use crate::emoji;
use crate::links::{CheckedUrl, LinkBar, LinkKind};
use crate::notifications::ModeChoice;
use crate::rooms::RoomsMessage;
use crate::screens::loading::sync_label;
use crate::screens::timeline_pane::RichEnv;
use crate::screens::{
    call, media_viewer, room_dialogs, room_list_pane, security, sessions, spaces_sidebar,
    thread_panel, timeline_pane,
};
use crate::sessions::SessionsMessage;
use crate::settings::SettingsMessage;
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder, one_line_capped};
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::kit::{self, Severity};
use crate::ui::tokens;
use crate::verification::elide_middle;

/// Width of the left pane.
pub const LIST_PANE_WIDTH: f32 = 300.0;

/// A room name ready to draw: one line, capped, a placeholder when blank.
pub fn display_name(name: &str) -> String {
    name_or_placeholder(name, MAX_NAME_CHARS)
}

/// What the header of a direct message says about the other person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Badge {
    /// Verified by the user.
    Verified,
    /// Not verified.
    NotVerified,
    /// A verified person's identity changed.
    IdentityChanged,
}

impl Badge {
    /// The badge's words.
    pub fn label(self) -> &'static str {
        match self {
            Self::Verified => "Verified",
            Self::NotVerified => "Not verified",
            Self::IdentityChanged => "Identity changed",
        }
    }

    /// How serious the badge is, which sets the colour of its tag: an
    /// identity that changed must never look like a verified one.
    pub fn severity(self) -> Severity {
        match self {
            Self::Verified => Severity::Success,
            Self::NotVerified => Severity::Info,
            Self::IdentityChanged => Severity::Danger,
        }
    }

    /// Whether the header offers **Verify** next to it.
    pub fn can_verify(self) -> bool {
        !matches!(self, Self::Verified)
    }
}

/// The badge for a person's trust (plan Q10); nothing for a person without a
/// cross-signing identity.
pub fn trust_badge(trust: UserTrust) -> Option<Badge> {
    match trust {
        UserTrust::Verified => Some(Badge::Verified),
        UserTrust::Unverified => Some(Badge::NotVerified),
        UserTrust::IdentityChanged => Some(Badge::IdentityChanged),
        UserTrust::NoIdentity => None,
    }
}

/// Longest peer ID in the header, in characters. Longer IDs are cut in the
/// middle, so the server name stays visible.
pub const HEADER_PEER_ID_CHARS: usize = 48;

/// The peer's user ID as the header shows it: one sanitized line, cut only in
/// the middle (see [`crate::verification::elide_middle`]), never from the end.
/// The full ID is in the verification dialog.
pub fn peer_id_line(peer: &UserId) -> String {
    elide_middle(peer.as_str(), HEADER_PEER_ID_CHARS)
}

/// Longest topic line in the header, in characters.
const TOPIC_CHARS: usize = 120;

/// The topic as the header shows it: one sanitised line, capped; `None`
/// when there is no topic.
pub fn topic_line(topic: Option<&str>) -> Option<String> {
    let topic = topic?.trim();
    (!topic.is_empty()).then(|| one_line_capped(topic, TOPIC_CHARS))
}

/// The topic of the selected room. The room list carries no topic (the
/// core reports it to the room settings panel only), so the header shows
/// none yet.
fn room_topic(_app: &App) -> Option<&str> {
    None
}

/// A control in the room header, with the message a press sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderAction {
    /// **Members**.
    Members,
    /// The room's **Settings**.
    Settings,
    /// **People**: the participants panel of a call.
    People,
    /// **Verify** the other person of a direct message.
    Verify(UserId),
    /// **Accept** an invitation.
    Accept(RoomId),
    /// **Decline** an invitation.
    Decline(RoomId),
}

impl HeaderAction {
    /// The message a press sends.
    pub fn message(self) -> Message {
        match self {
            Self::Members => Message::Rooms(RoomsMessage::OpenMembers),
            Self::Settings => Message::Rooms(RoomsMessage::OpenSettings),
            Self::People => Message::Call(CallMessage::TogglePanel),
            Self::Verify(peer) => Message::VerifyPeerRequested(peer),
            Self::Accept(room) => Message::Rooms(RoomsMessage::Accept(room)),
            Self::Decline(room) => Message::Rooms(RoomsMessage::Decline(room)),
        }
    }
}

/// The room header: the avatar, the name and a line under it, tags for the
/// lock, Voice and the trust of the other person (with Verify), then the buttons. Always
/// one row, so the column around it keeps its widget types.
fn header<'a>(app: &'a App, title: String) -> Element<'a, Message> {
    let selected = app
        .selected
        .as_ref()
        .and_then(|id| app.rooms.iter().find(|room| room.id == *id));
    let trust = app
        .dm_peer_trust()
        .and_then(|(peer, trust)| trust_badge(trust).map(|badge| (peer, badge)));

    let mut line = Row::new().spacing(tokens::S).align_y(iced::Center);
    if let Some(room) = selected {
        // A room's placeholder colour comes from its ID: the same string as
        // a user ID gives the same colour as the room list's row.
        let id = UserId::from(room.id.as_str());
        line = line.push(kit::avatar(&id, &title, room.avatar.as_ref(), &app.avatars));
    }
    // Under the name: a row of tags (it wraps rather than clips, so the
    // trust tag is always seen), then the topic or, in a direct message, the
    // other person's ID on its own line.
    let mut tag_list: Vec<Element<'a, Message>> = Vec::new();
    if let Some(room) = selected {
        if room.is_encrypted {
            tag_list.push(kit::tag(Icon::Lock.label()));
        }
        if room.is_call_room {
            tag_list.push(kit::tag("Voice"));
        }
    }
    if let Some((_, badge)) = trust {
        tag_list.push(kit::severity_tag(badge.severity(), badge.label()));
    }
    // Verify sits with the trust tag, in the row that wraps: in the header's
    // own row it would take the width the tag and the ID need.
    if let Some((peer, _)) = trust.filter(|(_, badge)| badge.can_verify()) {
        tag_list.push(
            kit::secondary_button("Verify", Some(HeaderAction::Verify(peer.clone()).message()))
                .into(),
        );
    }
    let tags = Row::with_children(tag_list)
        .spacing(tokens::S)
        .align_y(iced::Center)
        .wrap()
        .vertical_spacing(2.0);
    let mut under = Column::new().spacing(2.0).push(tags);
    match trust {
        // Cut by `elide_middle` to a budget, never by clipping: if the pane
        // is narrower still, the line wraps so no part of it is lost.
        Some((peer, _)) => {
            under = under.push(
                text(peer_id_line(peer))
                    .size(tokens::CAPTION)
                    .wrapping(Wrapping::WordOrGlyph)
                    .style(kit::muted),
            );
        }
        None => {
            if let Some(topic) = topic_line(room_topic(app)) {
                under = under.push(
                    text(topic)
                        .size(tokens::CAPTION)
                        .wrapping(Wrapping::None)
                        .style(kit::muted),
                );
            }
        }
    }
    let names = Column::from_vec(vec![
        text(title)
            .size(tokens::TITLE)
            .wrapping(Wrapping::None)
            .into(),
        under.into(),
    ]);
    line = line.push(container(names).width(Fill).clip(true));

    if let Some(room) = selected.filter(|room| !room.is_invite) {
        line = line.push(notification_menu(app, &room.id));
        line = line.push(kit::icon_button(
            Icon::Members,
            HeaderAction::Members.message(),
        ));
        line = line.push(kit::icon_button(
            Icon::Settings,
            HeaderAction::Settings.message(),
        ));
        if let Some(call) = app.calls.button(room) {
            line = line.push(call_button(call));
        }
        if app.calls.call_in(&room.id).is_some() {
            // The participants panel (plan Q4).
            line = line.push(kit::toggle_icon_button(
                Icon::People,
                app.calls.panel_open,
                HeaderAction::People.message(),
            ));
        }
    }
    if let Some(room) = selected.filter(|room| room.is_invite) {
        line = line.push(invite_buttons(app, &room.id));
    }
    line.into()
}

/// The message a press of the call button sends, if it is on.
pub fn call_message(call: &CallButton) -> Option<Message> {
    call.press.clone().map(Message::Call)
}

/// Whether the call button leaves the call (and so is a danger button).
fn is_leave(call: &CallButton) -> bool {
    call.press == Some(CallMessage::Leave)
}

/// The call button of a joined room (plan Q6): **Call**, **Join call (N)**,
/// **Join voice**, **Leave call**, or **Joining...** while it is off.
fn call_button<'a>(call: CallButton) -> Element<'a, Message> {
    let press = call_message(&call);
    if is_leave(&call) {
        kit::danger_button(call.label, press).into()
    } else {
        kit::primary_button(call.label, press).into()
    }
}

/// **Accept** and **Decline**, in the header of an invitation. They are off
/// while the answer is on its way.
fn invite_buttons<'a>(app: &App, room: &RoomId) -> Element<'a, Message> {
    let idle = !app.rooms_ui.invite_busy.contains(room);
    row![
        text("You were invited").size(tokens::CAPTION),
        kit::primary_button(
            "Accept",
            idle.then(|| HeaderAction::Accept(room.clone()).message()),
        ),
        kit::secondary_button(
            "Decline",
            idle.then(|| HeaderAction::Decline(room.clone()).message()),
        ),
    ]
    .spacing(tokens::S)
    .align_y(iced::Center)
    .into()
}

/// The room's notification menu: a drop-down with the four choices of plan
/// Q11. It shows what the user picked at once, since the room list lags
/// behind a change.
fn notification_menu<'a>(app: &'a App, room: &RoomId) -> Element<'a, Message> {
    let menu = pick_list(
        ModeChoice::ALL,
        Some(app.room_mode_choice(room)),
        Message::RoomModeChosen,
    )
    .text_size(tokens::CAPTION)
    .padding([tokens::XS, tokens::S])
    .style(kit::menu_style);
    kit::tip(menu, "Notifications")
}

/// The main screen: panes above a status bar.
pub fn view(app: &App) -> Element<'_, Message> {
    let content = main_column(
        security::banner_slot(&app.security),
        main_row(app),
        status_bar(app),
    );
    security::layer(
        content,
        security::top_layer(app).or_else(|| sessions::layer(app)),
    )
}

/// The panes, always `[spaces sidebar, room list, room pane, thread panel]`:
/// the sidebar and the panel are empty spaces when they have nothing to show,
/// so the room pane (timeline scroll, editor focus) keeps its place in the
/// widget tree when a thread opens or closes. `from_vec` does not look at its
/// children, so the row is told to fill the window both ways: left at its
/// default width it shrinks to its content and the room pane loses the free
/// width.
pub(crate) fn main_row(app: &App) -> Element<'_, Message> {
    Row::from_vec(vec![
        spaces_sidebar::view(app),
        list_pane(app),
        room_pane(app),
        thread_panel::slot(app),
        call::panel_slot(app),
    ])
    .width(Fill)
    .height(Fill)
    .into()
}

/// The room list pane and the line on its right, as one slot of the row.
fn list_pane(app: &App) -> Element<'_, Message> {
    row![room_list_pane::view(app), kit::vertical_divider()]
        .height(Fill)
        .into()
}

/// What the timeline pane needs besides the timeline, for the open room.
pub fn rich_env<'a>(app: &'a App, theme: &'a iced::Theme) -> RichEnv<'a> {
    RichEnv {
        theme,
        own_user: app.user_id.as_ref(),
        revealed: &app.revealed,
        actions: &app.actions,
        rooms: &app.rooms,
        members: app.known_members(),
        media: &app.media,
        room: app.selected.as_ref(),
    }
}

/// The main column, always `[banner slot, panes, status bar]`: the slot is
/// an empty column when there is no banner, so the panes (timeline scroll,
/// editor focus) keep their place in the widget tree when a banner comes
/// or goes. `from_vec` does not look at its children, so the column is told
/// to fill the window: left at its default it shrinks to the banner slot and
/// the whole window collapses.
fn main_column<'a>(
    banner: Element<'a, Message>,
    panes: Element<'a, Message>,
    status: Element<'a, Message>,
) -> Element<'a, Message> {
    Column::from_vec(vec![banner, panes, status])
        .width(Fill)
        .height(Fill)
        .into()
}

/// The selected room: name, timeline, composer.
fn room_pane(app: &App) -> Element<'_, Message> {
    let selected = app
        .selected
        .as_ref()
        .and_then(|id| app.rooms.iter().find(|room| room.id == *id));
    let title = selected.map_or_else(
        || "Select a room".to_owned(),
        |room| display_name(&room.name),
    );

    let theme = app.current_theme();
    let env = rich_env(app, &theme);
    let timeline = container(timeline_pane::view(&app.timeline, env)).height(Fill);
    // The picker floats over the timeline's bottom right corner.
    let picker = app.actions.picker.as_ref().map(|picker| {
        container(opaque(picker_panel(picker)))
            .width(Fill)
            .height(Fill)
            .align_right(Fill)
            .align_bottom(Fill)
            .padding(8)
            .into()
    });
    let mut bars: Vec<Element<'_, Message>> = Vec::new();
    if let Some(bar) = &app.link_bar {
        bars.push(link_bar(bar));
    }
    if let Some(bar) = &app.rooms_ui.join_bar {
        bars.push(room_dialogs::join_bar(bar));
    }
    if let Some(bar) = &app.media.open_bar {
        bars.push(media_viewer::open_bar(bar));
    }
    if let Some(name) = &app.command_bar {
        bars.push(command_bar(name));
    }
    if let Some(error) = &app.send_error {
        bars.push(text(error.as_str()).style(text::danger).into());
    }
    if let Some(notice) = &app.actions.notice {
        bars.push(text(notice.text.as_str()).style(text::danger).into());
    }
    let enabled = selected.is_some();
    if let Some(line) = timeline_pane::typing_line(&app.typing) {
        bars.push(
            text(line)
                .size(13)
                .style(|theme: &iced::Theme| text::Style {
                    color: Some(theme::semantic(theme).muted),
                })
                .into(),
        );
    }
    if let Some(list) = suggestion_list(&app.composer.suggestions) {
        bars.push(list);
    }
    if let Some(bar) = mode_bar(app) {
        bars.push(bar);
    }
    let body = room_column(
        header(app, title),
        timeline_layer(timeline.into(), picker),
        bars,
        editor(app, enabled),
    );
    container(body).width(Fill).height(Fill).padding(8).into()
}

/// The room pane's column, always `[title, timeline, bars, editor]`. iced
/// keeps a widget's state only while the widget at that place in the tree
/// keeps its type: with the bars (link and command bars, notices, typing
/// line, suggestion list, reply bar) as siblings, the editor moved whenever
/// one came or went and lost its focus. `from_vec` keeps an empty bar column.
fn room_column<'a>(
    title: Element<'a, Message>,
    timeline: Element<'a, Message>,
    bars: Vec<Element<'a, Message>>,
    editor: Element<'a, Message>,
) -> Element<'a, Message> {
    let bars = Column::from_vec(bars).spacing(8).width(Fill);
    Column::from_vec(vec![title, timeline, bars.into(), editor])
        .spacing(8)
        .width(Fill)
        .height(Fill)
        .into()
}

/// The timeline with the reaction picker as an optional layer: always a
/// stack with the timeline first, so opening the picker keeps the scroll.
fn timeline_layer<'a>(
    timeline: Element<'a, Message>,
    picker: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    stack([timeline, picker.unwrap_or_else(|| space().into())]).into()
}

/// Tallest the editor grows before it scrolls, in pixels.
const EDITOR_MAX_HEIGHT: f32 = 160.0;

/// The composer's text editor. Without a selected room it has no `on_action`,
/// which disables it.
fn editor(app: &App, enabled: bool) -> Element<'_, Message> {
    let open = app.composer.suggestions.is_open();
    let mut editor = text_editor(app.composer.content())
        .id(composer::COMPOSER_ID)
        .placeholder("Message")
        .padding(8)
        .max_height(EDITOR_MAX_HEIGHT)
        .key_binding(move |press| composer::binding(press, open));
    if enabled {
        editor = editor.on_action(|action| Message::Composer(EditorAction(action)));
    }
    // The button is always there, so the editor keeps its place in the tree;
    // without a room it does nothing.
    let attach = button(text("Attach\u{2026}"))
        .on_press_maybe(enabled.then_some(Message::AttachRequested))
        .style(button::secondary);
    row![attach, editor]
        .spacing(8)
        .align_y(iced::Alignment::End)
        .into()
}

/// The text of the bar above the editor: the quote of the message being
/// replied to, or a note about the edit. `None` in the normal mode.
pub fn mode_text(app: &App) -> Option<String> {
    match &app.composer.mode {
        Mode::Normal => None,
        Mode::Reply(id) => Some(format!(
            "Replying to {}",
            actions::quote_for(&app.timeline.items, id).unwrap_or_else(|| "a message".to_owned())
        )),
        Mode::Edit(_) => Some("Editing a message".to_owned()),
    }
}

/// The reply or edit bar above the editor, with a cross that leaves the mode.
fn mode_bar(app: &App) -> Option<Element<'_, Message>> {
    let label = mode_text(app)?;
    Some(
        container(
            row![
                text(label),
                space().width(Fill),
                button(text("\u{2715}"))
                    .on_press(Message::ModeCancelled)
                    .style(button::secondary),
            ]
            .spacing(8)
            .align_y(iced::Center),
        )
        .padding(4)
        .style(container::bordered_box)
        .into(),
    )
}

/// Width of the reaction picker, in pixels.
const PICKER_WIDTH: f32 = 360.0;

/// The reaction picker: a search field and the emoji it finds, the quick row
/// first while the search is empty.
fn picker_panel(picker: &Picker) -> Element<'_, Message> {
    let mut grid: Row<'_, Message> = Row::new();
    for glyph in emoji::picker_results(&picker.query) {
        grid = grid.push(
            button(text(glyph).size(22))
                .padding(4)
                .style(button::text)
                .on_press(Message::PickerPicked(MessageText::from(glyph))),
        );
    }
    container(
        column![
            row![
                text_input("Search emoji", &picker.query)
                    .id(actions::PICKER_INPUT_ID)
                    .on_input(|query| Message::PickerQuery(MessageText::from(query)))
                    .on_submit(Message::PickerSubmitted),
                button(text("\u{2715}"))
                    .on_press(Message::PickerClosed)
                    .style(button::secondary),
            ]
            .spacing(8),
            grid.wrap(),
        ]
        .spacing(8),
    )
    .width(PICKER_WIDTH)
    .padding(8)
    .style(container::bordered_box)
    .into()
}

/// The autocomplete rows (members or emojis) above the editor, or `None`
/// while there is nothing to pick.
fn suggestion_list(suggestions: &Suggestions) -> Option<Element<'_, Message>> {
    let rows: Vec<(String, bool)> = match suggestions {
        Suggestions::Closed => return None,
        Suggestions::Mentions {
            members, selected, ..
        } => members
            .iter()
            .enumerate()
            .map(|(i, member)| (member_row_label(member), i == *selected))
            .collect(),
        Suggestions::Emoji {
            items, selected, ..
        } => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                (
                    format!("{} :{}:", item.glyph, item.shortcode),
                    i == *selected,
                )
            })
            .collect(),
    };
    if rows.is_empty() {
        return None;
    }
    let mut list = column![].spacing(2);
    for (index, (label, selected)) in rows.into_iter().enumerate() {
        list = list.push(
            button(text(label))
                .on_press(Message::SuggestionPicked(index))
                .width(Fill)
                .style(if selected {
                    button::primary
                } else {
                    button::text
                }),
        );
    }
    Some(container(list).padding(4).into())
}

/// One member row: the display name and the user ID, sanitised and capped.
pub fn member_row_label(member: &twicecat_core::api::Member) -> String {
    let id = one_line_capped(member.user_id.as_str(), MAX_NAME_CHARS);
    match member.display_name.as_deref() {
        Some(name) if !name.trim().is_empty() => {
            format!("{} ({id})", one_line_capped(name, MAX_NAME_CHARS))
        }
        _ => id,
    }
}

/// The confirmation question for a link. It depends on the link kind and
/// shows only the sanitised host (for `mailto`, the domain), never the URL.
pub fn confirm_text(url: &CheckedUrl) -> String {
    let host = one_line_capped(url.host(), MAX_NAME_CHARS);
    match url.kind() {
        LinkKind::Https => format!("Open {host}?"),
        LinkKind::Http => format!("Open {host}? (not encrypted)"),
        LinkKind::Mailto => format!("Write an email to {host}?"),
    }
}

/// What the link bar says. Only the host of a link is ever shown.
pub fn link_bar_text(bar: &LinkBar) -> String {
    match bar {
        LinkBar::Confirm(url) => confirm_text(url),
        LinkBar::Refused => "This link cannot be opened.".to_owned(),
        LinkBar::Failed => "The link could not be opened.".to_owned(),
    }
}

/// Longest command name shown in the unknown-command bar.
const MAX_COMMAND_CHARS: usize = 64;

/// What the unknown-command bar says (Element's wording).
pub fn command_bar_text(name: &str) -> String {
    format!(
        "Unrecognised command: /{}. Begin your message with // to start it with a slash.",
        one_line_capped(name, MAX_COMMAND_CHARS)
    )
}

/// The bar above the composer about an unknown `/command`: send the text as a
/// message, or go back to editing it.
fn command_bar(name: &str) -> Element<'_, Message> {
    let bar_row = row![
        text(command_bar_text(name)),
        space().width(Fill),
        button(text("Send as message")).on_press(Message::CommandSendAnyway),
        button(text("Cancel"))
            .on_press(Message::CommandCancelled)
            .style(button::secondary),
    ]
    .spacing(8)
    .align_y(iced::Center);
    container(bar_row).padding(4).into()
}

/// The bar above the composer about a clicked link.
fn link_bar(bar: &LinkBar) -> Element<'_, Message> {
    let mut bar_row = row![text(link_bar_text(bar)), space().width(Fill)]
        .spacing(8)
        .align_y(iced::Center);
    match bar {
        LinkBar::Confirm(_) => {
            bar_row = bar_row
                .push(button(text("Open")).on_press(Message::LinkConfirmed))
                .push(
                    button(text("Cancel"))
                        .on_press(Message::LinkDismissed)
                        .style(button::secondary),
                );
        }
        LinkBar::Refused | LinkBar::Failed => {
            bar_row = bar_row.push(
                button(text("Dismiss"))
                    .on_press(Message::LinkDismissed)
                    .style(button::secondary),
            );
        }
    }
    container(bar_row).padding(4).into()
}

/// The signed-in user ID for the status bar: one line, capped. It comes from
/// the server, so it is sanitised like any other remote text.
pub fn account_label(user_id: Option<&UserId>) -> String {
    user_id.map_or_else(String::new, |id| {
        one_line_capped(&id.to_string(), MAX_NAME_CHARS)
    })
}

/// An entry of the account menu in the status bar. Log out is not one: the
/// menu has nothing selected, and iced's `pick_list` then picks its last
/// option on Ctrl + wheel up (the zoom gesture), so no destructive action may
/// be in it. Log out is its own button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountAction {
    /// **Settings**.
    Settings,
    /// **Voice & audio**.
    VoiceAudio,
    /// **Sessions**.
    Sessions,
}

impl AccountAction {
    /// The menu's entries, in order. Log out is deliberately absent: the
    /// menu is a `pick_list` with nothing selected, and Ctrl + wheel up (the
    /// zoom gesture) makes it pick its last entry, which must never be
    /// destructive. Log out is a separate button, see [`log_out_message`].
    pub const ALL: [Self; 3] = [Self::Settings, Self::VoiceAudio, Self::Sessions];

    /// The message choosing the entry sends.
    pub fn message(self) -> Message {
        match self {
            Self::Settings => Message::Settings(SettingsMessage::Open),
            Self::VoiceAudio => Message::Audio(AudioMessage::Open),
            Self::Sessions => Message::Sessions(SessionsMessage::Open),
        }
    }
}

impl fmt::Display for AccountAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Settings => "Settings",
            Self::VoiceAudio => "Voice & audio",
            Self::Sessions => "Sessions",
        })
    }
}

/// What the status bar's **Log out** button sends.
pub fn log_out_message() -> Message {
    Message::LogoutRequested
}

/// How well the sync is going, for the colour of the status dot. The words
/// next to the dot say the same, so the colour is never the only sign.
pub fn sync_severity(state: &SyncState) -> Severity {
    match state {
        SyncState::Running => Severity::Success,
        SyncState::Idle => Severity::Info,
        SyncState::Offline | SyncState::Reconnecting { .. } => Severity::Warning,
        SyncState::Error(_) | SyncState::Terminated => Severity::Danger,
    }
}

/// The words of the status bar's sync part.
pub fn sync_text(state: &SyncState) -> String {
    format!("Sync: {}", sync_label(state))
}

/// A thin line, then the sync dot with words, the account and the account
/// menu (Settings, Voice & audio, Sessions), then a Log out button.
fn status_bar(app: &App) -> Element<'_, Message> {
    let account = account_label(app.user_id.as_ref());
    let severity = sync_severity(&app.sync_state);
    let dot =
        container(icons::icon(Icon::Dot)).style(move |theme: &iced::Theme| container::Style {
            text_color: Some(severity.color(theme)),
            ..container::Style::default()
        });
    let menu = pick_list(AccountAction::ALL, None::<AccountAction>, |action| {
        action.message()
    })
    .placeholder(if account.is_empty() {
        "Account".to_owned()
    } else {
        account
    })
    .text_size(tokens::BODY)
    .padding([tokens::XS, tokens::S])
    .style(kit::menu_style);
    column![
        kit::divider(),
        container(
            row![
                dot,
                text(sync_text(&app.sync_state))
                    .size(tokens::CAPTION)
                    .style(kit::muted),
                space().width(Fill),
                menu,
                kit::secondary_button("Log out", Some(log_out_message())),
            ]
            .spacing(tokens::S)
            .align_y(iced::Center),
        )
        .padding([tokens::XS, tokens::M])
        .width(Fill)
        .style(kit::surface_style),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debug(message: &Message) -> String {
        format!("{message:?}")
    }

    fn app() -> App {
        use crate::state::Startup;
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let (_etx, erx) = tokio::sync::mpsc::channel(1);
        App::new(Startup {
            sink: Box::new(tx),
            events: crate::bridge::EventsSlot::new(erx),
        })
        .0
    }

    #[test]
    fn the_status_bar_still_reaches_every_message_the_old_one_sent() {
        // The old bar was one pick_list: Settings, Sessions, Log out. Settings
        // and Sessions are menu entries now; Log out is its own button.
        assert_eq!(debug(&log_out_message()), debug(&Message::LogoutRequested));
        let menu: Vec<String> = AccountAction::ALL
            .into_iter()
            .map(|a| debug(&a.message()))
            .collect();
        for old in [
            Message::Settings(SettingsMessage::Open),
            Message::Sessions(SessionsMessage::Open),
        ] {
            assert!(menu.contains(&debug(&old)), "{old:?} unreachable");
        }
    }

    #[test]
    fn the_account_menu_has_no_log_out_and_adds_voice_and_audio() {
        let menu: Vec<(String, String)> = AccountAction::ALL
            .into_iter()
            .map(|a| (a.to_string(), debug(&a.message())))
            .collect();
        let expect = [
            ("Settings", debug(&Message::Settings(SettingsMessage::Open))),
            (
                "Voice & audio",
                debug(&Message::Audio(crate::audio_settings::AudioMessage::Open)),
            ),
            ("Sessions", debug(&Message::Sessions(SessionsMessage::Open))),
        ];
        let expect: Vec<(String, String)> = expect
            .into_iter()
            .map(|(label, msg)| (label.to_owned(), msg))
            .collect();
        assert_eq!(menu, expect);
    }

    #[test]
    fn every_header_action_sends_what_the_old_header_sent() {
        let room = RoomId::from("!r:example.org");
        let peer = UserId::from("@bob:example.org");
        let cases = [
            (
                HeaderAction::Members,
                Message::Rooms(RoomsMessage::OpenMembers),
            ),
            (
                HeaderAction::Settings,
                Message::Rooms(RoomsMessage::OpenSettings),
            ),
            (
                HeaderAction::People,
                Message::Call(CallMessage::TogglePanel),
            ),
            (
                HeaderAction::Verify(peer.clone()),
                Message::VerifyPeerRequested(peer),
            ),
            (
                HeaderAction::Accept(room.clone()),
                Message::Rooms(RoomsMessage::Accept(room.clone())),
            ),
            (
                HeaderAction::Decline(room.clone()),
                Message::Rooms(RoomsMessage::Decline(room)),
            ),
        ];
        for (action, old) in cases {
            assert_eq!(debug(&action.message()), debug(&old));
        }
    }

    #[test]
    fn the_call_button_presses_are_still_reachable() {
        let room = RoomId::from("!r:example.org");
        let call = |label: &str, press| CallButton {
            label: label.to_owned(),
            press,
        };
        for press in [
            CallMessage::Join(room.clone()),
            CallMessage::Leave,
            CallMessage::TogglePanel,
        ] {
            let button = call("x", Some(press.clone()));
            assert_eq!(
                call_message(&button).as_ref().map(debug),
                Some(debug(&Message::Call(press)))
            );
        }
        assert!(call_message(&call("Joining\u{2026}", None)).is_none());

        // Every state of the real button: Call, Join call (N), Join voice.
        let app = app();
        let mut summary = twicecat_core::api::RoomSummary::new(room.clone(), "Lounge".into());
        let button = app.calls.button(&summary).expect("a call button");
        assert_eq!(button.label, "Call");
        assert_eq!(
            call_message(&button).as_ref().map(debug),
            Some(debug(&Message::Call(CallMessage::Join(room.clone()))))
        );
        summary.is_call_room = true;
        let button = app.calls.button(&summary).expect("a call button");
        assert_eq!(button.label, "Join voice");
        assert!(call_message(&button).is_some());
    }

    #[test]
    fn leave_is_the_only_danger_button_and_keeps_its_words() {
        let leave = CallButton {
            label: "Leave call".to_owned(),
            press: Some(CallMessage::Leave),
        };
        assert!(is_leave(&leave));
        for press in [None, Some(CallMessage::TogglePanel)] {
            let other = CallButton {
                label: "Call".to_owned(),
                press,
            };
            assert!(!is_leave(&other));
        }
    }

    #[test]
    fn the_sync_dot_has_a_tone_for_every_state_and_the_words_stay() {
        use crate::ui::kit::Severity;
        use twicecat_core::api::SyncState;
        assert_eq!(sync_severity(&SyncState::Running), Severity::Success);
        assert_eq!(sync_severity(&SyncState::Idle), Severity::Info);
        assert_eq!(sync_severity(&SyncState::Offline), Severity::Warning);
        assert_eq!(
            sync_severity(&SyncState::Reconnecting { attempt: 2 }),
            Severity::Warning
        );
        assert_eq!(
            sync_severity(&SyncState::Error("x".into())),
            Severity::Danger
        );
        assert_eq!(sync_severity(&SyncState::Terminated), Severity::Danger);
        assert_eq!(sync_text(&SyncState::Running), "Sync: Running");
    }

    #[test]
    fn the_topic_is_one_capped_line_or_nothing() {
        assert_eq!(topic_line(None), None);
        assert_eq!(topic_line(Some("  \n ")), None);
        assert_eq!(
            topic_line(Some("Plans\nfor lunch")).as_deref(),
            Some("Plans\u{23CE}for lunch")
        );
        let long = topic_line(Some(&"t".repeat(1000))).expect("a line");
        assert_eq!(long.chars().count(), TOPIC_CHARS);
        assert!(long.ends_with('\u{2026}'));
    }

    #[test]
    fn the_frame_builds_for_every_header_state() {
        use twicecat_core::api::RoomSummary;
        let mut app = app();
        let mut joined = RoomSummary::new(RoomId::from("!j:example.org"), "Lounge".into());
        joined.is_encrypted = true;
        joined.is_call_room = true;
        let mut invite = RoomSummary::new(RoomId::from("!i:example.org"), "Club".into());
        invite.is_invite = true;
        app.rooms = vec![joined.clone(), invite.clone()];
        for room in [None, Some(joined), Some(invite)] {
            app.selected = room.map(|r| r.id);
            let _ = header(&app, "title".to_owned());
            let _ = status_bar(&app);
        }
    }

    #[test]
    fn the_peer_line_is_one_line_and_keeps_the_server_name() {
        let id = UserId::from("@bob:example.org");
        assert_eq!(peer_id_line(&id), "@bob:example.org");
        let long = UserId::from(format!("@{}:evil.example", "a".repeat(400)));
        let line = peer_id_line(&long);
        assert!(line.ends_with(":evil.example"), "{line}");
        assert!(line.chars().count() <= HEADER_PEER_ID_CHARS);
        let split = UserId::from("@bo\nb:x");
        assert!(!peer_id_line(&split).contains('\n'));
    }

    #[test]
    fn each_trust_badge_has_its_own_severity() {
        assert_eq!(Badge::Verified.severity(), Severity::Success);
        assert_eq!(Badge::NotVerified.severity(), Severity::Info);
        assert_eq!(Badge::IdentityChanged.severity(), Severity::Danger);
    }

    #[test]
    fn trust_maps_to_a_badge() {
        assert_eq!(trust_badge(UserTrust::Verified), Some(Badge::Verified));
        assert_eq!(trust_badge(UserTrust::Unverified), Some(Badge::NotVerified));
        assert_eq!(
            trust_badge(UserTrust::IdentityChanged),
            Some(Badge::IdentityChanged)
        );
        assert_eq!(trust_badge(UserTrust::NoIdentity), None);
    }

    #[test]
    fn verify_is_offered_unless_already_verified() {
        assert!(!Badge::Verified.can_verify());
        assert!(Badge::NotVerified.can_verify());
        assert!(Badge::IdentityChanged.can_verify());
    }

    #[test]
    fn badges_have_distinct_words() {
        assert_eq!(Badge::Verified.label(), "Verified");
        assert_eq!(Badge::NotVerified.label(), "Not verified");
        assert_eq!(Badge::IdentityChanged.label(), "Identity changed");
    }

    #[test]
    fn the_editor_and_the_timeline_keep_their_place_when_bars_come_and_go() {
        use iced::advanced::widget::Tree;

        let content = text_editor::Content::new();
        let build = |bars: usize, picker: bool| -> Element<'_, Message> {
            let bars = (0..bars).map(|_| text("bar").into()).collect();
            let picker = picker.then(|| text("picker").into());
            room_column(
                text("title").into(),
                timeline_layer(space().into(), picker),
                bars,
                text_editor(&content).into(),
            )
        };
        let mut tree = Tree::new(build(0, false));
        let tags = |tree: &Tree| {
            (
                tree.children.iter().map(|c| c.tag).collect::<Vec<_>>(),
                tree.children[1].children[0].tag,
            )
        };
        let before = tags(&tree);
        // A suggestion list, a notice, the reaction picker...: the editor
        // (keeping its focus) and the timeline (its scroll) keep their tags,
        // so `Tree::diff` keeps their state.
        for (bars, picker) in [(1, false), (3, true), (0, true), (0, false)] {
            tree.diff(build(bars, picker));
            assert_eq!(tags(&tree), before, "{bars} bars, picker {picker}");
        }
    }

    #[test]
    fn the_panes_keep_their_place_when_the_banner_and_security_screens_come_and_go() {
        use crate::security::{Banner, Overlay, RecoverySetup, SecurityModel};
        use iced::advanced::widget::Tree;
        use twicecat_core::api::{DeviceVerification, RecoveryKey, RecoveryStatus};

        let content = text_editor::Content::new();
        fn build<'a>(
            model: &'a SecurityModel,
            content: &'a text_editor::Content,
        ) -> Element<'a, Message> {
            let panes = row![timeline_layer(space().into(), None), text_editor(content)].into();
            let base = main_column(security::banner_slot(model), panes, text("status").into());
            security::layer(base, security::screen(model))
        }
        let tags = |tree: &Tree| {
            (
                tree.children[0].tag,
                tree.children[0]
                    .children
                    .iter()
                    .map(|c| c.tag)
                    .collect::<Vec<_>>(),
                tree.children[0].children[1]
                    .children
                    .iter()
                    .map(|c| c.tag)
                    .collect::<Vec<_>>(),
            )
        };
        let mut model = SecurityModel::default();
        let mut tree = Tree::new(build(&model, &content));
        let before = tags(&tree);
        type Step = Box<dyn Fn(&mut SecurityModel)>;
        let states: Vec<Step> = vec![
            Box::new(|m| m.status.device = DeviceVerification::Unverified),
            Box::new(|m| m.overlay = Some(Overlay::ConfirmIdentity)),
            Box::new(|m| m.overlay = Some(Overlay::Restore)),
            Box::new(|m| m.overlay = Some(Overlay::ResetConfirm)),
            Box::new(|m| {
                m.setup = Some(RecoverySetup::new(RecoveryKey::from("secret-key-x"), false));
            }),
            Box::new(|m| {
                m.setup = None;
                m.overlay = None;
                m.status.device = DeviceVerification::Verified;
                m.status.recovery = RecoveryStatus::Disabled;
            }),
            Box::new(|m| m.status.recovery = RecoveryStatus::Enabled),
        ];
        for apply in &states {
            apply(&mut model);
            tree.diff(build(&model, &content));
            assert_eq!(tags(&tree), before, "{model:?}");
        }
        assert!(!Banner::DeviceUnverified.dismissible());
    }

    #[test]
    fn the_account_label_is_sanitised_and_capped() {
        assert_eq!(account_label(None), "");
        let id = UserId::from("@a\u{202E}b:x");
        assert_eq!(account_label(Some(&id)), "@a\u{FFFD}b:x");
        let long = UserId::from(format!("@{}:x", "u".repeat(1000)));
        assert_eq!(account_label(Some(&long)).chars().count(), 256);
    }

    #[test]
    fn the_command_bar_names_the_command_and_gives_the_hint() {
        let text = command_bar_text("foo");
        assert!(text.contains("Unrecognised command: /foo"), "{text}");
        assert!(text.contains("//"), "{text}");
        let evil = command_bar_text(&format!("x\u{202E}{}", "y".repeat(200)));
        assert!(!evil.contains('\u{202E}'));
        assert!(evil.chars().count() < 160, "{}", evil.chars().count());
    }

    #[test]
    fn the_link_bar_shows_only_the_host() {
        let url = crate::links::check_link_url("https://Evil.example:8443/secret/path?token=abc")
            .expect("valid link");
        let text = link_bar_text(&LinkBar::Confirm(url));
        assert_eq!(text, "Open evil.example?");
        assert!(!link_bar_text(&LinkBar::Refused).is_empty());
        assert!(!link_bar_text(&LinkBar::Failed).is_empty());
    }

    #[test]
    fn the_confirm_text_names_the_link_kind() {
        let text = |u: &str| confirm_text(&crate::links::check_link_url(u).expect("valid link"));
        assert_eq!(text("https://Evil.example/x?t=1"), "Open evil.example?");
        assert_eq!(
            text("http://Evil.example:8080/x?t=1"),
            "Open evil.example? (not encrypted)"
        );
        assert_eq!(
            text("mailto:Bob@Mail.Example?body=secret"),
            "Write an email to mail.example?"
        );
    }

    #[test]
    fn room_names_are_sanitised_and_capped() {
        assert_eq!(display_name("Lounge"), "Lounge");
        assert_eq!(display_name("a\nb\u{202E}c"), "a\u{23CE}b\u{FFFD}c");
        let out = display_name(&"n".repeat(1000));
        assert_eq!(out.chars().count(), 256);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn the_mode_bar_quotes_the_replied_message() {
        use crate::state::Startup;
        use twicecat_core::api::{
            ListDiff, MessageDetails, SendStatus, TimelineItem, TimelineItemKind,
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let (_etx, erx) = tokio::sync::mpsc::channel(1);
        let (mut app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: crate::bridge::EventsSlot::new(erx),
        });
        assert_eq!(mode_text(&app), None);
        app.timeline.apply_diffs(vec![ListDiff::Append {
            values: vec![TimelineItem::new(
                "m".to_owned(),
                TimelineItemKind::Message {
                    sender: UserId::from("@a:x"),
                    body: "hello\nthere".into(),
                    is_own: false,
                    timestamp_ms: 1,
                    sent: SendStatus::Sent,
                    details: Box::new(MessageDetails::default()),
                },
            )],
        }]);
        app.composer.mode = Mode::Reply("m".into());
        assert_eq!(mode_text(&app).as_deref(), Some("Replying to @a:x: hello"));
        app.composer.mode = Mode::Reply("gone".into());
        assert_eq!(mode_text(&app).as_deref(), Some("Replying to a message"));
        app.composer.mode = Mode::Edit("m".into());
        assert_eq!(mode_text(&app).as_deref(), Some("Editing a message"));
    }
}
