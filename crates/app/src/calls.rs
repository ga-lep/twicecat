//! Calls and audio (Phase 6 call UI, plan `docs/plans/phase6-call-ui.md`):
//! the current call, the call counts of the room list, the rooms that ring,
//! and the core's audio settings, devices and meters. Pure: the core's events
//! change this state, and only `App`'s handlers in this file send commands.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::time::Duration;

use iced::Task;
use twicecat_core::api::{
    AudioDevices, AudioSettings, AudioStats, CallEndReason, CallIntent, CallState, CleaningStatus,
    Command, CoreError, CoreEvent, GpuEnhancement, InputLevel, Participant, RoomId, RoomMemberView,
    RoomSummary, SkipReason, SoundSource, UserId, VoiceCleaning,
};

use crate::state::{App, Message};
use crate::text_safety::one_line_capped;
use crate::ui::icons::Icon;

/// How many ringing rooms are kept; the oldest goes first.
pub const MAX_RINGS: usize = 16;
/// How many people who declined our ring are kept for the call bar.
pub const MAX_DECLINED: usize = 32;
/// How many characters a call notice may show.
pub const MAX_NOTICE_CHARS: usize = 256;
/// The speaker mark of the call pill in the room list.
pub const CALL_MARK: &str = Icon::Call.glyph();
/// The mark of a muted participant.
pub const MUTED_MARK: &str = Icon::Muted.glyph();
/// Highest per-user volume, percent (the core's `limits::MAX_VOLUME_PERCENT`).
pub const MAX_VOLUME: u16 = 200;
/// What the participants panel's **−** and **+** change, percent.
pub const VOLUME_STEP: u16 = 10;
/// A volume slider moved by the wheel or the keys has no release: its value
/// is sent after this pause.
pub const VOLUME_SETTLE: Duration = Duration::from_millis(400);
/// Width of the participants panel.
pub const PANEL_WIDTH: f32 = 320.0;

/// A call control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallMessage {
    /// Join (or start) the call of a room: the header's call button.
    Join(RoomId),
    /// Join (or start) the call of a room as a video call: the camera comes
    /// on once connected (the header's **Video** button).
    JoinWithVideo(RoomId),
    /// Leave the current call.
    Leave,
    /// Mute or unmute the microphone (**Mute**, Ctrl+Shift+M). Unmuting
    /// while deafened also undeafens, as in Discord.
    ToggleMute,
    /// Deafen or undeafen (**Deafen**, Ctrl+Shift+D).
    ToggleDeafen,
    /// Open or close the participants panel (the header's **People**).
    TogglePanel,
    /// Open the call's room with the participants panel (the call bar's
    /// room name).
    ShowCall,
    /// Close the call notice.
    DismissNotice,
    /// A volume slider moved.
    Volume {
        /// Whose voice.
        user_id: UserId,
        /// The slider's value, percent.
        percent: u16,
    },
    /// The volume slider was let go: send its value.
    VolumeReleased,
    /// The pause after a slider move ended; stale numbers do nothing.
    VolumeDue(u64),
    /// **−** or **+** beside a volume slider.
    VolumeStep {
        /// Whose voice.
        user_id: UserId,
        /// **+**.
        up: bool,
    },
    /// The voice cleaning menu of a participant.
    Cleaning {
        /// Whose voice.
        user_id: UserId,
        /// The choice.
        choice: CleaningChoice,
    },
    /// **Accept** on the incoming-call card: join that room's call.
    Accept(RoomId),
    /// **Decline** on the incoming-call card.
    Decline(RoomId),
    /// Esc on the incoming-call card: hide the cards of every ring sounding
    /// now and stop the ringtone; the rooms still ring (their pills, the
    /// header's button).
    HideRing,
}

/// A command sent and not answered yet: a second press sends nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// `JoinCall` for this room, until its first call state.
    Join(RoomId),
    /// `LeaveCall`, until `Left`.
    Leave,
}

/// The header's call button: its label, and what a press does (`None`
/// draws it off).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallButton {
    /// The words on the button.
    pub label: String,
    /// The message of a press; `None` while nothing can be done.
    pub press: Option<CallMessage>,
}

/// The call pill of a room list row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallPill {
    /// Devices in the room's call (0 while only a ring is known).
    pub count: u32,
    /// The room rings (or notifies of a call) for us.
    pub ringing: bool,
}

impl CallPill {
    /// The pill's words: the speaker and the count.
    pub fn label(self) -> String {
        if self.count == 0 {
            CALL_MARK.to_owned()
        } else {
            format!("{CALL_MARK} {}", self.count)
        }
    }
}

/// The per-person voice cleaning menu: [`VoiceCleaning`] with words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleaningChoice {
    /// As the global switch says.
    Automatic,
    /// Always clean this voice.
    Always,
    /// Never clean this voice.
    Never,
}

impl CleaningChoice {
    /// The menu's choices, in order.
    pub const ALL: [Self; 3] = [Self::Automatic, Self::Always, Self::Never];

    /// The core's setting.
    pub fn cleaning(self) -> VoiceCleaning {
        match self {
            Self::Automatic => VoiceCleaning::Automatic,
            Self::Always => VoiceCleaning::Always,
            Self::Never => VoiceCleaning::Never,
        }
    }

    /// The choice for the core's setting.
    pub fn from_cleaning(cleaning: VoiceCleaning) -> Self {
        match cleaning {
            VoiceCleaning::Automatic => Self::Automatic,
            VoiceCleaning::Always => Self::Always,
            VoiceCleaning::Never => Self::Never,
        }
    }
}

impl fmt::Display for CleaningChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Automatic => "Clean: automatic",
            Self::Always => "Clean: always",
            Self::Never => "Clean: never",
        })
    }
}

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

impl CallPhase {
    /// The call bar's line about the phase.
    pub fn label(self) -> String {
        match self {
            Self::Joining => "Connecting\u{2026}".to_owned(),
            Self::Connected { encrypted: true } => "Voice connected, encrypted".to_owned(),
            Self::Connected { encrypted: false } => "Voice connected".to_owned(),
            Self::Reconnecting { attempt } => format!("Reconnecting (attempt {attempt})\u{2026}"),
        }
    }
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
    /// The microphone is muted (the core resets it on join).
    pub muted: bool,
    /// Nothing plays and the microphone is off (the core resets it on join).
    pub deafened: bool,
    /// The call room's members, for names and pictures; `None` until the
    /// core answered `ListRoomMembers`.
    pub members: Option<Vec<RoomMemberView>>,
    /// Participants not in the member list when it was last asked for, so
    /// one newcomer asks once.
    pub members_asked: Vec<UserId>,
}

impl CurrentCall {
    pub(crate) fn new(room_id: RoomId, phase: CallPhase) -> Self {
        Self {
            room_id,
            phase,
            participants: Vec::new(),
            declined: Vec::new(),
            muted: false,
            deafened: false,
            members: None,
            members_asked: Vec::new(),
        }
    }

    /// The member entry of `user`, once the member list is known.
    pub fn member(&self, user: &UserId) -> Option<&RoomMemberView> {
        self.members.as_ref()?.iter().find(|m| m.user_id == *user)
    }

    /// Participants whose user the member list lacks and who were not asked
    /// for yet. Empty while the list is on its way.
    fn unknown_users(&self) -> Vec<UserId> {
        if self.members.is_none() {
            return Vec::new();
        }
        let mut unknown: Vec<UserId> = Vec::new();
        for p in &self.participants {
            if self.member(&p.user_id).is_none()
                && !self.members_asked.contains(&p.user_id)
                && !unknown.contains(&p.user_id)
            {
                unknown.push(p.user_id.clone());
            }
        }
        unknown
    }
}

// Counts only: no room, user or display name in logs.
impl fmt::Debug for CurrentCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CurrentCall")
            .field("phase", &self.phase)
            .field("participants", &self.participants.len())
            .field("declined", &self.declined.len())
            .field("muted", &self.muted)
            .field("deafened", &self.deafened)
            .field("members", &self.members.as_ref().map(Vec::len))
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
    /// Esc hid its card.
    pub hidden: bool,
    /// Accept or Decline was pressed; its buttons are off until the core
    /// answers (or refuses).
    pub answered: bool,
    /// Counts the calls that rang this room: a new call in the same room is
    /// a new ring, whose card arms again.
    pub epoch: u64,
}

/// How long a card that just showed (or changed to another ring) ignores
/// answers: the clicks and keys already on their way when it came up were
/// meant for what was under it.
pub const RING_ARMING: Duration = Duration::from_millis(600);

/// The card's arming: which ring it is for and whether the wait is over.
/// View state only; nothing of it is sent to the core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingArm {
    /// The ring the card shows.
    pub room_id: RoomId,
    /// That ring's [`Ring::epoch`].
    pub epoch: u64,
    /// Tells this wait's timer from an earlier one's.
    pub seq: u64,
    /// The wait is over: the card answers.
    pub live: bool,
}

/// One row of the participants panel, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticipantRow {
    /// Account.
    pub user_id: UserId,
    /// The name to show: the call's, the member list's, else the user ID;
    /// one line, capped.
    pub name: String,
    /// The member's picture, when known.
    pub avatar: Option<twicecat_core::api::AvatarId>,
    /// This device.
    pub is_local: bool,
    /// Talking now.
    pub speaking: bool,
    /// Their microphone is off (for this device, also while deafened).
    pub muted: bool,
    /// How this device cleans their voice, when there is something to say.
    pub cleaning: Option<&'static str>,
    /// The volume and cleaning controls: on the first row of each other
    /// person (the core's settings are per person, not per device).
    pub controls: Option<UserControls>,
}

/// The controls of a participant row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UserControls {
    /// Playback volume, percent.
    pub volume: u16,
    /// The cleaning choice.
    pub cleaning: CleaningChoice,
}

