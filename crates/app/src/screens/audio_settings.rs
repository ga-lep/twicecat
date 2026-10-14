//! The Voice & audio card (plan Task 6, as pages in UI/UX rework Task 12): a
//! modal above the main view with a list of pages at its left and one page at
//! its right. Always a stack of two, so opening it keeps the widget state
//! under it.
//!
//! Two lines stay on screen on every page, outside the page's scroll: what
//! the microphone sends (the send mode) and whether it is open, in words. A
//! drop-down is never the only place that says it.

use iced::widget::text::Wrapping;
use iced::widget::{
    Column, Row, button, checkbox, column, container, pick_list, row, scrollable, slider, space,
    stack, text,
};
use iced::{Element, Fill, Length};
use twicecat_core::api::{AudioSettings, InputMode, NoiseSuppression};
use twicecat_ptt::{KeyboardBackend, Source};

use crate::audio_settings::{
    AUTO_GAIN_MAX_RANGE, AudioMessage, AudioPage, AudioUi, ButtonChoice, CLEANING_LIMIT_RANGE,
    GpuView, INPUT_GAIN_RANGE, LevelChoice, ModeChoice, NO_GPU_FOUND, NoiseChoice,
    RELEASE_DELAY_RANGE, SkipChoice, Slider, THRESHOLD_RANGE, backend_label, bitrate_menu,
    device_menu, diagnostic_rows, diagnostics, dropout_words, gpu_name, gpu_status, gpu_used,
    gpu_view, key_label, meter, meter_zone, mic_status, unloaded_mic_words, zone_words,
};
use crate::ptt::{BadBinding, SourceState};
use crate::state::{App, Message};
use crate::text_safety::one_line_capped;
use crate::ui::fonts;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::{self, Severity, banner, modal, muted};
use crate::ui::tokens;

/// Width of the card.
const CARD_WIDTH: f32 = 760.0;
/// Height of the card, with its padding.
const CARD_HEIGHT: f32 = 640.0;
/// The least height of the card's content, in a small window.
const MIN_CARD_HEIGHT: f32 = 240.0;
/// The padding `kit::modal` puts around the card's content.
const CARD_PADDING: f32 = 20.0;
/// Width of the list of pages.
pub const NAV_WIDTH: f32 = 168.0;
/// Width of a control's label.
const LABEL_WIDTH: f32 = 150.0;
/// Width of a slider's value.
const VALUE_WIDTH: f32 = 72.0;
/// Width of the label of a diagnostics row.
const DIAGNOSTIC_LABEL_WIDTH: f32 = 220.0;
/// Most characters of a key name or a device name on the card.
const MAX_IDENTIFIER_CHARS: usize = 120;
/// What a call notice or a source's reason is cut to.
const MAX_NOTICE_CHARS: usize = 300;
/// How text wraps on this card: a break inside a word when it has none, so a
/// long identifier is wrapped and its end is never cut off.
const WRAP: Wrapping = Wrapping::WordOrGlyph;

fn msg(message: AudioMessage) -> Message {
    Message::Audio(message)
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

fn caption<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    words(label).size(tokens::CAPTION).style(muted).into()
}

/// The base with the card on top when it is open.
pub fn layer<'a>(base: Element<'a, Message>, app: &'a App) -> Element<'a, Message> {
    let top = if app.audio_ui.open {
        view(app)
    } else {
        space().into()
    };
    stack([base, top]).into()
}

/// The list of pages, the page, and above them the lines that stay.
fn view(app: &App) -> Element<'_, Message> {
    // The card is 640 high, or as high as the window leaves room for.
    iced::widget::responsive(move |size| card(app, size.height)).into()
}

