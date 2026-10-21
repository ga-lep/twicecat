//! The room list pane: tabs, search and the windowed list of rows (plan Q12).
//!
//! Only the rows near the viewport are built, between two spacers that keep
//! the scrollable's content `len * row height` tall (the row height is the
//! density's, or the rail's fixed 56 px). Names and previews come from other
//! people, so they pass through [`crate::text_safety`] first (see
//! [`row_name`] and [`preview_line`]).
//!
//! The pane is either expanded or collapsed to a rail
//! ([`RoomListChoice`]). The column is always `[actions, tabs, search, list,
//! call bar]` in both, so the list keeps its place when Ctrl+B switches.

use chrono::{Local, TimeZone};
use iced::widget::text::Wrapping;
use iced::widget::tooltip::{Position, Tooltip};
use iced::widget::{
    Column, button, column, container, image, mouse_area, row, scrollable, space, stack, text,
    text_input,
};
use iced::{Background, Color, Element, Fill, Length, border, mouse};
use twicecat_core::api::{
    RoomId, RoomListChoice, RoomListTab, RoomNotificationModeView, RoomSummary, SectionKey,
};

use crate::avatars::{AvatarState, Avatars, NEUTRAL_INITIALS, initials, placeholder_color};
use crate::calls::{CallPill, member_name};
use crate::dates::{day_label, time_label};
use crate::drag::Zone;
use crate::list_drop::{self, ListKey};
use crate::notifications::muted_mark;
use crate::room_list::{
    Badge, OVERSCAN_ROWS, RAIL_ROW_HEIGHT, badge_for, empty_label, preview_line, row_height,
    row_name, spacer_heights, visible_range,
};
use crate::rooms::RoomsMessage;
use crate::screens::{call, main_view};
use crate::sections;
use crate::sections::{
    EMPTY_HINT, Header, Line, More, Person, line_height, line_offsets, line_spacers, visible_lines,
};
use crate::settings::SettingsMessage;
use crate::sidebar::SidebarState;
use crate::sidebar_menu::SidebarMenuMessage;
use crate::spaces::SpacesMessage;
use crate::state::{App, Message};
use crate::theme;
use crate::ui::fonts;
use crate::ui::icons::{self, Icon};
use crate::ui::kit;
use crate::ui::tokens::{self, Density};

/// Width of the collapsed room list, in logical pixels.
pub const RAIL_WIDTH: f32 = 72.0;

/// The id of the list's scrollable, so a change of row height can move it.
pub const LIST_SCROLL_ID: &str = "room-list";

/// Room kept free at the right of the list for its scrollbar, so the bar
/// never covers a count or the end of a preview (the bar is 10 px wide).
pub const SCROLLBAR_GUTTER: f32 = 12.0;

/// Width of the accent bar at the left of the selected row.
const ACCENT_BAR: f32 = 3.0;

/// The width of the pane: 300 px expanded, 72 px as the rail.
pub const fn pane_width(list: RoomListChoice) -> f32 {
    match list {
        RoomListChoice::Expanded => main_view::LIST_PANE_WIDTH,
        RoomListChoice::Rail => RAIL_WIDTH,
    }
}

/// Whether a row has a second line with the preview: Compact rows are one
/// line.
pub const fn shows_preview(density: Density) -> bool {
    matches!(density, Density::Comfortable)
}

/// **New room**, **Direct message** and **Join**, above the tabs.
fn room_actions<'a>() -> Element<'a, Message> {
    let action = |label: &'static str, message: RoomsMessage| {
        button(text(label).size(tokens::CAPTION).wrapping(Wrapping::None))
            .on_press(Message::Rooms(message))
            .padding([tokens::XS, tokens::S])
            .style(kit::secondary_style)
    };
    row![
        action("New room", RoomsMessage::OpenCreate),
        action("Direct message", RoomsMessage::OpenDm),
        action("Join", RoomsMessage::OpenJoin),
    ]
    .spacing(tokens::XS)
    .into()
}

/// The same three actions as icons in a column, for the rail. Each has a
/// tooltip with its words.
fn rail_actions<'a>() -> Element<'a, Message> {
    let action = |icon: Icon, label: &'static str, message: RoomsMessage| {
        kit::tip(
            button(icons::icon(icon))
                .on_press(Message::Rooms(message))
                .padding(tokens::S)
                .style(kit::icon_style),
            label,
        )
    };
    container(
        column![
            action(Icon::Plus, "New room", RoomsMessage::OpenCreate),
            action(Icon::Members, "Direct message", RoomsMessage::OpenDm),
            action(Icon::ChevronRight, "Join a room", RoomsMessage::OpenJoin),
        ]
        .spacing(tokens::XS)
        .align_x(iced::Center),
    )
    .center_x(Fill)
    .into()
}

/// The tabs, left to right.
pub const TABS: [RoomListTab; 4] = [
    RoomListTab::All,
    RoomListTab::People,
    RoomListTab::Rooms,
    RoomListTab::Spaces,
];

/// Side of the round avatar left of each row.
pub const AVATAR_SLOT: f32 = 40.0;
/// Side of the round avatar of a Compact row.
pub const COMPACT_AVATAR_SLOT: f32 = 28.0;

/// The label of a tab.
pub fn tab_label(tab: RoomListTab) -> &'static str {
    match tab {
        RoomListTab::All => "All",
        RoomListTab::People => "DMs",
        RoomListTab::Rooms => "Rooms",
        RoomListTab::Spaces => "Spaces",
    }
}

/// A badge count as text: `99+` above 99.
///
/// ```
/// use twicecat_app::screens::room_list_pane::count_label;
/// assert_eq!(count_label(7), "7");
/// assert_eq!(count_label(100), "99+");
/// ```
pub fn count_label(count: u64) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// The mark of an encrypted room, in the list row and the room header.
pub const LOCK_MARK: &str = Icon::Lock.glyph();
/// The word at the right of the Starred header: the stars are the user's own.
pub const PERSONAL_LABEL: &str = "personal";
/// The tag of a voice room (MSC3417 call room).
pub const VOICE_TAG: &str = "Voice";

/// The lock of `room`, when it is known to be encrypted. The core reports
/// `is_encrypted` as false while the SDK does not know the room's encryption
/// yet, so a missing lock does not say "not encrypted": only the lock is
/// ever drawn, never a mark for the opposite.
pub fn encrypted_mark(room: &RoomSummary) -> Option<&'static str> {
    room.is_encrypted.then_some(LOCK_MARK)
}

