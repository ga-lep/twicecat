//! The call's video on screen (Phase 9 Tasks 10 and 11): the stage above the
//! timeline of the call's room, the full-screen view, the share picker and
//! the bar that says you are sharing.
//!
//! The stage is a slot of the room column: an empty space while there is no
//! picture to show, so the timeline and the composer keep their place in the
//! widget tree when a call gains or loses video. Pictures are drawn by
//! [`Surface`], never rebuilt per frame; what a tile writes about its
//! person, and the buttons on it, are plain widgets over it.

use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, button, checkbox, column, container, image, mouse_area, pick_list, responsive,
    row, scrollable, space, stack, text,
};
use iced::{Background, Border, Color, Element, Fill, Length, Rectangle, Size, Theme};
use twicecat_core::api::{ScreenSourceKind, VideoKind};

use crate::screens::call::{DockLock, dock_call};
use crate::state::{App, Message};
use crate::theme;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::{self, Severity, avatar_sized, banner, modal, muted, tip};
use crate::ui::tokens;
use crate::ui::video_view::{Place, Surface};
use crate::video::{self, QualityOption, SourceCard, Tile, VideoMessage};

/// How the stage takes part in the room column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// Nothing to show.
    Nothing,
    /// The stage is folded: one line with a button to open it.
    Folded,
    /// The pictures.
    Open,
}

/// Which of the three the room column has now.
pub fn slot_state(app: &App) -> Slot {
    let Some(call) = app.calls.call.as_ref() else {
        return Slot::Nothing;
    };
    let video = &app.calls.video;
    if app.selected.as_ref() != Some(&call.room_id)
        || app.pane_layout().panel_fills
        || !video.has_pictures(call)
    {
        Slot::Nothing
    } else if video.stage_open {
        Slot::Open
    } else {
        Slot::Folded
    }
}

/// Height share of the stage against the timeline when both show.
pub const STAGE_PORTION: u16 = 5;
/// Height share of the timeline while the stage is open.
pub const TIMELINE_PORTION: u16 = 4;
/// The least height of the stage's pictures.
const STAGE_MIN_HEIGHT: f32 = 180.0;
/// The frame of a picture in the share picker.
const THUMB_WIDTH: f32 = crate::video::THUMBNAIL_FRAME.0;
const THUMB_HEIGHT: f32 = crate::video::THUMBNAIL_FRAME.1;
/// A tile narrower than this has no buttons on it.
const BUTTONS_MIN_WIDTH: f32 = 150.0;

fn msg(message: VideoMessage) -> Message {
    Message::Video(message)
}

fn words<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(label).wrapping(Wrapping::WordOrGlyph)
}

/// The slot of the room column: the stage, the folded line or nothing.
pub fn slot(app: &App) -> Element<'_, Message> {
    match slot_state(app) {
        Slot::Nothing => space().height(0).into(),
        Slot::Folded => container(folded_bar(app)).width(Fill).into(),
        Slot::Open => stage(app),
    }
}

/// The camera button: on or off, in words and by mark.
pub fn camera_button(app: &App) -> Element<'_, Message> {
    let video = &app.calls.video;
    let on = video.camera_on();
    let press = (!video.camera_busy).then(|| msg(VideoMessage::ToggleCamera));
    let words = if on {
        "Turn the camera off"
    } else {
        "Turn the camera on"
    };
    toggle(Icon::Camera, on, words, press)
}

/// The share button: stops the share, or opens the picker.
pub fn share_button(app: &App) -> Element<'_, Message> {
    let video = &app.calls.video;
    let on = video.sharing();
    let press =
        (!video.share_busy && video.picker.is_none()).then(|| msg(VideoMessage::ToggleShare));
    let words = if on {
        "Stop sharing your screen"
    } else {
        "Share your screen"
    };
    toggle(Icon::ScreenShare, on, words, press)
}

/// A toggle that is off (takes no press) while `press` is `None`.
fn toggle<'a>(
    icon: Icon,
    on: bool,
    words: &'static str,
    press: Option<Message>,
) -> Element<'a, Message> {
    match press {
        Some(press) => kit::toggle_icon_button_labelled(icon, on, words, press),
        None => {
            // The same widget with no press: a pressed-and-waiting button.
            let mark = if on { Icon::Check.glyph() } else { "" };
            tip(
                button(row![crate::ui::icons::icon(icon), text(mark)].spacing(tokens::XS))
                    .padding(tokens::S)
                    .style(kit::icon_style),
                words,
            )
        }
    }
}

