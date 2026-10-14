//! The component kit: buttons, cards, banners and the other pieces the
//! screens are built from.
//!
//! Every style is a pure `fn(&Theme, Status) -> Style` (or `fn(&Theme) ->
//! Style`) over the palette and [`crate::theme::Semantic`], so the colours
//! follow the theme, and the tests check their contrast in both palettes.

use iced::widget::button::{Button, Status};
use iced::widget::pick_list;
use iced::widget::text::{IntoFragment, Wrapping};
use iced::widget::tooltip::Position;
use iced::widget::{
    Column, button, column, container, image, opaque, progress_bar, row, rule, scrollable, space,
    text, tooltip,
};
use iced::{Background, Border, Color, Element, Fill, Theme, border};
use twicecat_core::api::{AvatarId, RoomId, UserId};

use crate::avatars::{AvatarState, Avatars, NEUTRAL_INITIALS, initials, placeholder_color};
use crate::theme;
use crate::ui::icons::{self, Icon};
use crate::ui::tokens;

/// Size of a person's picture in the card and the member list.
pub const PERSON_SLOT: f32 = 36.0;
/// Width of the ring around a person who is talking.
pub const SPEAKING_RING: f32 = 2.0;

/// How serious a [`banner`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Something to know.
    Info,
    /// Something went well.
    Success,
    /// Something needs care.
    Warning,
    /// Something failed.
    Danger,
}

impl Severity {
    /// The icon at the start of the banner.
    pub const fn icon(self) -> Icon {
        match self {
            Self::Info => Icon::Dot,
            Self::Success => Icon::Check,
            Self::Warning => Icon::Warning,
            Self::Danger => Icon::Close,
        }
    }

    /// The colour that stands for the severity: a banner's outline, a status dot.
    pub fn color(self, theme: &Theme) -> Color {
        let palette = theme.palette();
        match self {
            Self::Info => palette.primary,
            Self::Success => palette.success,
            Self::Warning => palette.warning,
            Self::Danger => palette.danger,
        }
    }
}

// ---- colours -------------------------------------------------------------

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

/// Black or white, whichever reads better on `background` (at least 4.5:1
/// for any opaque colour).
fn on_color(background: Color) -> Color {
    if theme::contrast(Color::BLACK, background) >= theme::contrast(Color::WHITE, background) {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

fn shape() -> Border {
    border::rounded(tokens::MEDIUM)
}

fn filled(background: Color, text_color: Color, border: Border) -> button::Style {
    button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border,
        ..button::Style::default()
    }
}

/// A solid button of `base`; the hover and press states mix toward its text.
fn solid(base: Color, status: Status, disabled: (Color, Color)) -> button::Style {
    let background = match status {
        Status::Active => base,
        Status::Hovered => mix(base, on_color(base), 0.12),
        Status::Pressed => mix(base, on_color(base), 0.22),
        Status::Disabled => return filled(disabled.0, disabled.1, shape()),
    };
    filled(background, on_color(background), shape())
}

// ---- button styles -------------------------------------------------------

/// The main action of a screen.
pub fn primary_style(theme: &Theme, status: Status) -> button::Style {
    let sem = theme::semantic(theme);
    solid(
        theme.palette().primary,
        status,
        (sem.surface_raised, sem.muted),
    )
}

/// A normal action.
pub fn secondary_style(theme: &Theme, status: Status) -> button::Style {
    let sem = theme::semantic(theme);
    let text_color = theme.palette().text;
    let outline = Border {
        color: sem.border,
        width: 1.0,
        ..shape()
    };
    match status {
        Status::Active => filled(sem.surface_raised, text_color, outline),
        Status::Hovered => filled(sem.selection, text_color, outline),
        Status::Pressed => filled(mix(sem.selection, text_color, 0.08), text_color, outline),
        Status::Disabled => filled(sem.surface, sem.muted, outline),
    }
}

/// A quiet action with no background until it is pointed at.
pub fn ghost_style(theme: &Theme, status: Status) -> button::Style {
    let sem = theme::semantic(theme);
    let text_color = theme.palette().text;
    match status {
        Status::Active => button::Style {
            text_color,
            border: shape(),
            ..button::Style::default()
        },
        Status::Hovered => filled(sem.message_hover, text_color, shape()),
        Status::Pressed => filled(sem.selection, text_color, shape()),
        Status::Disabled => button::Style {
            text_color: sem.muted,
            border: shape(),
            ..button::Style::default()
        },
    }
}

/// An action that cannot be undone.
pub fn danger_style(theme: &Theme, status: Status) -> button::Style {
    let sem = theme::semantic(theme);
    solid(
        theme.palette().danger,
        status,
        (sem.surface_raised, sem.muted),
    )
}

/// A button that holds one icon.
pub fn icon_style(theme: &Theme, status: Status) -> button::Style {
    ghost_style(theme, status)
}

/// A button that holds one icon whose action cannot be undone: the glyph is
/// in the danger colour (nudged until it reads at 7:1 on what is behind it), the
/// rest is the quiet icon style.
pub fn danger_icon_style(theme: &Theme, status: Status) -> button::Style {
    let mut style = ghost_style(theme, status);
    if status != Status::Disabled {
        let behind = match style.background {
            Some(Background::Color(c)) => c,
            _ => theme::semantic(theme).surface_raised,
        };
        style.text_color = theme::readable_at(theme.palette().danger, behind, 7.0);
    }
    style
}

/// A toggle that is on: the selection colour and the focus colour outline.
pub fn active_icon_style(theme: &Theme, status: Status) -> button::Style {
    let sem = theme::semantic(theme);
    let text_color = theme.palette().text;
    let outline = Border {
        color: sem.focus,
        width: 1.0,
        ..shape()
    };
    match status {
        Status::Active => filled(sem.selection, text_color, outline),
        Status::Hovered => filled(mix(sem.selection, text_color, 0.06), text_color, outline),
        Status::Pressed => filled(mix(sem.selection, text_color, 0.12), text_color, outline),
        Status::Disabled => filled(sem.surface, sem.muted, outline),
    }
}

/// A toggle that is live: no fill, an outline in the success colour (a
/// microphone that is open). It differs from [`active_icon_style`] (the
/// selection fill) and from the quiet [`icon_style`].
pub fn open_icon_style(theme: &Theme, status: Status) -> button::Style {
    let sem = theme::semantic(theme);
    let text_color = theme.palette().text;
    let outline = Border {
        color: Severity::Success.color(theme),
        width: 2.0,
        ..shape()
    };
    match status {
        Status::Active => button::Style {
            text_color,
            border: outline,
            ..button::Style::default()
        },
        Status::Hovered => filled(sem.message_hover, text_color, outline),
        Status::Pressed => filled(sem.selection, text_color, outline),
        Status::Disabled => button::Style {
            text_color: sem.muted,
            border: Border {
                color: sem.border,
                ..outline
            },
            ..button::Style::default()
        },
    }
}

// ---- buttons -------------------------------------------------------------

/// A button's words stay on one line: a squeezed header clips them instead
/// of wrapping, which would cut the second line off.
fn labelled<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    on_press: Option<M>,
    style: fn(&Theme, Status) -> button::Style,
) -> Button<'a, M> {
    button(text(label).wrapping(Wrapping::None))
        .padding([tokens::S, tokens::M])
        .on_press_maybe(on_press)
        .style(style)
}