/// The time shown on a row: the clock for a message of today, else the day
/// ("Yesterday", a weekday, `2 October`).
pub fn row_time<Tz: TimeZone>(timestamp_ms: u64, now_ms: u64, tz: &Tz) -> String {
    let day = day_label(timestamp_ms, now_ms, tz);
    if day == "Today" {
        time_label(timestamp_ms, tz)
    } else {
        day
    }
}

/// What the rail's tooltip says about a room: its name, then the words for
/// the dots on its avatar (unread, mentions, call) and the muted mark.
pub fn rail_tip(
    name: &str,
    badge: Badge,
    call: Option<CallPill>,
    muted: Option<&str>,
    marks: &[RailMark],
) -> String {
    let mut parts = vec![name.to_owned()];
    match badge {
        Badge::None => {}
        Badge::Highlight(1) => parts.push("1 mention".to_owned()),
        Badge::Highlight(count) => parts.push(format!("{} mentions", count_label(count))),
        Badge::Count(count) => parts.push(format!("{} unread", count_label(count))),
        Badge::Dot => parts.push("unread".to_owned()),
    }
    if let Some(call) = call {
        parts.push(if call.ringing { "ringing" } else { "in a call" }.to_owned());
    }
    parts.extend(marks.iter().map(|mark| mark.words().to_owned()));
    parts.extend(muted.map(str::to_owned));
    parts.join(", ")
}

/// A mark a rail row draws on its avatar besides the dots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailMark {
    /// An invitation to join.
    Invite,
    /// A voice room (MSC3417 call room).
    Voice,
    /// An encrypted room.
    Encrypted,
}

impl RailMark {
    /// The words of the mark, for the row's tooltip.
    pub const fn words(self) -> &'static str {
        match self {
            Self::Invite => "invite",
            Self::Voice => "voice room",
            Self::Encrypted => "encrypted",
        }
    }

    /// The icon drawn on the avatar.
    pub const fn icon(self) -> Icon {
        match self {
            Self::Invite => Icon::Plus,
            Self::Voice => Icon::Call,
            Self::Encrypted => Icon::Lock,
        }
    }
}

/// The marks of a rail row, Invite first.
pub fn rail_marks(room: &RoomSummary) -> Vec<RailMark> {
    let mut marks = Vec::new();
    if room.is_invite {
        marks.push(RailMark::Invite);
    }
    if room.is_call_room {
        marks.push(RailMark::Voice);
    }
    if encrypted_mark(room).is_some() {
        marks.push(RailMark::Encrypted);
    }
    marks
}

/// The words of the call pill: the number of devices in the call, or what
/// the call is when nobody is counted.
fn call_label(call: CallPill) -> String {
    if call.count > 0 {
        call.count.to_string()
    } else if call.ringing {
        "Ringing".to_owned()
    } else {
        "Call".to_owned()
    }
}

/// What an empty list adds to its title.
fn empty_hint(label: &str) -> &'static str {
    if label == "No match" {
        "Try another search"
    } else {
        "Rooms you join will appear here"
    }
}

/// The tabs as a segmented control: one track, the chosen tab filled and
/// outlined.
fn segmented_tabs<'a>(active: RoomListTab) -> Element<'a, Message> {
    let tabs = row(TABS.into_iter().map(|tab| {
        button(
            text(tab_label(tab))
                .size(tokens::CAPTION)
                .wrapping(Wrapping::None)
                .width(Fill)
                .align_x(iced::Center),
        )
        .on_press(Message::TabSelected(tab))
        .width(Fill)
        .padding([tokens::XS, tokens::S])
        .style(if tab == active {
            kit::active_icon_style
        } else {
            kit::ghost_style
        })
        .into()
    }))
    .spacing(tokens::XS);
    container(tabs)
        .padding(tokens::XS)
        .width(Fill)
        .style(kit::card_style)
        .into()
}

/// The search field with a search icon and, once there is text, a button
/// that clears it. The clear slot is a space while empty, so the field keeps
/// its place and its focus.
fn search_field(app: &App) -> Element<'_, Message> {
    let clear: Element<'_, Message> = if app.search_input.is_empty() {
        space().width(0).into()
    } else {
        kit::tip(
            button(icons::icon(Icon::Close))
                .on_press(Message::SearchChanged(String::new()))
                .padding(tokens::S)
                .style(kit::icon_style),
            "Clear search",
        )
    };
    row![
        icons::icon(Icon::Search),
        text_input("Search rooms", &app.search_input)
            .style(kit::text_input_style)
            .on_input(Message::SearchChanged)
            .padding(tokens::S),
        clear,
    ]
    .spacing(tokens::S)
    .align_y(iced::Center)
    .into()
}

/// The rail's search: a button that expands the list, where the field is.
fn rail_search<'a>() -> Element<'a, Message> {
    container(kit::tip(
        button(icons::icon(Icon::Search))
            .on_press(Message::Settings(SettingsMessage::RoomList(
                RoomListChoice::Expanded,
            )))
            .padding(tokens::S)
            .style(kit::icon_style),
        "Search rooms (expands the list)",
    ))
    .center_x(Fill)
    .into()
}

/// What an empty list says.
fn empty_view(app: &App, rail: bool) -> Element<'_, Message> {
    let label = empty_label(app.query.tab, &app.query.search);
    if rail {
        // 56 px wide: the title in small type, no hint.
        return container(
            text(label)
                .size(tokens::CAPTION)
                .style(kit::muted)
                .align_x(iced::Center),
        )
        .center_x(Fill)
        .padding([tokens::L, 0.0])
        .into();
    }
    kit::empty_state(label, empty_hint(label))
}

/// The pane: tabs, search, then the list or an empty-state line.
pub fn view(app: &App) -> Element<'_, Message> {
    let rail = app.pane_layout().list == RoomListChoice::Rail;
    let density = app.density();
    let list = if app.rooms.is_empty() {
        empty_view(app, rail)
    } else {
        windowed_list(app)
    };
    let tabs: Element<'_, Message>;
    let (actions, search, dock) = if rail {
        tabs = space().height(0).into();
        (rail_actions(), rail_search(), call::rail_dock(app))
    } else {
        tabs = segmented_tabs(app.query.tab);
        (room_actions(), search_field(app), call::dock(app))
    };
    // The call bar is last, so the list keeps its place when a call starts.
    // `from_vec` keeps the empty slots (`push` drops a zero-sized child), so
    // the five slots stay put between the expanded list and the rail.
    let slots = Column::from_vec(vec![actions, tabs, search, list, dock])
        .spacing(density.gap())
        .width(Fill)
        .height(Fill);
    container(slots)
        .width(Length::Fixed(pane_width(app.pane_layout().list)))
        .height(Fill)
        .padding(if rail { tokens::XS } else { tokens::S })
        .into()
}

