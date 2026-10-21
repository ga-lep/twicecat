//! The Video page of Settings: the camera and the picture quality in a few
//! plain choices, and, with **Show advanced settings** on, every knob the
//! core allows for the camera and for screen sharing, with what each one
//! does, what the encoders of this computer can do, and what is really
//! being sent.
//!
//! Every control sends a [`VideoSettingsMessage`]; the page only reads the
//! app's state.

use iced::widget::text::Wrapping;
use iced::widget::{Column, checkbox, column, container, pick_list, row, space, text, text_input};
use iced::{Element, Fill, Length, Theme};
use twicecat_core::api::{
    EncoderBackend, EncoderChoice, QualityLimitation, VideoCodec, VideoDirection, VideoEncoderInfo,
    VideoKind, VideoSettings, VideoTrackStats, VideoTuning,
};

use crate::state::{App, Message};
use crate::text_safety::one_line_capped;
use crate::theme;
use crate::ui::kit::{self, Severity, banner, muted};
use crate::ui::tokens;
use crate::ui::video_view::{Place, Surface};
use crate::video::{QualityOption, quality_words};
use crate::video_settings::{
    CameraChoice, CapOption, CodecOption, DegradationOption, Dim, EncoderOption, Field,
    FormatOption, GeneralCodecOption, LayersOption, ModeOption, Pick, PickOption, SvcOption,
    VideoSettingsMessage, camera_menu, get, text_of,
};

/// Width of a control's label.
const LABEL_WIDTH: f32 = 170.0;
/// How text wraps on this page.
const WRAP: Wrapping = Wrapping::WordOrGlyph;
/// Width and height of the camera preview.
const PREVIEW: (f32, f32) = (320.0, 180.0);
/// Most characters of a camera or GPU name written on the page.
const MAX_NAME: usize = 120;

fn msg(message: VideoSettingsMessage) -> Message {
    Message::VideoSettings(message)
}

fn words<'a>(label: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(label).wrapping(WRAP)
}

fn heading<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    words(label).size(tokens::TITLE).into()
}

fn subheading<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    words(label).size(tokens::BODY_LARGE).into()
}

fn note<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    words(label).size(tokens::CAPTION).style(muted).into()
}

fn labelled<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![
        words(label).width(Length::Fixed(LABEL_WIDTH)),
        container(control).width(Fill)
    ]
    .spacing(tokens::S)
    .align_y(iced::Center)
    .into()
}

/// The kind in a sentence.
const fn kind_name(kind: VideoKind) -> &'static str {
    match kind {
        VideoKind::Camera => "camera",
        VideoKind::Screen => "screen share",
    }
}

/// The page. `advanced` is the Settings switch.
pub fn page(app: &App) -> Element<'_, Message> {
    let advanced = app.ui_settings.advanced_settings;
    let video = &app.calls.video;
    let mut page = Column::new().spacing(tokens::M).width(Fill);
    page = page.push(heading("Video"));
    if let Some(notice) = &app.calls.notice {
        page = page.push(banner(
            Severity::Danger,
            one_line_capped(notice, MAX_NAME * 2),
        ));
    }
    let Some(settings) = &video.settings else {
        return page.push(note("Loading the video settings\u{2026}")).into();
    };
    if !video.adjusted.is_empty() {
        page = page.push(adjusted_banner(app));
    }
    page = page.push(camera_section(app, settings));
    page = page.push(screen_section(app, settings));
    page = page.push(encoding_section(app, settings));
    if advanced {
        page = page
            .push(kit::divider())
            .push(words("Advanced").size(tokens::HEADLINE))
            .push(note(
                "Every option the video can be tuned by. Left on Automatic, Twicecat \
                 chooses for you, and the choices above are all you need.",
            ))
            .push(tuning_section(app, settings, VideoKind::Camera))
            .push(tuning_section(app, settings, VideoKind::Screen))
            .push(network_section(app, settings))
            .push(status_section(app));
    }
    page.into()
}

/// What the core changed in the last settings it was given, in words.
fn adjusted_banner(app: &App) -> Element<'_, Message> {
    let lines: Vec<String> = app
        .calls
        .video
        .adjusted
        .iter()
        .map(|a| {
            one_line_capped(
                &format!("{}: {} was changed to {}", a.setting, a.asked, a.used),
                MAX_NAME,
            )
        })
        .collect();
    banner(
        Severity::Warning,
        format!(
            "Some values were outside what is allowed and were changed. {}",
            lines.join("; ")
        ),
    )
}