/// The modal, `window_height` high at most.
fn card(app: &App, window_height: f32) -> Element<'_, Message> {
    let header = row![
        words("Voice & audio").size(tokens::HEADLINE).width(Fill),
        kit::secondary_button("Close", Some(msg(AudioMessage::Close))),
    ]
    .spacing(tokens::M)
    .align_y(iced::Center);
    // What is sent and whether the microphone is open: on every page.
    let status: Element<'_, Message> = match mic_status(app) {
        Some(status) => banner(status.severity, status.words),
        None => banner(Severity::Info, unloaded_mic_words(app)),
    };
    // The call bar's line (a refused mic test, a device that failed) is under
    // this card: it shows here too, on every page. The slot is always there.
    let notice: Element<'_, Message> = match &app.calls.notice {
        Some(notice) => container(banner(
            Severity::Danger,
            one_line_capped(notice, MAX_NOTICE_CHARS),
        ))
        .padding(iced::Padding::ZERO.top(tokens::S))
        .into(),
        None => space().into(),
    };
    let labels = AudioPage::ALL.map(AudioPage::label);
    let nav = kit::page_nav(&labels, app.audio_ui.page.index(), |index| {
        msg(AudioMessage::Page(
            AudioPage::ALL.get(index).copied().unwrap_or_default(),
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
    let content =
        column![column![header, status, notice].spacing(tokens::S), body].spacing(tokens::L);
    let height = (window_height - 2.0 * CARD_PADDING - 2.0 * tokens::L)
        .clamp(MIN_CARD_HEIGHT, CARD_HEIGHT - 2.0 * CARD_PADDING);
    modal(container(content).height(Length::Fixed(height)), CARD_WIDTH)
}

fn page_view(app: &App) -> Element<'_, Message> {
    let Some(settings) = &app.calls.settings else {
        return column![
            heading(app.audio_ui.page.label()),
            note("Loading the audio settings\u{2026}")
        ]
        .spacing(tokens::S)
        .into();
    };
    match app.audio_ui.page {
        AudioPage::Devices => devices_page(app, settings),
        AudioPage::Talking => talking_page(app, settings),
        AudioPage::Microphone => microphone_page(app, settings),
        AudioPage::Levels => levels_page(app, settings),
        AudioPage::MicTest => mic_test_page(app),
        AudioPage::Ringtone => ringtone_page(settings),
        AudioPage::OtherVoices => others_page(app, settings),
        AudioPage::Announce => announce_page(settings),
        AudioPage::Diagnostics => diagnostics_page(app),
    }
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

fn a_checkbox<'a>(
    label: &'a str,
    on: bool,
    toggle: Option<fn(bool) -> Message>,
) -> Element<'a, Message> {
    checkbox(on)
        .label(label)
        .text_wrapping(WRAP)
        .on_toggle_maybe(toggle)
        .into()
}

fn devices_page<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let empty = Vec::new();
    let (inputs, outputs) = app
        .calls
        .devices
        .as_ref()
        .map_or((&empty, &empty), |d| (&d.inputs, &d.outputs));
    let (input_menu, input) =
        device_menu(inputs, settings.input_device.as_ref(), &app.audio_ui.names);
    let (output_menu, output) = device_menu(
        outputs,
        settings.output_device.as_ref(),
        &app.audio_ui.names,
    );
    // A closed menu cuts a long name short: the full name (capped) is
    // written out under it, wrapped.
    let in_use = |label: &str| caption(format!("In use: {}", one_line_capped(label, 200)));
    let mut page = column![
        heading(AudioPage::Devices.label()),
        labelled(
            "Microphone",
            kit::menu(
                pick_list(input_menu, Some(input.clone()), |c| msg(
                    AudioMessage::InputDevice(c)
                ))
                .style(kit::menu_style)
                .width(Fill)
            )
        ),
        in_use(&input.label),
        labelled(
            "Speakers",
            kit::menu(
                pick_list(output_menu, Some(output.clone()), |c| msg(
                    AudioMessage::OutputDevice(c)
                ))
                .style(kit::menu_style)
                .width(Fill)
            )
        ),
        in_use(&output.label),
    ]
    .spacing(tokens::S);
    if app.calls.call.is_some() {
        page = page.push(note("A new device is used from the next call."));
    }
    page.into()
}

fn value_text(slider: Slider, value: f32) -> String {
    match slider {
        Slider::InputGain => format!("{value:+.1} dB"),
        Slider::MasterVolume => format!("{value:.0}\u{a0}%"),
        Slider::MicCleaningLimit | Slider::OthersCleaningLimit | Slider::AutoGainMax => {
            format!("{value:.0} dB")
        }
        Slider::Threshold => format!("{value:.0} dB"),
        Slider::ReleaseDelay => format!("{value:.0} ms"),
    }
}

