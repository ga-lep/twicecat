//! Call and audio commands of `tessera-cli`: parsing, settings edits, and
//! the call shown in the terminal. Pure apart from what the caller prints
//! and sends, so most of it is unit-tested.
//!
//! A terminal reports key presses, not releases, so `/ptt` toggles
//! transmitting instead of holding it.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use tessera_core::api::{
    AudioDeviceId, AudioDevices, AudioSettings, CallEndReason, CallIntent, CallState, Command,
    CoreEvent, InputLevel, InputMode, ListDiff, NoiseSuppression, NsLevel, Participant, RoomId,
    RoomSummary, UserId,
};

use super::{resolve_room, sanitize};

/// Push-to-talk release tail when `/gate ptt` gives none and none is set.
const DEFAULT_RELEASE_MS: u32 = 200;
/// How often the mic test prints a level line.
const LEVEL_EVERY: Duration = Duration::from_secs(1);

/// A parsed call or audio command.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CallCmd {
    /// Join the call in the given room, or the open one.
    Call(Option<String>),
    Leave,
    /// Decline the ring in the given room, or the last one that rang.
    Decline(Option<String>),
    /// List the call's participants.
    Who,
    /// `None` toggles.
    Mute(Option<bool>),
    Deafen(Option<bool>),
    Ptt(Option<bool>),
    Devices,
    /// Device index from `/devices`; `None` follows the system default.
    Input(Option<usize>),
    Output(Option<usize>),
    Ns(NoiseSuppression),
    Aec(Option<bool>),
    /// Cancel other applications' sound too (`/aec system`).
    AecSystem(Option<bool>),
    Agc(Option<bool>),
    Gate(GateArg),
    /// Participant index from `/who`, or a user ID.
    Vol {
        who: String,
        percent: u16,
    },
    /// `Some(loopback)` starts, `None` stops.
    MicTest(Option<bool>),
    /// Show the audio settings.
    Audio,
}

/// `/gate` arguments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum GateArg {
    /// Voice activity, threshold picked automatically.
    Auto,
    /// Voice activity above this level, dBFS.
    Threshold(f32),
    /// Push-to-talk with this release tail, ms (`None` keeps the current).
    Ptt(Option<u32>),
}

/// Parses `/name arg` if it is a call or audio command: `None` when it is
/// not one, `Err(usage)` when its arguments are wrong.
pub(crate) fn parse(name: &str, arg: &str) -> Option<Result<CallCmd, &'static str>> {
    let arg = arg.trim();
    let optional = || (!arg.is_empty()).then(|| arg.to_owned());
    let parsed = match name {
        "call" => Ok(CallCmd::Call(optional())),
        "leave" => none(arg, "/leave").map(|()| CallCmd::Leave),
        "decline" => Ok(CallCmd::Decline(optional())),
        "who" => none(arg, "/who").map(|()| CallCmd::Who),
        "mute" => switch(arg, "/mute [on|off]").map(CallCmd::Mute),
        "deafen" => switch(arg, "/deafen [on|off]").map(CallCmd::Deafen),
        "ptt" => switch(arg, "/ptt [on|off]").map(CallCmd::Ptt),
        "devices" => none(arg, "/devices").map(|()| CallCmd::Devices),
        "input" => device(arg, "/input <n|default>").map(CallCmd::Input),
        "output" => device(arg, "/output <n|default>").map(CallCmd::Output),
        "ns" => noise_suppression(arg).map(CallCmd::Ns),
        "aec" => match arg.strip_prefix("system") {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => {
                switch(rest.trim(), "/aec system [on|off]").map(CallCmd::AecSystem)
            }
            _ => switch(arg, "/aec [on|off] | /aec system [on|off]").map(CallCmd::Aec),
        },
        "agc" => switch(arg, "/agc [on|off]").map(CallCmd::Agc),
        "gate" => gate(arg).map(CallCmd::Gate),
        "vol" => volume(arg),
        "mictest" => match arg {
            "" => Ok(CallCmd::MicTest(Some(false))),
            "loopback" => Ok(CallCmd::MicTest(Some(true))),
            "stop" => Ok(CallCmd::MicTest(None)),
            _ => Err("/mictest [loopback|stop]"),
        },
        "audio" => none(arg, "/audio").map(|()| CallCmd::Audio),
        _ => return None,
    };
    Some(parsed)
}