/// The main action of a screen; disabled when `on_press` is `None`.
pub fn primary_button<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    on_press: Option<M>,
) -> Button<'a, M> {
    labelled(label, on_press, primary_style)
}

/// A normal action; disabled when `on_press` is `None`.
pub fn secondary_button<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    on_press: Option<M>,
) -> Button<'a, M> {
    labelled(label, on_press, secondary_style)
}

/// A quiet action; disabled when `on_press` is `None`.
pub fn ghost_button<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    on_press: Option<M>,
) -> Button<'a, M> {
    labelled(label, on_press, ghost_style)
}

/// An action that cannot be undone; disabled when `on_press` is `None`.
pub fn danger_button<'a, M: Clone + 'a>(
    label: impl IntoFragment<'a>,
    on_press: Option<M>,
) -> Button<'a, M> {
    labelled(label, on_press, danger_style)
}

/// `content` with `label` in a card that shows when it is pointed at.
pub fn tip<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    label: impl IntoFragment<'a>,
) -> Element<'a, M> {
    tooltip(content, text(label).size(tokens::CAPTION), Position::Bottom)
        .gap(tokens::XS)
        .padding(tokens::S)
        .style(card_style)
        .into()
}

/// Like [`icon_button`], but small: a caption-size glyph in a 4 px frame, for
/// a bar that floats over a line of text and must not be taller than it.
pub fn compact_icon_button<'a, M: Clone + 'a>(icon: Icon, on_press: M) -> Element<'a, M> {
    compact(icon, on_press, icon_style)
}

/// [`compact_icon_button`] for an action that cannot be undone: the glyph is
/// in the danger colour (the words stay in the tooltip).
pub fn compact_danger_icon_button<'a, M: Clone + 'a>(icon: Icon, on_press: M) -> Element<'a, M> {
    compact(icon, on_press, danger_icon_style)
}

fn compact<'a, M: Clone + 'a>(
    icon: Icon,
    on_press: M,
    style: fn(&Theme, Status) -> button::Style,
) -> Element<'a, M> {
    let button = button(text(icon.glyph()).size(tokens::CAPTION))
        .padding(tokens::XS)
        .on_press(on_press)
        .style(style);
    tip(button, icon.label())
}

/// A button that holds one icon. It always has a tooltip with the icon's
/// words, so no action is a bare symbol.
pub fn icon_button<'a, M: Clone + 'a>(icon: Icon, on_press: M) -> Element<'a, M> {
    icon_button_maybe(icon, Some(on_press))
}

/// [`icon_button`] that is disabled (and takes no press) when `on_press` is
/// `None`. The widget tree is the same either way.
pub fn icon_button_maybe<'a, M: Clone + 'a>(icon: Icon, on_press: Option<M>) -> Element<'a, M> {
    let button = button(icons::icon(icon))
        .padding(tokens::S)
        .on_press_maybe(on_press)
        .style(icon_style);
    tip(button, icon.label())
}

/// The send button: an icon on the accent colour, disabled when `on_press`
/// is `None`.
pub fn send_icon_button<'a, M: Clone + 'a>(on_press: Option<M>) -> Element<'a, M> {
    let button = button(icons::icon(Icon::Send))
        .padding(tokens::S)
        .on_press_maybe(on_press)
        .style(primary_style);
    tip(button, Icon::Send.label())
}

/// An icon button with an on and an off state, such as mute or deafen. On
/// shows by colour (selection fill and outline) and by glyph (a check after
/// the icon), so it does not rely on colour alone. Its widget tree is the
/// same in both states.
pub fn toggle_icon_button<'a, M: Clone + 'a>(
    icon: Icon,
    active: bool,
    on_press: M,
) -> Element<'a, M> {
    toggle_icon_button_labelled(icon, active, icon.label(), on_press)
}