fn quality_menu<'a>(
    current: twicecat_core::api::VideoQuality,
    on_pick: fn(QualityOption) -> VideoSettingsMessage,
) -> Element<'a, Message> {
    kit::menu(
        pick_list(
            QualityOption::ALL,
            Some(QualityOption(current)),
            move |q: QualityOption| msg(on_pick(q)),
        )
        .style(kit::menu_style)
        .width(Fill),
    )
}

fn camera_section<'a>(app: &'a App, settings: &'a VideoSettings) -> Element<'a, Message> {
    let video = &app.calls.video;
    let cameras = video
        .devices
        .as_ref()
        .map_or(&[][..], |d| d.cameras.as_slice());
    let (menu, chosen) = camera_menu(cameras, settings.camera.as_ref());
    let picker: Element<'a, Message> = if video.devices.is_none() {
        note("Looking for cameras\u{2026}")
    } else if cameras.is_empty() {
        note("No camera was found. Plug one in and it appears here.")
    } else {
        kit::menu(
            pick_list(menu, Some(chosen), |c: CameraChoice| {
                msg(VideoSettingsMessage::Camera(c.id))
            })
            .style(kit::menu_style)
            .width(Fill),
        )
    };
    let on = video.ui.preview;
    let picture: Element<'a, Message> = match (&video.preview, on) {
        (Some(feed), true) => Surface::new(
            &twicecat_core::api::VideoTrackId::from("preview"),
            Place::Preview,
            feed.clone(),
            app.scale_factor() * app.window_scale,
            video.gpu.clone(),
        )
        .mirrored(true)
        .into(),
        (None, true) => container(note("Opening the camera\u{2026}"))
            .center(Fill)
            .into(),
        _ => container(note(
            "The camera is off. Nothing is sent or shown until you ask.",
        ))
        .center(Fill)
        .into(),
    };
    let preview = container(picture)
        .width(Length::Fixed(PREVIEW.0))
        .height(Length::Fixed(PREVIEW.1))
        .style(preview_style);
    let button = if on {
        kit::secondary_button(
            "Hide my camera",
            Some(msg(VideoSettingsMessage::Preview(false))),
        )
    } else {
        kit::secondary_button(
            "Show my camera",
            Some(msg(VideoSettingsMessage::Preview(true))),
        )
    };
    kit::card(
        column![
            subheading("Camera"),
            labelled("Which camera", picker),
            preview,
            button,
            labelled(
                "Picture quality",
                quality_menu(settings.camera_quality, VideoSettingsMessage::CameraQuality)
            ),
            note(quality_note(settings.camera_quality, VideoKind::Camera)),
        ]
        .spacing(tokens::S),
    )
}

fn preview_style(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(iced::Color::BLACK)),
        border: iced::Border {
            color: theme::semantic(theme).border,
            width: 1.0,
            radius: tokens::MEDIUM.into(),
        },
        ..container::Style::default()
    }
}

/// What a quality means, in a line.
fn quality_note(quality: twicecat_core::api::VideoQuality, kind: VideoKind) -> String {
    use twicecat_core::api::VideoQuality as Q;
    let how = match quality {
        Q::Auto => "Twicecat picks the best your computer and connection can send.".to_owned(),
        Q::Hd720p30 => "Light on your computer and connection. Good for most calls.".to_owned(),
        Q::Hd1080p60 => "Sharp and smooth. Needs a good connection.".to_owned(),
        Q::Qhd1440p60 => "Very sharp and smooth. Needs a good connection and computer.".to_owned(),
        Q::Uhd2160p30 | Q::Uhd2160p60 => {
            "The sharpest. Needs a fast connection; most computers cannot send it smoothly."
                .to_owned()
        }
    };
    format!(
        "{} ({}). {how}",
        quality_words(quality),
        match kind {
            VideoKind::Camera => "your camera",
            VideoKind::Screen => "a screen share",
        }
    )
}

fn screen_section<'a>(app: &'a App, settings: &'a VideoSettings) -> Element<'a, Message> {
    let _ = app;
    kit::card(
        column![
            subheading("Sharing your screen"),
            labelled(
                "Picture quality",
                quality_menu(settings.screen_quality, VideoSettingsMessage::ScreenQuality)
            ),
            note(quality_note(settings.screen_quality, VideoKind::Screen)),
            checkbox(settings.remember_screen_choice)
                .style(kit::checkbox_style)
                .label("Remember the screen or window I pick")
                .text_wrapping(WRAP)
                .on_toggle(|on| msg(VideoSettingsMessage::RememberScreen(on))),
            note(
                "Where your desktop asks what to share, the next share starts without \
                 asking.",
            ),
        ]
        .spacing(tokens::S),
    )
}