/// Where a source listens, in words.
pub fn source_name(source: Source) -> &'static str {
    match source {
        Source::Hotkey => "Global shortcut",
        Source::Portal => "Desktop shortcut",
        Source::Gamepad => "Gamepad",
        Source::Window => "Twicecat's window",
    }
}

/// One line per source the service reported.
pub fn source_line(source: Source, state: &SourceState) -> String {
    match state {
        SourceState::Ready(trigger) if trigger.is_empty() => {
            format!("{}: ready.", source_name(source))
        }
        SourceState::Ready(trigger) => format!(
            "{}: ready, press {}.",
            source_name(source),
            crate::text_safety::one_line_capped(trigger, 80)
        ),
        SourceState::Unavailable(reason) => format!(
            "{}: not available ({}).",
            source_name(source),
            crate::text_safety::one_line_capped(reason, 200)
        ),
    }
}

/// What the session's keyboard mechanism means for the key.
pub fn keyboard_note(keyboard: KeyboardBackend) -> &'static str {
    match keyboard {
        KeyboardBackend::Hotkey => {
            "During a call, the mic test, or while this card is open, the key works in every \
             app, and other apps do not receive it. It also works in Twicecat's window."
        }
        KeyboardBackend::Portal => {
            "On Wayland the desktop asks once to confirm the shortcut, and may give it another \
             key: the key it chose is shown below. It also works in Twicecat's window."
        }
        KeyboardBackend::WindowOnly => {
            "There is no global shortcut here (game mode, or no desktop shortcut service): the \
             key works while Twicecat's window has focus. A gamepad button works everywhere."
        }
    }
}

/// The send mode in words, under the menu that sets it.
fn mode_note(mode: &InputMode) -> &'static str {
    match mode {
        InputMode::VoiceActivity { .. } => {
            "Your voice is sent whenever the microphone hears you above the level set under \
             Microphone. The microphone is open for the whole call."
        }
        InputMode::PushToTalk { toggle: false, .. } => {
            "Nothing is sent until you hold the key or the gamepad button, and for a moment \
             after you let go."
        }
        InputMode::PushToTalk { toggle: true, .. } => {
            "Nothing is sent until you press the key or the gamepad button; it keeps sending \
             until you press it again."
        }
    }
}

fn talking_page<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let mut page = column![
        heading(AudioPage::Talking.label()),
        labelled(
            "Send",
            kit::menu(
                pick_list(
                    ModeChoice::ALL,
                    Some(ModeChoice::of(&settings.input_mode)),
                    |c| msg(AudioMessage::Mode(c))
                )
                .style(kit::menu_style)
                .width(Fill)
            )
        ),
        note(mode_note(&settings.input_mode)),
    ]
    .spacing(tokens::S);
    match settings.input_mode {
        InputMode::VoiceActivity { .. } => {
            page = page
                .push(note(
                    "The level at which your voice is sent is set under Microphone, in the \
                     voice activity gate. The mic test's level line says when your voice \
                     would be sent.",
                ))
                .push(
                    kit::secondary_button(
                        "Open the gate settings",
                        Some(msg(AudioMessage::Page(AudioPage::Microphone))),
                    )
                    .width(Length::Shrink),
                );
        }
        InputMode::PushToTalk { .. } => {
            page = page
                .push(key_row(app, settings))
                .push(labelled("Gamepad button", button_menu(settings)))
                .push(a_slider(
                    &app.audio_ui,
                    settings,
                    Slider::ReleaseDelay,
                    "Release delay",
                    RELEASE_DELAY_RANGE,
                    50.0,
                ))
                .push(note(
                    "Keeps sending this long after you let go, so word endings are not cut.",
                ))
                .push(note(keyboard_note(app.ptt.keyboard())));
            if let Some(setup) = app.ptt.setup() {
                for bad in &setup.bad {
                    page = page.push(banner(
                        Severity::Danger,
                        match bad {
                            BadBinding::Key => "The saved key cannot be used: set it again.",
                            BadBinding::Button => {
                                "The saved gamepad button cannot be used: pick it again."
                            }
                        },
                    ));
                }
            }
            for (source, state) in &app.ptt.sources {
                let line = source_line(*source, state);
                page = page.push(match state {
                    SourceState::Ready(_) => Row::new()
                        .push(icon(Icon::Check))
                        .push(words(line).size(tokens::BODY).style(muted).width(Fill))
                        .spacing(tokens::S)
                        .into(),
                    SourceState::Unavailable(_) => banner(Severity::Warning, line),
                });
            }
        }
    }
    page.into()
}