fn none(arg: &str, usage: &'static str) -> Result<(), &'static str> {
    if arg.is_empty() { Ok(()) } else { Err(usage) }
}

fn switch(arg: &str, usage: &'static str) -> Result<Option<bool>, &'static str> {
    match arg {
        "" => Ok(None),
        "on" => Ok(Some(true)),
        "off" => Ok(Some(false)),
        _ => Err(usage),
    }
}

fn device(arg: &str, usage: &'static str) -> Result<Option<usize>, &'static str> {
    match arg {
        "default" => Ok(None),
        n => n.parse().map(Some).map_err(|_| usage),
    }
}

fn noise_suppression(arg: &str) -> Result<NoiseSuppression, &'static str> {
    const USAGE: &str =
        "/ns off|rnnoise|webrtc [low|moderate|high|very_high]|deepfilter [limit dB]";
    let mut words = arg.split_whitespace();
    let ns = match (words.next(), words.next()) {
        (Some("off"), None) => NoiseSuppression::Off,
        (Some("rnnoise"), None) => NoiseSuppression::RnNoise,
        (Some("webrtc"), level) => NoiseSuppression::WebRtc {
            level: match level {
                None | Some("moderate") => NsLevel::Moderate,
                Some("low") => NsLevel::Low,
                Some("high") => NsLevel::High,
                Some("very_high") => NsLevel::VeryHigh,
                Some(_) => return Err(USAGE),
            },
        },
        (Some("deepfilter"), limit) => NoiseSuppression::DeepFilter {
            attenuation_limit_db: match limit {
                None => 100.0,
                Some(db) => db.parse().map_err(|_| USAGE)?,
            },
        },
        _ => return Err(USAGE),
    };
    if words.next().is_some() {
        return Err(USAGE);
    }
    Ok(ns)
}

fn gate(arg: &str) -> Result<GateArg, &'static str> {
    const USAGE: &str = "/gate auto|<threshold dB>|ptt [release ms]";
    let mut words = arg.split_whitespace();
    let gate = match (words.next(), words.next()) {
        (Some("auto"), None) => GateArg::Auto,
        (Some("ptt"), None) => GateArg::Ptt(None),
        (Some("ptt"), Some(ms)) => GateArg::Ptt(Some(ms.parse().map_err(|_| USAGE)?)),
        (Some(db), None) => GateArg::Threshold(
            db.parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or(USAGE)?,
        ),
        _ => return Err(USAGE),
    };
    if words.next().is_some() {
        return Err(USAGE);
    }
    Ok(gate)
}

fn volume(arg: &str) -> Result<CallCmd, &'static str> {
    const USAGE: &str = "/vol <n|@user:server> <percent>";
    let mut words = arg.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some(who), Some(percent), None) => Ok(CallCmd::Vol {
            who: who.to_owned(),
            percent: percent.trim_end_matches('%').parse().map_err(|_| USAGE)?,
        }),
        _ => Err(USAGE),
    }
}

/// What the CLI knows about calls and audio.
#[derive(Default)]
pub(crate) struct Calls {
    settings: Option<AudioSettings>,
    devices: AudioDevices,
    /// Print the next `AudioDevices` / `AudioSettings` (asked for).
    show_devices: bool,
    show_settings: bool,
    /// The room of our call (joining, in it, or reconnecting).
    call: Option<RoomId>,
    participants: Vec<Participant>,
    /// Devices in each room's call, for `/rooms`.
    counts: HashMap<String, u32>,
    ringing: BTreeSet<String>,
    last_ring: Option<RoomId>,
    muted: bool,
    deafened: bool,
    transmitting: bool,
    mic_test: bool,
    last_level: Option<Instant>,
}

/// The commands to send and the lines to print for one input or event.
#[derive(Debug, Default)]
pub(crate) struct Reply {
    pub commands: Vec<Command>,
    pub lines: Vec<String>,
}

impl Reply {
    fn line(line: impl Into<String>) -> Self {
        Self {
            commands: Vec::new(),
            lines: vec![line.into()],
        }
    }

    fn send(command: Command, line: impl Into<String>) -> Self {
        Self {
            commands: vec![command],
            lines: vec![line.into()],
        }
    }

    fn quiet(command: Command) -> Self {
        Self {
            commands: vec![command],
            lines: Vec::new(),
        }
    }
}

/// A room by its `/rooms` index or ID; any `!room:server` ID is taken as
/// given, so a room can be called before it shows up in the list.
fn pick_room(rooms: &[RoomSummary], arg: &str) -> Option<RoomId> {
    resolve_room(rooms, arg).or_else(|| arg.starts_with('!').then(|| RoomId::from(arg)))
}

/// A room's printable name.
fn room_label(rooms: &[RoomSummary], id: &RoomId) -> String {
    let name = rooms
        .iter()
        .find(|room| &room.id == id)
        .map_or(id.as_str(), |room| room.name.as_str());
    sanitize(name)
}

