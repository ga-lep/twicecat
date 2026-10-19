//! Call and audio commands of `twicecat-cli`: parsing, settings edits, and
//! the call shown in the terminal. Pure apart from what the caller prints
//! and sends, so most of it is unit-tested.
//!
//! A terminal reports key presses, not releases, so `/ptt` toggles
//! transmitting instead of holding it.

mod parse;
mod show;

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, Instant};

use twicecat_core::api::{
    AudioDevices, AudioSettings, CallEndReason, CallIntent, CallState, Command, CoreEvent,
    EnhancerBackend, InputLevel, InputMode, ListDiff, Participant, RoomId, RoomSummary,
    SoundSource, UserId,
};

pub(crate) use self::parse::{CallCmd, CleanArg, GateArg, SoundsArg, parse};
use self::show::{person, room_label};
use super::{resolve_room, sanitize};

/// Push-to-talk release tail when `/gate ptt` gives none and none is set.
const DEFAULT_RELEASE_MS: u32 = 200;
/// How often the mic test prints a level line.
const LEVEL_EVERY: Duration = Duration::from_secs(1);

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
    /// Where other voices were cleaned, from the last stats.
    remote_cleaning: Option<EnhancerBackend>,
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
                lines: if self.call.is_some() {
                    show::who(&self.participants)
                } else {
                    vec!["Not in a call.".into()]
                },
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
            CallCmd::Gpu => Reply::send(
                Command::ProbeGpuEnhancement,
                "Probing GPU speech enhancement…",
            ),
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
            CallCmd::AgcMax(db) => self.edit(|s| {
                s.processing.auto_gain = true;
                s.processing.auto_gain_max_db = db;
            }),
            CallCmd::Ringtone(on) => self.edit(|s| s.ringtone = on.unwrap_or(!s.ringtone)),
            CallCmd::Silence => Reply::send(Command::SilenceRinging, "Ringtone silenced."),
            CallCmd::Sounds(arg) => self.edit(|s| {
                let sounds = &mut s.call_sounds;
                match arg {
                    SoundsArg::Switch(on) => sounds.enabled = on.unwrap_or(!sounds.enabled),
                    SoundsArg::Others(on) => sounds.others_joining = on,
                    SoundsArg::Leave(on) => sounds.leaving = on,
                    SoundsArg::Volume(percent) => sounds.volume_percent = percent,
                }
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
                    (GateArg::Ptt { release, toggle }, now) => InputMode::PushToTalk {
                        release_delay_ms: release.unwrap_or(match now {
                            InputMode::PushToTalk {
                                release_delay_ms, ..
                            } => release_delay_ms,
                            InputMode::VoiceActivity { .. } => DEFAULT_RELEASE_MS,
                        }),
                        toggle,
                    },
                };
            }),
            CallCmd::Vol {
                who,
                percent,
                share,
            } => match self.user_of(&who) {
                Some(user_id) => Reply::send(
                    Command::SetUserVolume {
                        user_id: user_id.clone(),
                        source: if share {
                            SoundSource::ScreenShare
                        } else {
                            SoundSource::Voice
                        },
                        percent,
                    },
                    format!(
                        "{} of {} set to {percent} %.",
                        if share {
                            "Screen-share volume"
                        } else {
                            "Volume"
                        },
                        sanitize(user_id.as_str())
                    ),
                ),
                None => Reply::line("No such participant. Use /who to list them."),
            },
            CallCmd::Clean(CleanArg::Switch(on)) => self.edit(|s| {
                let c = &mut s.remote_cleaning;
                c.enabled = on.unwrap_or(!c.enabled);
            }),
            CallCmd::Clean(CleanArg::Limit(db)) => {
                self.edit(|s| s.remote_cleaning.attenuation_limit_db = db)
            }
            CallCmd::Clean(CleanArg::Skip(policy)) => {
                self.edit(|s| s.remote_cleaning.skip_cleaned = policy)
            }
            CallCmd::Clean(CleanArg::Announce(on)) => {
                self.edit(|s| s.announce_cleaning.enabled = on)
            }
            CallCmd::Clean(CleanArg::Outside(on)) => {
                self.edit(|s| s.announce_cleaning.cleaned_outside = on)
            }
            CallCmd::Clean(CleanArg::Gpu(on)) => self.edit(|s| s.remote_cleaning.gpu = on),
            CallCmd::Clean(CleanArg::Person { who, choice }) => match self.user_of(&who) {
                Some(user_id) => Reply::send(
                    Command::SetUserCleaning {
                        user_id: user_id.clone(),
                        cleaning: choice,
                    },
                    format!(
                        "Cleaning of {}: {}.",
                        sanitize(user_id.as_str()),
                        format!("{choice:?}").to_lowercase()
                    ),
                ),
                None => Reply::line("No such participant. Use /who to list them."),
            },
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
                    lines: show::describe(settings, &self.devices),
                },
                None => {
                    self.show_settings = true;
                    Reply::quiet(Command::GetAudioSettings)
                }
            },
        }
    }

    /// A participant index from `/who`, or a user ID.
    fn user_of(&self, who: &str) -> Option<UserId> {
        match who.parse::<usize>() {
            Ok(index) => self.participants.get(index).map(|p| p.user_id.clone()),
            Err(_) if who.starts_with('@') => Some(UserId::from(who)),
            Err(_) => None,
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
                        "Incoming call from {caller} in {room}. /call to answer, /decline to decline, /silence to stop the ringtone."
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
            CoreEvent::GpuEnhancement(gpu) => vec![show::gpu_line(&gpu)],
            CoreEvent::AudioDevices(devices) => {
                self.devices = devices;
                if !std::mem::take(&mut self.show_devices) {
                    return Vec::new();
                }
                show::device_list(&self.devices)
            }
            CoreEvent::AudioSettings(settings) => {
                let lines = if std::mem::take(&mut self.show_settings) {
                    show::describe(&settings, &self.devices)
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
            CoreEvent::AudioStats(stats) => {
                let now = stats.remote_cleaning;
                if std::mem::replace(&mut self.remote_cleaning, now) == now {
                    return Vec::new();
                }
                now.map(show::remote_cleaning_line).into_iter().collect()
            }
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
        vec![show::level_line(level)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use twicecat_core::api::{AudioDevice, AudioDeviceId, CleaningStatus, DeviceId, ParticipantId};

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
            share_volume_percent: 100,
            cleaning: CleaningStatus::Off,
        }
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
        let reply = calls.command(
            CallCmd::Gate(GateArg::Ptt {
                release: None,
                toggle: false,
            }),
            &[],
            None,
        );
        assert_eq!(
            update(&reply).input_mode,
            InputMode::PushToTalk {
                release_delay_ms: DEFAULT_RELEASE_MS,
                toggle: false,
            }
        );
        let echoed = calls.event(CoreEvent::AudioSettings(update(&reply).clone()), &[]);
        assert!(
            echoed
                .iter()
                .any(|l| l.contains("push-to-talk, 200 ms release")),
            "{echoed:?}"
        );
        let reply = calls.command(
            CallCmd::Gate(GateArg::Ptt {
                release: None,
                toggle: true,
            }),
            &[],
            None,
        );
        let echoed = calls.event(CoreEvent::AudioSettings(update(&reply).clone()), &[]);
        assert!(
            echoed
                .iter()
                .any(|l| l.contains("toggle-to-talk, 200 ms release")),
            "the delay is kept: {echoed:?}"
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
                share: false,
            },
            &rooms,
            None,
        );
        assert_eq!(
            sent(&vol.commands),
            sent(&[Command::SetUserVolume {
                user_id: UserId::from("@carol:x"),
                source: SoundSource::Voice,
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
                toggle: false,
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
            [
                "Incoming call from @alice:x in !dm:x. /call to answer, /decline to decline, /silence to stop the ringtone."
            ]
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

    #[test]
    fn each_settings_command_changes_its_setting() {
        use twicecat_core::api::{NoiseSuppression, SkipCleaned};
        let ptt = InputMode::PushToTalk {
            release_delay_ms: 300,
            toggle: false,
        };
        type Check = fn(&AudioSettings) -> bool;
        let cases: [(CallCmd, Check); 20] = [
            (CallCmd::Sounds(SoundsArg::Switch(None)), |s| {
                !s.call_sounds.enabled
            }),
            (CallCmd::Sounds(SoundsArg::Others(false)), |s| {
                !s.call_sounds.others_joining
            }),
            (CallCmd::Sounds(SoundsArg::Leave(false)), |s| {
                !s.call_sounds.leaving
            }),
            (CallCmd::Sounds(SoundsArg::Volume(40)), |s| {
                s.call_sounds.volume_percent == 40
            }),
            (CallCmd::Ringtone(None), |s| !s.ringtone),
            (CallCmd::Ringtone(Some(true)), |s| s.ringtone),
            (CallCmd::Ns(NoiseSuppression::Off), |s| {
                s.processing.noise_suppression == NoiseSuppression::Off
            }),
            (CallCmd::AecSystem(Some(true)), |s| {
                s.processing.cancel_system_audio && s.processing.echo_cancellation
            }),
            (CallCmd::Agc(None), |s| !s.processing.auto_gain),
            (CallCmd::AgcMax(20.0), |s| {
                s.processing.auto_gain && s.processing.auto_gain_max_db == 20.0
            }),
            (CallCmd::Gate(GateArg::Auto), |s| {
                s.input_mode
                    == InputMode::VoiceActivity {
                        automatic: true,
                        threshold_db: -50.0,
                    }
            }),
            (CallCmd::Gate(GateArg::Threshold(-40.0)), |s| {
                s.input_mode
                    == InputMode::VoiceActivity {
                        automatic: false,
                        threshold_db: -40.0,
                    }
            }),
            (
                CallCmd::Gate(GateArg::Ptt {
                    release: None,
                    toggle: false,
                }),
                |s| {
                    s.input_mode
                        == InputMode::PushToTalk {
                            release_delay_ms: 300,
                            toggle: false,
                        }
                },
            ),
            (
                CallCmd::Gate(GateArg::Ptt {
                    release: Some(150),
                    toggle: true,
                }),
                |s| {
                    s.input_mode
                        == InputMode::PushToTalk {
                            release_delay_ms: 150,
                            toggle: true,
                        }
                },
            ),
            (CallCmd::Clean(CleanArg::Switch(None)), |s| {
                s.remote_cleaning.enabled
            }),
            (CallCmd::Clean(CleanArg::Limit(12.0)), |s| {
                s.remote_cleaning.attenuation_limit_db == 12.0
            }),
            (CallCmd::Clean(CleanArg::Skip(SkipCleaned::Never)), |s| {
                s.remote_cleaning.skip_cleaned == SkipCleaned::Never
            }),
            (CallCmd::Clean(CleanArg::Announce(false)), |s| {
                !s.announce_cleaning.enabled
            }),
            (CallCmd::Clean(CleanArg::Outside(true)), |s| {
                s.announce_cleaning.cleaned_outside
            }),
            (CallCmd::Clean(CleanArg::Gpu(true)), |s| {
                s.remote_cleaning.gpu
            }),
        ];
        for (cmd, check) in cases {
            let mut calls = Calls::default();
            // From push-to-talk without echo cancellation: `/gate auto` takes
            // the default threshold, `/gate ptt` keeps the release delay,
            // `/aec system on` turns echo cancellation on.
            let mut start = AudioSettings {
                input_mode: ptt,
                ..AudioSettings::default()
            };
            start.processing.echo_cancellation = false;
            calls.event(CoreEvent::AudioSettings(start), &[]);
            let name = format!("{cmd:?}");
            let reply = calls.command(cmd, &[], None);
            assert!(check(update(&reply)), "{name}: {:?}", update(&reply));
        }
    }

    #[test]
    fn people_are_cleaned_by_choice_and_call_events_are_reported() {
        use twicecat_core::api::{GpuEnhancement, VoiceCleaning};
        let room = RoomId::from("!r:x");
        let mut calls = loaded();
        assert_eq!(
            calls.command(CallCmd::Deafen(None), &[], None).lines,
            ["Not in a call."]
        );
        calls.event(
            CoreEvent::CallState(CallState::Connected {
                room_id: room.clone(),
                encrypted: false,
            }),
            &[],
        );
        calls.event(
            CoreEvent::CallParticipants {
                room_id: room.clone(),
                diffs: vec![ListDiff::Reset {
                    values: vec![participant("a", "@carol:x", false)],
                }],
            },
            &[],
        );
        let clean = |calls: &mut Calls, who: &str| {
            calls.command(
                CallCmd::Clean(CleanArg::Person {
                    who: who.into(),
                    choice: VoiceCleaning::Never,
                }),
                &[],
                None,
            )
        };
        let by_index = clean(&mut calls, "0");
        assert_eq!(
            sent(&by_index.commands),
            sent(&[Command::SetUserCleaning {
                user_id: UserId::from("@carol:x"),
                cleaning: VoiceCleaning::Never,
            }])
        );
        assert_eq!(by_index.lines, ["Cleaning of @carol:x: never."]);
        assert_eq!(
            clean(&mut calls, "@dave:y").commands.len(),
            1,
            "any user ID"
        );
        assert_eq!(
            clean(&mut calls, "5").lines,
            ["No such participant. Use /who to list them."]
        );

        let deafen = calls.command(CallCmd::Deafen(None), &[], None);
        assert_eq!(sent(&deafen.commands), sent(&[Command::SetDeafened(true)]));
        let test = calls.command(CallCmd::MicTest(Some(false)), &[], None);
        assert_eq!(
            sent(&test.commands),
            sent(&[Command::StartMicTest { loopback: false }])
        );
        let level = InputLevel {
            rms_db: -30.0,
            peak_db: -10.0,
            transmitting: true,
        };
        assert_eq!(calls.event(CoreEvent::InputLevel(level), &[]).len(), 1);
        assert!(
            calls.event(CoreEvent::InputLevel(level), &[]).is_empty(),
            "one level line a second"
        );

        let gpu = GpuEnhancement {
            available: Vec::new(),
            running: None,
        };
        let lines = |calls: &mut Calls, event| calls.event(event, &[]);
        assert_eq!(
            lines(&mut calls, CoreEvent::GpuEnhancement(gpu)),
            ["GPU enhancement: none usable here."]
        );
        assert_eq!(
            lines(
                &mut calls,
                CoreEvent::CallDeclined {
                    room_id: room.clone(),
                    user_id: UserId::from("@carol:x"),
                }
            ),
            ["@carol:x declined the call in !r:x."]
        );
        assert_eq!(
            lines(
                &mut calls,
                CoreEvent::CallState(CallState::Reconnecting {
                    room_id: room.clone(),
                    attempt: 2,
                })
            ),
            ["Call in !r:x lost its connection; reconnecting (attempt 2)."]
        );
        assert_eq!(
            lines(
                &mut calls,
                CoreEvent::CallState(CallState::Left {
                    room_id: room,
                    reason: CallEndReason::Failed("the server went away".into()),
                })
            ),
            ["The call in !r:x ended: the server went away."]
        );
        assert_eq!(
            calls.command(CallCmd::Deafen(None), &[], None).lines,
            ["Not in a call."],
            "left"
        );
    }
}
