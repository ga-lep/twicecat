//! What `twicecat-cli` prints about calls and audio. Every text that comes
//! from the network or the system goes through `sanitize`.

use twicecat_core::api::{
    AudioDeviceId, AudioDevices, AudioSettings, CleaningStatus, EnhancerBackend, GpuBackend,
    GpuEnhancement, InputLevel, InputMode, NoiseSuppression, Participant, RoomId, RoomSummary,
    SkipCleaned, SkipReason,
};

use super::super::sanitize;

/// A room's printable name.
pub(super) fn room_label(rooms: &[RoomSummary], id: &RoomId) -> String {
    let name = rooms
        .iter()
        .find(|room| &room.id == id)
        .map_or(id.as_str(), |room| room.name.as_str());
    sanitize(name)
}

/// A participant's printable name.
pub(super) fn person(p: &Participant) -> String {
    match &p.display_name {
        Some(name) if !name.trim().is_empty() => {
            format!("{} ({})", sanitize(name), sanitize(p.user_id.as_str()))
        }
        _ => sanitize(p.user_id.as_str()),
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn backend_name(backend: GpuBackend) -> String {
    format!("{backend:?}").to_lowercase()
}

/// Where DeepFilterNet runs, in words: `runs` (such as "DeepFilterNet
/// runs") is followed by where, `moved` by why it is on the CPU now.
fn where_cleaned(runs: &str, moved: &str, backend: EnhancerBackend) -> String {
    match backend {
        EnhancerBackend::Cpu => format!("{runs} on the CPU"),
        EnhancerBackend::Gpu { backend } => format!("{runs} on {}", backend_name(backend)),
        EnhancerBackend::CpuAfterGpuError => format!("{moved} to the CPU after a GPU error"),
        EnhancerBackend::CpuAfterGpuTooSlow => {
            format!("{moved} to the CPU: the GPU fell behind")
        }
    }
}

/// One line on GPU speech enhancement.
pub(super) fn gpu_line(gpu: &GpuEnhancement) -> String {
    let available = if gpu.available.is_empty() {
        "none usable here".to_owned()
    } else {
        gpu.available
            .iter()
            .map(|&b| backend_name(b))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let running = gpu.running.map_or_else(String::new, |backend| {
        format!(
            "; {}",
            where_cleaned("DeepFilterNet runs", "DeepFilterNet moved", backend)
        )
    });
    format!("GPU enhancement: {available}{running}.")
}

/// Where other voices are cleaned, when that changed.
pub(super) fn remote_cleaning_line(backend: EnhancerBackend) -> String {
    let line = where_cleaned("Other voices are cleaned", "Other voices moved", backend);
    format!("{line}.")
}

/// The call's participants, numbered for `/vol` and `/clean`.
pub(super) fn who(participants: &[Participant]) -> Vec<String> {
    if participants.is_empty() {
        return vec!["Nobody else is here yet.".into()];
    }
    participants
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut flags = Vec::new();
            if p.is_local {
                flags.push("you".to_owned());
            }
            if p.muted {
                flags.push("muted".to_owned());
            }
            if p.speaking {
                flags.push("speaking".to_owned());
            }
            if p.volume_percent != 100 {
                flags.push(format!("volume {} %", p.volume_percent));
            }
            let cleaning = match p.cleaning {
                CleaningStatus::Off => None,
                CleaningStatus::Starting => Some("cleaning soon"),
                CleaningStatus::Cleaned => Some("cleaned"),
                CleaningStatus::Skipped(SkipReason::Never) => Some("not cleaned (never)"),
                CleaningStatus::Skipped(SkipReason::Limit) => Some("not cleaned (limit reached)"),
                CleaningStatus::Skipped(SkipReason::AlreadyClean) => {
                    Some("not cleaned (cleaned at their end)")
                }
                CleaningStatus::Skipped(SkipReason::Unavailable) => Some("not cleaned (no model)"),
            };
            flags.extend(cleaning.map(str::to_owned));
            let flags = if flags.is_empty() {
                String::new()
            } else {
                format!("  [{}]", flags.join(", "))
            };
            format!(
                "{i}  {}  {}{flags}",
                person(p),
                sanitize(p.device_id.as_str())
            )
        })
        .collect()
}

/// The devices, numbered for `/input` and `/output`.
pub(super) fn device_list(devices: &AudioDevices) -> Vec<String> {
    let mut lines = Vec::new();
    for (title, list) in [("Inputs:", &devices.inputs), ("Outputs:", &devices.outputs)] {
        lines.push(title.to_owned());
        for (i, d) in list.iter().enumerate() {
            let default = if d.is_default { "  (default)" } else { "" };
            lines.push(format!("  {i}  {}{default}", sanitize(&d.name)));
        }
    }
    lines
}

fn device_name(devices: &AudioDevices, id: Option<&AudioDeviceId>, input: bool) -> String {
    let Some(id) = id else {
        return "system default".into();
    };
    let list = if input {
        &devices.inputs
    } else {
        &devices.outputs
    };
    list.iter()
        .find(|d| &d.id == id)
        .map_or_else(|| sanitize(id.as_str()), |d| sanitize(&d.name))
}

/// The audio settings, in a few lines.
pub(super) fn describe(s: &AudioSettings, devices: &AudioDevices) -> Vec<String> {
    let ns = match s.processing.noise_suppression {
        NoiseSuppression::Off => "off".to_owned(),
        NoiseSuppression::WebRtc { level } => format!("webrtc {level:?}").to_lowercase(),
        NoiseSuppression::RnNoise => "rnnoise".to_owned(),
        NoiseSuppression::DeepFilter {
            attenuation_limit_db,
            gpu,
        } => {
            let on = if gpu { ", on the GPU" } else { "" };
            format!("deepfilter (limit {attenuation_limit_db} dB{on})")
        }
    };
    let gate = match s.input_mode {
        InputMode::VoiceActivity {
            automatic: true, ..
        } => "voice activity, automatic threshold".to_owned(),
        InputMode::VoiceActivity { threshold_db, .. } => {
            format!("voice activity above {threshold_db} dB")
        }
        InputMode::PushToTalk {
            release_delay_ms,
            toggle: false,
        } => format!("push-to-talk, {release_delay_ms} ms release"),
        InputMode::PushToTalk {
            release_delay_ms,
            toggle: true,
        } => format!("toggle-to-talk, {release_delay_ms} ms release"),
    };
    let cleaning = &s.remote_cleaning;
    let others = if cleaning.enabled {
        format!(
            "Other voices cleaned (DeepFilterNet3{}, up to {} dB removed), {}.",
            if cleaning.gpu {
                " on a GPU if one keeps up"
            } else {
                ""
            },
            cleaning.attenuation_limit_db,
            match cleaning.skip_cleaned {
                SkipCleaned::Neural => "not those a neural suppressor cleans at their end",
                SkipCleaned::AnySuppressor => "not those any suppressor cleans at their end",
                SkipCleaned::Never => "even those cleaned at their end",
            }
        )
    } else {
        "Other voices not cleaned.".to_owned()
    };
    let announced = match (
        s.announce_cleaning.enabled,
        s.announce_cleaning.cleaned_outside,
    ) {
        (false, _) => "Your cleaning is not announced.",
        (true, false) => "Your suppressor is announced to the call.",
        (true, true) => "Your voice is announced as cleaned before Twicecat.",
    };
    vec![
        format!(
            "Input: {}. Output: {}.",
            device_name(devices, s.input_device.as_ref(), true),
            device_name(devices, s.output_device.as_ref(), false)
        ),
        format!(
            "Echo cancellation {}{}, noise suppression {ns}, auto gain {}{}, high-pass {}.",
            on_off(s.processing.echo_cancellation),
            if s.processing.cancel_system_audio {
                " (other apps' sound too)"
            } else {
                ""
            },
            on_off(s.processing.auto_gain),
            if s.processing.auto_gain {
                format!(" (at most {} dB)", s.processing.auto_gain_max_db)
            } else {
                String::new()
            },
            on_off(s.processing.high_pass)
        ),
        format!("Gate: {gate}."),
        format!(
            "Gain {} dB, volume {} %, voice bitrate {} kb/s.",
            s.input_gain_db, s.output_volume_percent, s.voice_bitrate_kbps
        ),
        others,
        announced.to_owned(),
        format!("Ringtone {}.", on_off(s.ringtone)),
    ]
}

/// The mic test's level line.
pub(super) fn level_line(level: InputLevel) -> String {
    let gate = if level.transmitting {
        "sending"
    } else {
        "gated"
    };
    format!(
        "[mic] {:.1} dB rms, {:.1} dB peak, {gate}",
        level.rms_db, level.peak_db
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use twicecat_core::api::{AudioDevice, DeviceId, ParticipantId, UserId};

    #[test]
    fn gpu_enhancement_is_reported_in_one_line() {
        let none = GpuEnhancement {
            available: Vec::new(),
            running: None,
        };
        assert_eq!(gpu_line(&none), "GPU enhancement: none usable here.");
        let fell_back = GpuEnhancement {
            available: vec![GpuBackend::Cuda],
            running: Some(EnhancerBackend::CpuAfterGpuTooSlow),
        };
        assert_eq!(
            gpu_line(&fell_back),
            "GPU enhancement: cuda; DeepFilterNet moved to the CPU: the GPU fell behind."
        );
        let running = GpuEnhancement {
            available: vec![GpuBackend::Cuda, GpuBackend::WebGpu],
            running: Some(EnhancerBackend::Gpu {
                backend: GpuBackend::Cuda,
            }),
        };
        assert_eq!(
            gpu_line(&running),
            "GPU enhancement: cuda, webgpu; DeepFilterNet runs on cuda."
        );
    }

    #[test]
    fn where_other_voices_are_cleaned_is_one_sentence() {
        assert_eq!(
            remote_cleaning_line(EnhancerBackend::Cpu),
            "Other voices are cleaned on the CPU."
        );
        assert_eq!(
            remote_cleaning_line(EnhancerBackend::Gpu {
                backend: GpuBackend::TensorRt
            }),
            "Other voices are cleaned on tensorrt."
        );
        assert_eq!(
            remote_cleaning_line(EnhancerBackend::CpuAfterGpuError),
            "Other voices moved to the CPU after a GPU error."
        );
    }

    #[test]
    fn participants_show_their_state_and_cleaning() {
        let p = |cleaning| Participant {
            id: ParticipantId::from("p"),
            user_id: UserId::from("@bob:x"),
            device_id: DeviceId::from("DEV"),
            display_name: Some("Bob \u{1b}[2J".into()),
            is_local: false,
            muted: true,
            speaking: true,
            volume_percent: 50,
            cleaning,
        };
        let lines = who(&[
            p(CleaningStatus::Cleaned),
            p(CleaningStatus::Skipped(SkipReason::AlreadyClean)),
        ]);
        assert_eq!(
            lines[0],
            "0  Bob ?[2J (@bob:x)  DEV  [muted, speaking, volume 50 %, cleaned]"
        );
        assert!(lines[1].ends_with("not cleaned (cleaned at their end)]"));
        assert_eq!(who(&[]), ["Nobody else is here yet."]);
    }

    #[test]
    fn settings_are_described_with_device_names() {
        let mut s = AudioSettings::default();
        let devices = AudioDevices {
            inputs: vec![AudioDevice {
                id: AudioDeviceId::from("mic"),
                name: "Desk mic".into(),
                is_default: false,
            }],
            outputs: Vec::new(),
        };
        s.input_device = Some(AudioDeviceId::from("mic"));
        s.output_device = Some(AudioDeviceId::from("gone"));
        s.remote_cleaning.enabled = true;
        s.remote_cleaning.gpu = true;
        let lines = describe(&s, &devices);
        assert_eq!(lines[0], "Input: Desk mic. Output: gone.");
        assert!(lines[4].contains("on a GPU if one keeps up"), "{lines:?}");
        assert_eq!(lines[5], "Your suppressor is announced to the call.");
        assert_eq!(
            level_line(InputLevel {
                rms_db: -20.0,
                peak_db: -6.04,
                transmitting: false
            }),
            "[mic] -20.0 dB rms, -6.0 dB peak, gated"
        );
    }
}
