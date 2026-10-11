//! The two-pane main layout: the room list pane on the left and the room view
//! (timeline pane and composer) on the right.

use iced::widget::{
    Column, Row, button, column, container, opaque, pick_list, row, space, stack, text,
    text_editor, text_input,
};
use iced::{Element, Fill};
use twicecat_core::api::{MessageText, RoomId, UserId, UserTrust};

use crate::actions::{self, Picker};
use crate::composer::{self, EditorAction, Mode, Suggestions};
use crate::emoji;
use crate::links::{CheckedUrl, LinkBar, LinkKind};
use crate::notifications::ModeChoice;
use crate::rooms::RoomsMessage;
use crate::screens::loading::sync_label;
use crate::screens::timeline_pane::RichEnv;
use crate::screens::{
    media_viewer, room_dialogs, room_list_pane, security, sessions, spaces_sidebar, thread_panel,
    timeline_pane,
};
use crate::sessions::SessionsMessage;
use crate::settings::SettingsMessage;
use crate::state::{App, Message};
use crate::text_safety::{MAX_NAME_CHARS, name_or_placeholder, one_line_capped};
use crate::theme;
use crate::verification::{MAX_PEER_USER_ID_CHARS, elide_middle};

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

/// The peer's user ID as the header shows it: one sanitized line, never cut
/// from the end (see [`crate::verification::elide_middle`]).
pub fn peer_id_line(peer: &UserId) -> String {
    elide_middle(peer.as_str(), MAX_PEER_USER_ID_CHARS)
}

/// The room header: the name, a lock for an encrypted room, and for a direct
/// message the trust of the other person with **Verify** where it helps. Always
/// one row, so the column around it keeps its widget types.
fn header<'a>(app: &'a App, title: String) -> Element<'a, Message> {
    let mut line = Row::new().spacing(8).align_y(iced::Center);
    line = line.push(text(title).size(22));
    let selected = app
        .selected
        .as_ref()
        .and_then(|id| app.rooms.iter().find(|room| room.id == *id));
    if let Some(lock) = selected.and_then(room_list_pane::encrypted_mark) {
        line = line.push(text(lock).size(16));
    }
    if let Some(room) = selected.filter(|room| !room.is_invite) {
        line = line.push(notification_menu(app, &room.id));
        line = line.push(room_buttons());
    }
    if let Some(room) = selected.filter(|room| room.is_invite) {
        line = line.push(invite_buttons(app, &room.id));
    }
    if let Some((peer, badge)) = app
        .dm_peer_trust()
        .and_then(|(peer, trust)| trust_badge(trust).map(|badge| (peer, badge)))
    {
        line = line.push(
            text(badge.label())
                .size(13)
                .style(move |theme: &iced::Theme| text::Style {
                    color: Some(match badge {
                        Badge::Verified => theme.palette().success,
                        Badge::NotVerified => theme::semantic(theme).muted,
                        Badge::IdentityChanged => theme.palette().danger,
                    }),
                }),
        );
        // Whose identity the badge vouches for: the full ID, one line, cut in
        // the middle if at all (the server name at the end tells users apart).
        line = line.push(
            text(peer_id_line(peer))
                .size(13)
                .style(|theme: &iced::Theme| text::Style {
                    color: Some(theme::semantic(theme).muted),
                }),
        );
        if badge.can_verify() {
            line = line.push(
                button(text("Verify").size(13))
                    .padding([2, 8])
                    .style(button::secondary)
                    .on_press(Message::VerifyPeerRequested(peer.clone())),
            );
        }
    }
    line.into()
}

/// **Members** and **Settings**, in the header of a joined room.
fn room_buttons<'a>() -> Element<'a, Message> {
    row![
        button(text("Members").size(13))
            .padding([2, 8])
            .style(button::secondary)
            .on_press(Message::Rooms(RoomsMessage::OpenMembers)),
        button(text("Settings").size(13))
            .padding([2, 8])
            .style(button::secondary)
            .on_press(Message::Rooms(RoomsMessage::OpenSettings)),
    ]
    .spacing(6)
    .into()
}

/// **Accept** and **Decline**, in the header of an invitation. They are off
/// while the answer is on its way.
fn invite_buttons<'a>(app: &App, room: &RoomId) -> Element<'a, Message> {
    let idle = !app.rooms_ui.invite_busy.contains(room);
    row![
        text("You were invited").size(13),
        button(text("Accept").size(13))
            .padding([2, 8])
            .on_press_maybe(idle.then(|| Message::Rooms(RoomsMessage::Accept(room.clone())))),
        button(text("Decline").size(13))
            .padding([2, 8])
            .style(button::secondary)
            .on_press_maybe(idle.then(|| Message::Rooms(RoomsMessage::Decline(room.clone())))),
    ]
    .spacing(6)
    .align_y(iced::Center)
    .into()
}

/// The room's notification menu: a drop-down with the four choices of plan
/// Q11. It shows what the user picked at once, since the room list lags
/// behind a change.
fn notification_menu<'a>(app: &'a App, room: &RoomId) -> Element<'a, Message> {
    let label = text("Notifications")
        .size(13)
        .style(|theme: &iced::Theme| text::Style {
            color: Some(theme::semantic(theme).muted),
        });
    let menu = pick_list(
        ModeChoice::ALL,
        Some(app.room_mode_choice(room)),
        Message::RoomModeChosen,
    )
    .text_size(13)
    .padding([2, 8]);
    row![label, menu].spacing(6).align_y(iced::Center).into()
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
        room_list_pane::view(app),
        room_pane(app),
        thread_panel::slot(app),
    ])
    .width(Fill)
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

/// Sync state, account and **Log out**.
fn status_bar(app: &App) -> Element<'_, Message> {
    let account = account_label(app.user_id.as_ref());
    container(
        row![
            text(format!("Sync: {}", sync_label(&app.sync_state))).style(|t| text::Style {
                color: Some(theme::semantic(t).muted),
            }),
            space().width(Fill),
            text(account),
            button(text("Settings"))
                .on_press(Message::Settings(SettingsMessage::Open))
                .style(button::secondary),
            button(text("Sessions"))
                .on_press(Message::Sessions(SessionsMessage::Open))
                .style(button::secondary),
            button(text("Log out"))
                .on_press(Message::LogoutRequested)
                .style(button::secondary),
        ]
        .spacing(12)
        .align_y(iced::Center),
    )
    .padding(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peer_line_is_one_line_and_keeps_the_server_name() {
        let id = UserId::from("@bob:example.org");
        assert_eq!(peer_id_line(&id), "@bob:example.org");
        let long = UserId::from(format!("@{}:evil.example", "a".repeat(400)));
        let line = peer_id_line(&long);
        assert!(line.ends_with(":evil.example"), "{line}");
        assert!(line.chars().count() <= MAX_PEER_USER_ID_CHARS);
        let split = UserId::from("@bo\nb:x");
        assert!(!peer_id_line(&split).contains('\n'));
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