/// What the call bar says about a cleaning status; nothing when off.
pub fn cleaning_label(status: CleaningStatus) -> Option<&'static str> {
    match status {
        CleaningStatus::Off => None,
        CleaningStatus::Starting => Some("Voice cleaning starts\u{2026}"),
        CleaningStatus::Cleaned => Some("Voice cleaned"),
        CleaningStatus::Skipped(SkipReason::Never) => Some("Not cleaned (your choice)"),
        CleaningStatus::Skipped(SkipReason::Limit) => Some("Not cleaned (too many voices)"),
        CleaningStatus::Skipped(SkipReason::Unavailable) => Some("Not cleaned (unavailable)"),
        CleaningStatus::Skipped(SkipReason::AlreadyClean) => {
            Some("Not cleaned (cleaned on their side)")
        }
    }
}

/// The call and audio state the screens show.
#[derive(Default)]
pub struct CallModel {
    /// The call this device is in.
    pub call: Option<CurrentCall>,
    /// A join or leave sent and not answered yet.
    pub pending: Option<Pending>,
    /// The room whose call was joined with **Video**: its camera starts when
    /// the call connects.
    pub camera_on_connect: Option<RoomId>,
    /// Devices in each room's call, for the room list; rooms without a call
    /// are not kept.
    pub counts: HashMap<RoomId, u32>,
    /// Rooms that ring, oldest first, one entry per room; at most
    /// [`MAX_RINGS`].
    pub rings: Vec<Ring>,
    /// A non-secret line about the last call failure: why the call ended, or
    /// a refused call or audio command.
    pub notice: Option<String>,
    /// The participants panel is open (it shows while the call's room is).
    pub panel_open: bool,
    /// A volume slider being moved: whose, and its value, until it is sent.
    pub volume_drag: Option<(UserId, u16)>,
    /// Counts slider moves, so only the last pause sends.
    pub volume_seq: u64,
    /// The people whose row in the participants panel shows its volume and
    /// cleaning controls (the **More** button). View state only: it is not
    /// sent anywhere and ends with the call.
    pub rows_open: HashSet<UserId>,
    /// The arming of the card now showing, if one shows. View state only.
    pub arm: Option<RingArm>,
    /// Counts armings, so a timer of an earlier wait does nothing.
    pub arm_counter: u64,
    /// Counts incoming calls, for [`Ring::epoch`].
    pub ring_epoch: u64,
    /// Ctrl+Shift+A or Ctrl+Shift+X went down and has not come up: its
    /// auto-repeats answer nothing, whichever card they find. The composer
    /// forwards repeats the window's key events could tell apart.
    pub chord_held: bool,
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
    /// The call's video: pictures, our camera and share, the video settings.
    pub video: crate::video::VideoModel,
}

// Counts only: no room, user or display name in logs.
impl fmt::Debug for CallModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallModel")
            .field("call", &self.call)
            .field("pending", &self.pending.is_some())
            .field("counts", &self.counts.len())
            .field("rings", &self.rings.len())
            .field("notice", &self.notice.is_some())
            .field("panel_open", &self.panel_open)
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

    /// Whether the participants panel shows beside `selected`.
    pub fn panel_shown(&self, selected: Option<&RoomId>) -> bool {
        self.panel_open && selected.is_some_and(|room| self.call_in(room).is_some())
    }

    /// The header's call button for `room`. A space or an invitation has
    /// none. Voice rooms (MSC3417) say "Join voice", other rooms "Call".
    pub fn button(&self, room: &RoomSummary) -> Option<CallButton> {
        if room.is_space || room.is_invite {
            return None;
        }
        let id = &room.id;
        let busy = self.pending.is_some();
        if let Some(call) = self.call_in(id) {
            let label = if call.phase == CallPhase::Joining {
                "Joining\u{2026}"
            } else {
                "Leave call"
            };
            let press = (call.phase != CallPhase::Joining && !busy).then_some(CallMessage::Leave);
            return Some(CallButton {
                label: label.to_owned(),
                press,
            });
        }
        let count = self.count(id);
        let label = match (room.is_call_room, count) {
            (true, 0) => "Join voice".to_owned(),
            (true, n) => format!("Join voice ({n})"),
            (false, 0) => "Call".to_owned(),
            (false, n) => format!("Join call ({n})"),
        };
        Some(CallButton {
            label,
            press: (!busy).then(|| CallMessage::Join(id.clone())),
        })
    }

    /// The header's **Video** button: a call with the camera on, offered where
    /// **Call** is (a room with no call of ours and no voice room).
    pub fn video_button(&self, room: &RoomSummary) -> Option<CallButton> {
        if room.is_space || room.is_invite || room.is_call_room || self.call_in(&room.id).is_some()
        {
            return None;
        }
        Some(CallButton {
            label: "Video".to_owned(),
            press: self
                .pending
                .is_none()
                .then(|| CallMessage::JoinWithVideo(room.id.clone())),
        })
    }

    /// The ring whose card shows: the oldest DM ring not hidden, unless we
    /// are joining or in that room's call (the ring stays until the core
    /// stops it, so a failed join shows the card again).
    pub fn card(&self) -> Option<&Ring> {
        self.rings
            .iter()
            .find(|r| r.ring && !r.hidden && self.call_in(&r.room_id).is_none())
    }

    /// Whether the card now showing is still waiting to answer.
    pub fn arming(&self) -> bool {
        self.card().is_some() && self.live_card().is_none()
    }

    /// The card now showing, once its wait is over.
    pub fn live_card(&self) -> Option<&Ring> {
        let ring = self.card()?;
        self.arm
            .as_ref()
            .filter(|a| a.live && a.room_id == ring.room_id && a.epoch == ring.epoch)
            .map(|_| ring)
    }

    /// The sequence number of the wait now running (what its timer carries).
    pub fn arm_seq(&self) -> u64 {
        self.arm.as_ref().map_or(0, |a| a.seq)
    }

    /// Whether a ring chord is down.
    pub fn chord_held(&self) -> bool {
        self.chord_held
    }

    /// Starts a wait when the card shows a ring it was not waiting for, and
    /// forgets it when no card shows. Returns the sequence to time, if one
    /// started.
    pub fn sync_arming(&mut self) -> Option<u64> {
        let Some(ring) = self.card() else {
            self.arm = None;
            return None;
        };
        if self
            .arm
            .as_ref()
            .is_some_and(|a| a.room_id == ring.room_id && a.epoch == ring.epoch)
        {
            return None;
        }
        let (room_id, epoch) = (ring.room_id.clone(), ring.epoch);
        self.arm_counter = self.arm_counter.wrapping_add(1);
        self.arm = Some(RingArm {
            room_id,
            epoch,
            seq: self.arm_counter,
            live: false,
        });
        Some(self.arm_counter)
    }

    /// The timer of wait `seq` ended: the card answers if it is still the
    /// one the wait was for.
    pub fn arm_ended(&mut self, seq: u64) {
        if let Some(arm) = self.arm.as_mut().filter(|a| a.seq == seq) {
            arm.live = true;
        }
    }

    /// The call pill of `room`'s row: when its call has devices or it rings.
    pub fn pill(&self, room: &RoomId) -> Option<CallPill> {
        let count = self.count(room);
        let ringing = self.ring(room).is_some();
        (count > 0 || ringing).then_some(CallPill { count, ringing })
    }

    /// Whether `user`'s row shows its controls.
    pub fn row_open(&self, user: &UserId) -> bool {
        self.rows_open.contains(user)
    }

    /// **More** on a row: shows or hides its controls. Only someone else in
    /// the call has controls, so any other user is ignored.
    pub fn toggle_row(&mut self, user: &UserId) {
        if self.rows_open.remove(user) {
            return;
        }
        let has_controls = self.call.as_ref().is_some_and(|call| {
            call.participants
                .iter()
                .any(|p| p.user_id == *user && !p.is_local)
        });
        if has_controls {
            self.rows_open.insert(user.clone());
        }
    }

    /// The volume a person's slider shows: the value being moved, else the
    /// core's.
    pub fn shown_volume(&self, user: &UserId, reported: u16) -> u16 {
        match &self.volume_drag {
            Some((dragged, percent)) if dragged == user => *percent,
            _ => reported,
        }
    }

    /// The participants panel's rows, oldest membership first.
    pub fn rows(&self) -> Vec<ParticipantRow> {
        let Some(call) = &self.call else {
            return Vec::new();
        };
        let mut with_controls: Vec<&UserId> = Vec::new();
        call.participants
            .iter()
            .map(|p| {
                let member = call.member(&p.user_id);
                let controls = (!p.is_local && !with_controls.contains(&&p.user_id)).then(|| {
                    with_controls.push(&p.user_id);
                    let volume = self
                        .settings
                        .as_ref()
                        .map_or(p.volume_percent, |s| s.user_volume(&p.user_id));
                    UserControls {
                        volume: self.shown_volume(&p.user_id, volume),
                        cleaning: CleaningChoice::from_cleaning(
                            self.settings
                                .as_ref()
                                .map(|s| s.user_cleaning(&p.user_id))
                                .unwrap_or_default(),
                        ),
                    }
                });
                ParticipantRow {
                    user_id: p.user_id.clone(),
                    name: call.name_of(p),
                    avatar: member.and_then(|m| m.avatar.clone()),
                    is_local: p.is_local,
                    speaking: p.speaking,
                    muted: p.muted || (p.is_local && (call.muted || call.deafened)),
                    cleaning: cleaning_label(p.cleaning),
                    controls,
                }
            })
            .collect()
    }

    /// Applies a call state change.
    pub fn on_state(&mut self, state: CallState) {
        // Any call state answers the join or leave sent.
        self.pending = None;
        match state {
            CallState::Idle => self.end_call(),
            CallState::Joining { room_id } => {
                // The room's ring stays until the core says it stopped (a
                // join that fails keeps it ringing); its card hides meanwhile.
                self.notice = None;
                self.volume_drag = None;
                self.rows_open.clear();
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
                    self.end_call();
                }
                // A ring answered by a join that failed can be answered again.
                for ring in self.rings.iter_mut().filter(|r| r.room_id == room_id) {
                    ring.answered = false;
                }
                if let CallEndReason::Failed(why) = reason {
                    self.notice = Some(notice_line(&format!("The call ended: {why}")));
                }
            }
        }
    }

    fn end_call(&mut self) {
        self.camera_on_connect = None;
        self.video.end_call();
        self.call = None;
        self.volume_drag = None;
        self.rows_open.clear();
        self.panel_open = false;
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
                // Someone who left takes their open row with them.
                if let Some(call) = self.call.as_ref().filter(|c| c.room_id == room_id) {
                    self.rows_open
                        .retain(|u| call.participants.iter().any(|p| p.user_id == *u));
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
            video @ (CoreEvent::VideoDevices(_)
            | CoreEvent::ScreenSources(_)
            | CoreEvent::VideoSettings(_)
            | CoreEvent::VideoTracks { .. }
            | CoreEvent::VideoFeed { .. }
            | CoreEvent::CameraPreview(_)
            | CoreEvent::LocalVideo(_)
            | CoreEvent::VideoEncoders(_)
            | CoreEvent::VideoStats(_)) => return self.video.on_event(video),
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
        self.ring_epoch = self.ring_epoch.wrapping_add(1);
        let epoch = self.ring_epoch;
        if let Some(known) = self.rings.iter_mut().find(|r| r.room_id == room_id) {
            // A new call in the same room: its card shows again.
            *known = Ring {
                room_id,
                caller,
                ring,
                hidden: false,
                answered: false,
                epoch,
            };
            return;
        }
        if self.rings.len() >= MAX_RINGS {
            self.rings.remove(0);
        }
        self.rings.push(Ring {
            room_id,
            caller,
            ring,
            hidden: false,
            answered: false,
            epoch,
        });
    }

    /// Shows a refused call or audio command.
    pub fn on_error(&mut self, error: &CoreError) {
        // A refused leave (`NotInCall`) or join is answered, and so is a
        // refused decline: the card's buttons work again.
        self.pending = None;
        self.camera_on_connect = None;
        self.video.on_error(error);
        for ring in &mut self.rings {
            ring.answered = false;
        }
        self.notice = Some(notice_line(&error_text(error)));
    }
}