/// [`toggle_icon_button`] with its own tooltip words, for a toggle whose
/// action reads differently from its icon ("Unmute" on a muted microphone).
pub fn toggle_icon_button_labelled<'a, M: Clone + 'a>(
    icon: Icon,
    active: bool,
    words: impl IntoFragment<'a>,
    on_press: M,
) -> Element<'a, M> {
    // The check is always there, drawn clear while the toggle is off: the
    // widget tree and the button's width do not change with the state.
    let mark = container(icons::icon(Icon::Check)).style(move |_: &Theme| container::Style {
        text_color: (!active).then_some(Color::TRANSPARENT),
        ..container::Style::default()
    });
    let content: Element<'a, M> = row![icons::icon(icon), mark].spacing(tokens::XS).into();
    let button = button(content)
        .padding(tokens::S)
        .on_press(on_press)
        .style(if active {
            active_icon_style
        } else {
            icon_style
        });
    tip(button, words)
}

/// A drop-down that Ctrl + wheel (the zoom gesture) cannot change. Every
/// `pick_list` in the app goes through this: iced's closed pick list takes
/// the wheel as "select the next option", which would swap a device or turn
/// push-to-talk into an open microphone while the user only zooms. A test
/// scans the sources for a `pick_list(` that is not wrapped.
pub fn menu<'a, M: 'a>(list: impl Into<Element<'a, M>>) -> Element<'a, M> {
    crate::ui::guard::no_wheel(list)
}

/// A drop-down's closed box: the raised colour with the normal text colour
/// for the choice and for a placeholder that names the account.
pub fn menu_style(theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let sem = theme::semantic(theme);
    let text_color = theme.palette().text;
    let hovered = matches!(
        status,
        pick_list::Status::Hovered | pick_list::Status::Opened { .. }
    );
    pick_list::Style {
        text_color,
        placeholder_color: text_color,
        handle_color: text_color,
        background: Background::Color(if hovered {
            sem.selection
        } else {
            sem.surface_raised
        }),
        border: Border {
            color: sem.border,
            width: 1.0,
            ..shape()
        },
    }
}

// ---- text ----------------------------------------------------------------

/// Secondary text, such as hints and timestamps.
pub fn muted(theme: &Theme) -> text::Style {
    text::Style {
        color: Some(theme::semantic(theme).muted),
    }
}

// ---- containers ----------------------------------------------------------

/// A raised card: dialogs, menus and tooltips.
pub fn card_style(theme: &Theme) -> container::Style {
    let sem = theme::semantic(theme);
    container::Style {
        background: Some(Background::Color(sem.surface_raised)),
        border: Border {
            color: sem.border,
            width: 1.0,
            ..border::rounded(tokens::LARGE)
        },
        ..container::Style::default()
    }
}

fn panel_style(theme: &Theme) -> container::Style {
    let sem = theme::semantic(theme);
    container::Style {
        background: Some(Background::Color(sem.surface)),
        border: Border {
            color: sem.border,
            width: 1.0,
            ..border::rounded(tokens::MEDIUM)
        },
        ..container::Style::default()
    }
}

/// The spaces sidebar's background.
pub fn sidebar_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::semantic(theme).sidebar)),
        ..container::Style::default()
    }
}

/// The background of a side panel, such as the thread panel.
pub fn surface_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::semantic(theme).surface)),
        ..container::Style::default()
    }
}

fn backdrop_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),
        ..container::Style::default()
    }
}

fn tag_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::semantic(theme).selection)),
        text_color: Some(theme.palette().text),
        border: border::rounded(tokens::SMALL),
        ..container::Style::default()
    }
}

/// The style of a [`severity_tag`]: the severity's colour for the text (nudged
/// toward black or white where the palette's colour is under 4.5:1) and a
/// 1px outline, on the raised surface (the soft red for danger).
pub fn severity_tag_style(severity: Severity) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let sem = theme::semantic(theme);
        let background = match severity {
            Severity::Danger => sem.danger_soft,
            _ => sem.surface_raised,
        };
        let color = severity.color(theme);
        container::Style {
            background: Some(Background::Color(background)),
            text_color: Some(theme::readable_on(color, background)),
            border: Border {
                color,
                width: 1.0,
                radius: border::rounded(tokens::SMALL).radius,
            },
            ..container::Style::default()
        }
    }
}

fn badge_style(theme: &Theme) -> container::Style {
    let sem = theme::semantic(theme);
    container::Style {
        background: Some(Background::Color(sem.badge)),
        text_color: Some(sem.badge_text),
        border: border::rounded(tokens::ROUND),
        ..container::Style::default()
    }
}

/// Width of the bar at the start of a quote, a reply and a code block.
pub const QUOTE_BAR_WIDTH: f32 = 3.0;

/// The accent bar of a quote: a non-text mark in the accent colour.
pub fn quote_bar_style(theme: &Theme) -> rule::Style {
    rule::Style {
        color: theme.palette().primary,
        radius: border::rounded(tokens::SMALL).radius,
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

/// The bar at the start of a quote or a reply, [`QUOTE_BAR_WIDTH`] wide.
pub fn quote_bar<'a, M: 'a>() -> Element<'a, M> {
    rule::vertical(QUOTE_BAR_WIDTH)
        .style(quote_bar_style)
        .into()
}

/// The surface of a quote, a reply and a code block: the panel colour, with
/// small rounded corners.
pub fn quote_surface_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(theme::semantic(theme).surface)),
        border: border::rounded(tokens::SMALL),
        ..container::Style::default()
    }
}

