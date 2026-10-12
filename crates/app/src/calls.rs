//! Calls and audio (Phase 6 call UI, plan `docs/plans/phase6-call-ui.md`):
//! the current call, the call counts of the room list, the rooms that ring,
//! and the core's audio settings, devices and meters. Pure: the core's events
//! change this state, and only `App`'s handlers in this file send commands.

use std::collections::HashMap;
use std::fmt;

use iced::Task;
use twicecat_core::api::{
    AudioDevices, AudioSettings, AudioStats, CallEndReason, CallState, CoreError, CoreEvent,
    GpuEnhancement, InputLevel, Participant, RoomId, UserId,
};

use crate::state::{App, Message};
use crate::text_safety::one_line_capped;

/// How many ringing rooms are kept; the oldest goes first.
pub const MAX_RINGS: usize = 16;
/// How many people who declined our ring are kept for the call bar.
pub const MAX_DECLINED: usize = 32;
/// How many characters a call notice may show.
pub const MAX_NOTICE_CHARS: usize = 256;

/// Where the local call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallPhase {
    /// Getting the token, connecting, publishing.
    Joining,
    /// In the call.
    Connected {
        /// Media is end-to-end encrypted.
        encrypted: bool,
    },
    /// The call server connection dropped; the core tries again.
    Reconnecting {
        /// Failed attempts so far.
        attempt: u32,
    },
}

/// The call this device is in, from `Joining` until `Left`.
#[derive(Clone, PartialEq)]
pub struct CurrentCall {
    /// Room of the call.
    pub room_id: RoomId,
    /// Where the call is.
    pub phase: CallPhase,
    /// Everyone in the call, this device included, oldest first.
    pub participants: Vec<Participant>,
    /// People who declined the ring of the call we started.
    pub declined: Vec<UserId>,
}

impl CurrentCall {
    fn new(room_id: RoomId, phase: CallPhase) -> Self {
        Self {
            room_id,
            phase,
            participants: Vec::new(),
            declined: Vec::new(),
        }
    }
}

// Counts only: no room, user or display name in logs.
impl fmt::Debug for CurrentCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurrentCall")
            .field("phase", &self.phase)
            .field("participants", &self.participants.len())
            .field("declined", &self.declined.len())
            .finish_non_exhaustive()
    }
}

/// A room that rings (a DM call) or lets us know a call started (a group
/// room), until the core says the ringing stopped.
#[derive(Clone, PartialEq, Eq)]
pub struct Ring {
    /// Room that rings.
    pub room_id: RoomId,
    /// Who started the call.
    pub caller: UserId,
    /// A ring; `false` for a quiet notification.
    pub ring: bool,
}

/// The call and audio state the screens show.
#[derive(Default)]
pub struct CallModel {
    /// The call this device is in.
    pub call: Option<CurrentCall>,
    /// Devices in each room's call, for the room list; rooms without a call
    /// are not kept.
    pub counts: HashMap<RoomId, u32>,
    /// Rooms that ring, oldest first, one entry per room; at most
    /// [`MAX_RINGS`].
    pub rings: Vec<Ring>,
    /// A non-secret line about the last call failure: why the call ended, or
    /// a refused call or audio command.
    pub notice: Option<String>,
    /// The audio settings as the core last echoed them. Edits are made on a
    /// copy and sent whole.
    pub settings: Option<AudioSettings>,
    /// The audio devices as the core last listed them.
    pub devices: Option<AudioDevices>,
    /// The latest microphone level (mic test or call).
    pub level: Option<InputLevel>,
    /// The latest processing statistics.
    pub stats: Option<AudioStats>,
    /// Where speech enhancement can run on the GPU, and where it runs.
    pub gpu: Option<GpuEnhancement>,
}

// Counts only: no room, user or display name in logs.
impl fmt::Debug for CallModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallModel")
            .field("call", &self.call)
            .field("counts", &self.counts.len())
            .field("rings", &self.rings.len())
            .field("notice", &self.notice.is_some())
            .field("settings", &self.settings.is_some())
            .field("devices", &self.devices.is_some())
            .finish_non_exhaustive()
    }
}

impl CallModel {
    /// Devices in `room`'s call; 0 without a call.
    pub fn count(&self, room: &RoomId) -> u32 {
        self.counts.get(room).copied().unwrap_or(0)
    }

    /// The ring of `room`, if it rings.
    pub fn ring(&self, room: &RoomId) -> Option<&Ring> {
        self.rings.iter().find(|r| r.room_id == *room)
    }