/// The encoder that would send now: the one in use in a call, else the best
/// the setting allows.
fn encoder_in_use<'a>(app: &'a App, settings: &VideoSettings) -> Option<&'a VideoEncoderInfo> {
    let encoders = app.calls.video.encoders.as_ref()?;
    let wanted = encoders.in_use.or(match settings.encoder {
        EncoderChoice::Prefer(backend) => Some(backend),
        EncoderChoice::Auto => None,
    });
    wanted
        .and_then(|b| encoders.encoders.iter().find(|e| e.backend == b))
        .or_else(|| encoders.encoders.first())
}

fn encoding_section<'a>(app: &'a App, settings: &'a VideoSettings) -> Element<'a, Message> {
    let hardware_on = settings.encoder != EncoderChoice::Prefer(EncoderBackend::Software);
    let line = match (&app.calls.video.encoders, encoder_in_use(app, settings)) {
        (None, _) => "Looking for the video encoders of this computer\u{2026}".to_owned(),
        (Some(_), Some(e)) if e.hardware => match &e.gpu {
            Some(gpu) => format!(
                "Video is encoded by your graphics card ({}).",
                one_line_capped(gpu, MAX_NAME)
            ),
            None => "Video is encoded by your graphics card.".to_owned(),
        },
        (Some(encoders), _) => {
            if encoders.encoders.iter().any(|e| e.hardware) {
                "Video is encoded by your processor. Your graphics card could do it faster."
                    .to_owned()
            } else {
                "Video is encoded by your processor: no graphics card that can do it was found."
                    .to_owned()
            }
        }
    };
    kit::card(
        column![
            subheading("Encoding"),
            checkbox(hardware_on)
                .style(kit::checkbox_style)
                .label("Use the graphics card to encode video when there is one")
                .text_wrapping(WRAP)
                .on_toggle(|on| msg(VideoSettingsMessage::Hardware(on))),
            note(line),
            note(
                "Turn it off if video makes your computer slow or stutter. The processor \
                 sends smaller pictures (up to 1080p at 30 frames a second).",
            ),
        ]
        .spacing(tokens::S)
        .width(Fill),
    )
}

// ---- advanced -----------------------------------------------------------

/// A menu of numbers for `field`, and while it says Custom a field to type
/// the number in.
fn number_row<'a>(
    app: &'a App,
    tuning: &VideoTuning,
    kind: VideoKind,
    field: Field,
    label: &'static str,
) -> Element<'a, Message> {
    let ui = &app.calls.video.ui;
    let value = get(tuning, field);
    let typing = ui.typing.contains(&(kind, field));
    let selected = PickOption::showing(field.dim, value, typing);
    let menu = kit::menu(
        pick_list(
            PickOption::menu(field.dim),
            Some(selected),
            move |o: PickOption| msg(VideoSettingsMessage::Number(kind, field, o.pick)),
        )
        .style(kit::menu_style)
        .width(Fill),
    );
    let mut rows = Column::new()
        .spacing(tokens::XS)
        .push(labelled(label, menu));
    if selected.pick == Pick::Custom {
        let typed = ui
            .text
            .get(&(kind, field))
            .cloned()
            .unwrap_or_else(|| text_of(value));
        let hint = match field.dim {
            Dim::Size => "Width and height, such as 1600x900",
            Dim::Fps => "Frames a second",
            Dim::Bitrate => "Kilobits a second",
        };
        rows = rows.push(
            row![
                space().width(Length::Fixed(LABEL_WIDTH)),
                text_input(hint, &typed)
                    .style(kit::text_input_style)
                    .on_input(move |t| msg(VideoSettingsMessage::Text(kind, field, t)))
                    .on_submit(msg(VideoSettingsMessage::Apply(kind, field)))
                    .padding(tokens::S),
                kit::secondary_button("Apply", Some(msg(VideoSettingsMessage::Apply(kind, field)))),
            ]
            .spacing(tokens::S)
            .align_y(iced::Center),
        );
        if ui.bad.contains(&(kind, field)) {
            rows = rows.push(banner(
                Severity::Warning,
                match field.dim {
                    Dim::Size => {
                        "That is not a size. Type the width and the height, like 1600x900."
                    }
                    _ => "That is not a number. Type a whole number above zero.",
                },
            ));
        }
    }
    rows.into()
}