/// What a row needs besides its room.
struct RowEnv<'a> {
    avatars: &'a Avatars,
    density: Density,
    /// The clock, for the time of the last message.
    now_ms: u64,
}

/// The scrollable with only the rows near the viewport built.
fn windowed_list(app: &App) -> Element<'_, Message> {
    if let Some(lines) = app.section_lines() {
        return sectioned_list(app, &lines);
    }
    let len = app.rooms.len();
    let rail = app.pane_layout().list == RoomListChoice::Rail;
    let row_h = app.room_row_height();
    let range = visible_range(
        app.list_offset_y,
        app.list_viewport_h,
        row_h,
        len,
        OVERSCAN_ROWS,
    );
    let (top, bottom) = spacer_heights(&range, len, row_h);
    let env = RowEnv {
        avatars: &app.avatars,
        density: app.density(),
        now_ms: u64::try_from(Local::now().timestamp_millis()).unwrap_or(0),
    };
    let rows = app.rooms[range].iter().map(|room| {
        let selected = app.selected.as_ref() == Some(&room.id);
        let mode = app.effective_room_mode(&room.id);
        let call = app.calls.pill(&room.id);
        if rail {
            rail_row(room, env.avatars, selected, mode, call)
        } else {
            room_row(room, &env, selected, mode, call)
        }
    });
    scroller(top, rows, bottom)
}

/// The scrollable of both lists: `rows` between two spacers that keep its
/// content as tall as the whole list, with room for the scrollbar.
fn scroller<'a>(
    top: f32,
    rows: impl IntoIterator<Item = Element<'a, Message>>,
    bottom: f32,
) -> Element<'a, Message> {
    let content = column![space().height(top)]
        .extend(rows)
        .push(space().height(bottom))
        .padding(iced::Padding {
            right: SCROLLBAR_GUTTER,
            ..iced::Padding::ZERO
        });
    scrollable(content)
        .id(LIST_SCROLL_ID)
        .on_scroll(|viewport| Message::RoomListScrolled {
            offset_y: viewport.absolute_offset().y,
            viewport_h: viewport.bounds().height,
        })
        .height(Fill)
        .into()
}

/// The sectioned list (a space is selected): headers, rooms and hints, each
/// at its own height, with only the lines near the viewport built. Rooms keep
/// today's row.
fn sectioned_list<'a>(app: &'a App, lines: &[Line<'a>]) -> Element<'a, Message> {
    let density = app.density();
    let offsets = line_offsets(lines, density);
    let range = visible_lines(
        &offsets,
        app.list_offset_y,
        app.list_viewport_h,
        OVERSCAN_ROWS,
    );
    let (top, bottom) = line_spacers(&offsets, &range);
    let env = RowEnv {
        avatars: &app.avatars,
        density,
        now_ms: u64::try_from(Local::now().timestamp_millis()).unwrap_or(0),
    };
    let rows = lines[range].iter().map(|line| match line {
        Line::Header(header) => section_header(app, header, density),
        Line::Room(room) => {
            let selected = app.selected.as_ref() == Some(&room.id);
            let mode = app.effective_room_mode(&room.id);
            let call = app.calls.pill(&room.id);
            let cue = list_cue(app, &ListKey::Room(room.id.clone()));
            let pointer = Pointer::Area {
                hovered: cue.hovered,
                dragged: cue.dragged,
            };
            sectioned_room_row(
                room_row_in(room, &env, selected, mode, call, pointer),
                room,
                selected,
                density,
                cue,
            )
        }
        Line::Participant(person) => person_line(app, person, density),
        Line::More(more) => more_line(more, density),
        Line::EmptyHint(key) => {
            let target = ListKey::Header(key.clone());
            let cue = list_cue(app, &target);
            let height = line_height(line, density);
            let hint = container(text(EMPTY_HINT).size(tokens::CAPTION).style(kit::muted))
                .width(Fill)
                .padding([0.0, tokens::S])
                .center_y(Length::Fixed(height))
                .style(move |t: &iced::Theme| mark_style(t, cue.mark, false));
            // A drop target only: a press on the hint does nothing, and the
            // section's own header is what clicks and drags.
            let mut area = mouse_area(hint)
                .on_enter(Message::Spaces(SpacesMessage::ListEnter(target.clone())))
                .on_exit(Message::Spaces(SpacesMessage::ListExit(target.clone())));
            if cue.dragging {
                let at = target;
                area = area.interaction(cue.cursor).on_move(move |point| {
                    Message::Spaces(SpacesMessage::ListOver(
                        at.clone(),
                        list_drop::halves(point.y, height),
                    ))
                });
            }
            area.into()
        }
    });
    scroller(top, rows, bottom)
}

/// Thickness of the line that shows where a drop would put the dragged item.
const DROP_LINE: f32 = 3.0;

/// Width of the ring around a header that takes a drop, or refuses it.
const MARK_RING: f32 = 2.0;

/// What dragging shows on one row or header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    /// Nothing.
    None,
    /// A line above: a drop would put the item before this one.
    Before,
    /// A line below.
    After,
    /// A ring: a drop would put the item into this section.
    Into,
    /// A red ring and the words "Not allowed": the item may not go here.
    Refused,
}

/// What dragging shows on and does to one row or header.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ListCue {
    /// Where a drop here would land, while a drag runs.
    mark: Mark,
    /// The cursor: `Grab` for what can be dragged, `Grabbing` during a drag,
    /// `NotAllowed` over a refused place.
    cursor: mouse::Interaction,
    /// A drag runs: the area reports the pointer's place in it.
    dragging: bool,
    /// The pointer is over it, and no drag runs.
    hovered: bool,
    /// It is the one being dragged.
    dragged: bool,
}

/// The cue of the row or header `key`.
fn list_cue(app: &App, key: &ListKey) -> ListCue {
    let dragging = app.list_drag.is_dragging();
    let may_drag = matches!(
        app.sidebar.state(),
        SidebarState::Loaded | SidebarState::Full
    );
    let refused = app.list_drag.refused().is_some_and(|t| t.item == *key);
    let mark = if refused {
        Mark::Refused
    } else {
        match app.list_drag.target().filter(|t| t.item == *key) {
            Some(t) if t.zone == Zone::Before => Mark::Before,
            Some(t) if t.zone == Zone::After => Mark::After,
            Some(_) => Mark::Into,
            None => Mark::None,
        }
    };
    let cursor = if refused {
        mouse::Interaction::NotAllowed
    } else if dragging {
        mouse::Interaction::Grabbing
    } else if may_drag {
        mouse::Interaction::Grab
    } else {
        mouse::Interaction::Pointer
    };
    ListCue {
        mark,
        cursor,
        dragging,
        hovered: !dragging && app.list_hover.as_ref() == Some(key),
        dragged: app.list_drag.dragged() == Some(key),
    }
}