fn person(p: &Participant) -> String {
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

impl Calls {
    /// The number of devices in `room`'s call, as last reported.
    pub(crate) fn count(&self, room: &RoomId) -> u32 {
        self.counts.get(room.as_str()).copied().unwrap_or(0)
    }

    /// Handles one command. `selected` is the open room.
    pub(crate) fn command(
        &mut self,
        cmd: CallCmd,
        rooms: &[RoomSummary],
        selected: Option<&RoomId>,
    ) -> Reply {
        match cmd {
            CallCmd::Call(arg) => {
                let room = match arg {
                    Some(arg) => pick_room(rooms, &arg),
                    None => selected.cloned().or_else(|| self.last_ring.clone()),
                };
                match room {
                    Some(room_id) => Reply::quiet(Command::JoinCall {
                        room_id,
                        intent: CallIntent::Audio,
                    }),
                    None => Reply::line("No room: open one with /select, or /call <n|id>."),
                }
            }
            CallCmd::Leave => match self.call {
                Some(_) => Reply::quiet(Command::LeaveCall),
                None => Reply::line("Not in a call."),
            },
            CallCmd::Decline(arg) => {
                let room = match arg {
                    Some(arg) => pick_room(rooms, &arg),
                    None => self.last_ring.clone(),
                };
                match room {
                    Some(room_id) => Reply::quiet(Command::DeclineCall { room_id }),
                    None => Reply::line("Nothing is ringing."),
                }
            }
            CallCmd::Who => Reply {
                commands: Vec::new(),
                lines: self.who(),
            },
            CallCmd::Mute(on) => {
                if self.call.is_none() {
                    return Reply::line("Not in a call.");
                }
                self.muted = on.unwrap_or(!self.muted);
                let line = if self.muted {
                    "Microphone muted."
                } else {
                    "Microphone on."
                };
                Reply::send(Command::SetMicMuted(self.muted), line)
            }
            CallCmd::Deafen(on) => {
                if self.call.is_none() {
                    return Reply::line("Not in a call.");
                }
                self.deafened = on.unwrap_or(!self.deafened);
                let line = if self.deafened {
                    "Deafened: you hear nothing and your microphone is off."
                } else {
                    "Undeafened."
                };
                Reply::send(Command::SetDeafened(self.deafened), line)
            }
            CallCmd::Ptt(on) => {
                let ptt = self
                    .settings
                    .as_ref()
                    .is_some_and(|s| matches!(s.input_mode, InputMode::PushToTalk { .. }));
                if !ptt {
                    return Reply::line("Push-to-talk is off; turn it on with /gate ptt.");
                }
                self.transmitting = on.unwrap_or(!self.transmitting);
                let line = if self.transmitting {
                    "Transmitting (/ptt again to stop)."
                } else {
                    "Stopped transmitting."
                };
                Reply::send(
                    Command::PushToTalk {
                        pressed: self.transmitting,
                    },
                    line,
                )
            }
            CallCmd::Devices => {
                self.show_devices = true;
                Reply::quiet(Command::ListAudioDevices)
            }
            CallCmd::Input(index) => self.pick_device(index, true),
            CallCmd::Output(index) => self.pick_device(index, false),
            CallCmd::Ns(ns) => self.edit(|s| s.processing.noise_suppression = ns),
            CallCmd::Aec(on) => self.edit(|s| {
                let p = &mut s.processing;
                p.echo_cancellation = on.unwrap_or(!p.echo_cancellation);
            }),
            CallCmd::AecSystem(on) => self.edit(|s| {
                let p = &mut s.processing;
                p.cancel_system_audio = on.unwrap_or(!p.cancel_system_audio);
                if p.cancel_system_audio {
                    p.echo_cancellation = true;
                }
            }),
            CallCmd::Agc(on) => self.edit(|s| {
                let p = &mut s.processing;
                p.auto_gain = on.unwrap_or(!p.auto_gain);
            }),
            CallCmd::Gate(arg) => self.edit(|s| {
                s.input_mode = match (arg, s.input_mode) {
                    (GateArg::Auto, InputMode::VoiceActivity { threshold_db, .. }) => {
                        InputMode::VoiceActivity {
                            automatic: true,
                            threshold_db,
                        }
                    }
                    (GateArg::Auto, InputMode::PushToTalk { .. }) => InputMode::VoiceActivity {
                        automatic: true,
                        threshold_db: -50.0,
                    },
                    (GateArg::Threshold(threshold_db), _) => InputMode::VoiceActivity {
                        automatic: false,
                        threshold_db,
                    },
                    (GateArg::Ptt(Some(release_delay_ms)), _) => {
                        InputMode::PushToTalk { release_delay_ms }
                    }
                    (GateArg::Ptt(None), InputMode::PushToTalk { release_delay_ms }) => {
                        InputMode::PushToTalk { release_delay_ms }
                    }
                    (GateArg::Ptt(None), InputMode::VoiceActivity { .. }) => {
                        InputMode::PushToTalk {
                            release_delay_ms: DEFAULT_RELEASE_MS,
                        }
                    }
                };
            }),
            CallCmd::Vol { who, percent } => {
                let user = match who.parse::<usize>() {
                    Ok(index) => self.participants.get(index).map(|p| p.user_id.clone()),
                    Err(_) if who.starts_with('@') => Some(UserId::from(who.as_str())),
                    Err(_) => None,
                };
                match user {
                    Some(user_id) => Reply::send(
                        Command::SetUserVolume {
                            user_id: user_id.clone(),
                            percent,
                        },
                        format!(
                            "Volume of {} set to {percent} %.",
                            sanitize(user_id.as_str())
                        ),
                    ),
                    None => Reply::line("No such participant. Use /who to list them."),
                }
            }
            CallCmd::MicTest(Some(loopback)) => {
                self.mic_test = true;
                self.last_level = None;
                let line = if loopback {
                    "Mic test with loopback: you hear yourself. /mictest stop ends it."
                } else {
                    "Mic test: levels every second. /mictest stop ends it."
                };
                Reply::send(Command::StartMicTest { loopback }, line)
            }
            CallCmd::MicTest(None) => {
                self.mic_test = false;
                Reply::send(Command::StopMicTest, "Mic test stopped.")
            }
            CallCmd::Audio => match &self.settings {
                Some(settings) => Reply {
                    commands: Vec::new(),
                    lines: self.describe(settings),
                },
                None => {
                    self.show_settings = true;
                    Reply::quiet(Command::GetAudioSettings)
                }
            },
        }
    }

    /// Changes the settings and sends them whole; the core echoes the
    /// effective value, which is printed.
    fn edit(&mut self, change: impl FnOnce(&mut AudioSettings)) -> Reply {
        let Some(settings) = &self.settings else {
            self.show_settings = true;
            return Reply::send(
                Command::GetAudioSettings,
                "Audio settings are still loading; try again.",
            );
        };
        let mut settings = settings.clone();
        change(&mut settings);
        self.show_settings = true;
        Reply::quiet(Command::UpdateAudioSettings(settings))
    }

    fn pick_device(&mut self, index: Option<usize>, input: bool) -> Reply {
        let list = if input {
            &self.devices.inputs
        } else {
            &self.devices.outputs
        };
        let id = match index {
            None => None,
            Some(i) => match list.get(i) {
                Some(device) => Some(device.id.clone()),
                None => return Reply::line("No such device. Use /devices to list them."),
            },
        };
        self.edit(|s| {
            if input {
                s.input_device = id;
            } else {
                s.output_device = id;
            }
        })
    }

    fn who(&self) -> Vec<String> {
        if self.call.is_none() {
            return vec!["Not in a call.".into()];
        }
        if self.participants.is_empty() {
            return vec!["Nobody else is here yet.".into()];
        }
        self.participants
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

    fn device_name(&self, id: Option<&AudioDeviceId>, input: bool) -> String {
        let Some(id) = id else {
            return "system default".into();
        };
        let list = if input {
            &self.devices.inputs
        } else {
            &self.devices.outputs
        };
        list.iter()
            .find(|d| &d.id == id)
            .map_or_else(|| sanitize(id.as_str()), |d| sanitize(&d.name))
    }

    fn describe(&self, s: &AudioSettings) -> Vec<String> {
        let ns = match s.processing.noise_suppression {
            NoiseSuppression::Off => "off".to_owned(),
            NoiseSuppression::WebRtc { level } => format!("webrtc {level:?}").to_lowercase(),
            NoiseSuppression::RnNoise => "rnnoise".to_owned(),
            NoiseSuppression::DeepFilter {
                attenuation_limit_db,
            } => format!("deepfilter (limit {attenuation_limit_db} dB)"),
        };
        let gate = match s.input_mode {
            InputMode::VoiceActivity {
                automatic: true, ..
            } => "voice activity, automatic threshold".to_owned(),
            InputMode::VoiceActivity { threshold_db, .. } => {
                format!("voice activity above {threshold_db} dB")
            }
            InputMode::PushToTalk { release_delay_ms } => {
                format!("push-to-talk, {release_delay_ms} ms release")
            }
        };
        vec![
            format!(
                "Input: {}. Output: {}.",
                self.device_name(s.input_device.as_ref(), true),
                self.device_name(s.output_device.as_ref(), false)
            ),
            format!(
                "Echo cancellation {}{}, noise suppression {ns}, auto gain {}, high-pass {}.",
                on_off(s.processing.echo_cancellation),
                if s.processing.cancel_system_audio {
                    " (other apps' sound too)"
                } else {
                    ""
                },
                on_off(s.processing.auto_gain),
                on_off(s.processing.high_pass)
            ),
            format!("Gate: {gate}."),
            format!(
                "Gain {} dB, volume {} %, voice bitrate {} kb/s.",
                s.input_gain_db, s.output_volume_percent, s.voice_bitrate_kbps
            ),
        ]
    }

    /// Handles a call or audio event; returns the lines to print. Other
    /// events are ignored.
    pub(crate) fn event(&mut self, event: CoreEvent, rooms: &[RoomSummary]) -> Vec<String> {
        match event {
            CoreEvent::CallState(state) => self.call_state(state, rooms),
            CoreEvent::CallParticipants { room_id, diffs } => {
                if self.call.as_ref() != Some(&room_id) {
                    return Vec::new();
                }
                let before: Vec<Participant> = self.participants.clone();
                for diff in &diffs {
                    ListDiff::apply(diff, &mut self.participants);
                }
                let mut lines = Vec::new();
                for p in &self.participants {
                    if !p.is_local && !before.iter().any(|b| b.id == p.id) {
                        lines.push(format!("+ {} joined the call.", person(p)));
                    }
                }
                for p in &before {
                    if !p.is_local && !self.participants.iter().any(|a| a.id == p.id) {
                        lines.push(format!("- {} left the call.", person(p)));
                    }
                }
                lines
            }
            CoreEvent::ActiveCall {
                room_id,
                participants,
            } => {
                if participants == 0 {
                    self.counts.remove(room_id.as_str());
                } else {
                    self.counts
                        .insert(room_id.as_str().to_owned(), participants);
                }
                Vec::new()
            }
            CoreEvent::IncomingCall {
                room_id,
                caller,
                ring,
                ..
            } => {
                let room = room_label(rooms, &room_id);
                let caller = sanitize(caller.as_str());
                self.ringing.insert(room_id.as_str().to_owned());
                self.last_ring = Some(room_id);
                if ring {
                    vec![format!(
                        "Incoming call from {caller} in {room}. /call to answer, /decline to decline."
                    )]
                } else {
                    vec![format!("{caller} started a call in {room}. /call to join.")]
                }
            }
            CoreEvent::RingingStopped { room_id } => {
                if self.last_ring.as_ref() == Some(&room_id) {
                    self.last_ring = None;
                }
                if self.ringing.remove(room_id.as_str()) {
                    vec![format!(
                        "Ringing stopped in {}.",
                        room_label(rooms, &room_id)
                    )]
                } else {
                    Vec::new()
                }
            }
            CoreEvent::CallDeclined { room_id, user_id } => vec![format!(
                "{} declined the call in {}.",
                sanitize(user_id.as_str()),
                room_label(rooms, &room_id)
            )],
            CoreEvent::AudioDevices(devices) => {
                self.devices = devices;
                if !std::mem::take(&mut self.show_devices) {
                    return Vec::new();
                }
                let mut lines = vec!["Inputs:".to_owned()];
                for (i, d) in self.devices.inputs.iter().enumerate() {
                    let default = if d.is_default { "  (default)" } else { "" };
                    lines.push(format!("  {i}  {}{default}", sanitize(&d.name)));
                }
                lines.push("Outputs:".into());
                for (i, d) in self.devices.outputs.iter().enumerate() {
                    let default = if d.is_default { "  (default)" } else { "" };
                    lines.push(format!("  {i}  {}{default}", sanitize(&d.name)));
                }
                lines
            }
            CoreEvent::AudioSettings(settings) => {
                let lines = if std::mem::take(&mut self.show_settings) {
                    self.describe(&settings)
                } else {
                    Vec::new()
                };
                if !matches!(settings.input_mode, InputMode::PushToTalk { .. }) {
                    self.transmitting = false;
                }
                self.settings = Some(settings);
                lines
            }
            CoreEvent::InputLevel(level) => self.level(level),
            _ => Vec::new(),
        }
    }

    fn call_state(&mut self, state: CallState, rooms: &[RoomSummary]) -> Vec<String> {
        match state {
            CallState::Idle => Vec::new(),
            CallState::Joining { room_id } => {
                let line = format!("Joining the call in {}...", room_label(rooms, &room_id));
                self.call = Some(room_id);
                self.participants.clear();
                self.muted = false;
                self.deafened = false;
                vec![line]
            }
            CallState::Connected { room_id, encrypted } => {
                let how = if encrypted {
                    "end-to-end encrypted"
                } else {
                    "not encrypted"
                };
                let line = format!(
                    "In the call in {} ({how}). /leave to hang up.",
                    room_label(rooms, &room_id)
                );
                self.call = Some(room_id);
                vec![line]
            }
            CallState::Reconnecting { room_id, attempt } => vec![format!(
                "Call in {} lost its connection; reconnecting (attempt {attempt}).",
                room_label(rooms, &room_id)
            )],
            CallState::Left { room_id, reason } => {
                self.call = None;
                self.participants.clear();
                self.muted = false;
                self.deafened = false;
                self.transmitting = false;
                let room = room_label(rooms, &room_id);
                match reason {
                    CallEndReason::Hangup => vec![format!("Left the call in {room}.")],
                    CallEndReason::Failed(why) => {
                        vec![format!("The call in {room} ended: {}.", sanitize(&why))]
                    }
                }
            }
        }
    }

    fn level(&mut self, level: InputLevel) -> Vec<String> {
        if !self.mic_test {
            return Vec::new();
        }
        let now = Instant::now();
        if self
            .last_level
            .is_some_and(|last| now.duration_since(last) < LEVEL_EVERY)
        {
            return Vec::new();
        }
        self.last_level = Some(now);
        let gate = if level.transmitting {
            "sending"
        } else {
            "gated"
        };
        vec![format!(
            "[mic] {:.1} dB rms, {:.1} dB peak, {gate}",
            level.rms_db, level.peak_db
        )]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::api::{AudioDevice, DeviceId, ParticipantId};

    fn cmd(line: &str) -> Result<CallCmd, &'static str> {
        let rest = line.strip_prefix('/').unwrap();
        let (name, arg) = rest.split_once(' ').unwrap_or((rest, ""));
        parse(name, arg).expect("a call command")
    }

    /// Commands have no `PartialEq` (passwords); compare their debug form.
    fn sent(commands: &[Command]) -> String {
        format!("{commands:?}")
    }

    fn loaded() -> Calls {
        let mut calls = Calls::default();
        calls.event(CoreEvent::AudioSettings(AudioSettings::default()), &[]);
        calls
    }

    fn update(reply: &Reply) -> &AudioSettings {
        match reply.commands.as_slice() {
            [Command::UpdateAudioSettings(s)] => s,
            other => panic!("{other:?}"),
        }
    }

    fn participant(id: &str, user: &str, local: bool) -> Participant {
        Participant {
            id: ParticipantId::from(id),
            user_id: UserId::from(user),
            device_id: DeviceId::from("DEV"),
            display_name: None,
            is_local: local,
            muted: false,
            speaking: false,
            volume_percent: 100,
        }
    }

    #[test]
    fn parses_every_command_and_refuses_bad_arguments() {
        assert_eq!(cmd("/call"), Ok(CallCmd::Call(None)));
        assert_eq!(cmd("/call 2"), Ok(CallCmd::Call(Some("2".into()))));
        assert_eq!(cmd("/leave"), Ok(CallCmd::Leave));
        assert_eq!(cmd("/mute"), Ok(CallCmd::Mute(None)));
        assert_eq!(cmd("/deafen off"), Ok(CallCmd::Deafen(Some(false))));
        assert_eq!(cmd("/ptt on"), Ok(CallCmd::Ptt(Some(true))));
        assert_eq!(cmd("/input default"), Ok(CallCmd::Input(None)));
        assert_eq!(cmd("/output 1"), Ok(CallCmd::Output(Some(1))));
        assert_eq!(
            cmd("/ns webrtc high"),
            Ok(CallCmd::Ns(NoiseSuppression::WebRtc {
                level: NsLevel::High
            }))
        );
        assert_eq!(
            cmd("/ns rnnoise"),
            Ok(CallCmd::Ns(NoiseSuppression::RnNoise))
        );
        assert_eq!(cmd("/aec off"), Ok(CallCmd::Aec(Some(false))));
        assert_eq!(cmd("/aec system"), Ok(CallCmd::AecSystem(None)));
        assert_eq!(cmd("/aec system on"), Ok(CallCmd::AecSystem(Some(true))));
        assert_eq!(
            cmd("/gate -42.5"),
            Ok(CallCmd::Gate(GateArg::Threshold(-42.5)))
        );
        assert_eq!(
            cmd("/gate ptt 300"),
            Ok(CallCmd::Gate(GateArg::Ptt(Some(300))))
        );
        assert_eq!(
            cmd("/vol 1 150%"),
            Ok(CallCmd::Vol {
                who: "1".into(),
                percent: 150
            })
        );
        assert_eq!(cmd("/mictest loopback"), Ok(CallCmd::MicTest(Some(true))));
        assert_eq!(cmd("/mictest stop"), Ok(CallCmd::MicTest(None)));
        for bad in [
            "/leave now",
            "/mute maybe",
            "/input x",
            "/ns loud",
            "/ns webrtc max",
            "/ns off please",
            "/gate NaN",
            "/gate ptt soon",
            "/vol 1",
            "/vol 1 -5",
            "/mictest twice",
            "/aec systems",
            "/aec system maybe",
        ] {
            assert!(cmd(bad).is_err(), "{bad}");
        }
        assert_eq!(parse("rooms", ""), None, "not a call command");
    }

    #[test]
    fn settings_edits_send_the_whole_settings_and_print_the_echo() {
        let mut calls = Calls::default();
        let early = calls.command(CallCmd::Aec(Some(false)), &[], None);
        assert_eq!(
            sent(&early.commands),
            sent(&[Command::GetAudioSettings]),
            "not loaded yet"
        );

        let mut calls = loaded();
        let reply = calls.command(CallCmd::Aec(None), &[], None);
        assert!(!update(&reply).processing.echo_cancellation, "toggled");
        let reply = calls.command(CallCmd::Gate(GateArg::Ptt(None)), &[], None);
        assert_eq!(
            update(&reply).input_mode,
            InputMode::PushToTalk {
                release_delay_ms: DEFAULT_RELEASE_MS
            }
        );
        let echoed = calls.event(CoreEvent::AudioSettings(update(&reply).clone()), &[]);
        assert!(
            echoed
                .iter()
                .any(|l| l.contains("push-to-talk, 200 ms release")),
            "{echoed:?}"
        );
        // An echo nobody asked for is not printed.
        let quiet = calls.event(CoreEvent::AudioSettings(AudioSettings::default()), &[]);
        assert!(quiet.is_empty());
    }

    #[test]
    fn devices_are_picked_by_index_from_the_last_list() {
        let mut calls = loaded();
        assert_eq!(
            calls.command(CallCmd::Input(Some(0)), &[], None).lines,
            ["No such device. Use /devices to list them."]
        );
        let list = calls.command(CallCmd::Devices, &[], None);
        assert_eq!(sent(&list.commands), sent(&[Command::ListAudioDevices]));
        let mic = AudioDevice {
            id: AudioDeviceId::from("pipewire:mic"),
            name: "USB \u{1b}[31mMic".into(),
            is_default: true,
        };
        let lines = calls.event(
            CoreEvent::AudioDevices(AudioDevices {
                inputs: vec![mic],
                outputs: Vec::new(),
            }),
            &[],
        );
        assert_eq!(
            lines[1], "  0  USB ?[31mMic  (default)",
            "escapes are neutralised"
        );
        let reply = calls.command(CallCmd::Input(Some(0)), &[], None);
        assert_eq!(
            update(&reply).input_device,
            Some(AudioDeviceId::from("pipewire:mic"))
        );
    }

    #[test]
    fn in_call_commands_need_a_call_and_follow_its_state() {
        let room = RoomId::from("!r:x");
        let rooms = [RoomSummary::new(room.clone(), "Lounge".into())];
        let mut calls = loaded();
        let lines = |reply: Reply| reply.lines;
        assert_eq!(
            lines(calls.command(CallCmd::Mute(None), &rooms, None)),
            ["Not in a call."]
        );
        assert_eq!(
            lines(calls.command(CallCmd::Leave, &rooms, None)),
            ["Not in a call."]
        );
        let join = calls.command(CallCmd::Call(None), &rooms, Some(&room));
        assert_eq!(
            sent(&join.commands),
            sent(&[Command::JoinCall {
                room_id: room.clone(),
                intent: CallIntent::Audio
            }])
        );
        calls.event(
            CoreEvent::CallState(CallState::Joining {
                room_id: room.clone(),
            }),
            &rooms,
        );
        let connected = calls.event(
            CoreEvent::CallState(CallState::Connected {
                room_id: room.clone(),
                encrypted: true,
            }),
            &rooms,
        );
        assert_eq!(
            connected,
            ["In the call in Lounge (end-to-end encrypted). /leave to hang up."]
        );
        let mute = calls.command(CallCmd::Mute(None), &rooms, None);
        assert_eq!(sent(&mute.commands), sent(&[Command::SetMicMuted(true)]));
        let unmute = calls.command(CallCmd::Mute(None), &rooms, None);
        assert_eq!(
            sent(&unmute.commands),
            sent(&[Command::SetMicMuted(false)]),
            "toggles back"
        );
        // Participants: others joining and leaving are announced.
        let carol = participant("p2", "@carol:x", false);
        let joined = calls.event(
            CoreEvent::CallParticipants {
                room_id: room.clone(),
                diffs: vec![ListDiff::Reset {
                    values: vec![participant("p1", "@alice:x", true), carol],
                }],
            },
            &rooms,
        );
        assert_eq!(joined, ["+ @carol:x joined the call."]);
        let vol = calls.command(
            CallCmd::Vol {
                who: "1".into(),
                percent: 50,
            },
            &rooms,
            None,
        );
        assert_eq!(
            sent(&vol.commands),
            sent(&[Command::SetUserVolume {
                user_id: UserId::from("@carol:x"),
                percent: 50
            }])
        );
        let leave = calls.command(CallCmd::Leave, &rooms, None);
        assert_eq!(sent(&leave.commands), sent(&[Command::LeaveCall]));
        let left = calls.event(
            CoreEvent::CallState(CallState::Left {
                room_id: room,
                reason: CallEndReason::Hangup,
            }),
            &rooms,
        );
        assert_eq!(left, ["Left the call in Lounge."]);
        assert_eq!(
            lines(calls.command(CallCmd::Who, &rooms, None)),
            ["Not in a call."]
        );
    }

    #[test]
    fn rooms_are_called_by_index_or_by_any_room_id() {
        let listed = RoomId::from("!listed:x");
        let rooms = [RoomSummary::new(listed.clone(), "Lounge".into())];
        let mut calls = loaded();
        let join = |room_id: &str| {
            sent(&[Command::JoinCall {
                room_id: RoomId::from(room_id),
                intent: CallIntent::Audio,
            }])
        };
        let by_index = calls.command(CallCmd::Call(Some("0".into())), &rooms, None);
        assert_eq!(sent(&by_index.commands), join("!listed:x"));
        let unlisted = calls.command(CallCmd::Call(Some("!new:x".into())), &rooms, None);
        assert_eq!(sent(&unlisted.commands), join("!new:x"), "not synced yet");
        let nothing = calls.command(CallCmd::Call(Some("7".into())), &rooms, None);
        assert!(nothing.commands.is_empty());
    }

    #[test]
    fn push_to_talk_toggles_only_in_push_to_talk_mode() {
        let mut calls = loaded();
        assert!(
            calls
                .command(CallCmd::Ptt(None), &[], None)
                .commands
                .is_empty()
        );
        let ptt = AudioSettings {
            input_mode: InputMode::PushToTalk {
                release_delay_ms: 100,
            },
            ..AudioSettings::default()
        };
        calls.event(CoreEvent::AudioSettings(ptt), &[]);
        let press = calls.command(CallCmd::Ptt(None), &[], None);
        assert_eq!(
            sent(&press.commands),
            sent(&[Command::PushToTalk { pressed: true }])
        );
        let release = calls.command(CallCmd::Ptt(None), &[], None);
        assert_eq!(
            sent(&release.commands),
            sent(&[Command::PushToTalk { pressed: false }])
        );
    }

    #[test]
    fn rings_are_answered_or_declined_by_default() {
        let room = RoomId::from("!dm:x");
        let mut calls = loaded();
        assert_eq!(
            calls.command(CallCmd::Decline(None), &[], None).lines,
            ["Nothing is ringing."]
        );
        let lines = calls.event(
            CoreEvent::IncomingCall {
                room_id: room.clone(),
                caller: UserId::from("@alice:x"),
                intent: CallIntent::Audio,
                ring: true,
            },
            &[],
        );
        assert_eq!(
            lines,
            ["Incoming call from @alice:x in !dm:x. /call to answer, /decline to decline."]
        );
        let decline = calls.command(CallCmd::Decline(None), &[], None);
        assert_eq!(
            sent(&decline.commands),
            sent(&[Command::DeclineCall {
                room_id: room.clone()
            }])
        );
        let answer = calls.command(CallCmd::Call(None), &[], None);
        assert_eq!(
            sent(&answer.commands),
            sent(&[Command::JoinCall {
                room_id: room.clone(),
                intent: CallIntent::Audio
            }]),
            "no room open: answers the ring"
        );
        let stopped = calls.event(
            CoreEvent::RingingStopped {
                room_id: room.clone(),
            },
            &[],
        );
        assert_eq!(stopped, ["Ringing stopped in !dm:x."]);
        assert!(
            calls
                .event(CoreEvent::RingingStopped { room_id: room }, &[])
                .is_empty()
        );
    }
}