fn folded_bar(app: &App) -> Element<'_, Message> {
    container(
        row![
            icon::<Message>(Icon::Camera),
            words("Video is hidden").width(Fill),
            camera_button(app),
            share_button(app),
            kit::secondary_button("Show video", Some(msg(VideoMessage::ToggleStage))),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
    )
    .padding(tokens::S)
    .style(kit::surface_style)
    .into()
}

/// The stage: a line of controls above the tiles.
fn stage(app: &App) -> Element<'_, Message> {
    let mut header = Row::new().spacing(tokens::S).align_y(iced::Center);
    header = header
        .push(icon::<Message>(Icon::Camera))
        .push(words(stage_title(app)).size(tokens::BODY).width(Fill));
    if let Some(lock) = dock_call(app).and_then(|d| d.lock) {
        header = header.push(tip(
            text(lock.icon().glyph()).style(lock_text(lock)),
            lock.words(),
        ));
    }
    header = header
        .push(camera_button(app))
        .push(share_button(app))
        .push(kit::secondary_button(
            "Hide",
            Some(msg(VideoMessage::ToggleStage)),
        ));
    let mut top = Column::new().spacing(tokens::XS).push(header);
    if renderer_missing(app) {
        top = top.push(banner(
            Severity::Warning,
            "This window is not using the GPU renderer, so it cannot draw video. Start \
             Twicecat without ICED_BACKEND=tiny-skia, or on a machine with a working graphics \
             driver.",
        ));
    }
    let tiles = responsive(move |size| tiles_view(app, size));
    column![top, container(tiles).height(Fill).width(Fill)]
        .spacing(tokens::XS)
        .height(Length::FillPortion(STAGE_PORTION))
        .width(Fill)
        .into()
}

/// The title of the stage: who is in it, in words.
pub fn stage_title(app: &App) -> String {
    let Some(call) = app.calls.call.as_ref() else {
        return String::new();
    };
    let video = &app.calls.video;
    let pictures = video
        .tracks
        .iter()
        .filter(|t| t.kind == VideoKind::Camera && !t.muted)
        .count();
    let shares = video
        .tracks
        .iter()
        .filter(|t| t.kind == VideoKind::Screen && !t.is_local)
        .count();
    let people = call.participants.len();
    match (pictures, shares) {
        (0, 0) => format!("{people} in the call"),
        (c, 0) => format!("{c} of {people} with the camera on"),
        (0, s) => format!("{s} screen {}", if s == 1 { "share" } else { "shares" }),
        (c, s) => format!(
            "{c} with the camera on, {s} screen {}",
            if s == 1 { "share" } else { "shares" }
        ),
    }
}

/// Frames reach the app but no wgpu renderer ever drew one: iced is on its
/// software renderer, which cannot draw a shader widget.
fn renderer_missing(app: &App) -> bool {
    let video = &app.calls.video;
    video.stats.is_some() && !video.gpu.get() && video.feeds.values().any(|f| f.latest().is_some())
}

fn lock_text(lock: DockLock) -> impl Fn(&Theme) -> text::Style {
    move |t| text::Style {
        color: Some(match lock.severity() {
            Severity::Success => theme::semantic(t).muted,
            _ => t.palette().danger,
        }),
    }
}

/// The tiles in an area of `size`.
fn tiles_view(app: &App, size: Size) -> Element<'_, Message> {
    let Some(call) = app.calls.call.as_ref() else {
        return space().into();
    };
    let video = &app.calls.video;
    let tiles = video::tiles(video, call);
    let focus = video::focus(video, &tiles);
    let size = Size::new(
        size.width,
        size.height.max(STAGE_MIN_HEIGHT.min(size.height)),
    );
    let rects = video::arrange(size, tiles.len(), focus);
    let placed = tiles
        .into_iter()
        .zip(rects)
        .enumerate()
        .map(|(i, (tile, rect))| {
            let big = focus == Some(i);
            container(tile_view(app, &tile, rect, big))
                .padding(video::place(rect))
                .width(Fill)
                .height(Fill)
                .into()
        });
    stack(placed).width(Fill).height(Fill).into()
}