/// What the encoder that would send `kind` says about `codec`.
fn codec_note(app: &App, settings: &VideoSettings, kind: VideoKind) -> Option<String> {
    let tuning = settings.tuning(kind);
    let codec = tuning.codec?;
    let encoders = app.calls.video.encoders.as_ref()?;
    let choice = tuning.encoder.unwrap_or(settings.encoder);
    let info = match choice {
        EncoderChoice::Prefer(backend) => encoders
            .encoders
            .iter()
            .find(|e| e.backend == backend)
            .or_else(|| encoders.encoders.first()),
        EncoderChoice::Auto => encoders.encoders.first(),
    }?;
    let name = CodecOption(Some(codec)).to_string();
    let Some(limits) = info.codecs.iter().find(|l| l.codec == codec) else {
        return Some(format!(
            "{name} is not available with {}: the default codec is sent instead.",
            EncoderOption(EncoderChoice::Prefer(info.backend))
        ));
    };
    let mut line = String::new();
    if !limits.tested {
        line.push_str(&format!("{name} has not been tried on a call yet. "));
    }
    if codec == VideoCodec::Av1 {
        line.push_str(
            "Browsers and Element Call cannot decode AV1 in an encrypted call; it works \
             between Twicecat apps and in calls that are not encrypted.",
        );
    }
    (!line.is_empty()).then_some(line)
}

fn tuning_section<'a>(
    app: &'a App,
    settings: &'a VideoSettings,
    kind: VideoKind,
) -> Element<'a, Message> {
    let tuning = settings.tuning(kind);
    let title = match kind {
        VideoKind::Camera => "Camera, in depth",
        VideoKind::Screen => "Screen sharing, in depth",
    };
    let mut body = Column::new().spacing(tokens::S);
    body = body
        .push(subheading(title))
        .push(note(format!(
            "What the {} sends, when you do not want the picture quality above to decide.",
            kind_name(kind)
        )))
        .push(number_row(
            app,
            tuning,
            kind,
            Field::top(Dim::Size),
            "Picture size",
        ))
        .push(number_row(
            app,
            tuning,
            kind,
            Field::top(Dim::Fps),
            "Frame rate",
        ))
        .push(number_row(
            app,
            tuning,
            kind,
            Field::top(Dim::Bitrate),
            "Most data (top layer)",
        ))
        .push(note(
            "A bigger size, a faster frame rate and more data look better and use more of \
             your computer and connection.",
        ));
    // Codec and encoder.
    body = body
        .push(labelled(
            "Codec",
            kit::menu(
                pick_list(
                    CodecOption::ALL,
                    Some(CodecOption(tuning.codec)),
                    move |c: CodecOption| msg(VideoSettingsMessage::Codec(kind, c)),
                )
                .style(kit::menu_style)
                .width(Fill),
            ),
        ))
        .push(note(
            "How the picture is squeezed. Automatic is H.264 on a graphics card and VP8 \
             on the processor, which everyone can watch.",
        ));
    if let Some(line) = codec_note(app, settings, kind) {
        body = body.push(banner(Severity::Info, line));
    }
    let encoders: Vec<EncoderOption> = std::iter::once(EncoderOption(EncoderChoice::Auto))
        .chain(
            app.calls
                .video
                .encoders
                .iter()
                .flat_map(|e| e.encoders.iter())
                .map(|e| EncoderOption(EncoderChoice::Prefer(e.backend))),
        )
        .collect();
    body = body.push(labelled(
        "Encoder",
        kit::menu(
            pick_list(
                encoders,
                Some(EncoderOption(tuning.encoder.unwrap_or(EncoderChoice::Auto))),
                move |e: EncoderOption| msg(VideoSettingsMessage::Encoder(kind, e)),
            )
            .style(kit::menu_style)
            .width(Fill),
        ),
    ));
    // Layers.
    body = body
        .push(labelled(
            "Quality layers",
            kit::menu(
                pick_list(
                    LayersOption::ALL,
                    Some(LayersOption(tuning.layers)),
                    move |l: LayersOption| msg(VideoSettingsMessage::Layers(kind, l)),
                )
                .style(kit::menu_style)
                .width(Fill),
            ),
        ))
        .push(note(
            "People with a slow connection or a small window are sent a smaller layer. With \
             one layer everyone gets the same picture.",
        ));
    for layer in 1..tuning.layers.map_or(0, |n| n.min(3)) {
        body = body
            .push(words(format!("Layer {}", layer + 1)).size(tokens::BODY))
            .push(number_row(
                app,
                tuning,
                kind,
                Field {
                    layer,
                    dim: Dim::Size,
                },
                "Picture size",
            ))
            .push(number_row(
                app,
                tuning,
                kind,
                Field {
                    layer,
                    dim: Dim::Fps,
                },
                "Frame rate",
            ))
            .push(number_row(
                app,
                tuning,
                kind,
                Field {
                    layer,
                    dim: Dim::Bitrate,
                },
                "Most data",
            ));
    }
    if matches!(tuning.codec, Some(VideoCodec::Vp9 | VideoCodec::Av1)) {
        body = body
            .push(labelled(
                "Scalable video (SVC)",
                kit::menu(
                    pick_list(
                        SvcOption::all(),
                        Some(SvcOption(tuning.scalability_mode.clone())),
                        move |m: SvcOption| msg(VideoSettingsMessage::Svc(kind, m)),
                    )
                    .style(kit::menu_style)
                    .width(Fill),
                ),
            ))
            .push(note(
                "One stream that carries several layers (L3T3 is three sizes and three \
                 speeds). It replaces the separate layers above.",
            ));
    }
    body = body.push(labelled(
        "When the connection is slow",
        kit::menu(
            pick_list(
                DegradationOption::ALL,
                Some(DegradationOption(tuning.degradation)),
                move |d: DegradationOption| msg(VideoSettingsMessage::Degradation(kind, d)),
            )
            .style(kit::menu_style)
            .width(Fill),
        ),
    ));
    if kind == VideoKind::Camera {
        body = body.push(capture_rows(app, settings));
    }
    if let Some(sent) = app.calls.video.sending.get(&kind) {
        let said: Vec<String> = sent
            .iter()
            .map(|a| format!("{}: {} is sent as {}", a.setting, a.asked, a.used))
            .collect();
        body = body.push(banner(
            Severity::Warning,
            format!(
                "This computer cannot send the {} exactly as asked. {}",
                kind_name(kind),
                one_line_capped(&said.join("; "), MAX_NAME * 3)
            ),
        ));
    }
    body = body.push(kit::secondary_button(
        "Reset to automatic",
        (!tuning.is_automatic()).then(|| msg(VideoSettingsMessage::Reset(kind))),
    ));
    kit::card(body)
}