/// What the key row says while it waits for the key.
pub const CAPTURING_WORDS: &str = "Press a key\u{2026}";

/// The words under the key row: the way out while it waits, else why the last
/// key was refused.
pub const CAPTURING_HINT: &str =
    "Press the key together with the modifiers you want (Ctrl, Shift, Alt). Esc cancels.";

/// The key row. The same four widgets whether it waits or not
/// (`[box, Set key or Cancel, Clear]` and a hint slot), so the focus stays
/// where it is when the capture starts or ends.
fn key_row<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let ui = &app.audio_ui;
    let bound = settings.push_to_talk.key.as_deref();
    let shown = if ui.capturing {
        words(CAPTURING_WORDS).size(tokens::BODY_LARGE)
    } else {
        words(one_line_capped(
            &bound.map_or_else(|| "None".to_owned(), key_label),
            MAX_IDENTIFIER_CHARS,
        ))
        .font(fonts::MONO)
    };
    let action = if ui.capturing {
        kit::secondary_button("Cancel", Some(msg(AudioMessage::CancelCapture)))
    } else {
        kit::secondary_button("Set key\u{2026}", Some(msg(AudioMessage::CaptureKey)))
    };
    let clear = kit::secondary_button(
        "Clear",
        (bound.is_some() && !ui.capturing).then_some(msg(AudioMessage::ClearKey)),
    );
    // The words take what the buttons leave: they are never under them.
    let control = row![kit::focus_surface(shown, ui.capturing), action, clear]
        .spacing(tokens::S)
        .align_y(iced::Center);
    let hint: Element<'a, Message> = if ui.capturing {
        caption(CAPTURING_HINT)
    } else if let Some(why) = ui.capture_note {
        banner(Severity::Warning, why)
    } else {
        space().into()
    };
    column![labelled("Key", control.into()), hint]
        .spacing(tokens::XS)
        .into()
}

fn button_menu(settings: &AudioSettings) -> Element<'_, Message> {
    let (menu, chosen) = ButtonChoice::menu(settings.push_to_talk.gamepad_button.as_deref());
    kit::menu(
        pick_list(menu, Some(chosen), |c| msg(AudioMessage::Button(c)))
            .style(kit::menu_style)
            .width(Fill),
    )
}

fn a_slider<'a>(
    ui: &AudioUi,
    settings: &AudioSettings,
    which: Slider,
    label: &'a str,
    (lo, hi): (f32, f32),
    step: f32,
) -> Element<'a, Message> {
    let value = ui.shown(which, settings);
    // The slider takes no keyboard focus in iced 0.14: − and + do.
    let nudge = |glyph: Icon, up: bool| {
        button(icon(glyph))
            .padding([tokens::XS, tokens::M])
            .style(kit::secondary_style)
            .on_press(msg(AudioMessage::Step(which, up)))
    };
    labelled(
        label,
        row![
            nudge(Icon::Minus, false),
            // Ctrl + wheel is the zoom gesture, which iced's slider takes as
            // a step: the - and + buttons are the way to nudge it.
            crate::ui::guard::no_wheel(
                slider(lo..=hi, value, move |v| msg(AudioMessage::Slide(which, v)))
                    .step(step)
                    .on_release(msg(AudioMessage::SlideReleased))
                    .width(Fill)
            ),
            nudge(Icon::Plus, true),
            words(value_text(which, value)).width(Length::Fixed(VALUE_WIDTH)),
        ]
        .spacing(tokens::S)
        .align_y(iced::Center)
        .into(),
    )
}

fn limit_slider<'a>(
    ui: &AudioUi,
    settings: &AudioSettings,
    which: Slider,
    label: &'a str,
) -> Element<'a, Message> {
    a_slider(ui, settings, which, label, CLEANING_LIMIT_RANGE, 1.0)
}