    /// The call, if it is in `room`.
    pub fn call_in(&self, room: &RoomId) -> Option<&CurrentCall> {
        self.call.as_ref().filter(|c| c.room_id == *room)
    }

    fn call_in_mut(&mut self, room: &RoomId) -> Option<&mut CurrentCall> {
        self.call.as_mut().filter(|c| c.room_id == *room)
    }

    /// Applies a call state change.
    pub fn on_state(&mut self, state: CallState) {
        match state {
            CallState::Idle => self.call = None,
            CallState::Joining { room_id } => {
                // Joining answers the room's ring; the core also stops it.
                self.rings.retain(|r| r.room_id != room_id);
                self.notice = None;
                self.call = Some(CurrentCall::new(room_id, CallPhase::Joining));
            }
            CallState::Connected { room_id, encrypted } => {
                self.set_phase(room_id, CallPhase::Connected { encrypted });
            }
            CallState::Reconnecting { room_id, attempt } => {
                self.set_phase(room_id, CallPhase::Reconnecting { attempt });
            }
            CallState::Left { room_id, reason } => {
                if self.call_in(&room_id).is_some() {
                    // The core sends no empty participant list on leave.
                    self.call = None;
                }
                if let CallEndReason::Failed(why) = reason {
                    self.notice = Some(notice_line(&format!("The call ended: {why}")));
                }
            }
        }
    }

    /// A phase for `room`: a call in another room is replaced, as the core
    /// keeps one call at a time.
    fn set_phase(&mut self, room_id: RoomId, phase: CallPhase) {
        match self.call_in_mut(&room_id) {
            Some(call) => call.phase = phase,
            None => self.call = Some(CurrentCall::new(room_id, phase)),
        }
    }

    /// Applies an event of the calls and audio part of the core API. Returns
    /// `false` for any other event.
    pub fn on_event(&mut self, event: CoreEvent) -> bool {
        match event {
            CoreEvent::CallState(state) => self.on_state(state),
            CoreEvent::CallParticipants { room_id, diffs } => {
                if let Some(call) = self.call_in_mut(&room_id) {
                    for diff in &diffs {
                        diff.apply(&mut call.participants);
                    }
                }
            }
            CoreEvent::ActiveCall {
                room_id,
                participants,
            } => {
                if participants == 0 {
                    self.counts.remove(&room_id);
                } else {
                    self.counts.insert(room_id, participants);
                }
            }
            CoreEvent::IncomingCall {
                room_id,
                caller,
                ring,
                ..
            } => self.on_incoming(room_id, caller, ring),
            CoreEvent::RingingStopped { room_id } => self.rings.retain(|r| r.room_id != room_id),
            CoreEvent::CallDeclined { room_id, user_id } => {
                if let Some(call) = self.call_in_mut(&room_id)
                    && !call.declined.contains(&user_id)
                {
                    if call.declined.len() >= MAX_DECLINED {
                        call.declined.remove(0);
                    }
                    call.declined.push(user_id);
                }
            }
            CoreEvent::AudioDevices(devices) => self.devices = Some(devices),
            CoreEvent::AudioSettings(settings) => self.settings = Some(settings),
            CoreEvent::InputLevel(level) => self.level = Some(level),
            CoreEvent::AudioStats(stats) => self.stats = Some(stats),
            CoreEvent::GpuEnhancement(gpu) => self.gpu = Some(gpu),
            _ => return false,
        }
        true
    }

    fn on_incoming(&mut self, room_id: RoomId, caller: UserId, ring: bool) {
        if self.call_in(&room_id).is_some() {
            // Already there; the core only says so when another of our
            // devices joined before this one.
            return;
        }
        if let Some(known) = self.rings.iter_mut().find(|r| r.room_id == room_id) {
            known.caller = caller;
            known.ring = ring;
            return;
        }
        if self.rings.len() >= MAX_RINGS {
            self.rings.remove(0);
        }
        self.rings.push(Ring {
            room_id,
            caller,
            ring,
        });
    }

    /// Shows a refused call or audio command.
    pub fn on_error(&mut self, error: &CoreError) {
        self.notice = Some(notice_line(&error_text(error)));
    }
}

/// Whether `error` is about calls or audio (`Call`, `Audio`, `NotInCall`).
pub fn is_call_error(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Call(_) | CoreError::Audio(_) | CoreError::NotInCall
    )
}