/// The look of a row or header under the pointer or a drag. The colours are
/// the theme's: the selection, the hover and the focus ring of a target, the
/// danger colour of a refused place.
fn mark_style(t: &iced::Theme, mark: Mark, hovered: bool) -> container::Style {
    let sem = theme::semantic(t);
    match mark {
        Mark::Into => container::Style {
            background: Some(Background::Color(sem.selection)),
            border: border::rounded(tokens::SMALL)
                .color(sem.focus)
                .width(MARK_RING),
            ..container::Style::default()
        },
        Mark::Refused => container::Style {
            background: Some(Background::Color(sem.danger_soft)),
            border: border::rounded(tokens::SMALL)
                .color(t.palette().danger)
                .width(MARK_RING),
            ..container::Style::default()
        },
        Mark::None | Mark::Before | Mark::After => container::Style {
            background: hovered.then_some(Background::Color(sem.message_hover)),
            border: border::rounded(tokens::SMALL),
            ..container::Style::default()
        },
    }
}

/// `content`, `height` tall, with the line of a before or after mark over its
/// top or bottom edge. Always a stack, so the widget tree keeps its shape
/// when a mark comes and goes.
fn with_mark<'a>(content: Element<'a, Message>, mark: Mark, height: f32) -> Element<'a, Message> {
    let line = |top: bool| -> Element<'a, Message> {
        let bar = container(space().width(Fill).height(DROP_LINE)).style(|t: &iced::Theme| {
            container::Style {
                background: Some(Background::Color(theme::semantic(t).focus)),
                border: border::rounded(DROP_LINE / 2.0),
                ..container::Style::default()
            }
        });
        // Clear of the accent bar at the left of a row.
        let layer = container(bar)
            .width(Fill)
            .height(Length::Fixed(height))
            .padding(iced::Padding {
                left: ACCENT_BAR,
                ..iced::Padding::ZERO
            });
        if top {
            layer.align_top(Length::Fixed(height)).into()
        } else {
            layer.align_bottom(Length::Fixed(height)).into()
        }
    };
    let mut layers = stack![content].width(Fill).height(Length::Fixed(height));
    match mark {
        Mark::Before => layers = layers.push(line(true)),
        Mark::After => layers = layers.push(line(false)),
        Mark::None | Mark::Into | Mark::Refused => {}
    }
    layers.into()
}

/// The area that makes a row or header a click and a drag: one press that is
/// a click if the pointer stays and a drag if it goes (`drag.rs`), a
/// right-click for the options card, and while a drag runs the pointer's
/// place in it.
fn list_area<'a>(
    content: Element<'a, Message>,
    key: ListKey,
    cue: ListCue,
    height: f32,
    open: Message,
) -> Element<'a, Message> {
    let mut area = mouse_area(with_mark(content, cue.mark, height))
        .interaction(cue.cursor)
        .on_enter(Message::Spaces(SpacesMessage::ListEnter(key.clone())))
        .on_exit(Message::Spaces(SpacesMessage::ListExit(key.clone())))
        .on_press(Message::Spaces(SpacesMessage::ListPress(key.clone())))
        .on_release(Message::Spaces(SpacesMessage::ListRelease(key.clone())))
        .on_right_press(open);
    if cue.dragging {
        area = area.on_move(move |point| {
            Message::Spaces(SpacesMessage::ListOver(
                key.clone(),
                list_drop::halves(point.y, height),
            ))
        });
    }
    area.into()
}

/// A room row of the sectioned list with its options: a right-click opens the
/// room's card, and the selected row also has the "⋯" button at its right. A
/// click opens the room and a drag moves it (see [`list_area`]).
fn sectioned_room_row<'a>(
    row_view: Element<'a, Message>,
    room: &RoomSummary,
    selected: bool,
    density: Density,
    cue: ListCue,
) -> Element<'a, Message> {
    let open = Message::SidebarMenu(SidebarMenuMessage::OpenRoom(room.id.clone()));
    let content: Element<'a, Message> = if selected {
        row![
            row_view,
            container(kit::compact_icon_button(Icon::More, open.clone()))
                .center_y(Length::Fixed(row_height(density)))
        ]
        .into()
    } else {
        row_view
    };
    list_area(
        content,
        ListKey::Room(room.id.clone()),
        cue,
        row_height(density),
        open,
    )
}

/// The header of a section: a chevron, the name, "personal" on Starred, the
/// rolled-up badge while it is collapsed, and the "⋯" button. A click on
/// the name collapses or expands it; a right-click or the button opens the
/// section's card.
fn section_header<'a>(app: &App, header: &Header<'a>, density: Density) -> Element<'a, Message> {
    let key = ListKey::Header(header.key.clone());
    let cue = list_cue(app, &key);
    let chevron = if header.collapsed {
        Icon::SectionClosed
    } else {
        Icon::SectionOpen
    };
    let mut content = row![
        text(chevron.glyph())
            .size(tokens::CAPTION)
            .style(kit::muted)
            .width(Length::Fixed(14.0))
            .wrapping(Wrapping::None),
        container(
            text(row_name(header.name))
                .size(tokens::CAPTION)
                .font(fonts::BODY_SEMIBOLD)
                .style(kit::muted)
                .wrapping(Wrapping::None)
        )
        .width(Fill)
        .clip(true),
    ]
    .spacing(tokens::XS)
    .align_y(iced::Center);
    if header.key == SectionKey::Starred {
        content = content.push(
            text(PERSONAL_LABEL)
                .size(tokens::CAPTION)
                .style(kit::muted)
                .wrapping(Wrapping::None),
        );
    }
    if header.collapsed {
        content = content.push(badge_view(header.badge, false));
    }
    if cue.mark == Mark::Refused {
        // Not by colour alone.
        content = content.push(
            text(NOT_ALLOWED)
                .size(tokens::CAPTION)
                .style(iced::widget::text::danger)
                .wrapping(Wrapping::None),
        );
    }
    let height = line_height(&Line::Header(header.clone()), density);
    let open = Message::SidebarMenu(SidebarMenuMessage::OpenSection(header.key.clone()));
    let (mark, hovered) = (cue.mark, cue.hovered || cue.dragged);
    // A click on the name collapses or expands it; a drag moves the section.
    // No button: it would swallow the press (see `list_area`).
    let toggle = container(content)
        .width(Fill)
        .center_y(Length::Fixed(height))
        .padding([0.0, tokens::S])
        .style(move |t: &iced::Theme| mark_style(t, mark, hovered));
    let line = row![
        toggle,
        container(kit::compact_icon_button(Icon::More, open.clone()))
            .center_y(Length::Fixed(height))
    ]
    .align_y(iced::Center)
    .height(Length::Fixed(height));
    list_area(line.into(), key, cue, height, open)
}