/// The style of a reaction chip: a pill on the raised surface, outlined in
/// the accent when `own` (the count includes your reaction). The count is in
/// the chip's words either way, so the outline is not the only sign.
pub fn chip_style(own: bool) -> impl Fn(&Theme, Status) -> button::Style {
    move |theme, status| {
        let sem = theme::semantic(theme);
        let text_color = theme.palette().text;
        let (fill, outline) = if own {
            (sem.selection, theme.palette().primary)
        } else {
            (sem.surface_raised, sem.border)
        };
        let background = match status {
            Status::Hovered => mix(fill, text_color, 0.08),
            Status::Pressed => mix(fill, text_color, 0.16),
            Status::Active | Status::Disabled => fill,
        };
        filled(
            background,
            text_color,
            Border {
                color: outline,
                width: 1.0,
                radius: border::rounded(tokens::ROUND).radius,
            },
        )
    }
}

/// The style of a [`banner`] of `severity`.
pub fn banner_style(severity: Severity) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let sem = theme::semantic(theme);
        let background = match severity {
            Severity::Danger => sem.danger_soft,
            _ => sem.surface_raised,
        };
        container::Style {
            background: Some(Background::Color(background)),
            text_color: Some(theme.palette().text),
            border: Border {
                color: severity.color(theme),
                width: 1.0,
                ..shape()
            },
            ..container::Style::default()
        }
    }
}

/// A panel on the surface colour.
pub fn card<'a, M: 'a>(content: impl Into<Element<'a, M>>) -> Element<'a, M> {
    container(content)
        .padding(tokens::L)
        .style(panel_style)
        .into()
}

/// A card in the middle of a dimmed window; it takes every click.
pub fn modal<'a, M: 'a>(content: impl Into<Element<'a, M>>, width: f32) -> Element<'a, M> {
    opaque(
        container(
            container(content)
                .padding(20)
                .max_width(width)
                .style(card_style),
        )
        .center(Fill)
        .style(backdrop_style),
    )
}

/// What a dialog's header, footer and spacing take from the height of its
/// card (the card's own padding included), for [`dialog`]: the room left for
/// the body is the window's height less this.
pub const DIALOG_CHROME: f32 = 260.0;
/// The least height a dialog's body keeps, in a very small window.
const DIALOG_MIN_BODY: f32 = 120.0;

/// Text that breaks inside a word when it has no other break, so the end of
/// a long name or ID is never drawn past its box (and under a button).
pub fn words<'a>(label: impl IntoFragment<'a>) -> text::Text<'a> {
    text(label).wrapping(BANNER_WRAPPING)
}

/// The top line of a dialog: its title, then a close button with the words
/// "Close" in its tooltip. The title wraps rather than runs under the button.
pub fn dialog_header<'a, M: Clone + 'a>(
    title: impl IntoFragment<'a>,
    on_close: M,
) -> Element<'a, M> {
    row![
        words(title).size(tokens::HEADLINE).width(Fill),
        icon_button(Icon::Close, on_close)
    ]
    .spacing(tokens::M)
    .align_y(iced::Center)
    .into()
}

/// The bottom line of a dialog: free room at the left, then `actions` with
/// the main one last. The gap between two actions is [`tokens::M`], so a
/// destructive action is never pressed by a slip at its neighbour.
pub fn dialog_footer<'a, M: 'a>(actions: Vec<Element<'a, M>>) -> Element<'a, M> {
    row![space().width(Fill)]
        .extend(actions)
        .spacing(tokens::M)
        .align_y(iced::Center)
        .into()
}

/// A dialog: `header`, a `body` that scrolls when the window is too short for
/// it, and a `footer` that stays in view. The scrollbar takes room of its own,
/// so a button at the body's right edge is never under it. `window_height`
/// is what the window leaves for the card; the footer slot is always there
/// (empty without a footer), so the widgets keep their places.
pub fn dialog<'a, M: 'a>(
    header: Element<'a, M>,
    body: Element<'a, M>,
    footer: Option<Element<'a, M>>,
    width: f32,
    window_height: f32,
) -> Element<'a, M> {
    let room = (window_height - DIALOG_CHROME).max(DIALOG_MIN_BODY);
    let body = container(scrollable(body).spacing(tokens::S).width(Fill)).max_height(room);
    let footer = footer.unwrap_or_else(|| space().into());
    modal(column![header, body, footer].spacing(tokens::L), width)
}

/// A card near the top of the window, such as the room switcher; it takes
/// every click, like [`modal`].
pub fn palette<'a, M: 'a>(content: impl Into<Element<'a, M>>, width: f32) -> Element<'a, M> {
    opaque(
        container(
            container(content)
                .padding(tokens::L)
                .max_width(width)
                .style(card_style),
        )
        .center_x(Fill)
        .height(Fill)
        .padding(iced::Padding {
            top: PALETTE_TOP,
            ..iced::Padding::ZERO
        })
        .style(backdrop_style),
    )
}

/// How far from the top of the window a [`palette`] starts.
pub const PALETTE_TOP: f32 = 72.0;

/// A titled group of controls.
pub fn section<'a, M: 'a>(title: &'a str, body: impl Into<Element<'a, M>>) -> Element<'a, M> {
    column![text(title).size(tokens::TITLE), body.into()]
        .spacing(tokens::S)
        .into()
}