/// The GPU block: a card with what the probe found, in words and tags.
fn gpu_block(app: &App) -> Element<'_, Message> {
    let gpu = app.calls.gpu.as_ref();
    let title = row![
        icon(Icon::Gpu),
        words("Graphics card").size(tokens::BODY_LARGE)
    ]
    .spacing(tokens::S)
    .align_y(iced::Center);
    let mut block = column![title].spacing(tokens::S);
    match gpu_view(gpu) {
        GpuView::Looking => {
            block = block.push(banner(Severity::Info, gpu_status(gpu)));
        }
        GpuView::NotInBuild => {
            // The light build: the note as it was, in an info banner.
            block = block.push(banner(Severity::Info, gpu_status(gpu)));
        }
        GpuView::NoneFound => {
            block = block
                .push(banner(
                    Severity::Warning,
                    "No GPU is used: noise cleaning runs on the CPU.",
                ))
                .push(kit::empty_state("No usable GPU found", NO_GPU_FOUND));
        }
        GpuView::Found {
            in_use,
            running,
            others,
            fallback,
        } => {
            let mut tags = Row::new().spacing(tokens::S).align_y(iced::Center);
            tags = tags.push(if running {
                kit::severity_tag(Severity::Success, gpu_name(in_use))
            } else {
                kit::tag(gpu_name(in_use))
            });
            for backend in others {
                tags = tags.push(kit::outline_tag(gpu_name(backend)));
            }
            block = block
                .push(note(gpu_status(gpu)))
                .push(tags.wrap().vertical_spacing(tokens::XS));
            if let Some(fallback) = fallback {
                block = block.push(banner(Severity::Warning, fallback));
            }
        }
    }
    kit::card(block)
}

fn microphone_page<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let p = &settings.processing;
    let gpu = app.calls.gpu.as_ref();
    let chosen = NoiseChoice::of(&p.noise_suppression, gpu);
    let mut page = column![
        heading(AudioPage::Microphone.label()),
        labelled(
            "Noise suppression",
            kit::menu(
                pick_list(
                    NoiseChoice::menu(gpu, &p.noise_suppression),
                    Some(chosen),
                    |c| msg(AudioMessage::Noise(c))
                )
                .style(kit::menu_style)
                .width(Fill)
            )
        ),
        // A closed menu cuts a long choice short: it is written out here.
        caption(format!("In use: {chosen}")),
    ]
    .spacing(tokens::S);
    match p.noise_suppression {
        NoiseSuppression::WebRtc { level } => {
            page = page.push(labelled(
                "Strength",
                kit::menu(
                    pick_list(LevelChoice::ALL, Some(LevelChoice(level)), |c| {
                        msg(AudioMessage::Level(c))
                    })
                    .style(kit::menu_style)
                    .width(Fill),
                ),
            ));
        }
        NoiseSuppression::DeepFilter { .. } => {
            page = page.push(limit_slider(
                &app.audio_ui,
                settings,
                Slider::MicCleaningLimit,
                "Most removed",
            ));
            // Once it runs, its model is there; a missing one is reported
            // as an error when a call or mic test starts.
            match app.calls.gpu.as_ref().and_then(|g| g.running) {
                Some(running) => page = page.push(note(backend_label(running))),
                None => {
                    page = page.push(note(
                        "Without its model, RNNoise runs instead (scripts/fetch-models.sh).",
                    ));
                }
            }
        }
        NoiseSuppression::Off | NoiseSuppression::RnNoise => {}
    }
    page = page
        .push(gpu_block(app))
        .push(a_checkbox(
            "Echo cancellation",
            p.echo_cancellation,
            Some(|on| msg(AudioMessage::EchoCancellation(on))),
        ))
        .push(a_checkbox(
            "Also remove other apps' sound (a game, a video)",
            p.cancel_system_audio,
            p.echo_cancellation
                .then_some(|on| msg(AudioMessage::CancelSystemAudio(on))),
        ))
        .push(a_checkbox(
            "Automatic gain",
            p.auto_gain,
            Some(|on| msg(AudioMessage::AutoGain(on))),
        ));
    page = if p.auto_gain {
        page.push(a_slider(
            &app.audio_ui,
            settings,
            Slider::AutoGainMax,
            "Most boost",
            AUTO_GAIN_MAX_RANGE,
            1.0,
        ))
    } else {
        page.push(note("Set the level with Microphone gain, under Levels."))
    };
    page = page
        .push(a_checkbox(
            "Remove low rumble",
            p.high_pass,
            Some(|on| msg(AudioMessage::HighPass(on))),
        ))
        .push(note(
            "These run together in this order: low rumble, echo, noise, gain. One noise \
             suppressor runs at a time. Removing other apps' sound needs echo cancellation \
             (Linux with PipeWire, Windows). Processing outside Twicecat (NVIDIA Broadcast, \
             Krisp) is not detected: turn ours off if you use it.",
        ))
        .push(gate_block(app, settings));
    page.into()
}

