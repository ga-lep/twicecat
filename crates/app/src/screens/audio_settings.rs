//! The Voice & audio card (plan Task 6): a modal above the main view, one
//! scrollable column of sections. Always a stack of two, so opening it keeps
//! the widget state under it.

use iced::widget::{
    Column, button, checkbox, column, container, pick_list, progress_bar, row, scrollable, slider,
    space, stack, text,
};
use iced::{Element, Fill, Length};
use twicecat_core::api::{AudioSettings, NoiseSuppression};

use crate::audio_settings::{
    AudioMessage, AudioUi, CLEANING_LIMIT_RANGE, INPUT_GAIN_RANGE, LevelChoice, NoiseChoice,
    SkipChoice, Slider, backend_label, bitrate_menu, device_menu, diagnostics, gpu_name,
    gpu_status, gpu_used, meter,
};
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
            if let Some(running) = app.calls.gpu.as_ref().and_then(|g| g.running) {
                section = section.push(note(backend_label(running)));
            }
            section = section.push(note(
                "Without its model, RNNoise runs instead (scripts/fetch-models.sh).",
            ));
        }
        NoiseSuppression::Off | NoiseSuppression::RnNoise => {}
    }
    section
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
        )
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
        Slider::MicCleaningLimit | Slider::OthersCleaningLimit => format!("{value:.0} dB"),
    }
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