/// Width of a person's picture under a voice room.
pub const PERSON_AVATAR: f32 = 20.0;

/// How far a person's line is moved in, so it sits under the room's name.
const PERSON_INDENT: f32 = ACCENT_BAR + tokens::S + tokens::XL;

/// The words of the "and N more" line.
pub fn more_label(hidden: usize) -> String {
    format!("and {hidden} more")
}

/// A person in a voice room's call: a 20 px picture (ringed while they
/// speak), the name and, when muted, the muted icon in the danger colour. A
/// click opens the voice room. Speaking and muted are known only for the call
/// this device is in, so the other rooms' people carry neither mark.
fn person_line<'a>(app: &App, person: &Person<'a>, density: Density) -> Element<'a, Message> {
    let name = member_name(person.member);
    let picture = kit::speaking_ring_sized(
        kit::avatar_sized(
            &person.member.user_id,
            &name,
            person.member.avatar.as_ref(),
            &app.avatars,
            PERSON_AVATAR,
        ),
        PERSON_AVATAR,
        person.speaking,
    );
    let mut content = row![
        picture,
        container(text(name).size(tokens::CAPTION).wrapping(Wrapping::None))
            .width(Fill)
            .clip(true),
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    if person.muted {
        content = content.push(
            text(Icon::Muted.glyph())
                .size(tokens::BODY)
                .wrapping(Wrapping::None)
                .style(iced::widget::text::danger),
        );
    }
    person_button(content.into(), person.room, density)
}

/// "and N more" under a voice room.
fn more_line<'a>(more: &More<'a>, density: Density) -> Element<'a, Message> {
    let content = text(more_label(more.hidden))
        .size(tokens::CAPTION)
        .wrapping(Wrapping::None)
        .style(kit::muted);
    person_button(content.into(), more.room, density)
}

/// A line under a voice room: indented, quiet, and a click selects the room.
fn person_button<'a>(
    content: Element<'a, Message>,
    room: &RoomId,
    density: Density,
) -> Element<'a, Message> {
    let height = match density {
        Density::Comfortable => sections::PERSON_HEIGHT,
        Density::Compact => sections::COMPACT_PERSON_HEIGHT,
    };
    button(content)
        .on_press(Message::SelectRoom(room.clone()))
        .width(Fill)
        .height(Length::Fixed(height))
        .padding(iced::Padding {
            top: 0.0,
            right: tokens::S,
            bottom: 0.0,
            left: PERSON_INDENT,
        })
        .clip(true)
        .style(kit::ghost_style)
        .into()
}

/// What a refused header says, besides its red ring.
pub const NOT_ALLOWED: &str = "Not allowed";

/// The 3 px bar at the left of a row: the accent colour while selected,
/// clear otherwise, so the row keeps its shape.
fn accent_bar<'a>(selected: bool) -> Element<'a, Message> {
    container(space().width(ACCENT_BAR).height(Fill))
        .style(move |t: &iced::Theme| container::Style {
            background: selected.then(|| Background::Color(t.palette().primary)),
            ..container::Style::default()
        })
        .into()
}

/// The style of a row's button: quiet, with the selection colour while the
/// room is open.
fn row_style(selected: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |t, status| {
        let base = kit::ghost_style(t, status);
        if selected {
            button::Style {
                background: Some(Background::Color(theme::semantic(t).selection)),
                ..base
            }
        } else {
            base
        }
    }
}

/// One fixed-height row: avatar, name, time, the lock, the preview (not in
/// Compact), the Voice, Invite and Muted tags, the call pill and the count.
/// `mode` is the room's notification mode as the app knows it; `call` the
/// room's call, if any.
fn room_row<'a>(
    room: &'a RoomSummary,
    env: &RowEnv<'_>,
    selected: bool,
    mode: Option<RoomNotificationModeView>,
    call: Option<CallPill>,
) -> Element<'a, Message> {
    room_row_in(room, env, selected, mode, call, Pointer::Button)
}

/// How a row takes the pointer.
#[derive(Debug, Clone, Copy)]
enum Pointer {
    /// A button: a click opens the room. The flat list, where nothing drags.
    Button,
    /// A plain box inside the sectioned list's `mouse_area`, which decides
    /// between a click and a drag. A button would swallow the press.
    Area {
        /// The pointer is over it.
        hovered: bool,
        /// It is being dragged.
        dragged: bool,
    },
}

