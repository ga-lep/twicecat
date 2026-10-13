//! The Voice & audio card (plan Task 6): a modal above the main view, one
//! scrollable column of sections. Always a stack of two, so opening it keeps
//! the widget state under it.

use iced::widget::{
    Column, button, checkbox, column, container, pick_list, progress_bar, row, scrollable, slider,
    space, stack, text,
};
use iced::{Element, Fill, Length};
use twicecat_core::api::{AudioSettings, InputMode, NoiseSuppression};
use twicecat_ptt::{KeyboardBackend, Source};

use crate::audio_settings::{
    AUTO_GAIN_MAX_RANGE, AudioMessage, AudioUi, ButtonChoice, CLEANING_LIMIT_RANGE,
    INPUT_GAIN_RANGE, LevelChoice, ModeChoice, NoiseChoice, RELEASE_DELAY_RANGE, SkipChoice,
    Slider, THRESHOLD_RANGE, backend_label, bitrate_menu, device_menu, diagnostics, gpu_name,
    gpu_status, gpu_used, key_label, meter,
};
use crate::ptt::{BadBinding, SourceState};
use crate::screens::room_dialogs::{modal, muted};
use crate::state::{App, Message};

/// Width of the card.
const CARD_WIDTH: f32 = 600.0;
/// Width of a slider's value.
const VALUE_WIDTH: f32 = 64.0;

fn msg(message: AudioMessage) -> Message {
    Message::Audio(message)
}

fn heading(label: &str) -> Element<'_, Message> {
    text(label).size(18).into()
}

fn note(label: impl text::IntoFragment<'static>) -> Element<'static, Message> {
    text(label).size(13).style(muted).into()
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

fn view(app: &App) -> Element<'_, Message> {
    let header = row![
        text("Voice & audio").size(24),
        space().width(Fill),
        button(text("Close"))
            .on_press(msg(AudioMessage::Close))
            .style(button::secondary),
    ]
    .align_y(iced::Center);
    let mut body = Column::new().spacing(16).push(header);
    // The call bar's line (a refused mic test, a device that failed) is
    // under this card: it shows here too.
    if let Some(notice) = &app.calls.notice {
        body = body.push(text(notice.as_str()).size(13).style(text::danger));
    }
    match &app.calls.settings {
        None => body = body.push(note("Loading the audio settings\u{2026}")),
        Some(settings) => {
            body = body
                .push(devices_section(app, settings))
                .push(talking_section(app, settings))
                .push(processing_section(app, settings))
                .push(levels_section(&app.audio_ui, settings))
                .push(mic_test_section(app))
                .push(ringtone_section(settings))
                .push(others_section(app, settings))
                .push(announce_section(settings))
                .push(diagnostics_section(app));
        }
    }
    modal(container(scrollable(body)).max_height(640.0), CARD_WIDTH)
}

fn labelled<'a>(label: &'a str, control: Element<'a, Message>) -> Element<'a, Message> {
    row![text(label).width(Length::Fixed(140.0)), control]
        .spacing(8)
        .align_y(iced::Center)
        .into()
}

fn devices_section<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
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
    let mut section = column![
        heading("Devices"),
        labelled(
            "Microphone",
            pick_list(input_menu, Some(input), |c| msg(AudioMessage::InputDevice(
                c
            )))
            .width(Fill)
            .into()
        ),
        labelled(
            "Speakers",
            pick_list(output_menu, Some(output), |c| msg(
                AudioMessage::OutputDevice(c)
            ))
            .width(Fill)
            .into()
        ),
    ]
    .spacing(8);
    if app.calls.call.is_some() {
        section = section.push(note("A new device is used from the next call."));
    }
    section.into()
}