/// A short label in a small rounded box, such as the Voice tag.
pub fn tag<'a, M: 'a>(label: impl IntoFragment<'a>) -> Element<'a, M> {
    container(text(label).size(tokens::CAPTION))
        .padding([1.0, tokens::S])
        .style(tag_style)
        .into()
}

/// The style of an [`icon_tag`]: the tag's, or the highlight colours.
pub fn icon_tag_style(highlight: bool) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        if highlight {
            let sem = theme::semantic(theme);
            container::Style {
                background: Some(Background::Color(sem.highlight)),
                text_color: Some(sem.highlight_text),
                border: border::rounded(tokens::SMALL),
                ..container::Style::default()
            }
        } else {
            tag_style(theme)
        }
    }
}

/// The style of an [`outline_tag`]: no fill, a 1px outline.
fn outline_tag_style(theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: theme::semantic(theme).border,
            width: 1.0,
            ..border::rounded(tokens::SMALL)
        },
        ..container::Style::default()
    }
}

/// A [`tag`] that is muted: no fill and muted words, for what is there but
/// not in use (the other graphics backends that were found).
pub fn outline_tag<'a, M: 'a>(label: impl IntoFragment<'a>) -> Element<'a, M> {
    container(
        text(label)
            .size(tokens::CAPTION)
            .style(muted)
            .wrapping(BANNER_WRAPPING),
    )
    .padding([1.0, tokens::S])
    .style(outline_tag_style)
    .into()
}

/// The style of a [`focus_surface`]: the selection fill with the focus colour
/// as a 2px outline.
pub fn focus_surface_style(theme: &Theme) -> container::Style {
    let sem = theme::semantic(theme);
    container::Style {
        background: Some(Background::Color(sem.selection)),
        text_color: Some(theme.palette().text),
        border: Border {
            color: sem.focus,
            width: 2.0,
            ..shape()
        },
        ..container::Style::default()
    }
}

/// The same box as a [`focus_surface`] while nothing waits: same padding and
/// outline width, so the box does not change size when it starts waiting.
fn idle_surface_style(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: Color::TRANSPARENT,
            width: 2.0,
            ..shape()
        },
        ..container::Style::default()
    }
}

/// A box that says "this is waiting for you", such as the key capture: the
/// focus surface while `waiting`, else the same box without the fill and the
/// outline, so that it keeps its size.
pub fn focus_surface<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    waiting: bool,
) -> Element<'a, M> {
    container(content)
        .padding(tokens::M)
        .width(Fill)
        .style(if waiting {
            focus_surface_style
        } else {
            idle_surface_style
        })
        .into()
}

/// The style of a [`meter_bar`] in the colour of `zone`.
pub fn meter_bar_style(zone: Severity) -> impl Fn(&Theme) -> progress_bar::Style {
    move |theme| {
        let sem = theme::semantic(theme);
        progress_bar::Style {
            background: Background::Color(sem.surface),
            bar: Background::Color(zone.color(theme)),
            border: Border {
                color: sem.border,
                width: 1.0,
                ..border::rounded(tokens::SMALL)
            },
        }
    }
}

/// A level meter: `fill` from 0 to 1, in the colour of `zone`. The words
/// beside it say the same.
pub fn meter_bar<'a, M: 'a>(fill: f32, zone: Severity) -> Element<'a, M> {
    progress_bar(0.0..=1.0, fill.clamp(0.0, 1.0))
        .girth(tokens::M)
        .style(meter_bar_style(zone))
        .into()
}

/// A [`tag`] that starts with an icon, such as the call pill. With
/// `highlight` it is drawn in the highlight colours.
pub fn icon_tag<'a, M: 'a>(icon: Icon, label: String, highlight: bool) -> Element<'a, M> {
    container(
        row![
            text(icon.glyph()).size(tokens::CAPTION),
            text(label).size(tokens::CAPTION)
        ]
        .spacing(tokens::XS)
        .align_y(iced::Center),
    )
    .padding([1.0, tokens::S])
    .style(icon_tag_style(highlight))
    .into()
}

/// A [`tag`] whose colour says how serious it is, for the trust of a person.
/// The words say the same, so the colour is never the only sign.
pub fn severity_tag<'a, M: 'a>(severity: Severity, label: &'a str) -> Element<'a, M> {
    container(text(label).size(tokens::CAPTION))
        .padding([1.0, tokens::S])
        .style(severity_tag_style(severity))
        .into()
}

/// The text of a count badge: the number, or 99+ above 99.
pub fn badge_text(count: u64) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// A count in a round badge.
pub fn count_badge<'a, M: 'a>(count: u64) -> Element<'a, M> {
    container(text(badge_text(count)).size(tokens::CAPTION))
        .padding([1.0, 6.0])
        .style(badge_style)
        .into()
}

/// How banner text wraps: a glyph break when a word (a long host) has no
/// other, so no character is drawn past the bar. Shared with the layout
/// tests, which measure with it.
pub(crate) const BANNER_WRAPPING: Wrapping = Wrapping::WordOrGlyph;