/// The voice activity gate: when the microphone opens by itself.
fn gate_block<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let mut block =
        column![words("Voice activity gate").size(tokens::BODY_LARGE)].spacing(tokens::S);
    match settings.input_mode {
        InputMode::VoiceActivity { automatic, .. } => {
            block = block.push(a_checkbox(
                "Set the level automatically",
                automatic,
                Some(|on| msg(AudioMessage::AutoThreshold(on))),
            ));
            if !automatic {
                block = block.push(a_slider(
                    &app.audio_ui,
                    settings,
                    Slider::Threshold,
                    "Send above",
                    THRESHOLD_RANGE,
                    1.0,
                ));
            }
            block = block.push(note(
                "The mic test's level line says when your voice would be sent.",
            ));
        }
        InputMode::PushToTalk { .. } => {
            block = block.push(note(
                "The gate is used in voice activity mode. You send with push-to-talk now: the \
                 key decides. Change it under Talking.",
            ));
        }
    }
    block.into()
}

/// The live input meter and its words: how loud, whether it is sending, and
/// which zone it is in. Always the same two lines, so nothing jumps.
fn level_meter(app: &App) -> Element<'_, Message> {
    let in_use = app.audio_ui.mic_test.is_some() || app.calls.call.is_some();
    let (fill, zone, label, zone_label) = match app.calls.level {
        Some(level) => {
            let (fill, label) = meter(level);
            let zone = meter_zone(level.rms_db);
            (fill, zone, label, zone_words(zone))
        }
        None => (
            0.0,
            Severity::Success,
            if in_use {
                "Waiting for the microphone\u{2026}".to_owned()
            } else {
                "No level: the microphone is closed.".to_owned()
            },
            "",
        ),
    };
    column![
        kit::meter_bar(fill, zone),
        row![
            words(label).size(tokens::BODY).width(Fill),
            words(zone_label).size(tokens::BODY),
        ]
        .spacing(tokens::S)
    ]
    .spacing(tokens::XS)
    .into()
}

fn levels_page<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let ui = &app.audio_ui;
    column![
        heading(AudioPage::Levels.label()),
        level_meter(app),
        a_slider(
            ui,
            settings,
            Slider::InputGain,
            "Microphone gain",
            INPUT_GAIN_RANGE,
            0.5
        ),
        a_slider(
            ui,
            settings,
            Slider::MasterVolume,
            "Volume",
            (0.0, 200.0),
            5.0
        ),
        labelled(
            "Voice quality",
            kit::menu(
                pick_list(
                    bitrate_menu(settings.voice_bitrate_kbps),
                    Some(crate::audio_settings::Bitrate(settings.voice_bitrate_kbps)),
                    |b| msg(AudioMessage::Bitrate(b))
                )
                .style(kit::menu_style)
                .width(Fill)
            )
        ),
        note("Higher sounds better and uses more data; 64 kb/s is near transparent for voice."),
    ]
    .spacing(tokens::S)
    .into()
}

fn mic_test_page(app: &App) -> Element<'_, Message> {
    let in_call = app.calls.call.is_some();
    let running = app.audio_ui.mic_test.is_some();
    let label = if running {
        "Stop the test"
    } else {
        "Test the microphone"
    };
    let start = if running {
        kit::primary_button(label, Some(msg(AudioMessage::MicTest)))
    } else {
        kit::secondary_button(label, (!in_call).then_some(msg(AudioMessage::MicTest)))
    };
    let mut page = column![
        heading(AudioPage::MicTest.label()),
        row![
            start,
            a_checkbox(
                "Hear myself (use headphones)",
                app.audio_ui.loopback,
                Some(|on| msg(AudioMessage::Loopback(on))),
            ),
        ]
        .spacing(tokens::M)
        .align_y(iced::Center),
    ]
    .spacing(tokens::S);
    if in_call {
        page = page.push(note("The mic test is not available during a call."));
    }
    page.push(level_meter(app)).into()
}