/// The line shown for a call or audio error. The core's descriptions are
/// non-secret.
pub fn error_text(error: &CoreError) -> String {
    match error {
        CoreError::Call(why) => format!("Call: {why}"),
        CoreError::Audio(why) => format!("Audio: {why}"),
        CoreError::NotInCall => "You are not in a call.".to_owned(),
        other => other.to_string(),
    }
}

/// `text` on one line, capped at [`MAX_NOTICE_CHARS`].
fn notice_line(text: &str) -> String {
    one_line_capped(text, MAX_NOTICE_CHARS)
}

impl App {
    /// A call or audio event from the core.
    pub(crate) fn on_call_event(&mut self, event: CoreEvent) -> Task<Message> {
        let handled = self.calls.on_event(event);
        debug_assert!(handled, "on_call_event got another kind of event");
        Task::none()
    }

    /// After a login: the audio settings, so the call controls show the
    /// stored choices.
    pub(crate) fn calls_after_login(&mut self) -> Task<Message> {
        self.calls = CallModel::default();
        self.send(twicecat_core::api::Command::GetAudioSettings)
    }
}

#[cfg(test)]
mod tests {
    //! `App::update` with the core's call and audio events (plan Task 1).

    use tokio::sync::mpsc;
    use twicecat_core::api::{
        AudioDevice, AudioDeviceId, AudioDevices, AudioSettings, AudioStats, CallEndReason,
        CallIntent, CallState, CleaningStatus, Command, CoreError, CoreEvent, DeviceId, GpuBackend,
        GpuEnhancement, InputLevel, ListDiff, Participant, ParticipantId, RoomId, RoomSummary,
        UserId,
    };

    use super::*;
    use crate::bridge::EventsSlot;
    use crate::state::{App, Message, Screen, Startup};

    fn app_with(capacity: usize) -> (App, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel(capacity);
        let (_events_tx, events_rx) = mpsc::channel(1);
        let (app, _boot) = App::new(Startup {
            sink: Box::new(tx),
            events: EventsSlot::new(events_rx),
        });
        (app, rx)
    }

    fn core(event: CoreEvent) -> Message {
        Message::Core(Box::new(event))
    }

    fn drain(rx: &mut mpsc::Receiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    }

    fn logged_in() -> CoreEvent {
        CoreEvent::LoggedIn {
            user_id: UserId::from("@a:x"),
            device_id: DeviceId::from("D"),
            restored: false,
        }
    }