/// [`room_row`] for the way the row takes the pointer.
fn room_row_in<'a>(
    room: &'a RoomSummary,
    env: &RowEnv<'_>,
    selected: bool,
    mode: Option<RoomNotificationModeView>,
    call: Option<CallPill>,
    pointer: Pointer,
) -> Element<'a, Message> {
    let comfortable = shows_preview(env.density);
    let name = row_name(&room.name);
    let avatar = avatar_view(
        room,
        &name,
        env.avatars,
        if comfortable {
            AVATAR_SLOT
        } else {
            COMPACT_AVATAR_SLOT
        },
    );
    // Text that does not wrap is drawn past its box, so the box clips it:
    // the end of a long name never lands under the lock, the time, the tags
    // or the scrollbar.
    let mut title = row![
        container(text(name).size(tokens::BODY).wrapping(Wrapping::None))
            .width(Fill)
            .clip(true)
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    if let Some(lock) = encrypted_mark(room) {
        title = title.push(text(lock).size(tokens::CAPTION));
    }
    let mut marks = row![].spacing(tokens::XS).align_y(iced::Center);
    if room.is_call_room {
        marks = marks.push(kit::tag(VOICE_TAG));
    }
    if room.is_invite {
        marks = marks.push(kit::tag("Invite"));
    }
    if let Some(mark) = muted_mark(mode) {
        marks = marks.push(kit::tag(mark));
    }
    marks = marks
        .push(call_pill(call))
        .push(badge_view(badge_for(room), muted_mark(mode).is_some()));
    let body: Element<'a, Message> = if comfortable {
        if let Some(last) = &room.last_message {
            title = title.push(
                text(row_time(last.timestamp_ms, env.now_ms, &Local))
                    .size(tokens::CAPTION)
                    .wrapping(Wrapping::None)
                    .style(kit::muted),
            );
        }
        let preview = preview_line(room).unwrap_or_default();
        column![
            title,
            row![
                container(
                    text(preview)
                        .size(tokens::CAPTION)
                        .wrapping(Wrapping::None)
                        .style(kit::muted)
                )
                .width(Fill)
                .clip(true),
                marks
            ]
            .spacing(tokens::S)
            .align_y(iced::Center)
        ]
        .spacing(2)
        .width(Fill)
        .into()
    } else {
        title.push(marks).into()
    };
    let content = row![avatar, body].spacing(tokens::S).align_y(iced::Center);
    let height = row_height(env.density);
    let body: Element<'a, Message> = match pointer {
        Pointer::Button => button(content)
            .on_press(Message::SelectRoom(room.id.clone()))
            .width(Fill)
            .height(Length::Fixed(height))
            .padding([tokens::XS, tokens::S])
            .clip(true)
            .style(row_style(selected))
            .into(),
        Pointer::Area { hovered, dragged } => container(content)
            .width(Fill)
            .height(Length::Fixed(height))
            .padding([tokens::XS, tokens::S])
            .clip(true)
            .style(row_box_style(selected, hovered || dragged))
            .into(),
    };
    row![accent_bar(selected), body]
        .height(Length::Fixed(height))
        .into()
}

/// The look of a row that is a box, not a button: the selection colour while
/// the room is open, the hover colour under the pointer (as [`row_style`]).
fn row_box_style(selected: bool, hovered: bool) -> impl Fn(&iced::Theme) -> container::Style {
    move |t| {
        let sem = theme::semantic(t);
        let fill = if selected {
            Some(sem.selection)
        } else if hovered {
            Some(sem.message_hover)
        } else {
            None
        };
        container::Style {
            text_color: Some(t.palette().text),
            background: fill.map(Background::Color),
            border: border::rounded(tokens::SMALL),
            ..container::Style::default()
        }
    }
}

/// One row of the rail: the avatar with an unread dot and, in a call, a
/// speaker dot, and a tooltip with the room's name and what the dots say.
fn rail_row<'a>(
    room: &'a RoomSummary,
    avatars: &Avatars,
    selected: bool,
    mode: Option<RoomNotificationModeView>,
    call: Option<CallPill>,
) -> Element<'a, Message> {
    let name = row_name(&room.name);
    let badge = badge_for(room);
    let marks = rail_marks(room);
    let words = rail_tip(&name, badge, call, muted_mark(mode), &marks);
    let avatar = avatar_view(room, &name, avatars, AVATAR_SLOT);
    let corner = |dot: Element<'a, Message>, top: bool| {
        let layer = container(dot).width(Fill).height(Fill).align_right(Fill);
        if top {
            layer.align_top(Fill)
        } else {
            layer.align_bottom(Fill)
        }
    };
    let picture = stack![
        avatar,
        corner(unread_dot(badge, muted_mark(mode).is_some()), true),
        corner(speaker_dot(call), false),
        rail_mark_layer(&marks),
    ];
    let height = Length::Fixed(RAIL_ROW_HEIGHT);
    let entry = button(container(picture).center(Fill))
        .on_press(Message::SelectRoom(room.id.clone()))
        .width(Fill)
        .height(height)
        .padding(tokens::XS)
        .style(row_style(selected));
    row![
        accent_bar(selected),
        Tooltip::new(entry, text(words).size(tokens::CAPTION), Position::Right)
            .gap(tokens::XS)
            .padding(tokens::S)
            .style(kit::card_style),
    ]
    .height(height)
    .into()
}

/// The unread dot on a rail avatar: the highlight colour for mentions, the
/// badge colour for other unread messages, an outline for a muted room, and
/// nothing when all is read.
fn unread_dot<'a>(badge: Badge, muted: bool) -> Element<'a, Message> {
    let size = 12.0;
    match badge {
        Badge::None => space().width(0).into(),
        badge => container(space().width(size).height(size))
            .style(move |t: &iced::Theme| {
                let sem = theme::semantic(t);
                let (fill, ring) = match badge {
                    Badge::Highlight(_) => (Some(sem.highlight), t.palette().background),
                    _ if muted => (None, sem.muted),
                    _ => (Some(sem.badge), t.palette().background),
                };
                container::Style {
                    background: fill.map(Background::Color),
                    border: border::rounded(size / 2.0).color(ring).width(2),
                    ..container::Style::default()
                }
            })
            .into(),
    }
}

/// Side of a mark or call chip on a rail avatar. Two chips along the bottom
/// left and the call chip at the bottom right must fit the 40 px avatar with
/// room between them: 12 + 1 + 12 and 12 leave 3 px.
const CHIP: f32 = 12.0;