fn ringtone_page(settings: &AudioSettings) -> Element<'_, Message> {
    column![
        heading(AudioPage::Ringtone.label()),
        a_checkbox(
            "Play a ringtone when someone calls you directly",
            settings.ringtone,
            Some(|on| msg(AudioMessage::Ringtone(on))),
        ),
        note("It plays on the speakers above, at the volume above, and never during a call."),
    ]
    .spacing(tokens::S)
    .into()
}

fn others_page<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let cleaning = &settings.remote_cleaning;
    let mut page = column![
        heading("Other people's voices"),
        a_checkbox(
            "Clean other people's voices (DeepFilterNet3)",
            cleaning.enabled,
            Some(|on| msg(AudioMessage::CleanOthers(on))),
        ),
        note(
            "Removes noise from the voices you hear. Costs CPU for each voice and adds about \
             30 ms to the voices it cleans."
        ),
    ]
    .spacing(tokens::S);
    if cleaning.enabled {
        page = page
            .push(limit_slider(
                &app.audio_ui,
                settings,
                Slider::OthersCleaningLimit,
                "Most removed",
            ))
            .push(labelled(
                "Already cleaned",
                kit::menu(
                    pick_list(
                        SkipChoice::ALL,
                        Some(SkipChoice(cleaning.skip_cleaned)),
                        |c| msg(AudioMessage::Skip(c)),
                    )
                    .style(kit::menu_style)
                    .width(Fill),
                ),
            ));
        if let Some(backend) = gpu_used(app.calls.gpu.as_ref()) {
            page = page.push(
                checkbox(cleaning.gpu)
                    .label(format!("Clean on the GPU ({})", gpu_name(backend)))
                    .text_wrapping(WRAP)
                    .on_toggle(|on| msg(AudioMessage::CleanOthersGpu(on))),
            );
        }
        if let Some(running) = app.calls.stats.and_then(|s| s.remote_cleaning) {
            page = page.push(note(backend_label(running)));
        }
        page = page.push(note(
            "Each person can also be set to always or never in the call's participants panel.",
        ));
    }
    page.into()
}

fn announce_page(settings: &AudioSettings) -> Element<'_, Message> {
    let announce = &settings.announce_cleaning;
    column![
        heading("Your microphone's cleaning"),
        a_checkbox(
            "Tell others my microphone is cleaned",
            announce.enabled,
            Some(|on| msg(AudioMessage::Announce(on))),
        ),
        note(
            "So their Twicecat does not clean your voice a second time. Everyone in the call, \
             and the operator of the call server, can see which suppressor you use."
        ),
        a_checkbox(
            "My microphone is cleaned outside Twicecat (NVIDIA Broadcast, Krisp, a headset)",
            announce.cleaned_outside,
            announce
                .enabled
                .then_some(|on| msg(AudioMessage::CleanedOutside(on))),
        ),
    ]
    .spacing(tokens::S)
    .into()
}

fn diagnostics_page(app: &App) -> Element<'_, Message> {
    let open = app.audio_ui.diagnostics;
    let mut page = column![
        heading(AudioPage::Diagnostics.label()),
        kit::secondary_button(
            if open {
                "Hide diagnostics"
            } else {
                "Show diagnostics"
            },
            Some(msg(AudioMessage::Diagnostics)),
        ),
    ]
    .spacing(tokens::S);
    if open {
        match &app.calls.stats {
            Some(stats) => {
                if let Some(dropped) = dropout_words(stats) {
                    page = page.push(banner(Severity::Danger, dropped));
                }
                let lines = diagnostics(stats);
                let mut table = Column::new().spacing(tokens::XS);
                for (label, value) in diagnostic_rows(&lines) {
                    table = table.push(
                        row![
                            words(label.to_owned())
                                .size(tokens::BODY)
                                .style(muted)
                                .width(Length::Fixed(DIAGNOSTIC_LABEL_WIDTH)),
                            words(value.to_owned())
                                .size(tokens::BODY)
                                .font(fonts::MONO)
                                .width(Fill),
                        ]
                        .spacing(tokens::S),
                    );
                }
                page = page.push(table);
            }
            None => {
                page = page.push(note(
                    "Numbers appear while the microphone is open (a call or the mic test).",
                ));
            }
        }
    }
    page.into()
}