/// How the camera is opened, apart from what is sent.
fn capture_rows<'a>(app: &'a App, settings: &'a VideoSettings) -> Element<'a, Message> {
    let camera = app.calls.video.devices.as_ref().and_then(|d| {
        d.cameras
            .iter()
            .find(|c| Some(&c.id) == settings.camera.as_ref())
            .or_else(|| d.cameras.iter().find(|c| c.is_default))
            .or_else(|| d.cameras.first())
    });
    let mut modes: Vec<ModeOption> = vec![ModeOption(None)];
    if let Some(camera) = camera {
        modes.extend(camera.modes.iter().copied().map(|m| ModeOption(Some(m))));
    }
    let chosen = ModeOption(settings.camera_capture.mode);
    if !modes.contains(&chosen) {
        modes.push(chosen);
    }
    column![
        subheading("How the camera is opened"),
        labelled(
            "Capture mode",
            kit::menu(
                pick_list(modes, Some(chosen), |m: ModeOption| {
                    msg(VideoSettingsMessage::CaptureMode(m))
                })
                .style(kit::menu_style)
                .width(Fill),
            ),
        ),
        labelled(
            "Pixel format",
            kit::menu(
                pick_list(
                    FormatOption::ALL,
                    Some(FormatOption(settings.camera_capture.format)),
                    |f: FormatOption| msg(VideoSettingsMessage::CaptureFormat(f)),
                )
                .style(kit::menu_style)
                .width(Fill),
            ),
        ),
        note(
            "The size and speed the camera itself works at, apart from what is sent. Some \
             cameras give a sharper, faster picture as Motion JPEG. Used the next time the \
             camera starts.",
        ),
    ]
    .spacing(tokens::S)
    .into()
}