fn processing_section<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let p = &settings.processing;
    let gpu = app.calls.gpu.as_ref();
    let mut section = column![
        heading("Microphone"),
        labelled(
            "Noise suppression",
            pick_list(
                NoiseChoice::menu(gpu, &p.noise_suppression),
                Some(NoiseChoice::of(&p.noise_suppression, gpu)),
                |c| msg(AudioMessage::Noise(c))
            )
            .width(Fill)
            .into()
        ),
        note(gpu_status(gpu)),
    ]
    .spacing(8);
    match p.noise_suppression {
        NoiseSuppression::WebRtc { level } => {
            section = section.push(labelled(
                "Strength",
                pick_list(LevelChoice::ALL, Some(LevelChoice(level)), |c| {
                    msg(AudioMessage::Level(c))
                })
                .width(Fill)
                .into(),
            ));
        }
        NoiseSuppression::DeepFilter { .. } => {
            section = section.push(limit_slider(
                &app.audio_ui,
                settings,
                Slider::MicCleaningLimit,
                "Most removed",
            ));
            // Once it runs, its model is there; a missing one is reported
            // as an error when a call or mic test starts.
            match app.calls.gpu.as_ref().and_then(|g| g.running) {
                Some(running) => section = section.push(note(backend_label(running))),
                None => {
                    section = section.push(note(
                        "Without its model, RNNoise runs instead (scripts/fetch-models.sh).",
                    ));
                }
            }
        }
        NoiseSuppression::Off | NoiseSuppression::RnNoise => {}
    }
    section = section
        .push(
            checkbox(p.echo_cancellation)
                .label("Echo cancellation")
                .on_toggle(|on| msg(AudioMessage::EchoCancellation(on))),
        )
        .push(
            checkbox(p.cancel_system_audio)
                .label("Also remove other apps' sound (a game, a video)")
                .on_toggle_maybe(
                    p.echo_cancellation
                        .then_some(|on| msg(AudioMessage::CancelSystemAudio(on))),
                ),
        )
        .push(
            checkbox(p.auto_gain)
                .label("Automatic gain")
                .on_toggle(|on| msg(AudioMessage::AutoGain(on))),
        );
    section = if p.auto_gain {
        section.push(a_slider(
            &app.audio_ui,
            settings,
            Slider::AutoGainMax,
            "Most boost",
            AUTO_GAIN_MAX_RANGE,
            1.0,
        ))
    } else {
        section.push(note("Set the level with Microphone gain, under Levels."))
    };
    section
        .push(
            checkbox(p.high_pass)
                .label("Remove low rumble")
                .on_toggle(|on| msg(AudioMessage::HighPass(on))),
        )
        .push(note(
            "These run together in this order: low rumble, echo, noise, gain. One noise \
             suppressor runs at a time. Removing other apps' sound needs echo cancellation \
             (Linux with PipeWire, Windows). Processing outside Twicecat (NVIDIA Broadcast, \
             Krisp) is not detected: turn ours off if you use it.",
        ))
        .into()
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

fn talking_section<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let mut section = column![
        heading("Talking"),
        labelled(
            "Send",
            pick_list(
                ModeChoice::ALL,
                Some(ModeChoice::of(&settings.input_mode)),
                |c| msg(AudioMessage::Mode(c))
            )
            .width(Fill)
            .into()
        ),
    ]
    .spacing(8);
    match settings.input_mode {
        InputMode::VoiceActivity { automatic, .. } => {
            section = section.push(
                checkbox(automatic)
                    .label("Set the level automatically")
                    .on_toggle(|on| msg(AudioMessage::AutoThreshold(on))),
            );
            if !automatic {
                section = section.push(a_slider(
                    &app.audio_ui,
                    settings,
                    Slider::Threshold,
                    "Send above",
                    THRESHOLD_RANGE,
                    1.0,
                ));
            }
            section = section.push(note(
                "The mic test's level line says when your voice would be sent.",
            ));
        }
        InputMode::PushToTalk { .. } => {
            section = section
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
                    section = section.push(
                        text(match bad {
                            BadBinding::Key => "The saved key cannot be used: set it again.",
                            BadBinding::Button => {
                                "The saved gamepad button cannot be used: pick it again."
                            }
                        })
                        .size(13)
                        .style(text::danger),
                    );
                }
            }
            for (source, state) in &app.ptt.sources {
                let line = text(source_line(*source, state)).size(13);
                section = section.push(match state {
                    SourceState::Ready(_) => line.style(muted),
                    SourceState::Unavailable(_) => line.style(text::danger),
                });
            }
        }
    }
    section.into()
}