    /// An app on the main screen with rooms `a` and `b`, commands drained.
    fn main_app() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = app_with(64);
        let _ = app.update(core(CoreEvent::NoSession));
        let _ = app.update(core(logged_in()));
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Reset {
            values: ["a", "b"]
                .iter()
                .map(|n| RoomSummary::new(RoomId::from(format!("!{n}:x")), (*n).to_owned()))
                .collect(),
        }])));
        assert_eq!(app.screen, Screen::Main);
        drain(&mut rx);
        (app, rx)
    }

    fn room(n: &str) -> RoomId {
        RoomId::from(format!("!{n}:x"))
    }

    fn user(n: &str) -> UserId {
        UserId::from(format!("@{n}:x"))
    }

    fn state(app: &mut App, state: CallState) {
        let _ = app.update(core(CoreEvent::CallState(state)));
    }

    fn joining(app: &mut App, n: &str) {
        state(app, CallState::Joining { room_id: room(n) });
    }

    fn person(n: &str) -> Participant {
        Participant {
            id: ParticipantId::from(format!("{n}-dev")),
            user_id: user(n),
            device_id: DeviceId::from("DEV"),
            display_name: Some(n.to_uppercase()),
            is_local: n == "a",
            muted: false,
            speaking: false,
            volume_percent: 100,
            cleaning: CleaningStatus::Off,
        }
    }

    fn participants(app: &mut App, n: &str, diffs: Vec<ListDiff<Participant>>) {
        let _ = app.update(core(CoreEvent::CallParticipants {
            room_id: room(n),
            diffs,
        }));
    }

    fn incoming(app: &mut App, n: &str, caller: &str, ring: bool) {
        let _ = app.update(core(CoreEvent::IncomingCall {
            room_id: room(n),
            caller: user(caller),
            intent: CallIntent::Audio,
            ring,
        }));
    }

    fn names(app: &App) -> Vec<String> {
        app.calls
            .call
            .as_ref()
            .map(|c| {
                c.participants
                    .iter()
                    .map(|p| p.user_id.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn logging_in_asks_for_the_audio_settings() {
        let (mut app, mut rx) = app_with(16);
        let _ = app.update(core(CoreEvent::NoSession));
        drain(&mut rx);
        let _ = app.update(core(logged_in()));
        let sent = drain(&mut rx);
        assert!(
            sent.iter().any(|c| matches!(c, Command::GetAudioSettings)),
            "{sent:?}"
        );
    }

    #[test]
    fn a_call_goes_from_joining_to_connected_to_reconnecting_and_back() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        let call = app.calls.call.as_ref().expect("in a call");
        assert_eq!(call.room_id, room("a"));
        assert_eq!(call.phase, CallPhase::Joining);
        state(
            &mut app,
            CallState::Connected {
                room_id: room("a"),
                encrypted: true,
            },
        );
        assert_eq!(
            app.calls.call.as_ref().map(|c| c.phase),
            Some(CallPhase::Connected { encrypted: true })
        );
        state(
            &mut app,
            CallState::Reconnecting {
                room_id: room("a"),
                attempt: 2,
            },
        );
        assert_eq!(
            app.calls.call.as_ref().map(|c| c.phase),
            Some(CallPhase::Reconnecting { attempt: 2 })
        );
        state(
            &mut app,
            CallState::Connected {
                room_id: room("a"),
                encrypted: true,
            },
        );
        assert_eq!(
            app.calls.call.as_ref().map(|c| c.phase),
            Some(CallPhase::Connected { encrypted: true })
        );
    }

    #[test]
    fn hanging_up_clears_the_call_and_its_participants_without_a_notice() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        );
        state(
            &mut app,
            CallState::Left {
                room_id: room("a"),
                reason: CallEndReason::Hangup,
            },
        );
        assert!(app.calls.call.is_none());
        assert!(app.calls.notice.is_none());
        // A participant list after the leave does not bring the call back.
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("b")],
            }],
        );
        assert!(app.calls.call.is_none());
    }

    #[test]
    fn a_failed_call_keeps_the_reason_until_the_next_join() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        state(
            &mut app,
            CallState::Left {
                room_id: room("a"),
                reason: CallEndReason::Failed("the call server\nconnection was lost".into()),
            },
        );
        assert!(app.calls.call.is_none());
        assert_eq!(
            app.calls.notice.as_deref(),
            Some("The call ended: the call server\u{23ce}connection was lost")
        );
        joining(&mut app, "a");
        assert!(app.calls.notice.is_none());
    }

    #[test]
    fn leaving_another_room_keeps_the_current_call() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        state(
            &mut app,
            CallState::Left {
                room_id: room("b"),
                reason: CallEndReason::Hangup,
            },
        );
        assert_eq!(
            app.calls.call.as_ref().map(|c| c.room_id.clone()),
            Some(room("a"))
        );
    }

    #[test]
    fn a_join_in_another_room_replaces_the_call() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        );
        joining(&mut app, "b");
        let call = app.calls.call.as_ref().expect("in a call");
        assert_eq!(call.room_id, room("b"));
        assert!(call.participants.is_empty());
    }

    #[test]
    fn a_connected_state_without_joining_starts_the_call() {
        let (mut app, _rx) = main_app();
        state(
            &mut app,
            CallState::Connected {
                room_id: room("b"),
                encrypted: false,
            },
        );
        assert_eq!(
            app.calls.call_in(&room("b")).map(|c| c.phase),
            Some(CallPhase::Connected { encrypted: false })
        );
        state(&mut app, CallState::Idle);
        assert!(app.calls.call.is_none());
    }

    #[test]
    fn participants_follow_the_diffs_of_the_call_room_only() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        );
        assert_eq!(names(&app), ["@a:x", "@b:x"]);
        participants(
            &mut app,
            "a",
            vec![
                ListDiff::PushBack { value: person("c") },
                ListDiff::Remove { index: 1 },
                ListDiff::Insert {
                    index: 0,
                    value: person("d"),
                },
            ],
        );
        assert_eq!(names(&app), ["@d:x", "@a:x", "@c:x"]);
        participants(
            &mut app,
            "b",
            vec![ListDiff::Reset {
                values: vec![person("e")],
            }],
        );
        assert_eq!(
            names(&app),
            ["@d:x", "@a:x", "@c:x"],
            "another room's list is ignored"
        );
        participants(&mut app, "a", vec![ListDiff::Remove { index: 9 }]);
        assert_eq!(
            names(&app),
            ["@d:x", "@a:x", "@c:x"],
            "an index out of range is ignored"
        );
    }

    #[test]
    fn speaking_and_muted_flags_follow_the_participants() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        );
        let mut b = person("b");
        b.speaking = true;
        let mut a = person("a");
        a.muted = true;
        participants(&mut app, "a", vec![ListDiff::Reset { values: vec![a, b] }]);
        let call = app.calls.call.as_ref().expect("in a call");
        assert!(call.participants[0].muted && !call.participants[0].speaking);
        assert!(call.participants[1].speaking && !call.participants[1].muted);
        let mut b = person("b");
        b.muted = true;
        participants(&mut app, "a", vec![ListDiff::Set { index: 1, value: b }]);
        let call = app.calls.call.as_ref().expect("in a call");
        assert!(call.participants[1].muted && !call.participants[1].speaking);
    }

    #[test]
    fn call_counts_are_kept_per_room_and_dropped_at_zero() {
        let (mut app, _rx) = main_app();
        for (n, count) in [("a", 2), ("b", 1), ("a", 3)] {
            let _ = app.update(core(CoreEvent::ActiveCall {
                room_id: room(n),
                participants: count,
            }));
        }
        assert_eq!(app.calls.count(&room("a")), 3);
        assert_eq!(app.calls.count(&room("b")), 1);
        let _ = app.update(core(CoreEvent::ActiveCall {
            room_id: room("a"),
            participants: 0,
        }));
        assert_eq!(app.calls.count(&room("a")), 0);
        assert!(!app.calls.counts.contains_key(&room("a")));
    }

    #[test]
    fn rings_are_kept_once_per_room_until_they_stop() {
        let (mut app, _rx) = main_app();
        incoming(&mut app, "a", "b", true);
        incoming(&mut app, "b", "c", false);
        incoming(&mut app, "a", "d", true);
        let rooms: Vec<_> = app.calls.rings.iter().map(|r| r.room_id.clone()).collect();
        assert_eq!(
            rooms,
            [room("a"), room("b")],
            "one entry per room, oldest first"
        );
        let a = app.calls.ring(&room("a")).expect("a rings");
        assert_eq!(a.caller, user("d"));
        assert!(a.ring);
        assert!(!app.calls.ring(&room("b")).expect("b notifies").ring);
        let _ = app.update(core(CoreEvent::RingingStopped { room_id: room("a") }));
        assert!(app.calls.ring(&room("a")).is_none());
        assert!(app.calls.ring(&room("b")).is_some());
    }

    #[test]
    fn joining_a_ringing_room_answers_it() {
        let (mut app, _rx) = main_app();
        incoming(&mut app, "a", "b", true);
        incoming(&mut app, "b", "c", true);
        joining(&mut app, "a");
        assert!(app.calls.ring(&room("a")).is_none());
        assert!(app.calls.ring(&room("b")).is_some());
    }

    #[test]
    fn a_ring_for_the_room_we_are_in_is_ignored() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        incoming(&mut app, "a", "b", true);
        assert!(app.calls.rings.is_empty());
    }

    #[test]
    fn at_most_max_rings_are_kept_and_the_oldest_goes_first() {
        let (mut app, _rx) = main_app();
        for n in 0..=MAX_RINGS {
            incoming(&mut app, &format!("r{n}"), "b", true);
        }
        assert_eq!(app.calls.rings.len(), MAX_RINGS);
        assert!(app.calls.ring(&room("r0")).is_none());
        assert!(app.calls.ring(&room(&format!("r{MAX_RINGS}"))).is_some());
    }

    #[test]
    fn declines_are_kept_for_the_call_we_started() {
        let (mut app, _rx) = main_app();
        let decline = |app: &mut App, n: &str, who: &str| {
            let _ = app.update(core(CoreEvent::CallDeclined {
                room_id: room(n),
                user_id: user(who),
            }));
        };
        decline(&mut app, "a", "b");
        assert!(
            app.calls.call.is_none(),
            "a decline without a call starts nothing"
        );
        joining(&mut app, "a");
        decline(&mut app, "a", "b");
        decline(&mut app, "a", "b");
        decline(&mut app, "b", "c");
        assert_eq!(
            app.calls.call.as_ref().map(|c| c.declined.clone()),
            Some(vec![user("b")])
        );
        for n in 0..MAX_DECLINED {
            decline(&mut app, "a", &format!("u{n}"));
        }
        let declined = &app.calls.call.as_ref().expect("in a call").declined;
        assert_eq!(declined.len(), MAX_DECLINED);
        assert!(!declined.contains(&user("b")), "the oldest went first");
    }

    fn device(id: &str) -> AudioDevice {
        AudioDevice {
            id: AudioDeviceId::from(id),
            name: id.to_uppercase(),
            is_default: false,
        }
    }

    #[test]
    fn audio_settings_devices_levels_stats_and_gpu_are_kept() {
        let (mut app, _rx) = main_app();
        let settings = AudioSettings {
            output_volume_percent: 150,
            ..AudioSettings::default()
        };
        let _ = app.update(core(CoreEvent::AudioSettings(settings.clone())));
        assert_eq!(app.calls.settings, Some(settings));
        let level = InputLevel {
            rms_db: -30.0,
            peak_db: -12.0,
            transmitting: true,
        };
        let _ = app.update(core(CoreEvent::InputLevel(level)));
        assert_eq!(app.calls.level, Some(level));
        let stats = AudioStats {
            processing_us: 80,
            ..AudioStats::default()
        };
        let _ = app.update(core(CoreEvent::AudioStats(stats)));
        assert_eq!(app.calls.stats, Some(stats));
        let gpu = GpuEnhancement {
            available: vec![GpuBackend::Cuda],
            running: None,
        };
        let _ = app.update(core(CoreEvent::GpuEnhancement(gpu.clone())));
        assert_eq!(app.calls.gpu, Some(gpu));
    }

    #[test]
    fn a_new_device_list_replaces_the_old_one() {
        let (mut app, _rx) = main_app();
        let first = AudioDevices {
            inputs: vec![device("mic"), device("usb")],
            outputs: vec![device("speakers")],
        };
        let _ = app.update(core(CoreEvent::AudioDevices(first)));
        let unplugged = AudioDevices {
            inputs: vec![device("mic")],
            outputs: vec![device("speakers"), device("headset")],
        };
        let _ = app.update(core(CoreEvent::AudioDevices(unplugged.clone())));
        assert_eq!(app.calls.devices, Some(unplugged));
    }

    #[test]
    fn call_and_audio_errors_become_the_call_notice() {
        let (mut app, _rx) = main_app();
        for (error, line) in [
            (
                CoreError::Call("nothing is ringing in this room".into()),
                "Call: nothing is ringing in this room",
            ),
            (
                CoreError::Audio("the mic test is not available during a call".into()),
                "Audio: the mic test is not available during a call",
            ),
            (CoreError::NotInCall, "You are not in a call."),
        ] {
            let _ = app.update(core(CoreEvent::Error(error)));
            assert_eq!(app.calls.notice.as_deref(), Some(line));
            assert_eq!(app.screen, Screen::Main);
        }
    }

    #[test]
    fn other_errors_do_not_touch_the_call_notice() {
        let (mut app, _rx) = main_app();
        let _ = app.update(core(CoreEvent::Error(CoreError::Busy)));
        assert!(app.calls.notice.is_none());
    }

    #[test]
    fn a_call_error_while_restoring_still_returns_to_the_login_screen() {
        let (mut app, _rx) = app_with(8);
        let _ = app.update(core(CoreEvent::Error(CoreError::Audio("no device".into()))));
        assert_eq!(app.screen, Screen::Login);
        assert!(app.calls.notice.is_none());
    }

    #[test]
    fn logging_out_forgets_the_call_state() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "a");
        incoming(&mut app, "b", "c", true);
        let _ = app.update(core(CoreEvent::ActiveCall {
            room_id: room("a"),
            participants: 2,
        }));
        let _ = app.update(core(CoreEvent::AudioSettings(AudioSettings::default())));
        let _ = app.update(core(CoreEvent::LoggedOut));
        assert!(app.calls.call.is_none());
        assert!(app.calls.rings.is_empty());
        assert!(app.calls.counts.is_empty());
        assert!(app.calls.settings.is_none());
    }

    #[test]
    fn the_debug_output_names_no_room_user_or_reason() {
        let (mut app, _rx) = main_app();
        joining(&mut app, "secretroom");
        participants(
            &mut app,
            "secretroom",
            vec![ListDiff::Reset {
                values: vec![person("secretname")],
            }],
        );
        incoming(&mut app, "b", "secretcaller", true);
        let _ = app.update(core(CoreEvent::Error(CoreError::Call("secretwhy".into()))));
        let dbg = format!("{:?}", app.calls);
        for secret in [
            "secretroom",
            "secretname",
            "SECRETNAME",
            "secretcaller",
            "secretwhy",
        ] {
            assert!(!dbg.contains(secret), "{dbg}");
        }
        assert!(dbg.contains("participants: 1"), "{dbg}");
    }
}