/// The marks of a rail row on the avatar: Invite at the top left, the voice
/// and lock marks along the bottom left.
fn rail_mark_layer<'a>(marks: &[RailMark]) -> Element<'a, Message> {
    let chip = |mark: RailMark| -> Element<'a, Message> {
        let invite = mark == RailMark::Invite;
        container(
            text(mark.icon().glyph())
                .size(8)
                .wrapping(Wrapping::None)
                .center(),
        )
        .width(CHIP)
        .height(CHIP)
        .center(CHIP)
        .style(move |t: &iced::Theme| {
            let sem = theme::semantic(t);
            let (fill, on) = if invite {
                (sem.highlight, sem.highlight_text)
            } else {
                (sem.surface_raised, t.palette().text)
            };
            container::Style {
                background: Some(Background::Color(fill)),
                text_color: Some(on),
                border: border::rounded(CHIP / 2.0).color(sem.muted).width(1),
                ..container::Style::default()
            }
        })
        .into()
    };
    let top: Element<'a, Message> = if marks.contains(&RailMark::Invite) {
        chip(RailMark::Invite)
    } else {
        space().width(0).into()
    };
    let bottom = marks
        .iter()
        .filter(|m| **m != RailMark::Invite)
        .fold(row![].spacing(1), |r, m| r.push(chip(*m)));
    column![
        container(top).width(Fill).align_left(Fill),
        space().height(Fill),
        container(bottom).width(Fill).align_left(Fill),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

/// The speaker dot on a rail avatar while the room has a call: the highlight
/// colour while it rings, the badge colour otherwise.
fn speaker_dot<'a>(call: Option<CallPill>) -> Element<'a, Message> {
    let Some(call) = call else {
        return space().width(0).into();
    };
    container(
        text(Icon::Speaker.glyph())
            .size(8)
            .wrapping(Wrapping::None)
            .center(),
    )
    .width(CHIP)
    .height(CHIP)
    .center(CHIP)
    .style(move |t: &iced::Theme| {
        let sem = theme::semantic(t);
        let (fill, on) = if call.ringing {
            (sem.highlight, sem.highlight_text)
        } else {
            (sem.badge, sem.badge_text)
        };
        container::Style {
            background: Some(Background::Color(fill)),
            text_color: Some(on),
            border: border::rounded(CHIP / 2.0),
            ..container::Style::default()
        }
    })
    .into()
}

/// The room's avatar, `size` wide: the decoded image once it arrived,
/// otherwise (and when it is unavailable) a round placeholder with the
/// initials of the already sanitised `name`.
fn avatar_view<'a>(
    room: &RoomSummary,
    name: &str,
    avatars: &Avatars,
    size: f32,
) -> Element<'a, Message> {
    let ready = room
        .avatar
        .as_ref()
        .and_then(|id| avatars.state(id))
        .and_then(|state| match state {
            AvatarState::Ready(handle) => Some(handle),
            AvatarState::Unavailable => None,
        });
    if let Some(handle) = ready {
        return image(handle.clone())
            .width(size)
            .height(size)
            .border_radius(size / 2.0)
            .into();
    }
    let label = initials(name);
    let fill = if label == NEUTRAL_INITIALS {
        None
    } else {
        Some(placeholder_color(&room.id))
    };
    container(
        text(label)
            .size(if size > COMPACT_AVATAR_SLOT {
                tokens::BODY_LARGE
            } else {
                tokens::CAPTION
            })
            .color(Color::WHITE),
    )
    .width(size)
    .height(size)
    .center_x(size)
    .center_y(size)
    .style(move |t| container::Style {
        background: Some(Background::Color(
            fill.unwrap_or_else(|| theme::semantic(t).muted),
        )),
        border: border::rounded(size / 2.0),
        ..container::Style::default()
    })
    .into()
}

/// The call pill: the speaker and how many devices are in the room's call,
/// in the highlight colour while the room rings. An empty space without a
/// call, so a row keeps its widget types.
pub(crate) fn call_pill<'a>(call: Option<CallPill>) -> Element<'a, Message> {
    match call {
        None => space().width(0).into(),
        Some(call) => kit::icon_tag(Icon::Speaker, call_label(call), call.ringing),
    }
}

/// The badge at the right of a row. A muted room's count is drawn as an
/// outline in the muted text colour instead of a filled badge; mentions keep
/// their highlight.
pub(crate) fn badge_view<'a>(badge: Badge, muted: bool) -> Element<'a, Message> {
    match badge {
        Badge::None => space().width(0).into(),
        Badge::Highlight(count) => pill(count_label(count), Tone::Highlight),
        Badge::Count(count) if muted => pill(count_label(count), Tone::Muted),
        Badge::Count(count) => kit::count_badge(count),
        Badge::Dot => container(space().width(10).height(10))
            .style(|t| container::Style {
                background: Some(Background::Color(theme::semantic(t).badge)),
                border: border::rounded(5.0),
                ..container::Style::default()
            })
            .into(),
    }
}

/// How a count badge is drawn when it is not the plain [`kit::count_badge`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    /// Mentions.
    Highlight,
    /// Unread notifications of a muted room.
    Muted,
}