fn network_section<'a>(app: &'a App, settings: &'a VideoSettings) -> Element<'a, Message> {
    let ui = &app.calls.video.ui;
    let cap = CapOption::showing(settings.upload_cap_kbps, ui.cap_typing);
    let mut body = Column::new().spacing(tokens::S);
    body = body.push(subheading("All video")).push(labelled(
        "Upload limit",
        kit::menu(
            pick_list(CapOption::all(), Some(cap), |c: CapOption| {
                msg(VideoSettingsMessage::Cap(c))
            })
            .style(kit::menu_style)
            .width(Fill),
        ),
    ));
    if cap == CapOption::Custom {
        body = body.push(
            row![
                space().width(Length::Fixed(LABEL_WIDTH)),
                text_input("Kilobits a second", &ui.cap_text)
                    .style(kit::text_input_style)
                    .on_input(|t| msg(VideoSettingsMessage::CapText(t)))
                    .padding(tokens::S),
            ]
            .spacing(tokens::S),
        );
    }
    body = body
        .push(note(
            "The most all your video (camera and screen) may send together. Leave it off \
             unless you share a slow connection.",
        ))
        .push(labelled(
            "Codec when possible",
            kit::menu(
                pick_list(
                    GeneralCodecOption::ALL,
                    Some(GeneralCodecOption(settings.codec)),
                    |c: GeneralCodecOption| msg(VideoSettingsMessage::GeneralCodec(c)),
                )
                .style(kit::menu_style)
                .width(Fill),
            ),
        ))
        .push(
            checkbox(settings.prefer_integrated_gpu_on_battery)
                .style(kit::checkbox_style)
                .label("On battery, encode on the integrated graphics of a laptop with two")
                .text_wrapping(WRAP)
                .on_toggle(|on| msg(VideoSettingsMessage::PreferIntegrated(on))),
        );
    kit::card(body)
}

/// The encoders of this computer and what is being sent now.
fn status_section(app: &App) -> Element<'_, Message> {
    let video = &app.calls.video;
    let mut body = Column::new().spacing(tokens::S).width(Fill);
    body = body.push(subheading("This computer and this call"));
    match &video.encoders {
        None => body = body.push(note("Looking for the video encoders\u{2026}")),
        Some(encoders) => {
            for info in &encoders.encoders {
                let mut line = format!(
                    "{}{}:",
                    EncoderOption(EncoderChoice::Prefer(info.backend)),
                    info.gpu
                        .as_deref()
                        .map(|g| format!(" on {}", one_line_capped(g, MAX_NAME)))
                        .unwrap_or_default(),
                );
                let codecs: Vec<String> = info
                    .codecs
                    .iter()
                    .map(|limits| {
                        format!(
                            "{} up to {} \u{d7} {} at {} fps{}",
                            CodecOption(Some(limits.codec)),
                            limits.max_width,
                            limits.max_height,
                            limits.max_fps,
                            if limits.tested { "" } else { " (not tried)" }
                        )
                    })
                    .collect();
                line.push(' ');
                line.push_str(&codecs.join("; "));
                line.push('.');
                if encoders.in_use == Some(info.backend) {
                    line.push_str(" In use now.");
                }
                body = body.push(words(line).size(tokens::CAPTION));
            }
        }
    }
    let sent: Vec<&VideoTrackStats> = video
        .stats
        .as_ref()
        .map(|s| {
            s.tracks
                .iter()
                .filter(|t| t.direction == VideoDirection::Sent)
                .collect()
        })
        .unwrap_or_default();
    if app.calls.call.is_none() {
        body = body.push(note("Start a call with video to see what is sent."));
    } else if sent.is_empty() {
        body = body.push(note(
            "Nothing is being sent: the camera and the share are off.",
        ));
    }
    for stats in sent {
        let which = if video.local.camera.as_ref() == Some(&stats.track) {
            "Camera"
        } else if video.local.screen.as_ref() == Some(&stats.track) {
            "Screen share"
        } else {
            "Video"
        };
        let limit = match stats.limitation {
            QualityLimitation::None => "",
            QualityLimitation::Cpu => ", held back by the computer",
            QualityLimitation::Bandwidth => ", held back by the connection",
            QualityLimitation::Other => ", held back",
        };
        let how = match (&stats.implementation, stats.hardware) {
            (Some(name), true) => format!(", {} (graphics card)", one_line_capped(name, MAX_NAME)),
            (Some(name), false) => format!(", {} (processor)", one_line_capped(name, MAX_NAME)),
            (None, _) => String::new(),
        };
        body = body.push(words(format!(
            "{which} now: {} \u{d7} {}, {:.0} frames a second, {} kb/s{how}{limit}.",
            stats.width, stats.height, stats.fps, stats.bitrate_kbps
        )));
    }
    kit::card(body)
}