/// Whether `error` is about calls or audio (`Call`, `Audio`, `NotInCall`).
pub fn is_call_error(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Call(_)
            | CoreError::Audio(_)
            | CoreError::NotInCall
            | CoreError::NotSupported(_)
    )
}

/// The line shown for a call or audio error. The core's descriptions are
/// non-secret.
pub fn error_text(error: &CoreError) -> String {
    match error {
        CoreError::Call(why) => format!("Call: {why}"),
        CoreError::Audio(why) => format!("Audio: {why}"),
        CoreError::NotInCall => "You are not in a call.".to_owned(),
        CoreError::NotSupported(what) => {
            format!("This version of Twicecat cannot use {what} yet.")
        }
        other => other.to_string(),
    }
}

/// `text` on one line, capped at [`MAX_NOTICE_CHARS`].
fn notice_line(text: &str) -> String {
    one_line_capped(text, MAX_NOTICE_CHARS)
}

impl App {
    /// Whether `room` is the room of the card now showing, once its wait is
    /// over: the only ring a button or chord may answer.
    fn is_live_card(&self, room: &RoomId) -> bool {
        self.calls
            .live_card()
            .is_some_and(|ring| ring.room_id == *room)
    }

    /// A call control.
    pub(crate) fn on_call(&mut self, message: CallMessage) -> Task<Message> {
        match message {
            CallMessage::JoinWithVideo(room_id) => {
                let before = self.calls.pending.clone();
                self.calls.camera_on_connect = Some(room_id.clone());
                let task = self.on_call(CallMessage::Join(room_id));
                // Only a join that was really sent asks for the camera.
                if self.calls.pending == before {
                    self.calls.camera_on_connect = None;
                }
                task
            }
            CallMessage::Join(room_id) => {
                // The room list may be filtered (a search, a tab), so a room
                // missing from it is still joined; the core answers for an
                // unknown one. Only an invitation is refused here.
                if self.calls.pending.is_some()
                    || self.calls.call_in(&room_id).is_some()
                    || self.rooms.iter().any(|r| r.id == room_id && r.is_invite)
                {
                    return Task::none();
                }
                self.calls.pending = Some(Pending::Join(room_id.clone()));
                let intent = if self.calls.camera_on_connect.as_ref() == Some(&room_id) {
                    CallIntent::Video
                } else {
                    CallIntent::Audio
                };
                self.send(Command::JoinCall { room_id, intent })
            }
            CallMessage::Leave => {
                if self.calls.pending.is_some() || self.calls.call.is_none() {
                    return Task::none();
                }
                self.calls.pending = Some(Pending::Leave);
                self.send(Command::LeaveCall)
            }
            CallMessage::ToggleMute => {
                let Some(call) = self.calls.call.as_mut() else {
                    return Task::none();
                };
                if call.deafened {
                    call.deafened = false;
                    call.muted = false;
                    Task::batch([
                        self.send(Command::SetDeafened(false)),
                        self.send(Command::SetMicMuted(false)),
                    ])
                } else {
                    call.muted = !call.muted;
                    let muted = call.muted;
                    self.send(Command::SetMicMuted(muted))
                }
            }
            CallMessage::ToggleDeafen => {
                let Some(call) = self.calls.call.as_mut() else {
                    return Task::none();
                };
                call.deafened = !call.deafened;
                let deafened = call.deafened;
                self.send(Command::SetDeafened(deafened))
            }
            CallMessage::TogglePanel => {
                if self.calls.call.is_some() {
                    self.calls.panel_open = !self.calls.panel_open;
                }
                Task::none()
            }
            CallMessage::ShowCall => {
                let Some(room) = self.calls.call.as_ref().map(|c| c.room_id.clone()) else {
                    return Task::none();
                };
                self.calls.panel_open = true;
                if self.selected.as_ref() == Some(&room) {
                    Task::none()
                } else {
                    self.update(Message::SelectRoom(room))
                }
            }
            CallMessage::DismissNotice => {
                self.calls.notice = None;
                Task::none()
            }
            CallMessage::Volume { user_id, percent } => {
                if self.calls.call.is_none() {
                    return Task::none();
                }
                // Another person's slider moving first sends the last one's
                // value, which has no release when moved by the wheel.
                let earlier = match &self.calls.volume_drag {
                    Some((other, _)) if *other != user_id => self.send_dragged_volume(),
                    _ => Task::none(),
                };
                self.calls.volume_drag = Some((user_id, percent.min(MAX_VOLUME)));
                self.calls.volume_seq = self.calls.volume_seq.wrapping_add(1);
                let seq = self.calls.volume_seq;
                Task::batch([
                    earlier,
                    self.timer(VOLUME_SETTLE, Message::Call(CallMessage::VolumeDue(seq))),
                ])
            }
            CallMessage::VolumeReleased => self.send_dragged_volume(),
            CallMessage::VolumeDue(seq) if seq == self.calls.volume_seq => {
                self.send_dragged_volume()
            }
            CallMessage::VolumeDue(_) => Task::none(),
            CallMessage::VolumeStep { user_id, up } => {
                let Some(current) = self.current_volume(&user_id) else {
                    return Task::none();
                };
                let now = self.calls.shown_volume(&user_id, current);
                let next = if up {
                    now.saturating_add(VOLUME_STEP).min(MAX_VOLUME)
                } else {
                    now.saturating_sub(VOLUME_STEP)
                };
                self.calls.volume_drag = None;
                if next == current {
                    return Task::none();
                }
                self.set_user_volume(user_id, next)
            }
            CallMessage::Accept(room_id) => {
                // Only the card on screen, once it has armed: a click aimed
                // at an earlier card must not answer a new call in the same
                // room (the view hides the buttons' press while arming; this
                // is the handler's own guard).
                if !self.is_live_card(&room_id)
                    || self.calls.ring(&room_id).is_none_or(|r| r.answered)
                {
                    return Task::none();
                }
                let join = self.on_call(CallMessage::Join(room_id.clone()));
                if self.calls.pending == Some(Pending::Join(room_id.clone()))
                    && let Some(ring) = self.calls.rings.iter_mut().find(|r| r.room_id == room_id)
                {
                    ring.answered = true;
                }
                join
            }
            CallMessage::Decline(room_id) => {
                if !self.is_live_card(&room_id) {
                    return Task::none();
                }
                let Some(ring) = self
                    .calls
                    .rings
                    .iter_mut()
                    .find(|r| r.room_id == room_id && !r.answered)
                else {
                    return Task::none();
                };
                ring.answered = true;
                self.send(Command::DeclineCall { room_id })
            }
            CallMessage::HideRing => {
                if self.calls.card().is_none() {
                    return Task::none();
                }
                // The core silences every ring sounding now, so every card
                // hides; a later ring shows its card and sounds again.
                for ring in self.calls.rings.iter_mut().filter(|r| r.ring) {
                    ring.hidden = true;
                }
                self.send(Command::SilenceRinging)
            }
            CallMessage::Cleaning { user_id, choice } => {
                let now = self
                    .calls
                    .settings
                    .as_ref()
                    .map(|s| s.user_cleaning(&user_id))
                    .unwrap_or_default();
                if self.calls.call.is_none() || now == choice.cleaning() {
                    return Task::none();
                }
                // Kept here at once, so an edit made before the core's echo
                // builds on it (the echo replaces it).
                if let Some(settings) = self.calls.settings.as_mut() {
                    match choice.cleaning() {
                        VoiceCleaning::Automatic => settings.user_cleaning.remove(&user_id),
                        other => settings.user_cleaning.insert(user_id.clone(), other),
                    };
                }
                self.send(Command::SetUserCleaning {
                    user_id,
                    cleaning: choice.cleaning(),
                })
            }
        }
    }

    /// The volume the core reports for `user` in the call, if they are in it.
    fn reported_volume(&self, user: &UserId) -> Option<u16> {
        self.calls
            .call
            .as_ref()?
            .participants
            .iter()
            .find(|p| p.user_id == *user && !p.is_local)
            .map(|p| p.volume_percent)
    }

    /// Sends the moved slider's value, once.
    fn send_dragged_volume(&mut self) -> Task<Message> {
        let Some((user_id, percent)) = self.calls.volume_drag.take() else {
            return Task::none();
        };
        if self.current_volume(&user_id).is_none_or(|v| v == percent) {
            return Task::none();
        }
        self.set_user_volume(user_id, percent)
    }