/// A message bar: an icon for its severity, then the text.
pub fn banner<'a, M: 'a>(severity: Severity, message: impl IntoFragment<'a>) -> Element<'a, M> {
    container(
        row![
            icons::icon(severity.icon()),
            text(message).width(Fill).wrapping(BANNER_WRAPPING)
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
    )
    .padding(tokens::M)
    .width(Fill)
    .style(banner_style(severity))
    .into()
}

/// A [`banner`] with buttons at its end, for a bar that asks something (open
/// this link, send this as a message). The words and the severity colour are
/// the same as a plain banner's.
pub fn banner_with<'a, M: 'a>(
    severity: Severity,
    message: impl IntoFragment<'a>,
    actions: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    // The words get the whole width, with the buttons on a row of their own
    // below: beside the text the buttons would take their width first, and
    // text drawn past its bounds (a long host has no break point for
    // `Wrapping::Word`) would run under them, hiding the end of a host. A
    // glyph break keeps every character inside the bounds.
    let words = row![
        icons::icon(severity.icon()),
        text(message).width(Fill).wrapping(BANNER_WRAPPING)
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    let buttons = row(actions).spacing(tokens::S);
    container(
        column![words, buttons]
            .spacing(tokens::S)
            .align_x(iced::Right),
    )
    .padding(tokens::M)
    .width(Fill)
    .style(banner_style(severity))
    .into()
}

/// What a list shows when it has nothing to list.
pub fn empty_state<'a, M: 'a>(title: &'a str, hint: &'a str) -> Element<'a, M> {
    container(
        column![
            text(title).size(tokens::BODY_LARGE),
            text(hint).size(tokens::CAPTION).style(muted)
        ]
        .spacing(tokens::XS)
        .align_x(iced::Center),
    )
    .padding(tokens::XL)
    .center_x(Fill)
    .into()
}

/// A form field: its label above the input.
pub fn field<'a, M: 'a>(label: &'a str, input: impl Into<Element<'a, M>>) -> Element<'a, M> {
    column![text(label).size(tokens::CAPTION).style(muted), input.into()]
        .spacing(tokens::XS)
        .into()
}

/// A thin line across the available width.
pub fn divider<'a, M: 'a>() -> Element<'a, M> {
    rule::horizontal(1)
        .style(|theme: &Theme| rule::Style {
            color: theme::semantic(theme).border,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

/// A thin line down the available height, between two panes.
pub fn vertical_divider<'a, M: 'a>() -> Element<'a, M> {
    rule::vertical(1)
        .style(|theme: &Theme| rule::Style {
            color: theme::semantic(theme).border,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

/// A round picture or the initials of `name`, [`PERSON_SLOT`] wide.
pub fn avatar<'a, M: 'a>(
    user: &UserId,
    name: &str,
    avatar: Option<&AvatarId>,
    avatars: &Avatars,
) -> Element<'a, M> {
    avatar_sized(user, name, avatar, avatars, PERSON_SLOT)
}

/// [`avatar`] at `size` pixels, such as the large picture of the ring card.
pub fn avatar_sized<'a, M: 'a>(
    user: &UserId,
    name: &str,
    avatar: Option<&AvatarId>,
    avatars: &Avatars,
    size: f32,
) -> Element<'a, M> {
    let ready = avatar
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
    let fill = (label != NEUTRAL_INITIALS).then(|| placeholder_color(&RoomId::from(user.as_str())));
    container(
        text(label)
            .size(14.0 * size / PERSON_SLOT)
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

/// A ring around `content` (a round [`avatar`]) while the person talks. The
/// ring is drawn transparent when they do not, so the layout never moves.
pub fn speaking_ring<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    speaking: bool,
) -> Element<'a, M> {
    container(content)
        .padding(SPEAKING_RING)
        .style(move |theme: &Theme| container::Style {
            border: border::rounded(PERSON_SLOT / 2.0 + SPEAKING_RING)
                .width(SPEAKING_RING)
                .color(if speaking {
                    theme::semantic(theme).speaking
                } else {
                    Color::TRANSPARENT
                }),
            ..container::Style::default()
        })
        .into()
}

/// The list at the left of a paged card: the current page is selected.
pub fn page_nav<'a, M: Clone + 'a>(
    pages: &[&'a str],
    current: usize,
    on_select: impl Fn(usize) -> M,
) -> Element<'a, M> {
    let mut list = Column::new().spacing(tokens::XS);
    for (index, page) in pages.iter().enumerate() {
        let style = if index == current {
            secondary_style
        } else {
            ghost_style
        };
        list = list.push(
            button(text(*page))
                .width(Fill)
                .padding([tokens::S, tokens::M])
                .on_press(on_select(index))
                .style(style),
        );
    }
    list.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{contrast, theme_for};
    use iced::theme::Mode;
    use iced::widget::button::Status;
    use iced::{Background, Color, Theme};

    const STATUSES: [Status; 4] = [
        Status::Active,
        Status::Hovered,
        Status::Pressed,
        Status::Disabled,
    ];

    const SEVERITIES: [Severity; 4] = [
        Severity::Info,
        Severity::Success,
        Severity::Warning,
        Severity::Danger,
    ];

    fn themes() -> [(&'static str, Theme); 2] {
        [
            ("light", theme_for(Mode::Light)),
            ("dark", theme_for(Mode::Dark)),
        ]
    }

    /// The colour a style's text sits on: its own background, or the window's.
    fn behind(style: &button::Style, theme: &Theme) -> Color {
        match style.background {
            Some(Background::Color(c)) => c,
            _ => theme.palette().background,
        }
    }

    #[test]
    fn the_danger_icon_glyph_is_readable_in_all_eight_palettes_and_every_status() {
        for family in theme::Family::ALL {
            for dark in [false, true] {
                let theme = theme::theme_for_family(family, dark);
                let card = theme::semantic(&theme).surface_raised;
                for status in STATUSES {
                    let s = danger_icon_style(&theme, status);
                    let back = if status == Status::Active {
                        card
                    } else {
                        behind(&s, &theme)
                    };
                    let need = match (status, family == theme::Family::HighContrast) {
                        (Status::Disabled, _) => 3.0,
                        (_, true) => 7.0,
                        _ => 4.5,
                    };
                    let ratio = contrast(s.text_color, back);
                    assert!(
                        ratio >= need,
                        "{family:?} dark={dark} {status:?}: {ratio} < {need}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_button_style_is_readable_in_both_palettes_and_every_status() {
        type StyleFn = fn(&Theme, Status) -> button::Style;
        let styles: [(&str, StyleFn); 6] = [
            ("primary", primary_style),
            ("secondary", secondary_style),
            ("ghost", ghost_style),
            ("danger", danger_style),
            ("icon", icon_style),
            ("active icon", active_icon_style),
        ];
        for (theme_name, theme) in themes() {
            for (name, style) in styles {
                for status in STATUSES {
                    let s = style(&theme, status);
                    let ratio = contrast(s.text_color, behind(&s, &theme));
                    let need = if status == Status::Disabled { 3.0 } else { 4.5 };
                    assert!(
                        ratio >= need,
                        "{theme_name} {name} {status:?}: {ratio} < {need}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_focus_surface_and_the_outline_tag_are_readable_in_all_eight_palettes() {
        for family in theme::Family::ALL {
            for dark in [false, true] {
                let theme = theme::theme_for_family(family, dark);
                let sem = theme::semantic(&theme);
                let need = if family == theme::Family::HighContrast {
                    7.0
                } else {
                    4.5
                };
                let style = focus_surface_style(&theme);
                let Some(Background::Color(back)) = style.background else {
                    panic!("a fill");
                };
                let text = style.text_color.expect("a text colour");
                assert!(
                    contrast(text, back) >= need,
                    "{family:?} dark={dark}: text on the focus surface"
                );
                // The outline is a non-text mark (WCAG 1.4.11).
                assert!(
                    contrast(style.border.color, back) >= 3.0
                        && contrast(style.border.color, sem.surface_raised) >= 3.0,
                    "{family:?} dark={dark}: the focus outline"
                );
                assert!(
                    contrast(sem.muted, sem.surface_raised) >= need,
                    "{family:?} dark={dark}: muted words on the card"
                );
            }
        }
    }

    #[test]
    fn the_meter_zones_are_visible_on_the_card_in_all_eight_palettes() {
        for family in theme::Family::ALL {
            for dark in [false, true] {
                let theme = theme::theme_for_family(family, dark);
                let sem = theme::semantic(&theme);
                for zone in [Severity::Success, Severity::Warning, Severity::Danger] {
                    let style = meter_bar_style(zone)(&theme);
                    let Background::Color(bar) = style.bar else {
                        panic!("a colour");
                    };
                    let Background::Color(track) = style.background else {
                        panic!("a colour");
                    };
                    // A non-text mark: the bar against its track (WCAG 1.4.11).
                    assert!(
                        contrast(bar, track) >= 3.0,
                        "{family:?} dark={dark} {zone:?}: bar on track {}",
                        contrast(bar, track)
                    );
                    assert!(
                        contrast(style.border.color, sem.surface_raised) >= 1.2,
                        "{family:?} dark={dark}: the track has an outline"
                    );
                }
            }
        }
    }

    #[test]
    fn the_modal_card_is_raised_and_round() {
        for (name, theme) in themes() {
            let sem = crate::theme::semantic(&theme);
            let style = card_style(&theme);
            assert_eq!(
                style.background,
                Some(Background::Color(sem.surface_raised)),
                "{name}"
            );
            assert_eq!(
                style.border.radius,
                iced::border::Radius::from(tokens::LARGE),
                "{name}"
            );
        }
    }

    fn all_themes() -> Vec<(String, Theme)> {
        let mut out = Vec::new();
        for family in crate::theme::Family::ALL {
            for dark in [false, true] {
                let name = format!("{} {}", family.label(), if dark { "dark" } else { "light" });
                out.push((name, crate::theme::theme_for_family(family, dark)));
            }
        }
        assert_eq!(out.len(), 8);
        out
    }

    #[test]
    fn an_icon_tag_is_readable_in_all_eight_palettes_and_highlights_with_the_highlight() {
        for (name, theme) in all_themes() {
            for highlight in [false, true] {
                let style = icon_tag_style(highlight)(&theme);
                let Some(Background::Color(bg)) = style.background else {
                    panic!("{name}: no background");
                };
                let text = style.text_color.expect("the tag sets its text colour");
                assert!(contrast(text, bg) >= 4.5, "{name} highlight={highlight}");
                let sem = crate::theme::semantic(&theme);
                assert_eq!(bg == sem.highlight, highlight, "{name}");
            }
        }
        let _: Element<'_, ()> = icon_tag(Icon::Speaker, "2".to_owned(), true);
    }

    #[test]
    fn a_severity_tag_is_readable_and_in_its_severity_colour_in_all_eight_palettes() {
        for (name, theme) in all_themes() {
            for severity in SEVERITIES {
                let style = severity_tag_style(severity)(&theme);
                let Some(Background::Color(bg)) = style.background else {
                    panic!("{name} {severity:?}: no background");
                };
                let text = style.text_color.expect("the tag sets its text colour");
                assert!(
                    contrast(text, bg) >= 4.5,
                    "{name} {severity:?}: text {} < 4.5",
                    contrast(text, bg)
                );
                assert_eq!(style.border.width, 1.0, "{name} {severity:?}");
                assert!(
                    contrast(style.border.color, bg) >= 3.0,
                    "{name} {severity:?}: border"
                );
            }
            let danger = severity_tag_style(Severity::Danger)(&theme);
            assert_eq!(
                danger.text_color,
                Some(crate::theme::readable_on(
                    theme.palette().danger,
                    crate::theme::semantic(&theme).danger_soft
                )),
                "{name}"
            );
            assert_eq!(danger.border.color, theme.palette().danger, "{name}");
        }
    }

    #[test]
    fn the_severity_colours_differ_so_trust_states_do_not_look_alike() {
        for (name, theme) in all_themes() {
            let styles: Vec<_> = [Severity::Success, Severity::Info, Severity::Danger]
                .into_iter()
                .map(|s| {
                    let st = severity_tag_style(s)(&theme);
                    (st.text_color, st.border.color)
                })
                .collect();
            for (i, a) in styles.iter().enumerate() {
                for b in &styles[i + 1..] {
                    assert_ne!(a, b, "{name}");
                }
            }
        }
    }

    #[test]
    fn severities_have_distinct_marks_and_the_banner_text_is_readable() {
        let marks: std::collections::HashSet<_> =
            SEVERITIES.into_iter().map(|s| s.icon()).collect();
        assert_eq!(marks.len(), 4);
        for (name, theme) in themes() {
            for severity in SEVERITIES {
                let style = banner_style(severity)(&theme);
                let Some(Background::Color(bg)) = style.background else {
                    panic!("{name} {severity:?}: no background");
                };
                assert!(
                    contrast(theme.palette().text, bg) >= 4.5,
                    "{name} {severity:?}"
                );
            }
        }
    }

    #[test]
    fn badges_read_as_numbers() {
        assert_eq!(badge_text(0), "0");
        assert_eq!(badge_text(7), "7");
        assert_eq!(badge_text(99), "99");
        assert_eq!(badge_text(100), "99+");
    }

    #[test]
    fn the_widgets_build() {
        #[derive(Clone)]
        struct Msg;
        let built: Vec<Element<'_, Msg>> = vec![
            primary_button("Save", Some(Msg)).into(),
            secondary_button("Cancel", None).into(),
            ghost_button("More", Some(Msg)).into(),
            danger_button("Delete", Some(Msg)).into(),
            icon_button(Icon::Close, Msg),
            toggle_icon_button(Icon::Mic, true, Msg),
            toggle_icon_button(Icon::Mic, false, Msg),
            card("body"),
            modal("body", 400.0),
            section("Title", "body"),
            tag("Voice"),
            count_badge(120),
            banner(Severity::Warning, "Careful"),
            empty_state("Nothing here", "Try again"),
            field("Name", "input"),
            divider(),
            speaking_ring(iced::widget::text("a"), true),
            page_nav(&["One", "Two"], 1, |_| Msg),
            vertical_divider(),
            tag("Voice"),
            primary_button(String::from("Join call (3)"), Some(Msg)).into(),
        ];
        assert_eq!(built.len(), 21);
    }

    #[test]
    fn the_menu_box_is_readable_in_every_state() {
        for (name, theme) in themes() {
            for status in [
                pick_list::Status::Active,
                pick_list::Status::Hovered,
                pick_list::Status::Opened { is_hovered: false },
            ] {
                let style = menu_style(&theme, status);
                let Background::Color(bg) = style.background else {
                    panic!("{name}: no background");
                };
                assert!(contrast(style.text_color, bg) >= 4.5, "{name} {status:?}");
                assert!(
                    contrast(style.placeholder_color, bg) >= 4.5,
                    "{name} {status:?}"
                );
            }
        }
    }

    #[test]
    fn the_pane_backgrounds_are_the_sidebar_and_surface_colours() {
        for (name, theme) in themes() {
            let sem = crate::theme::semantic(&theme);
            assert_eq!(
                sidebar_style(&theme).background,
                Some(Background::Color(sem.sidebar)),
                "{name}"
            );
            assert_eq!(
                surface_style(&theme).background,
                Some(Background::Color(sem.surface)),
                "{name}"
            );
            for style in [sidebar_style(&theme), surface_style(&theme)] {
                let Some(Background::Color(bg)) = style.background else {
                    panic!("{name}: no background");
                };
                assert!(contrast(theme.palette().text, bg) >= 4.5, "{name}");
                assert!(contrast(sem.muted, bg) >= 4.5, "{name} muted");
            }
        }
    }

    #[test]
    fn the_person_slot_is_the_old_one() {
        assert_eq!(PERSON_SLOT, 36.0);
    }

    #[test]
    fn a_reaction_chip_is_outlined_in_the_accent_only_when_it_is_yours() {
        for (name, theme) in themes() {
            for status in STATUSES {
                let own = chip_style(true)(&theme, status);
                let other = chip_style(false)(&theme, status);
                assert_eq!(own.border.color, theme.palette().primary, "{name}");
                assert!(own.border.width >= 1.0, "{name}: an outline");
                assert_ne!(other.border.color, theme.palette().primary, "{name}");
                assert_eq!(own.border.radius, border::rounded(tokens::ROUND).radius);
                for style in [own, other] {
                    let ratio = contrast(style.text_color, behind(&style, &theme));
                    assert!(ratio >= 4.5, "{name} {status:?}: {ratio:.2}");
                }
            }
        }
    }

    #[test]
    fn quotes_sit_on_the_surface_behind_a_three_pixel_accent_bar() {
        for (name, theme) in themes() {
            let sem = theme::semantic(&theme);
            let style = quote_surface_style(&theme);
            assert_eq!(
                style.background,
                Some(Background::Color(sem.surface)),
                "{name}"
            );
            // Text on it is the palette's, which every theme keeps readable
            // on the surface.
            assert!(contrast(theme.palette().text, sem.surface) >= 4.5, "{name}");
            assert!(
                contrast(sem.muted, sem.surface) >= 4.5,
                "{name}: muted text"
            );
            assert_eq!(quote_bar_style(&theme).color, theme.palette().primary);
        }
        assert_eq!(QUOTE_BAR_WIDTH, 3.0);
    }
}