fn tile_style(speaking: bool, video: bool) -> impl Fn(&Theme) -> container::Style {
    move |t| {
        let sem = theme::semantic(t);
        container::Style {
            background: Some(Background::Color(if video {
                Color::BLACK
            } else {
                sem.surface_raised
            })),
            border: Border {
                color: if speaking { sem.speaking } else { sem.border },
                width: if speaking { 3.0 } else { 1.0 },
                radius: tokens::MEDIUM.into(),
            },
            ..container::Style::default()
        }
    }
}

/// What is written on a tile: the name, the marks, and for a share its kind.
fn tile_label(tile: &Tile) -> String {
    let mut label = tile.name.clone();
    if tile.is_local {
        label.push_str(" (you)");
    }
    if tile.kind == Some(VideoKind::Screen) {
        label.push_str(" \u{b7} screen");
    }
    label
}

/// One tile: the picture (or the avatar), the name, and the buttons.
fn tile_view<'a>(app: &'a App, tile: &Tile, rect: Rectangle, big: bool) -> Element<'a, Message> {
    let video = &app.calls.video;
    let has_video = tile.draws_video(video);
    let base: Element<'a, Message> = match (&tile.track, has_video) {
        (Some(track), true) => {
            let feed = video.feeds[track].clone();
            let id = track.clone();
            Surface::new(
                track,
                Place::Tile,
                feed,
                app.scale_factor() * app.window_scale,
                video.gpu.clone(),
            )
            .mirrored(tile.is_local && tile.kind == Some(VideoKind::Camera))
            .on_size(move |size| {
                msg(VideoMessage::TileSized {
                    track: id.clone(),
                    place: Place::Tile,
                    width: size.width,
                    height: size.height,
                })
            })
            .into()
        }
        _ => {
            let side = (rect.height * 0.4).clamp(32.0, 120.0);
            container(avatar_sized(
                &tile.user_id,
                &tile.name,
                tile.avatar.as_ref(),
                &app.avatars,
                side,
            ))
            .center(Fill)
            .into()
        }
    };
    let mut label = Row::new().spacing(tokens::XS).align_y(iced::Center);
    if tile.mic_off {
        label = label.push(text(Icon::MicOff.glyph()).size(tokens::CAPTION));
    }
    label = label.push(
        text(tile_label(tile))
            .size(tokens::CAPTION)
            .wrapping(Wrapping::None),
    );
    if tile.camera_off {
        label = label.push(text("Camera off").size(tokens::CAPTION));
    }
    let name = container(
        container(label)
            .padding([2.0, tokens::S])
            .style(overlay_style),
    )
    .align_x(iced::Left)
    .align_y(iced::Bottom)
    .padding(tokens::S)
    .width(Fill)
    .height(Fill);
    let mut layers: Vec<Element<'a, Message>> = vec![base, name.into()];
    if let (Some(track), true) = (&tile.track, has_video)
        && rect.width >= BUTTONS_MIN_WIDTH
    {
        let pinned = video.pinned.as_ref() == Some(track) || (big && video.pinned.is_none());
        let buttons = row![
            overlay_button(
                Icon::Pin,
                if video.pinned.as_ref() == Some(track) {
                    "Unpin"
                } else {
                    "Pin to the big place"
                },
                pinned,
                msg(VideoMessage::Pin(track.clone())),
            ),
            overlay_button(
                Icon::Fullscreen,
                "Full screen",
                false,
                msg(VideoMessage::Fullscreen(track.clone())),
            ),
        ]
        .spacing(tokens::XS);
        layers.push(
            container(buttons)
                .align_x(iced::Right)
                .align_y(iced::Top)
                .padding(tokens::S)
                .width(Fill)
                .height(Fill)
                .into(),
        );
    }
    let tile_box = container(stack(layers))
        .width(Length::Fixed(rect.width))
        .height(Length::Fixed(rect.height))
        .clip(true)
        .style(tile_style(tile.speaking, has_video));
    match (&tile.track, has_video) {
        // A click pins or unpins the picture (also where it is too small for
        // buttons), a double click opens it full screen.
        (Some(track), true) => mouse_area(tile_box)
            .on_press(msg(VideoMessage::Pin(track.clone())))
            .on_double_click(msg(VideoMessage::Fullscreen(track.clone())))
            .into(),
        _ => tile_box.into(),
    }
}

fn overlay_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.62))),
        text_color: Some(Color::WHITE),
        border: iced::border::rounded(tokens::SMALL),
        ..container::Style::default()
    }
}