    /// The volume of `user`, someone else in the call: the settings' (which
    /// already hold a change sent before its echo), else what the core
    /// reports.
    fn current_volume(&self, user: &UserId) -> Option<u16> {
        let reported = self.reported_volume(user)?;
        Some(
            self.calls
                .settings
                .as_ref()
                .map_or(reported, |s| s.user_volume(user)),
        )
    }

    /// Sends a volume, and keeps it in the settings at once, so the next
    /// change (a second **+**, the Voice & audio card) builds on it.
    fn set_user_volume(&mut self, user_id: UserId, percent: u16) -> Task<Message> {
        if let Some(settings) = self.calls.settings.as_mut() {
            if percent == 100 {
                settings.user_volumes.remove(&user_id);
            } else {
                settings.user_volumes.insert(user_id.clone(), percent);
            }
        }
        // The app sets voices only for now; screen-share sound gets its own
        // control with the call stage (Phase 9 Task 10).
        self.send(Command::SetUserVolume {
            user_id,
            source: SoundSource::Voice,
            percent,
        })
    }

    /// A call or audio event from the core.
    pub(crate) fn on_call_event(&mut self, event: CoreEvent) -> Task<Message> {
        let joining = match &event {
            CoreEvent::CallState(CallState::Joining { room_id }) => Some(room_id.clone()),
            _ => None,
        };
        let connected = match &event {
            CoreEvent::CallState(CallState::Connected { room_id, .. })
                if self.calls.camera_on_connect.as_ref() == Some(room_id) =>
            {
                Some(room_id.clone())
            }
            _ => None,
        };
        let participants = matches!(event, CoreEvent::CallParticipants { .. });
        let incoming = match &event {
            CoreEvent::IncomingCall { room_id, .. } if self.calls.ring(room_id).is_none() => {
                Some(room_id.clone())
            }
            _ => None,
        };
        let handled = self.calls.on_event(event);
        debug_assert!(handled, "on_call_event got another kind of event");
        if connected.is_some() {
            // Joined with **Video**: the camera comes on, once.
            self.calls.camera_on_connect = None;
            return self.update(Message::Video(crate::video::VideoMessage::ToggleCamera));
        }
        if let Some(room) = joining {
            // A voice room is the call: its people show at once (plan Q4).
            if self.rooms.iter().any(|r| r.id == room && r.is_call_room) {
                self.calls.panel_open = true;
            }
            return self.send(Command::ListRoomMembers { room_id: room });
        }
        if participants {
            return self.call_people_changed();
        }
        if let Some(room) = incoming {
            return self.announce_ring(&room);
        }
        Task::none()
    }

    /// A new ring: the window asks for attention (until focused for a DM
    /// ring), and a desktop notification shows while the window is not
    /// focused. A ring the model ignored (we are in that call) does nothing.
    fn announce_ring(&mut self, room: &RoomId) -> Task<Message> {
        let Some(ring) = self.calls.ring(room).cloned() else {
            return Task::none();
        };
        let level = if ring.ring {
            iced::window::UserAttention::Critical
        } else {
            iced::window::UserAttention::Informational
        };
        let room_name = self
            .rooms
            .iter()
            .find(|r| r.id == *room)
            .map(|r| r.name.as_str());
        let toast = crate::notifications::call_toast(
            room_name,
            &ring.caller,
            ring.ring,
            self.ui_settings.show_message_content,
        );
        let shown = self.notify(crate::notifications::Request {
            room_id: room.clone(),
            toast,
            replace: None,
        });
        Task::batch([crate::state::attention_at(level), shown])
    }

    /// Asks again for the member list when someone it lacks joined, and for
    /// the pictures of the people in the call.
    fn call_people_changed(&mut self) -> Task<Message> {
        let Some(call) = self.calls.call.as_mut() else {
            return Task::none();
        };
        let unknown = call.unknown_users();
        let ask = if unknown.is_empty() {
            Task::none()
        } else {
            call.members_asked.extend(unknown);
            let room_id = call.room_id.clone();
            self.send(Command::ListRoomMembers { room_id })
        };
        Task::batch([ask, self.fetch_call_avatars()])
    }

    fn fetch_call_avatars(&mut self) -> Task<Message> {
        let Some(call) = &self.calls.call else {
            return Task::none();
        };
        let ids: Vec<_> = call
            .participants
            .iter()
            .filter_map(|p| call.member(&p.user_id)?.avatar.clone())
            .collect();
        let ids = self.avatars.take_unrequested(ids);
        if ids.is_empty() {
            Task::none()
        } else {
            self.send(Command::FetchAvatars(ids))
        }
    }

    /// `RoomMembers` for the call's room: kept for the participants panel.
    pub(crate) fn on_call_members(
        &mut self,
        room: &RoomId,
        members: &[RoomMemberView],
    ) -> Task<Message> {
        let Some(call) = self.calls.call_in_mut(room) else {
            return Task::none();
        };
        call.members = Some(members.to_vec());
        self.fetch_call_avatars()
    }