fn key_row<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let ui = &app.audio_ui;
    let bound = settings.push_to_talk.key.as_deref();
    let mut control = row![].spacing(8).align_y(iced::Center);
    if ui.capturing {
        control = control
            .push(text("Press the key, with its modifiers (Esc cancels)\u{2026}").width(Fill))
            .push(
                button(text("Cancel"))
                    .style(button::secondary)
                    .on_press(msg(AudioMessage::CancelCapture)),
            );
    } else {
        control = control
            .push(text(bound.map_or_else(|| "None".to_owned(), key_label)).width(Fill))
            .push(
                button(text("Set key\u{2026}"))
                    .style(button::secondary)
                    .on_press(msg(AudioMessage::CaptureKey)),
            )
            .push(
                button(text("Clear"))
                    .style(button::secondary)
                    .on_press_maybe(bound.is_some().then_some(msg(AudioMessage::ClearKey))),
            );
    }
    let mut row_and_note = column![labelled("Key", control.into())].spacing(4);
    if let Some(why) = ui.capture_note {
        row_and_note = row_and_note.push(text(why).size(13).style(text::danger));
    }
    row_and_note.into()
}

fn button_menu(settings: &AudioSettings) -> Element<'_, Message> {
    let (menu, chosen) = ButtonChoice::menu(settings.push_to_talk.gamepad_button.as_deref());
    pick_list(menu, Some(chosen), |c| msg(AudioMessage::Button(c)))
        .width(Fill)
        .into()
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
    let nudge = |label: &'static str, up: bool| {
        button(text(label))
            .padding([0, 8])
            .style(button::secondary)
            .on_press(msg(AudioMessage::Step(which, up)))
    };
    labelled(
        label,
        row![
            nudge("\u{2212}", false),
            slider(lo..=hi, value, move |v| msg(AudioMessage::Slide(which, v)))
                .step(step)
                .on_release(msg(AudioMessage::SlideReleased))
                .width(Fill),
            nudge("+", true),
            text(value_text(which, value)).width(Length::Fixed(VALUE_WIDTH)),
        ]
        .spacing(8)
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

fn levels_section<'a>(ui: &AudioUi, settings: &'a AudioSettings) -> Element<'a, Message> {
    column![
        heading("Levels"),
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
            pick_list(
                bitrate_menu(settings.voice_bitrate_kbps),
                Some(crate::audio_settings::Bitrate(settings.voice_bitrate_kbps)),
                |b| msg(AudioMessage::Bitrate(b))
            )
            .into()
        ),
        note("Higher sounds better and uses more data; 64 kb/s is near transparent for voice."),
    ]
    .spacing(8)
    .into()
}