/// A small button over a picture: white on a dark pill, whatever the colours
/// under it.
fn overlay_button<'a>(
    glyph: Icon,
    words: &'static str,
    on: bool,
    press: Message,
) -> Element<'a, Message> {
    let content: Element<'a, Message> = if on {
        row![crate::ui::icons::icon(glyph), icon(Icon::Check)]
            .spacing(tokens::XS)
            .into()
    } else {
        crate::ui::icons::icon(glyph)
    };
    tip(
        button(content)
            .padding(tokens::XS)
            .on_press(press)
            .style(move |_: &Theme, status| {
                let alpha = match status {
                    button::Status::Hovered | button::Status::Pressed => 0.85,
                    _ => 0.62,
                };
                button::Style {
                    background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, alpha))),
                    text_color: Color::WHITE,
                    border: iced::border::rounded(tokens::SMALL),
                    ..button::Style::default()
                }
            }),
        words,
    )
}

/// The base with the full-screen view, the share picker and the sharing bar
/// on top when they are open: always a stack of three.
pub fn layers<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let fullscreen = fullscreen(app).unwrap_or_else(|| space().into());
    let picker = picker(app).unwrap_or_else(|| space().into());
    stack([base, fullscreen, picker]).into()
}

/// The bar that says you are sharing, over everything: a stack of two.
pub fn sharing_layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let bar = sharing_bar(app).unwrap_or_else(|| space().into());
    stack([base, bar]).into()
}

/// "Sharing your screen" with **Stop sharing**: always on top while a share
/// goes out, so it is never forgotten.
fn sharing_bar(app: &App) -> Option<Element<'_, Message>> {
    let video = &app.calls.video;
    if !video.sharing() {
        return None;
    }
    let words = if video.local.screen_audio {
        "Sharing your screen and its sound"
    } else {
        "Sharing your screen"
    };
    let pill = container(
        row![
            icon::<Message>(Icon::ScreenShare),
            text(words).size(tokens::BODY),
            kit::danger_button(
                "Stop sharing",
                (!video.share_busy).then(|| msg(VideoMessage::StopShare)),
            ),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
    )
    .padding([tokens::XS, tokens::M])
    .style(|t: &Theme| {
        let sem = theme::semantic(t);
        container::Style {
            background: Some(Background::Color(sem.surface_raised)),
            text_color: Some(t.palette().text),
            border: Border {
                color: t.palette().danger,
                width: 2.0,
                radius: tokens::ROUND.into(),
            },
            ..container::Style::default()
        }
    });
    Some(
        // At the bottom, in the status bar's empty middle: the room's name
        // and buttons at the top are never covered.
        container(pill)
            .center_x(Fill)
            .align_y(iced::Bottom)
            .padding(tokens::XS)
            .width(Fill)
            .height(Fill)
            .into(),
    )
}

/// The full-screen view of one track.
fn fullscreen(app: &App) -> Option<Element<'_, Message>> {
    let video = &app.calls.video;
    let call = app.calls.call.as_ref()?;
    let track = video.fullscreen.as_ref()?;
    let feed = video.feeds.get(track)?.clone();
    let tile = video::tiles(video, call)
        .into_iter()
        .find(|t| t.track.as_ref() == Some(track));
    let id = track.clone();
    let surface = Surface::new(
        track,
        Place::Fullscreen,
        feed,
        app.scale_factor() * app.window_scale,
        video.gpu.clone(),
    )
    .mirrored(
        tile.as_ref()
            .is_some_and(|t| t.is_local && t.kind == Some(VideoKind::Camera)),
    )
    .on_size(move |size| {
        msg(VideoMessage::TileSized {
            track: id.clone(),
            place: Place::Fullscreen,
            width: size.width,
            height: size.height,
        })
    });
    let title = tile.as_ref().map(tile_label).unwrap_or_default();
    let bar = container(
        row![
            words(title).size(tokens::BODY).width(Fill),
            kit::secondary_button("Exit full screen", Some(msg(VideoMessage::ExitFullscreen))),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
    )
    .padding([tokens::XS, tokens::M])
    .style(overlay_style)
    .width(Fill);
    Some(iced::widget::opaque(
        container(stack![
            Element::from(surface),
            container(bar)
                .align_y(iced::Top)
                .padding(tokens::S)
                .width(Fill)
                .height(Fill),
        ])
        .width(Fill)
        .height(Fill)
        .style(|_: &Theme| container::Style {
            background: Some(Background::Color(Color::BLACK)),
            ..container::Style::default()
        }),
    ))
}