    /// After a login: the audio settings, so the call controls show the
    /// stored choices.
    pub(crate) fn calls_after_login(&mut self) -> Task<Message> {
        self.calls = CallModel::default();
        Task::batch([
            self.send(Command::GetAudioSettings),
            self.video_after_login(),
        ])
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
            share_volume_percent: 100,
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

    /// The arming timer of the card now showing ends.
    fn arm(app: &mut App) {
        let seq = app.calls.arm_seq();
        let _ = app.update(Message::RingArmed(seq));
    }

    fn chord_up(app: &mut App) {
        let _ = app.update(Message::RingChordReleased);
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
    fn joining_a_ringing_room_hides_its_card_until_the_ring_stops() {
        let (mut app, _rx) = main_app();
        incoming(&mut app, "a", "b", true);
        incoming(&mut app, "b", "c", true);
        joining(&mut app, "a");
        assert!(app.calls.ring(&room("a")).is_some(), "the core stops it");
        assert_eq!(app.calls.card().map(|r| r.room_id.clone()), Some(room("b")));
        let _ = app.update(core(CoreEvent::RingingStopped { room_id: room("a") }));
        assert!(app.calls.ring(&room("a")).is_none());
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

    fn summary(n: &str) -> RoomSummary {
        RoomSummary::new(room(n), n.to_owned())
    }

    fn button_of(app: &App, room: &RoomSummary) -> Option<(String, Option<CallMessage>)> {
        app.calls.button(room).map(|b| (b.label, b.press))
    }

    fn call(app: &mut App, message: CallMessage) {
        let _ = app.update(Message::Call(message));
    }

    #[test]
    fn the_header_button_follows_the_room_and_the_call() {
        let (mut app, _rx) = main_app();
        let a = summary("a");
        assert_eq!(
            button_of(&app, &a),
            Some(("Call".into(), Some(CallMessage::Join(room("a")))))
        );
        let _ = app.update(core(CoreEvent::ActiveCall {
            room_id: room("a"),
            participants: 3,
        }));
        assert_eq!(
            button_of(&app, &a),
            Some(("Join call (3)".into(), Some(CallMessage::Join(room("a")))))
        );
        joining(&mut app, "a");
        assert_eq!(button_of(&app, &a), Some(("Joining\u{2026}".into(), None)));
        state(
            &mut app,
            CallState::Connected {
                room_id: room("a"),
                encrypted: false,
            },
        );
        assert_eq!(
            button_of(&app, &a),
            Some(("Leave call".into(), Some(CallMessage::Leave)))
        );
        state(
            &mut app,
            CallState::Reconnecting {
                room_id: room("a"),
                attempt: 1,
            },
        );
        assert_eq!(
            button_of(&app, &a),
            Some(("Leave call".into(), Some(CallMessage::Leave)))
        );
        // Another room offers a join, which moves the call there.
        assert_eq!(
            button_of(&app, &summary("b")),
            Some(("Call".into(), Some(CallMessage::Join(room("b")))))
        );
    }

    #[test]
    fn voice_rooms_offer_join_voice() {
        let (mut app, _rx) = main_app();
        let mut voice = summary("a");
        voice.is_call_room = true;
        assert_eq!(
            button_of(&app, &voice).map(|b| b.0),
            Some("Join voice".into())
        );
        let _ = app.update(core(CoreEvent::ActiveCall {
            room_id: room("a"),
            participants: 2,
        }));
        assert_eq!(
            button_of(&app, &voice).map(|b| b.0),
            Some("Join voice (2)".into())
        );
    }

    #[test]
    fn spaces_and_invitations_have_no_call_button() {
        let (app, _rx) = main_app();
        let mut space = summary("s");
        space.is_space = true;
        let mut invite = summary("i");
        invite.is_invite = true;
        assert_eq!(app.calls.button(&space), None);
        assert_eq!(app.calls.button(&invite), None);
    }

    #[test]
    fn a_join_sends_one_command_until_the_core_answers() {
        let (mut app, mut rx) = main_app();
        call(&mut app, CallMessage::Join(room("a")));
        call(&mut app, CallMessage::Join(room("a")));
        call(&mut app, CallMessage::Join(room("b")));
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert!(matches!(
            &sent[0],
            Command::JoinCall { room_id, intent: CallIntent::Audio } if *room_id == room("a")
        ));
        assert_eq!(
            button_of(&app, &summary("b")).and_then(|b| b.1),
            None,
            "off while pending"
        );
        joining(&mut app, "a");
        assert!(app.calls.pending.is_none());
    }

    #[test]
    fn a_refused_join_can_be_tried_again() {
        let (mut app, mut rx) = main_app();
        call(&mut app, CallMessage::Join(room("a")));
        let _ = app.update(core(CoreEvent::Error(CoreError::Call(
            "no call service".into(),
        ))));
        call(&mut app, CallMessage::Join(room("a")));
        assert_eq!(drain(&mut rx).len(), 2);
    }

    #[test]
    fn joining_an_invitation_or_the_current_call_sends_nothing() {
        let (mut app, mut rx) = main_app();
        let mut invite = summary("i");
        invite.is_invite = true;
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
            value: invite,
        }])));
        drain(&mut rx);
        call(&mut app, CallMessage::Join(room("i")));
        assert!(drain(&mut rx).is_empty());
        joining(&mut app, "a");
        drain(&mut rx);
        call(&mut app, CallMessage::Join(room("a")));
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn a_room_missing_from_the_filtered_list_is_still_joined() {
        let (mut app, mut rx) = main_app();
        call(&mut app, CallMessage::Join(room("hidden")));
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::JoinCall { room_id, .. }] if *room_id == room("hidden")),
            "{sent:?}"
        );
    }

    #[test]
    fn a_leave_sends_one_command_until_the_call_is_left() {
        let (mut app, mut rx) = main_app();
        call(&mut app, CallMessage::Leave);
        assert!(drain(&mut rx).is_empty(), "no call, nothing to leave");
        state(
            &mut app,
            CallState::Connected {
                room_id: room("a"),
                encrypted: false,
            },
        );
        call(&mut app, CallMessage::Leave);
        call(&mut app, CallMessage::Leave);
        let sent = drain(&mut rx);
        assert!(matches!(sent.as_slice(), [Command::LeaveCall]), "{sent:?}");
        assert_eq!(
            button_of(&app, &summary("a")),
            Some(("Leave call".into(), None))
        );
        state(
            &mut app,
            CallState::Left {
                room_id: room("a"),
                reason: CallEndReason::Hangup,
            },
        );
        assert!(app.calls.pending.is_none());
        assert_eq!(
            button_of(&app, &summary("a")).map(|b| b.0),
            Some("Call".into())
        );
    }

    #[test]
    fn the_row_pill_shows_the_count_and_the_ring() {
        let (mut app, _rx) = main_app();
        assert_eq!(app.calls.pill(&room("a")), None);
        let _ = app.update(core(CoreEvent::ActiveCall {
            room_id: room("a"),
            participants: 2,
        }));
        let pill = app.calls.pill(&room("a")).expect("a call");
        assert_eq!((pill.count, pill.ringing), (2, false));
        assert_eq!(pill.label(), format!("{CALL_MARK} 2"));
        incoming(&mut app, "b", "c", true);
        let pill = app.calls.pill(&room("b")).expect("ringing");
        assert_eq!((pill.count, pill.ringing), (0, true));
        assert_eq!(pill.label(), CALL_MARK);
        let _ = app.update(core(CoreEvent::RingingStopped { room_id: room("b") }));
        assert_eq!(app.calls.pill(&room("b")), None);
    }

    #[test]
    fn the_main_view_builds_in_every_call_state() {
        let (mut app, _rx) = main_app();
        let _ = app.update(Message::SelectRoom(room("a")));
        let _ = app.view();
        let _ = app.update(core(CoreEvent::ActiveCall {
            room_id: room("a"),
            participants: 2,
        }));
        incoming(&mut app, "b", "c", true);
        let _ = app.view();
        joining(&mut app, "a");
        let _ = app.view();
        state(
            &mut app,
            CallState::Connected {
                room_id: room("a"),
                encrypted: true,
            },
        );
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        );
        let _ = app.view();
    }

    // ---- Task 3: the in-call view ----

    use twicecat_core::api::{
        AvatarId, MembershipView, PowerLevelView, Role, RoomMemberView, SkipReason, VoiceCleaning,
    };

    use crate::keys::Shortcut;

    fn member(n: &str, name: Option<&str>, avatar: Option<&str>) -> RoomMemberView {
        RoomMemberView {
            user_id: user(n),
            display_name: name.map(str::to_owned),
            avatar: avatar.map(AvatarId::from),
            power: PowerLevelView::Level(0),
            role: Role::User,
            membership: MembershipView::Joined,
        }
    }

    fn members(app: &mut App, n: &str, list: Vec<RoomMemberView>) {
        let _ = app.update(core(CoreEvent::RoomMembers {
            room_id: room(n),
            members: list,
            truncated: false,
        }));
    }

    /// An app in a connected call in room `a` with `a` (this device) and
    /// `b`, room `b` selected, commands drained.
    fn in_call() -> (App, mpsc::Receiver<Command>) {
        let (mut app, mut rx) = main_app();
        let _ = app.update(Message::SelectRoom(room("b")));
        joining(&mut app, "a");
        state(
            &mut app,
            CallState::Connected {
                room_id: room("a"),
                encrypted: false,
            },
        );
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("b")],
            }],
        );
        drain(&mut rx);
        (app, rx)
    }

    fn shortcut(app: &mut App, shortcut: Shortcut) {
        let _ = app.update(Message::Shortcut {
            shortcut,
            captured: false,
        });
    }

    #[test]
    fn mute_and_deafen_send_their_commands() {
        let (mut app, mut rx) = in_call();
        call(&mut app, CallMessage::ToggleMute);
        call(&mut app, CallMessage::ToggleMute);
        call(&mut app, CallMessage::ToggleDeafen);
        call(&mut app, CallMessage::ToggleDeafen);
        let sent = drain(&mut rx);
        assert!(
            matches!(
                sent.as_slice(),
                [
                    Command::SetMicMuted(true),
                    Command::SetMicMuted(false),
                    Command::SetDeafened(true),
                    Command::SetDeafened(false),
                ]
            ),
            "{sent:?}"
        );
    }

    #[test]
    fn unmuting_while_deafened_also_undeafens() {
        let (mut app, mut rx) = in_call();
        call(&mut app, CallMessage::ToggleMute);
        call(&mut app, CallMessage::ToggleDeafen);
        drain(&mut rx);
        call(&mut app, CallMessage::ToggleMute);
        let sent = drain(&mut rx);
        assert!(
            matches!(
                sent.as_slice(),
                [Command::SetDeafened(false), Command::SetMicMuted(false)]
            ),
            "{sent:?}"
        );
        let current = app.calls.call.as_ref().expect("in a call");
        assert!(!current.muted && !current.deafened);
    }

    #[test]
    fn undeafening_keeps_the_mute_from_before() {
        let (mut app, mut rx) = in_call();
        call(&mut app, CallMessage::ToggleMute);
        call(&mut app, CallMessage::ToggleDeafen);
        call(&mut app, CallMessage::ToggleDeafen);
        let current = app.calls.call.as_ref().expect("in a call");
        assert!(current.muted && !current.deafened);
        // The core keeps the two apart, so undeafening sends only that.
        assert!(matches!(
            drain(&mut rx).last(),
            Some(Command::SetDeafened(false))
        ));
    }

    #[test]
    fn mute_and_deafen_reset_on_a_new_call_and_do_nothing_without_one() {
        let (mut app, mut rx) = in_call();
        call(&mut app, CallMessage::ToggleMute);
        call(&mut app, CallMessage::ToggleDeafen);
        joining(&mut app, "b");
        let current = app.calls.call.as_ref().expect("in a call");
        assert!(!current.muted && !current.deafened);
        state(
            &mut app,
            CallState::Left {
                room_id: room("b"),
                reason: CallEndReason::Hangup,
            },
        );
        drain(&mut rx);
        call(&mut app, CallMessage::ToggleMute);
        call(&mut app, CallMessage::ToggleDeafen);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_shortcuts_mute_and_deafen_only_in_a_call() {
        let (mut app, mut rx) = main_app();
        shortcut(&mut app, Shortcut::ToggleMute);
        shortcut(&mut app, Shortcut::ToggleDeafen);
        assert!(drain(&mut rx).is_empty());
        let (mut app, mut rx) = in_call();
        shortcut(&mut app, Shortcut::ToggleMute);
        shortcut(&mut app, Shortcut::ToggleDeafen);
        let sent = drain(&mut rx);
        assert!(
            matches!(
                sent.as_slice(),
                [Command::SetMicMuted(true), Command::SetDeafened(true)]
            ),
            "{sent:?}"
        );
    }

    #[test]
    fn the_shortcuts_are_in_the_table_and_reach_the_app_from_the_composer() {
        use iced::keyboard::{Key, Modifiers};
        let ctrl_shift = Modifiers::CTRL | Modifiers::SHIFT;
        let other = crate::keys::Platform::Other;
        assert_eq!(
            crate::keys::shortcut_for(&Key::Character("M".into()), ctrl_shift, other),
            Some(Shortcut::ToggleMute)
        );
        assert_eq!(
            crate::keys::shortcut_for(&Key::Character("D".into()), ctrl_shift, other),
            Some(Shortcut::ToggleDeafen)
        );
        assert_eq!(
            crate::keys::shortcut_for(&Key::Character("m".into()), Modifiers::CTRL, other),
            None,
            "Ctrl+M alone is not mute"
        );
        assert!(crate::keys::composer_takes(Shortcut::ToggleMute));
        assert!(crate::keys::composer_takes(Shortcut::ToggleDeafen));
    }

    #[test]
    fn the_ring_card_is_answered_from_the_keyboard_only_while_it_shows() {
        let (mut app, mut rx) = main_app();
        shortcut(&mut app, Shortcut::AcceptRing);
        shortcut(&mut app, Shortcut::DeclineRing);
        assert!(drain(&mut rx).is_empty(), "no card, no answer");
        chord_up(&mut app);
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        drain(&mut rx);
        shortcut(&mut app, Shortcut::DeclineRing);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::DeclineCall { room_id }] if *room_id == room("b")),
            "{sent:?}"
        );
        // A second press does nothing, as the button's.
        chord_up(&mut app);
        shortcut(&mut app, Shortcut::DeclineRing);
        assert!(drain(&mut rx).is_empty());
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        drain(&mut rx);
        shortcut(&mut app, Shortcut::AcceptRing);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::JoinCall { room_id, .. }] if *room_id == room("b")),
            "{sent:?}"
        );
        // Esc hid the card: the keys wait with it.
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        call(&mut app, CallMessage::HideRing);
        drain(&mut rx);
        shortcut(&mut app, Shortcut::AcceptRing);
        assert!(drain(&mut rx).is_empty());
        assert!(crate::keys::composer_takes(Shortcut::AcceptRing));
        assert!(crate::keys::composer_takes(Shortcut::DeclineRing));
    }

    fn card_room(app: &App) -> Option<RoomId> {
        app.calls.card().map(|r| r.room_id.clone())
    }

    #[test]
    fn a_new_card_is_inert_until_its_arming_timer_ends() {
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        assert!(app.calls.arming(), "just shown");
        assert!(app.calls.live_card().is_none());
        drain(&mut rx);
        shortcut(&mut app, Shortcut::AcceptRing);
        assert!(drain(&mut rx).is_empty(), "arming: no answer");
        arm(&mut app);
        assert!(!app.calls.arming());
        // The chord of the early press is still down: it never answers.
        shortcut(&mut app, Shortcut::AcceptRing);
        assert!(drain(&mut rx).is_empty(), "the held chord stays inert");
        chord_up(&mut app);
        shortcut(&mut app, Shortcut::AcceptRing);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::JoinCall { room_id, .. }] if *room_id == room("b")),
            "{sent:?}"
        );
    }

    #[test]
    fn a_stale_arming_timer_does_not_arm_the_card() {
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        let stale = app.calls.arm_seq();
        // Esc hides it; a later ring of the same room shows it again.
        call(&mut app, CallMessage::HideRing);
        assert!(card_room(&app).is_none());
        incoming(&mut app, "b", "cy", true);
        assert_ne!(app.calls.arm_seq(), stale, "arming started again");
        let _ = app.update(Message::RingArmed(stale));
        assert!(app.calls.arming(), "the old timer is ignored");
        drain(&mut rx);
        shortcut(&mut app, Shortcut::DeclineRing);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn a_held_chord_that_answers_one_ring_never_answers_the_next() {
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        incoming(&mut app, "a", "cy", true);
        arm(&mut app);
        assert_eq!(card_room(&app), Some(room("b")));
        drain(&mut rx);
        // The chord goes down: Accept B. Joining B puts C on the card.
        shortcut(&mut app, Shortcut::AcceptRing);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::JoinCall { room_id, .. }] if *room_id == room("b")),
            "{sent:?}"
        );
        joining(&mut app, "b");
        drain(&mut rx);
        assert_eq!(card_room(&app), Some(room("a")), "C shows now");
        // The auto-repeats (forwarded by the composer, so not told apart)
        // come while C arms, and after it armed.
        shortcut(&mut app, Shortcut::AcceptRing);
        arm(&mut app);
        for _ in 0..5 {
            shortcut(&mut app, Shortcut::AcceptRing);
            shortcut(&mut app, Shortcut::DeclineRing);
        }
        assert!(drain(&mut rx).is_empty(), "C was never answered");
        assert!(!app.calls.ring(&room("a")).expect("C rings").answered);
        // A fresh press after the keys went up works.
        chord_up(&mut app);
        shortcut(&mut app, Shortcut::DeclineRing);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::DeclineCall { room_id }] if *room_id == room("a")),
            "{sent:?}"
        );
    }

    #[test]
    fn arming_starts_again_for_each_ring_the_card_changes_to() {
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        incoming(&mut app, "a", "cy", true);
        arm(&mut app);
        assert!(!app.calls.arming());
        let first = app.calls.arm_seq();
        // The card changes to the other ring: arming again.
        let _ = app.update(core(CoreEvent::RingingStopped { room_id: room("b") }));
        assert_eq!(card_room(&app), Some(room("a")));
        assert!(app.calls.arming());
        assert_ne!(app.calls.arm_seq(), first);
        drain(&mut rx);
        chord_up(&mut app);
        shortcut(&mut app, Shortcut::AcceptRing);
        assert!(drain(&mut rx).is_empty());
        arm(&mut app);
        chord_up(&mut app);
        shortcut(&mut app, Shortcut::AcceptRing);
        assert_eq!(drain(&mut rx).len(), 1);
        // No card, nothing arms or waits.
        let (mut quiet, _rx) = main_app();
        assert!(!quiet.calls.arming());
        arm(&mut quiet);
        assert!(quiet.calls.live_card().is_none());
    }

    #[test]
    fn losing_the_window_lets_go_of_the_held_chord() {
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        drain(&mut rx);
        shortcut(&mut app, Shortcut::DeclineRing);
        assert_eq!(drain(&mut rx).len(), 1);
        assert!(app.calls.chord_held());
        let _ = app.update(Message::WindowFocus(false));
        assert!(!app.calls.chord_held(), "a release may never be seen");
    }

    #[test]
    fn the_held_chord_is_let_go_when_the_key_capture_dialog_opens_or_closes() {
        use crate::audio_settings::AudioMessage as A;
        let (mut app, mut rx) = main_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        let _ = app.update(Message::Audio(A::Open));
        drain(&mut rx);
        // Down before the dialog opens: its release may go unseen under it.
        shortcut(&mut app, Shortcut::DeclineRing);
        assert!(app.calls.chord_held(), "the chord is down");
        let _ = app.update(Message::Audio(A::CaptureKey));
        assert!(
            !app.calls.chord_held(),
            "the latch ends with the dialog's start"
        );
        // And when it closes, whatever was latched meanwhile.
        app.calls.chord_held = true;
        let _ = app.update(Message::Audio(A::CancelCapture));
        assert!(!app.calls.chord_held(), "and with its end");
    }

    #[test]
    fn more_expands_and_collapses_a_row_and_a_call_end_folds_them_all() {
        let (mut app, mut rx) = main_app();
        joining(&mut app, "a");
        drain(&mut rx);
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("bob"), person("cy")],
            }],
        );
        assert!(!app.calls.row_open(&user("bob")));
        let _ = app.update(Message::CallRowMore(user("bob")));
        assert!(app.calls.row_open(&user("bob")));
        assert!(!app.calls.row_open(&user("cy")), "rows open one by one");
        let _ = app.update(Message::CallRowMore(user("cy")));
        let _ = app.update(Message::CallRowMore(user("bob")));
        assert!(!app.calls.row_open(&user("bob")));
        assert!(app.calls.row_open(&user("cy")));
        // This device has no controls to fold out, and a stranger has none.
        let _ = app.update(Message::CallRowMore(user("a")));
        let _ = app.update(Message::CallRowMore(user("zed")));
        assert_eq!(app.calls.rows_open.len(), 1);
        // It is view state: nothing is sent to the core.
        assert!(drain(&mut rx).is_empty());
        // Someone leaving takes their open row with them.
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("bob")],
            }],
        );
        assert!(app.calls.rows_open.is_empty());
        let _ = app.update(Message::CallRowMore(user("bob")));
        state(&mut app, CallState::Idle);
        assert!(app.calls.rows_open.is_empty());
    }

    #[test]
    fn joining_asks_for_the_call_rooms_members() {
        let (mut app, mut rx) = main_app();
        joining(&mut app, "a");
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::ListRoomMembers { room_id }] if *room_id == room("a")),
            "{sent:?}"
        );
    }

    #[test]
    fn the_call_rooms_members_name_and_picture_the_participants() {
        let (mut app, mut rx) = in_call();
        let mut b = person("b");
        b.display_name = None;
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), b],
            }],
        );
        // The open room is `b`: the call room's list is kept all the same.
        members(
            &mut app,
            "a",
            vec![
                member("a", Some("Ann"), None),
                member("b", Some("Bob"), Some("mxc://x/bob")),
            ],
        );
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::FetchAvatars(ids)] if *ids == vec![AvatarId::from("mxc://x/bob")]),
            "{sent:?}"
        );
        let rows = app.calls.rows();
        assert_eq!(rows[0].name, "A", "the call's own name comes first");
        assert_eq!(rows[1].name, "Bob", "then the member list's");
        assert_eq!(rows[1].avatar, Some(AvatarId::from("mxc://x/bob")));
        assert!(
            app.rooms_ui.known.is_none(),
            "the open room's list is untouched"
        );
    }

    #[test]
    fn a_newcomer_missing_from_the_members_asks_again_once() {
        let (mut app, mut rx) = in_call();
        members(
            &mut app,
            "a",
            vec![member("a", None, None), member("b", None, None)],
        );
        drain(&mut rx);
        let mut c = person("c");
        c.display_name = None;
        let everyone = vec![person("a"), person("b"), c];
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: everyone.clone(),
            }],
        );
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::ListRoomMembers { room_id }] if *room_id == room("a")),
            "{sent:?}"
        );
        participants(&mut app, "a", vec![ListDiff::Reset { values: everyone }]);
        assert!(drain(&mut rx).is_empty(), "asked once");
        assert_eq!(app.calls.rows()[2].name, "@c:x", "the user ID until known");
    }

    #[test]
    fn participants_before_the_member_list_do_not_ask_twice() {
        let (mut app, mut rx) = main_app();
        joining(&mut app, "a");
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), person("c")],
            }],
        );
        let sent = drain(&mut rx);
        assert_eq!(sent.len(), 1, "only the join's request: {sent:?}");
    }

    #[test]
    fn rows_mark_this_device_speaking_and_muted_and_give_controls_once_per_person() {
        let (mut app, _rx) = in_call();
        let mut b1 = person("b");
        b1.speaking = true;
        b1.volume_percent = 150;
        b1.cleaning = CleaningStatus::Cleaned;
        let mut b2 = person("b");
        b2.id = ParticipantId::from("b-phone");
        b2.muted = true;
        b2.cleaning = CleaningStatus::Skipped(SkipReason::AlreadyClean);
        participants(
            &mut app,
            "a",
            vec![ListDiff::Reset {
                values: vec![person("a"), b1, b2],
            }],
        );
        call(&mut app, CallMessage::ToggleMute);
        let rows = app.calls.rows();
        assert!(rows[0].is_local && rows[0].muted, "our mute shows at once");
        assert_eq!(rows[0].controls, None, "no volume for ourselves");
        assert!(rows[1].speaking && !rows[1].muted);
        assert_eq!(rows[1].cleaning, Some("Voice cleaned"));
        assert_eq!(
            rows[1].controls,
            Some(UserControls {
                volume: 150,
                cleaning: CleaningChoice::Automatic
            })
        );
        assert!(rows[2].muted);
        assert_eq!(
            rows[2].cleaning,
            Some("Not cleaned (cleaned on their side)")
        );
        assert_eq!(rows[2].controls, None, "one set of controls per person");
    }

    #[test]
    fn a_deafened_device_shows_as_muted() {
        let (mut app, _rx) = in_call();
        call(&mut app, CallMessage::ToggleDeafen);
        assert!(app.calls.rows()[0].muted);
    }

    #[test]
    fn a_moved_volume_slider_sends_once_on_release() {
        let (mut app, mut rx) = in_call();
        for percent in [110, 140, 170] {
            call(
                &mut app,
                CallMessage::Volume {
                    user_id: user("b"),
                    percent,
                },
            );
        }
        assert!(drain(&mut rx).is_empty(), "nothing while it moves");
        assert_eq!(app.calls.rows()[1].controls.map(|c| c.volume), Some(170));
        call(&mut app, CallMessage::VolumeReleased);
        call(&mut app, CallMessage::VolumeReleased);
        let seq = app.calls.volume_seq;
        call(&mut app, CallMessage::VolumeDue(seq));
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::SetUserVolume { user_id, percent: 170, .. }] if *user_id == user("b")),
            "{sent:?}"
        );
    }

    #[test]
    fn a_slider_moved_without_a_release_sends_after_the_last_pause() {
        let (mut app, mut rx) = in_call();
        call(
            &mut app,
            CallMessage::Volume {
                user_id: user("b"),
                percent: 80,
            },
        );
        let stale = app.calls.volume_seq;
        call(
            &mut app,
            CallMessage::Volume {
                user_id: user("b"),
                percent: 60,
            },
        );
        call(&mut app, CallMessage::VolumeDue(stale));
        assert!(drain(&mut rx).is_empty(), "an older pause sends nothing");
        let seq = app.calls.volume_seq;
        call(&mut app, CallMessage::VolumeDue(seq));
        let sent = drain(&mut rx);
        assert!(
            matches!(
                sent.as_slice(),
                [Command::SetUserVolume { percent: 60, .. }]
            ),
            "{sent:?}"
        );
    }

    #[test]
    fn a_slider_put_back_where_it_was_sends_nothing() {
        let (mut app, mut rx) = in_call();
        call(
            &mut app,
            CallMessage::Volume {
                user_id: user("b"),
                percent: 100,
            },
        );
        call(&mut app, CallMessage::VolumeReleased);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_volume_steps_clamp_to_the_range() {
        let (mut app, mut rx) = in_call();
        let step = |app: &mut App, up: bool| {
            call(
                app,
                CallMessage::VolumeStep {
                    user_id: user("b"),
                    up,
                },
            );
        };
        step(&mut app, true);
        let mut b = person("b");
        b.volume_percent = 195;
        participants(&mut app, "a", vec![ListDiff::Set { index: 1, value: b }]);
        step(&mut app, true);
        let mut b = person("b");
        b.volume_percent = MAX_VOLUME;
        participants(&mut app, "a", vec![ListDiff::Set { index: 1, value: b }]);
        step(&mut app, true);
        let mut b = person("b");
        b.volume_percent = 5;
        participants(&mut app, "a", vec![ListDiff::Set { index: 1, value: b }]);
        step(&mut app, false);
        step(&mut app, false);
        let percents: Vec<u16> = drain(&mut rx)
            .into_iter()
            .filter_map(|c| match c {
                Command::SetUserVolume { percent, .. } => Some(percent),
                _ => None,
            })
            .collect();
        // 100 -> 110; 195 -> 200; at 200 nothing; 5 -> 0, twice (the core
        // has not reported 0 yet).
        assert_eq!(percents, [110, MAX_VOLUME, 0, 0]);
    }

    fn percents(rx: &mut mpsc::Receiver<Command>) -> Vec<u16> {
        drain(rx)
            .into_iter()
            .filter_map(|c| match c {
                Command::SetUserVolume { percent, .. } => Some(percent),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn steps_before_the_echo_add_up() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(core(CoreEvent::AudioSettings(AudioSettings::default())));
        for _ in 0..3 {
            call(
                &mut app,
                CallMessage::VolumeStep {
                    user_id: user("b"),
                    up: true,
                },
            );
        }
        assert_eq!(percents(&mut rx), [110, 120, 130]);
        assert_eq!(app.calls.rows()[1].controls.map(|c| c.volume), Some(130));
        // Back to 100 is stored as nothing, as the core does.
        let mut settings = app.calls.settings.clone().expect("echoed");
        settings.user_volumes.insert(user("b"), 110);
        let _ = app.update(core(CoreEvent::AudioSettings(settings)));
        call(
            &mut app,
            CallMessage::VolumeStep {
                user_id: user("b"),
                up: false,
            },
        );
        assert_eq!(percents(&mut rx), [100]);
        assert!(
            app.calls
                .settings
                .as_ref()
                .is_some_and(|s| s.user_volumes.is_empty())
        );
    }

    #[test]
    fn a_second_persons_slider_sends_the_first_ones_value() {
        let (mut app, mut rx) = in_call();
        let mut c = person("c");
        c.id = ParticipantId::from("c-dev");
        participants(&mut app, "a", vec![ListDiff::PushBack { value: c }]);
        call(
            &mut app,
            CallMessage::Volume {
                user_id: user("b"),
                percent: 60,
            },
        );
        call(
            &mut app,
            CallMessage::Volume {
                user_id: user("c"),
                percent: 140,
            },
        );
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::SetUserVolume { user_id, percent: 60, .. }] if *user_id == user("b")),
            "{sent:?}"
        );
        call(&mut app, CallMessage::VolumeReleased);
        assert_eq!(percents(&mut rx), [140]);
    }

    #[test]
    fn a_cleaning_choice_is_kept_before_its_echo() {
        let (mut app, mut rx) = in_call();
        let _ = app.update(core(CoreEvent::AudioSettings(AudioSettings::default())));
        call(
            &mut app,
            CallMessage::Cleaning {
                user_id: user("b"),
                choice: CleaningChoice::Always,
            },
        );
        call(
            &mut app,
            CallMessage::Cleaning {
                user_id: user("b"),
                choice: CleaningChoice::Always,
            },
        );
        assert_eq!(drain(&mut rx).len(), 1, "the second is already set");
        assert_eq!(
            app.calls.rows()[1].controls.map(|c| c.cleaning),
            Some(CleaningChoice::Always)
        );
    }

    #[test]
    fn volume_for_someone_not_in_the_call_or_ourselves_sends_nothing() {
        let (mut app, mut rx) = in_call();
        for user_id in [user("z"), user("a")] {
            call(
                &mut app,
                CallMessage::VolumeStep {
                    user_id: user_id.clone(),
                    up: true,
                },
            );
            call(
                &mut app,
                CallMessage::Volume {
                    user_id,
                    percent: 50,
                },
            );
            call(&mut app, CallMessage::VolumeReleased);
        }
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn the_cleaning_menu_sends_a_changed_choice() {
        let (mut app, mut rx) = in_call();
        let choose = |app: &mut App, choice| {
            call(
                app,
                CallMessage::Cleaning {
                    user_id: user("b"),
                    choice,
                },
            );
        };
        choose(&mut app, CleaningChoice::Automatic);
        assert!(drain(&mut rx).is_empty(), "already automatic");
        choose(&mut app, CleaningChoice::Never);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::SetUserCleaning { user_id, cleaning: VoiceCleaning::Never }] if *user_id == user("b")),
            "{sent:?}"
        );
        let mut echoed = AudioSettings::default();
        echoed.user_cleaning.insert(user("b"), VoiceCleaning::Never);
        let _ = app.update(core(CoreEvent::AudioSettings(echoed)));
        assert_eq!(
            app.calls.rows()[1].controls.map(|c| c.cleaning),
            Some(CleaningChoice::Never)
        );
        choose(&mut app, CleaningChoice::Never);
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn cleaning_choices_round_trip_and_read_well() {
        for choice in CleaningChoice::ALL {
            assert_eq!(CleaningChoice::from_cleaning(choice.cleaning()), choice);
        }
        assert_eq!(CleaningChoice::Always.to_string(), "Clean: always");
        assert_eq!(cleaning_label(CleaningStatus::Off), None);
        assert_eq!(
            cleaning_label(CleaningStatus::Skipped(SkipReason::Limit)),
            Some("Not cleaned (too many voices)")
        );
    }

    #[test]
    fn a_voice_room_opens_the_panel_and_an_ordinary_room_does_not() {
        let (mut app, _rx) = main_app();
        let mut voice = summary("v");
        voice.is_call_room = true;
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::PushBack {
            value: voice,
        }])));
        joining(&mut app, "a");
        assert!(!app.calls.panel_open);
        joining(&mut app, "v");
        assert!(app.calls.panel_open);
    }

    #[test]
    fn the_panel_shows_beside_the_calls_room_only_and_closes_with_the_call() {
        let (mut app, _rx) = in_call();
        call(&mut app, CallMessage::TogglePanel);
        assert!(app.calls.panel_open);
        assert!(
            !app.calls.panel_shown(app.selected.as_ref()),
            "room b is open"
        );
        call(&mut app, CallMessage::ShowCall);
        assert_eq!(app.selected, Some(room("a")));
        assert!(app.calls.panel_shown(app.selected.as_ref()));
        let _ = app.view();
        state(
            &mut app,
            CallState::Left {
                room_id: room("a"),
                reason: CallEndReason::Hangup,
            },
        );
        assert!(!app.calls.panel_open);
        call(&mut app, CallMessage::TogglePanel);
        assert!(!app.calls.panel_open, "no call, no panel");
    }

    #[test]
    fn escape_closes_the_participants_panel() {
        let (mut app, _rx) = in_call();
        call(&mut app, CallMessage::ShowCall);
        shortcut(&mut app, Shortcut::Escape);
        assert!(!app.calls.panel_open);
    }

    #[test]
    fn the_notice_can_be_dismissed() {
        let (mut app, _rx) = in_call();
        let _ = app.update(core(CoreEvent::Error(CoreError::NotInCall)));
        assert!(app.calls.notice.is_some());
        let _ = app.view();
        call(&mut app, CallMessage::DismissNotice);
        assert!(app.calls.notice.is_none());
    }

    #[test]
    fn phase_labels_read_well() {
        assert_eq!(CallPhase::Joining.label(), "Connecting\u{2026}");
        assert_eq!(
            CallPhase::Connected { encrypted: true }.label(),
            "Voice connected, encrypted"
        );
        assert_eq!(
            CallPhase::Reconnecting { attempt: 3 }.label(),
            "Reconnecting (attempt 3)\u{2026}"
        );
    }

    // ---- Task 5: incoming calls ----

    use crate::notifications::RecordingNotifier;

    /// An app with a recording notifier and an unfocused window; room `b`
    /// is a DM named "Bob".
    fn ring_app() -> (App, mpsc::Receiver<Command>, RecordingNotifier) {
        let (mut app, rx) = main_app();
        let notifier = RecordingNotifier::default();
        app.set_notifier(Box::new(notifier.clone()));
        let mut dm = summary("b");
        dm.is_dm = true;
        dm.name = "Bob".into();
        let _ = app.update(core(CoreEvent::RoomListChanged(vec![ListDiff::Set {
            index: 1,
            value: dm,
        }])));
        let _ = app.update(Message::WindowFocus(false));
        (app, rx, notifier)
    }

    fn shown(notifier: &RecordingNotifier) -> Vec<(RoomId, String, String)> {
        notifier
            .shown
            .lock()
            .expect("test mutex")
            .iter()
            .map(|r| {
                (
                    r.room_id.clone(),
                    r.toast.title.clone(),
                    r.toast.body.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn a_dm_ring_opens_the_card_and_notifies_once() {
        let (mut app, _rx, notifier) = ring_app();
        incoming(&mut app, "b", "bob", true);
        assert_eq!(app.calls.card().map(|r| r.room_id.clone()), Some(room("b")));
        assert!(app.any_overlay_open(), "room shortcuts wait under the card");
        let _ = app.view();
        assert_eq!(
            shown(&notifier),
            [(
                room("b"),
                "Bob".to_owned(),
                "@bob:x is calling you".to_owned()
            )]
        );
        // The same ring again (a sync replay) adds nothing.
        incoming(&mut app, "b", "bob", true);
        assert_eq!(shown(&notifier).len(), 1);
    }

    #[test]
    fn a_group_call_notifies_quietly_without_a_card() {
        let (mut app, _rx, notifier) = ring_app();
        incoming(&mut app, "a", "carol", false);
        assert!(app.calls.card().is_none());
        assert!(app.calls.pill(&room("a")).is_some_and(|p| p.ringing));
        assert_eq!(
            shown(&notifier),
            [(
                room("a"),
                "a".to_owned(),
                "@carol:x started a call".to_owned()
            )]
        );
    }

    #[test]
    fn a_focused_window_or_notifications_off_shows_no_notification() {
        let (mut app, _rx, notifier) = ring_app();
        let _ = app.update(Message::WindowFocus(true));
        incoming(&mut app, "b", "bob", true);
        assert!(shown(&notifier).is_empty());
        assert!(app.calls.card().is_some(), "the card shows all the same");
        let _ = app.update(Message::WindowFocus(false));
        app.ui_settings.desktop_notifications = false;
        incoming(&mut app, "a", "carol", false);
        assert!(shown(&notifier).is_empty());
    }

    #[test]
    fn without_message_content_the_caller_stays_out_of_the_notification() {
        let (mut app, _rx, notifier) = ring_app();
        app.ui_settings.show_message_content = false;
        incoming(&mut app, "b", "bob", true);
        assert_eq!(
            shown(&notifier),
            [(
                room("b"),
                "Bob".to_owned(),
                crate::notifications::BODY_CALLING.to_owned()
            )]
        );
    }

    #[test]
    fn a_ring_for_the_call_we_are_in_announces_nothing() {
        let (mut app, _rx, notifier) = ring_app();
        joining(&mut app, "b");
        incoming(&mut app, "b", "bob", true);
        assert!(app.calls.card().is_none());
        assert!(shown(&notifier).is_empty());
    }

    #[test]
    fn accept_joins_once_and_the_card_goes_when_joining() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        drain(&mut rx);
        call(&mut app, CallMessage::Accept(room("b")));
        call(&mut app, CallMessage::Accept(room("b")));
        call(&mut app, CallMessage::Decline(room("b")));
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::JoinCall { room_id, .. }] if *room_id == room("b")),
            "{sent:?}"
        );
        assert!(app.calls.card().is_some_and(|r| r.answered), "buttons off");
        joining(&mut app, "b");
        assert!(app.calls.card().is_none());
    }

    #[test]
    fn a_click_meant_for_an_old_card_answers_nothing_on_a_new_call_in_the_same_room() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        // The card was up and armed; its ring ends and the same room calls
        // again before the click reaches the app.
        let _ = app.update(core(CoreEvent::RingingStopped { room_id: room("b") }));
        incoming(&mut app, "b", "bob", true);
        assert!(app.calls.arming(), "the new card is still waiting");
        drain(&mut rx);
        call(&mut app, CallMessage::Accept(room("b")));
        call(&mut app, CallMessage::Decline(room("b")));
        assert!(
            drain(&mut rx).is_empty(),
            "the old click answered the new call"
        );
        assert!(app.calls.card().is_some_and(|r| !r.answered));
        // Once the new card has armed, the buttons work.
        arm(&mut app);
        call(&mut app, CallMessage::Decline(room("b")));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::DeclineCall { .. }]
        ));
    }

    #[test]
    fn accept_and_decline_for_a_room_other_than_the_live_card_do_nothing() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        incoming(&mut app, "c", "cy", true);
        arm(&mut app);
        drain(&mut rx);
        let live = app.calls.live_card().expect("live").room_id.clone();
        let other = if live == room("b") {
            room("c")
        } else {
            room("b")
        };
        call(&mut app, CallMessage::Accept(other.clone()));
        call(&mut app, CallMessage::Decline(other));
        assert!(drain(&mut rx).is_empty(), "only the card on screen answers");
    }

    #[test]
    fn a_failed_accept_shows_the_card_again() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        call(&mut app, CallMessage::Accept(room("b")));
        joining(&mut app, "b");
        assert!(app.calls.card().is_none());
        state(
            &mut app,
            CallState::Left {
                room_id: room("b"),
                reason: CallEndReason::Failed("no call service".into()),
            },
        );
        // The core's ringtone sounds again: the card is back, its buttons on.
        let card = app.calls.card().expect("the card again");
        assert!(!card.answered);
        // A card that comes back is a card to wait for again.
        assert!(app.calls.arming());
        arm(&mut app);
        drain(&mut rx);
        call(&mut app, CallMessage::Decline(room("b")));
        assert!(matches!(
            drain(&mut rx).as_slice(),
            [Command::DeclineCall { .. }]
        ));
    }

    #[test]
    fn decline_sends_once_and_the_card_goes_when_the_ringing_stops() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        drain(&mut rx);
        call(&mut app, CallMessage::Decline(room("b")));
        call(&mut app, CallMessage::Decline(room("b")));
        call(&mut app, CallMessage::Accept(room("b")));
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::DeclineCall { room_id }] if *room_id == room("b")),
            "{sent:?}"
        );
        let _ = app.update(core(CoreEvent::RingingStopped { room_id: room("b") }));
        assert!(app.calls.card().is_none());
    }

    #[test]
    fn a_ring_in_a_room_hidden_by_the_search_can_be_accepted() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "elsewhere", "dan", true);
        arm(&mut app);
        drain(&mut rx);
        call(&mut app, CallMessage::Accept(room("elsewhere")));
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::JoinCall { room_id, .. }] if *room_id == room("elsewhere")),
            "{sent:?}"
        );
        // The core answers an unknown room with Joining, then Left.
        joining(&mut app, "elsewhere");
        state(
            &mut app,
            CallState::Left {
                room_id: room("elsewhere"),
                reason: CallEndReason::Failed("unknown room".into()),
            },
        );
        assert!(app.calls.pending.is_none());
    }

    #[test]
    fn a_refused_answer_can_be_tried_again() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        arm(&mut app);
        call(&mut app, CallMessage::Decline(room("b")));
        let _ = app.update(core(CoreEvent::Error(CoreError::Call(
            "could not send the decline; the caller keeps ringing".into(),
        ))));
        assert!(app.calls.card().is_some_and(|r| !r.answered));
        drain(&mut rx);
        call(&mut app, CallMessage::Decline(room("b")));
        assert_eq!(drain(&mut rx).len(), 1);
    }

    #[test]
    fn escape_hides_the_card_and_silences_the_ringtone() {
        let (mut app, mut rx, _) = ring_app();
        incoming(&mut app, "b", "bob", true);
        incoming(&mut app, "a", "carol", true);
        drain(&mut rx);
        shortcut(&mut app, Shortcut::Escape);
        let sent = drain(&mut rx);
        assert!(
            matches!(sent.as_slice(), [Command::SilenceRinging]),
            "{sent:?}"
        );
        // The core silenced both: both cards hide, both rooms still ring.
        assert!(app.calls.card().is_none());
        assert!(app.calls.pill(&room("a")).is_some_and(|p| p.ringing));
        assert!(app.calls.pill(&room("b")).is_some_and(|p| p.ringing));
        assert!(!app.any_overlay_open());
        shortcut(&mut app, Shortcut::Escape);
        assert!(drain(&mut rx).is_empty(), "nothing left to silence");
        // A new call in that room shows its card again.
        incoming(&mut app, "b", "bob", true);
        assert_eq!(app.calls.card().map(|r| r.room_id.clone()), Some(room("b")));
    }

    #[test]
    fn the_card_closes_before_any_other_layer() {
        let (mut app, _rx, _) = ring_app();
        let _ = app.update(Message::Settings(crate::settings::SettingsMessage::Open));
        assert!(app.settings_ui.open);
        incoming(&mut app, "b", "bob", true);
        shortcut(&mut app, Shortcut::Escape);
        assert!(app.calls.card().is_none());
        assert!(app.settings_ui.open, "Settings stays for the next Esc");
    }
}