fn mic_test_section(app: &App) -> Element<'_, Message> {
    let in_call = app.calls.call.is_some();
    let running = app.audio_ui.mic_test.is_some();
    let label = if running {
        "Stop the test"
    } else {
        "Test the microphone"
    };
    let mut section = column![
        heading("Mic test"),
        row![
            button(text(label))
                .style(if running {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press_maybe((!in_call || running).then_some(msg(AudioMessage::MicTest))),
            checkbox(app.audio_ui.loopback)
                .label("Hear myself (use headphones)")
                .on_toggle(|on| msg(AudioMessage::Loopback(on))),
        ]
        .spacing(12)
        .align_y(iced::Center),
    ]
    .spacing(8);
    if in_call {
        section = section.push(note("The mic test is not available during a call."));
    }
    if running || in_call {
        let (fill, words) = app.calls.level.map_or_else(
            || (0.0, "Waiting for the microphone\u{2026}".to_owned()),
            meter,
        );
        section = section
            .push(progress_bar(0.0..=1.0, fill).girth(10))
            .push(note(words));
    }
    section.into()
}

fn ringtone_section(settings: &AudioSettings) -> Element<'_, Message> {
    column![
        heading("Ringtone"),
        checkbox(settings.ringtone)
            .label("Play a ringtone when someone calls you directly")
            .on_toggle(|on| msg(AudioMessage::Ringtone(on))),
        note("It plays on the speakers above, at the volume above, and never during a call."),
    ]
    .spacing(8)
    .into()
}

fn others_section<'a>(app: &'a App, settings: &'a AudioSettings) -> Element<'a, Message> {
    let cleaning = &settings.remote_cleaning;
    let mut section = column![
        heading("Other people's voices"),
        checkbox(cleaning.enabled)
            .label("Clean other people's voices (DeepFilterNet3)")
            .on_toggle(|on| msg(AudioMessage::CleanOthers(on))),
        note("Removes noise from the voices you hear. Costs CPU for each voice and adds about 30 ms to the voices it cleans."),
    ]
    .spacing(8);
    if cleaning.enabled {
        section = section
            .push(limit_slider(
                &app.audio_ui,
                settings,
                Slider::OthersCleaningLimit,
                "Most removed",
            ))
            .push(
                pick_list(
                    SkipChoice::ALL,
                    Some(SkipChoice(cleaning.skip_cleaned)),
                    |c| msg(AudioMessage::Skip(c)),
                )
                .width(Fill),
            );
        if let Some(backend) = gpu_used(app.calls.gpu.as_ref()) {
            section = section.push(
                checkbox(cleaning.gpu)
                    .label(format!("Clean on the GPU ({})", gpu_name(backend)))
                    .on_toggle(|on| msg(AudioMessage::CleanOthersGpu(on))),
            );
        }
        if let Some(running) = app.calls.stats.and_then(|s| s.remote_cleaning) {
            section = section.push(note(backend_label(running)));
        }
        section = section.push(note(
            "Each person can also be set to always or never in the call's participants panel.",
        ));
    }
    section.into()
}

fn announce_section(settings: &AudioSettings) -> Element<'_, Message> {
    let announce = &settings.announce_cleaning;
    column![
        heading("Your microphone's cleaning"),
        checkbox(announce.enabled)
            .label("Tell others my microphone is cleaned")
            .on_toggle(|on| msg(AudioMessage::Announce(on))),
        note(
            "So their Twicecat does not clean your voice a second time. Everyone in the call, \
             and the operator of the call server, can see which suppressor you use."
        ),
        checkbox(announce.cleaned_outside)
            .label("My microphone is cleaned outside Twicecat (NVIDIA Broadcast, Krisp, a headset)")
            .on_toggle_maybe(
                announce
                    .enabled
                    .then_some(|on| msg(AudioMessage::CleanedOutside(on)))
            ),
    ]
    .spacing(8)
    .into()
}

fn diagnostics_section(app: &App) -> Element<'_, Message> {
    let open = app.audio_ui.diagnostics;
    let mut section = column![
        button(text(if open {
            "Hide diagnostics"
        } else {
            "Show diagnostics"
        }))
        .style(button::secondary)
        .on_press(msg(AudioMessage::Diagnostics)),
    ]
    .spacing(6);
    if open {
        match &app.calls.stats {
            Some(stats) => {
                for line in diagnostics(stats) {
                    section = section.push(note(line));
                }
            }
            None => {
                section = section.push(note(
                    "Numbers appear while the microphone is open (a call or the mic test).",
                ));
            }
        }
    }
    section.into()
}