/// The share picker: what to share, its quality and sound.
fn picker(app: &App) -> Option<Element<'_, Message>> {
    let video = &app.calls.video;
    let picker = video.picker.as_ref()?;
    let settings = video.settings_or_default();
    let body: Element<'_, Message> = match (&picker.sources, picker.system_picker) {
        (None, _) => kit::empty_state("Looking for screens\u{2026}", "This takes a moment."),
        (Some(_), true) => kit::empty_state(
            "Your system asks what to share",
            "After you press Share, a window from your desktop lets you pick a screen or a window.",
        ),
        (Some(cards), false) if cards.is_empty() => {
            kit::empty_state("Nothing to share", "No screen or window was found.")
        }
        (Some(cards), false) => scrollable(
            Row::with_children(
                cards
                    .iter()
                    .map(|card| source_card(card, picker.selected.as_ref() == Some(&card.id))),
            )
            .spacing(tokens::S)
            .width(Fill)
            .wrap()
            .vertical_spacing(tokens::S),
        )
        .width(Fill)
        .height(Length::Fixed(280.0))
        .spacing(tokens::S)
        .into(),
    };
    let ready = picker.system_picker || picker.selected.is_some();
    let options = column![
        row![
            words("Quality").width(Length::Fixed(80.0)),
            kit::menu(
                pick_list(
                    QualityOption::ALL,
                    Some(QualityOption(settings.screen_quality)),
                    |q: QualityOption| msg(VideoMessage::ShareQuality(q.0)),
                )
                .style(kit::menu_style)
                .width(Fill)
            ),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center),
        checkbox(picker.audio)
            .label("Share the sound of the screen too")
            .text_wrapping(Wrapping::WordOrGlyph)
            .on_toggle(|on| msg(VideoMessage::ShareAudio(on))),
    ]
    .spacing(tokens::S);
    let card = column![
        words("Share your screen").size(tokens::HEADLINE),
        body,
        options,
        row![
            space().width(Fill),
            kit::secondary_button("Cancel", Some(msg(VideoMessage::CancelShare))),
            kit::primary_button("Share", ready.then(|| msg(VideoMessage::StartShare))),
        ]
        .spacing(tokens::S),
    ]
    .spacing(tokens::L);
    Some(modal(card, 640.0))
}

fn source_card(card: &SourceCard, selected: bool) -> Element<'_, Message> {
    let picture: Element<'_, Message> = match &card.thumbnail {
        // The picture at the size that keeps its shape inside the frame.
        Some((handle, size)) => container(
            image(handle.clone())
                .width(Length::Fixed(size.width))
                .height(Length::Fixed(size.height)),
        )
        .center(Fill)
        .width(Length::Fixed(THUMB_WIDTH))
        .height(Length::Fixed(THUMB_HEIGHT))
        .into(),
        None => container(icon::<Message>(match card.kind {
            ScreenSourceKind::Monitor => Icon::Gpu,
            ScreenSourceKind::Window => Icon::ScreenShare,
        }))
        .center(Fill)
        .width(Length::Fixed(THUMB_WIDTH))
        .height(Length::Fixed(THUMB_HEIGHT))
        .into(),
    };
    let kind = match card.kind {
        ScreenSourceKind::Monitor => "Screen",
        ScreenSourceKind::Window => "Window",
    };
    let content = column![
        picture,
        text(card.title.clone())
            .size(tokens::CAPTION)
            .wrapping(Wrapping::WordOrGlyph),
        text(format!("{kind}, {} \u{d7} {}", card.width, card.height))
            .size(tokens::CAPTION)
            .style(muted),
    ]
    .spacing(tokens::XS)
    .width(Length::Fixed(THUMB_WIDTH));
    button(content)
        .padding(tokens::S)
        .on_press(msg(VideoMessage::PickSource(card.id.clone())))
        .style(move |theme: &Theme, status| {
            let mut style = if selected {
                kit::secondary_style(theme, status)
            } else {
                kit::ghost_style(theme, status)
            };
            style.border = Border {
                color: if selected {
                    theme.palette().primary
                } else {
                    theme::semantic(theme).border
                },
                width: if selected { 3.0 } else { 1.0 },
                radius: tokens::MEDIUM.into(),
            };
            style
        })
        .into()
}