/// A rounded badge with a count; the highlight colour for mentions, an
/// outline for a muted room.
fn pill<'a>(label: String, tone: Tone) -> Element<'a, Message> {
    container(text(label).size(tokens::CAPTION))
        .padding([1.0, 7.0])
        .style(move |t| {
            let colours = theme::semantic(t);
            match tone {
                Tone::Highlight => container::Style {
                    text_color: Some(colours.highlight_text),
                    background: Some(Background::Color(colours.highlight)),
                    border: border::rounded(tokens::ROUND),
                    ..container::Style::default()
                },
                Tone::Muted => container::Style {
                    text_color: Some(colours.muted),
                    border: border::rounded(tokens::ROUND).color(colours.muted).width(1),
                    ..container::Style::default()
                },
            }
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(avatars: &Avatars) -> RowEnv<'_> {
        RowEnv {
            avatars,
            density: Density::Comfortable,
            now_ms: 0,
        }
    }

    #[test]
    fn only_a_room_known_to_be_encrypted_has_a_lock() {
        let mut room = RoomSummary::new(twicecat_core::api::RoomId::from("!r:x"), "r".to_owned());
        assert_eq!(encrypted_mark(&room), None);
        room.is_encrypted = true;
        assert_eq!(encrypted_mark(&room), Some(LOCK_MARK));
    }

    #[test]
    fn rows_build_in_every_notification_mode() {
        let mut room = RoomSummary::new(twicecat_core::api::RoomId::from("!r:x"), "r".to_owned());
        room.unread_notifications = 3;
        let avatars = Avatars::default();
        for mode in [
            None,
            Some(RoomNotificationModeView::AllMessages),
            Some(RoomNotificationModeView::MentionsAndKeywords),
            Some(RoomNotificationModeView::Mute),
        ] {
            let _: Element<'_, Message> = room_row(&room, &env(&avatars), false, mode, None);
        }
        room.highlights = 1;
        let _: Element<'_, Message> = room_row(
            &room,
            &env(&avatars),
            true,
            Some(RoomNotificationModeView::Mute),
            None,
        );
    }

    #[test]
    fn rows_build_with_a_voice_tag_and_every_call_pill() {
        let mut room = RoomSummary::new(twicecat_core::api::RoomId::from("!r:x"), "r".to_owned());
        room.is_call_room = true;
        let avatars = Avatars::default();
        for call in [
            None,
            Some(CallPill {
                count: 2,
                ringing: false,
            }),
            Some(CallPill {
                count: 0,
                ringing: true,
            }),
        ] {
            let _: Element<'_, Message> = room_row(&room, &env(&avatars), false, None, call);
        }
    }

    #[test]
    fn the_speaking_ring_stands_out_from_every_row_background_in_every_theme() {
        // A person's line sits on the pane (the sidebar colour), takes the
        // hover colour under the pointer, and the selection colour is the
        // neighbouring room's: the ring (non-text, 3:1) must show on all.
        for t in theme::test_themes() {
            let sem = theme::semantic(&t);
            for (name, bg) in [
                ("sidebar", sem.sidebar),
                ("surface", sem.surface),
                ("surface-raised", sem.surface_raised),
                ("hover", sem.message_hover),
                ("selection", sem.selection),
            ] {
                let ratio = theme::contrast(sem.speaking, bg);
                assert!(ratio >= 3.0, "speaking ring on {name}: {ratio}");
            }
        }
    }

    #[test]
    fn the_more_line_says_how_many_are_not_shown() {
        assert_eq!(more_label(1), "and 1 more");
        assert_eq!(more_label(17), "and 17 more");
    }

    #[test]
    fn tab_labels() {
        let labels: Vec<_> = TABS.iter().map(|t| tab_label(*t)).collect();
        assert_eq!(labels, ["All", "DMs", "Rooms", "Spaces"]);
    }

    #[test]
    fn count_labels_cap_at_99() {
        assert_eq!(count_label(1), "1");
        assert_eq!(count_label(99), "99");
        assert_eq!(count_label(100), "99+");
        assert_eq!(count_label(u64::MAX), "99+");
    }

    #[test]
    fn avatar_views_build_for_every_state() {
        use twicecat_core::api::{AvatarId, AvatarImage, RoomId};

        let mut avatars = Avatars::default();
        let id = AvatarId::from("mxc://x/a");
        let _ = avatars.take_unrequested([id.clone()]);
        let mut room = RoomSummary::new(RoomId::from("!r:x"), String::new());
        room.avatar = Some(id.clone());
        // Requested, not answered: placeholder.
        let _: Element<'_, Message> = avatar_view(&room, "", &avatars, AVATAR_SLOT);
        // Unavailable: placeholder.
        avatars.answer(&id, None);
        let _: Element<'_, Message> = avatar_view(&room, "Room", &avatars, AVATAR_SLOT);
        // No avatar at all.
        room.avatar = None;
        let _: Element<'_, Message> = avatar_view(&room, "#x", &avatars, COMPACT_AVATAR_SLOT);
        // Ready: the image.
        let img = ::image::RgbaImage::from_pixel(2, 2, ::image::Rgba([1, 2, 3, 255]));
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, ::image::ImageFormat::Png)
            .expect("encode");
        let id2 = AvatarId::from("mxc://x/b");
        let _ = avatars.take_unrequested([id2.clone()]);
        avatars.answer(&id2, Some(AvatarImage::from(png.into_inner())));
        room.avatar = Some(id2);
        let _: Element<'_, Message> = avatar_view(&room, "Room", &avatars, AVATAR_SLOT);
    }

    #[test]
    fn the_pane_is_300_wide_and_the_rail_72() {
        use twicecat_core::api::RoomListChoice;
        assert_eq!(pane_width(RoomListChoice::Expanded), 300.0);
        assert_eq!(pane_width(RoomListChoice::Rail), 72.0);
    }

    #[test]
    fn only_comfortable_rows_show_a_preview() {
        assert!(shows_preview(Density::Comfortable));
        assert!(!shows_preview(Density::Compact));
    }

    #[test]
    fn the_last_message_time_is_the_clock_today_and_the_day_before() {
        use chrono::{FixedOffset, TimeZone};
        let tz = FixedOffset::east_opt(0).expect("offset");
        let at = |d, h, m| {
            u64::try_from(
                tz.with_ymd_and_hms(2026, 10, d, h, m, 0)
                    .single()
                    .expect("time")
                    .timestamp_millis(),
            )
            .expect("after 1970")
        };
        let now = at(14, 12, 0);
        assert_eq!(row_time(at(14, 9, 5), now, &tz), "09:05");
        assert_eq!(row_time(at(13, 9, 5), now, &tz), "Yesterday");
        assert_eq!(row_time(at(2, 9, 5), now, &tz), "2 October");
    }

    #[test]
    fn a_rail_tooltip_names_the_room_and_says_what_the_dots_mean() {
        let call = Some(CallPill {
            count: 2,
            ringing: false,
        });
        assert_eq!(rail_tip("Ops", Badge::None, None, None, &[]), "Ops");
        assert_eq!(
            rail_tip("Ops", Badge::Count(3), None, None, &[]),
            "Ops, 3 unread"
        );
        assert_eq!(
            rail_tip("Ops", Badge::Highlight(1), call, None, &[]),
            "Ops, 1 mention, in a call"
        );
        assert_eq!(rail_tip("Ops", Badge::Dot, None, None, &[]), "Ops, unread");
        assert_eq!(
            rail_tip(
                "Ops",
                Badge::Count(120),
                Some(CallPill {
                    count: 0,
                    ringing: true
                }),
                Some("Muted"),
                &[]
            ),
            "Ops, 99+ unread, ringing, Muted"
        );
    }

    #[test]
    fn a_rail_row_marks_invites_voice_rooms_and_encryption() {
        let mut room = RoomSummary::new(twicecat_core::api::RoomId::from("!r:x"), "r".to_owned());
        assert!(rail_marks(&room).is_empty());
        room.is_invite = true;
        room.is_call_room = true;
        room.is_encrypted = true;
        let marks = rail_marks(&room);
        assert_eq!(
            marks,
            [RailMark::Invite, RailMark::Voice, RailMark::Encrypted]
        );
        assert_eq!(
            rail_tip("Ops", Badge::None, None, Some("Muted"), &marks),
            "Ops, invite, voice room, encrypted, Muted"
        );
        let _: Element<'_, Message> = rail_mark_layer(&marks);
    }

    #[test]
    fn rows_and_rail_rows_build_in_every_density() {
        let mut room = RoomSummary::new(twicecat_core::api::RoomId::from("!r:x"), "r".to_owned());
        room.unread_notifications = 3;
        room.is_call_room = true;
        room.is_invite = true;
        room.is_encrypted = true;
        let avatars = Avatars::default();
        let call = Some(CallPill {
            count: 2,
            ringing: true,
        });
        for density in [Density::Comfortable, Density::Compact] {
            for selected in [false, true] {
                let _: Element<'_, Message> = room_row(
                    &room,
                    &RowEnv {
                        avatars: &avatars,
                        density,
                        now_ms: 0,
                    },
                    selected,
                    Some(RoomNotificationModeView::Mute),
                    call,
                );
            }
        }
        for selected in [false, true] {
            let _: Element<'_, Message> = rail_row(&room, &avatars, selected, None, call);
        }
    }
}
