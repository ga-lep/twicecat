# Phase 6 Call UI Plan
Status: APPROVED (all questions answered and the plan approved by the human, 2026-10-12)

## Goal
Put the finished call engine on screen in `crates/app`. This covers [ga-lep/twicecat#14](https://github.com/ga-lep/twicecat/issues/14), the roadmap's Phase 6 "Call UI" item, which `docs/plans/phase6-calls.md` decision 2 ("core first, then UI") left for the Iced shell. The work happens on branch `feat/call-ui`, cut from `main`.

Today the app drops every call and audio event in one match arm (`crates/app/src/state.rs:1568-1579`). After this plan the app:
- joins and leaves calls from the room header;
- shows call counts and voice rooms in the room list;
- shows participants with speaking and muted marks, per-user volume and voice cleaning;
- offers mute and deafen;
- rings for DM calls and gives a quiet notification for group calls;
- has an audio settings screen: devices with hot-plug, processing, gain and volume, bitrate, the mic test with a level meter, remote cleaning and the cleaning announcement.

Push-to-talk is [ga-lep/twicecat#15](https://github.com/ga-lep/twicecat/issues/15), together with the whole input mode (Q7): voice activity and its threshold, push-to-talk, the release delay, the bindings, and the service. The Voice & audio card leaves room for that section, and this plan does not show it.

Rules for every task (AGENTS.md):
- **Rule 2:** every `iced` 0.14.0 widget or API that the app has not used before is read in the registry source of the pinned version first, and cited file:line in PROGRESS.md. Examples: `slider::on_release`, `vertical_slider`, `progress_bar`, `pick_list` with `Option`, `tooltip`. Core API facts come from `crates/core/src/api/{mod,call,audio}.rs` and are cited the same way.
- **Rule 3:** no pinned version changes and no new crate.
- **Rule 5:**
  - one `feat(app)` commit per task (Task 4: `feat(core)`), with a `Step: phase6.<n>` trailer (numbering in Q1);
  - review fixes are folded in with `--fixup` and `rebase --autosquash`;
  - PROGRESS.md and `docs/plans/phase6-call-ui.progress.md` are updated in the same commit.
- **Task 4 is a core task** (`feat(core)`, Q3b). It changes the public core API only as approved in Q3b, and `scripts/check-core-api.sh` must pass.
- **Rule 7:** the app only sends `Command`s and reads `CoreEvent`s. Every type it needs is already core's own and re-exported from `twicecat_core::api` (`AudioSettings`, `Participant`, `CallState`, `AudioDevices`, `InputLevel`, …). The app needs no `twicecat-audio` dependency (Q2).
- **Rule 9:** rendering, sound, devices, notifications and window attention are on the human checkpoint list and are never ticked by the agent.
- **Gate before each commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features`;
  - `cargo test --workspace`;
  - `scripts/check-app-deps.sh` (also with `livekit`), `scripts/check-core-api.sh`, `scripts/check-commits.sh main`;
  - from Task 4 on, the ignored stack tests (core `call_ringing.rs`; from Task 5, also those in `crates/app/tests/`) against `scripts/stack.sh`.
- **Never block `update`:** every call action is one `self.send(Command::…)`. Sliders send on release, not on every move (`SetUserVolume` and `UpdateAudioSettings` save to disk in the core). All audio work stays in the core.

## Facts verified while planning (2026-10-12)
- **Commands** (`crates/core/src/api/mod.rs:422-479`). `JoinCall { room_id, intent: CallIntent::Audio }`; joining another room leaves the current call first. `LeaveCall`. `DeclineCall { room_id }`. `SetMicMuted(bool)` and `SetDeafened(bool)` (deafen also mutes; neither is echoed back except `Participant::muted` on the local device; both give `NotInCall` without a call). `SetUserVolume { user_id, percent }` (0..=200). `SetUserCleaning { user_id, cleaning: VoiceCleaning }`. `ListAudioDevices`. `GetAudioSettings`. `ProbeGpuEnhancement`. `UpdateAudioSettings(AudioSettings)`, which is validated, saved, applied and always echoed back as `CoreEvent::AudioSettings`. `StartMicTest { loopback }` is refused during a call. `StopMicTest`.
- **Events** (`mod.rs:1021-1074`):
  - `CallState(Joining | Connected { encrypted } | Reconnecting { attempt } | Left { reason: Hangup | Failed(String) })`; `Idle` is never emitted.
  - `CallParticipants { room_id, diffs: Vec<ListDiff<Participant>> }`, in practice always `Reset`, with the local device included.
  - `ActiveCall { room_id, participants: u32 }` (0 = ended).
  - `IncomingCall { room_id, caller, intent, ring }`, `RingingStopped { room_id }`, `CallDeclined { room_id, user_id }`.
  - `AudioDevices`, `AudioSettings`, `InputLevel` (about 20 Hz while the mic is open), `AudioStats` (about 1 Hz), `GpuEnhancement`.
  - There is no call error event. Failures come as `CoreEvent::Error(CoreError::Call | Audio | NotInCall)`, plus `Left { Failed }` for a failed join.
- **Ordering** (`crates/core/src/engine/mod.rs` ~1640-1960). `Joining` comes first. A pending ring is stopped with `RingingStopped` before `Connected`. The first participants `Reset` can come before or after `Connected`. No empty `Reset` is sent on leave, so the app clears participants on `Left`. Hot-plug `AudioDevices` only start after the first `ListAudioDevices`. Note: `crates/core/src/engine/calls.rs` is a stale copy that is not compiled; a separate task is suggested for it.
- **Room list:** `RoomSummary::is_call_room` (`api/types.rs:321`) marks MSC3417 voice rooms. The app never reads it today. Live counts come only from `ActiveCall`.
- **Participants have no avatar.** The app looks them up in the call room's members (`Command::ListRoomMembers`). `App::on_room_members` (`crates/app/src/rooms.rs:1378`) drops answers for rooms other than the selected one, so the call model keeps its own copy for the call room.
- **App patterns to follow:** a model on `App` plus a `CallMessage` enum, with `impl App { on_call… }` in `crates/app/src/calls.rs`, like `verification.rs` and `sessions.rs`. Tests drive `App::update` with `main_app()` and the `mpsc::Sender<Command>` sink (`state.rs` tests from 2515). The overlay stack is in `state.rs:2435-2466`. `keys::any_overlay_open`, `keys::on_escape` and `settings::other_modal_open` must know about any new overlay.
- **Nothing in any crate plays a sound** (no ringtone; Q3b adds one in the core). The core already opens playback-only devices: the mic test's `AudioSession` (`crates/core/src/audio_session.rs:93`, `IoConfig` in `crates/audio/src/io/mod.rs:28`). Desktop notifications go through `Notifier` (`crates/app/src/notifications.rs:244`); window attention through `attention()` (`state.rs:87`).
- **Headless stack tests already exist for the app** (`crates/app/tests/bridge.rs`: the real `Bridge` with `App::update`). `CoreConfig::audio` takes `AudioBackend::Virtual`, so a test can join a call with no sound card.

## Questions for the human
Each question has a recommendation, marked (R).

**Q1. Plan, branch, step numbers. Answered (human, 2026-10-12): as recommended.** Plan `docs/plans/phase6-call-ui.md`, ledger `docs/plans/phase6-call-ui.progress.md`, branch `feat/call-ui`. The steps continue Phase 6 as `phase6.17` to `phase6.23` (R).

**Q2. App boundary for audio types. Answered (human, 2026-10-12): enforce.** No change is needed. Core already re-exports every type the UI needs from `twicecat_core::api`, and `api/` never names `twicecat_audio`. (R): keep it that way and make it enforced:
- `scripts/check-app-deps.sh` also rejects `twicecat-audio`;
- `scripts/check-core-api.sh` also rejects `twicecat_audio` in `api/`.

`twicecat-ptt` is left to #15.

**Q3. Ringing. Answered (human, 2026-10-12): the core plays a ringtone.** Nothing can play a sound today. The options were: (a) a visual ring only, (b) a ringtone played by the core on the call output device, (c) a ringtone played by the app (a new dependency, against "all audio in the core"). The human chose (b). The ring is also visual: the card, window attention, and a desktop notification.

**Q3b. Ringtone API shape. Answered (human, 2026-10-12): the core rings by itself, as below. This is the approved API addition.**
- **The core rings by itself.** When it emits `IncomingCall { ring: true }` and is not in a call, it plays a ringtone on the output device: `AudioSettings::output_device`, master volume. The ringtone is synthesised in code (a two-tone pattern, about 1 s on and 2 s off), so there is no asset file and no licence question.
- **It stops** on the same events that end the ring: `RingingStopped`, `JoinCall`, `DeclineCall`, expiry, logout and shutdown. While several rooms ring, one tone plays.
- **`Command::SilenceRinging`** (new) stops the sound and keeps the ring pending. The app sends it when the card is hidden with Esc.
- **`AudioSettings::ringtone: bool`** (new field, serde default `true`, so the on-disk format stays compatible) turns the sound off. Shown in the Voice & audio card.
- **Not in scope:** a ringback tone for the caller, choosing a ringtone, a separate ringtone device or volume.
- **Implementation:** a playback-only `AudioSession` mode (`Mode::Ring`, `IoConfig { playback: true, capture: false }`), with a tone generator instead of the voice engine. The pinned `twicecat-audio` API is read first (rule 2). If ringing would need a change to `twicecat-audio`'s public API, the task stops and asks. Unit tests use `AudioBackend::Virtual` to check that the tone starts, is not silent, and stops on each event.

The alternative is an app-driven `Command::StartRingtone` / `StopRingtone`. It is more flexible, but the app would have to mirror the core's ring lifecycle, and a crashed or frozen UI would leave the tone playing.

**Q4. Where the call lives on screen. Answered (human, 2026-10-12): (a).**
- (a) (R) Discord-style:
  - A **call bar** at the bottom of the room list, visible from any room while in a call. It holds the room name, state (Connecting / Connected / Reconnecting (n) / ended with reason), an encrypted mark, Mute, Deafen, Leave, and a Voice & audio button.
  - A **participants panel** on the right of the room pane, like the thread panel, opened from the header's Call button when viewing the call room. Voice rooms open it automatically.
- (b) A dedicated call view that replaces the timeline for voice rooms, with ordinary rooms as in (a).

**Q5. Audio settings placement. Answered (human, 2026-10-12): (a).** Settings is one flat scrollable card (`screens/settings.rs`).
- (a) (R) A separate **Voice & audio** card, opened from a button in Settings (replacing the push-to-talk placeholder) and from the call bar. Esc closes it.
- (b) A long section inside the existing Settings card.
- (c) Turn Settings into tabbed pages.

**Q6. Which rooms get a call button. Answered (human, 2026-10-12): (a).**
- (a) (R) Every joined room except spaces and invites. It reads "Call" when no call is active (in a DM this rings the other side), "Join call (N)" when N devices are in it, and "Leave call" when we are in it.
- (b) Only DMs, voice rooms and rooms with an active call.

**Q7. Split with #15 (push-to-talk). Answered (human, 2026-10-12): (c). The input mode is left entirely to #15.**
- (a) (R) This plan shows the input mode, which is plain `AudioSettings` fields: voice activity with its automatic/threshold, or push-to-talk with the release delay. The push-to-talk bindings are shown read-only. #15 adds the service, key and button capture, and the in-window fallback.
- (b) This plan also does #15's settings UI.
- (c) Input mode is left entirely to #15.

**Q8. Keyboard. Answered (human, 2026-10-12): Ctrl+Shift+M and Ctrl+Shift+D.** Every new control is a focusable button, checkbox, pick list or slider in widget-tree order (`docs/accessibility.md`). Esc on the incoming-call card hides the card and does not decline; the ring stays shown in the room list until it stops. (R): add **Ctrl+Shift+M** (mute) and **Ctrl+Shift+D** (deafen), active only in a call, to `keys::SHORTCUTS`. Neither is used by the app or the composer today. Alternatively, no new shortcuts.

## Architecture
- **`crates/app/src/calls.rs` (new)** holds `CallModel` on `App`:
  - `call: Option<ActiveCallUi { room_id, phase, participants: Vec<Participant>, muted, deafened, members: Vec<RoomMemberView>, declined: Vec<UserId>, panel_open }>`;
  - `counts: HashMap<RoomId, u32>` (from `ActiveCall`, entries removed at 0);
  - `rings: Vec<Ring { room_id, caller, ring, hidden }>`;
  - `ended: Option<CallEndReason>` (a short-lived notice);
  - `settings: Option<AudioSettings>` (the last echo; edits are made on a clone and sent whole);
  - `devices: Option<AudioDevices>`, `level: Option<InputLevel>`, `stats: Option<AudioStats>`, `gpu: Option<GpuEnhancement>`;
  - `mic_test: bool`, `settings_open: bool`.

  All event handling is pure state change plus `Command`s. Nothing blocks.
- **`crates/app/src/screens/call.rs` (new):** the call bar, the participants panel and the incoming-call card.
- **`crates/app/src/screens/audio_settings.rs` (new):** the Voice & audio card.
- `state.rs`:
  - the ignore arm is replaced by `self.on_call_event(event)`;
  - `GetAudioSettings` is sent once the session is ready;
  - `App`'s hand-written `Debug` gains the model (no user IDs or room names are printed, only counts);
  - logging follows the existing rule: no message bodies, tokens or keys, and no display names.
- `room_list_pane.rs`: a voice-room mark (`tag("Voice")`) and a call-count pill next to the unread badge.
- `main_view.rs`: the header's Call / Join / Leave button and the panel slot.

## Tasks (one commit each, `feat(app)` except Task 4 (`feat(core)`), `Step: phase6.<n>` per Q1)

### Task 1: Call model and core events (phase6.17)
Files: `crates/app/src/calls.rs` (new), `crates/app/src/state.rs`, `crates/app/src/lib.rs`, `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` (Q2), `PROGRESS.md`, ledger.

Acceptance criteria. Unit tests on `App::update` for:
- `Joining` → `Connected { encrypted }` → `Reconnecting { attempt }` → `Connected` → `Left { Hangup }` clears the call and its participants;
- `Left { Failed(reason) }` keeps the reason as a notice;
- a participants `Reset`, and the other diff kinds through `ListDiff::apply`;
- `speaking` and `muted` flags changing;
- participants for another room are ignored;
- `ActiveCall` counts, with 0 removing the entry;
- `IncomingCall { ring: true/false }` queued per room with no duplicates, cleared by `RingingStopped` and by joining that room;
- `CallDeclined` recorded for the call we started;
- `AudioSettings`, `AudioDevices` (a second event replaces the list), `InputLevel`, `AudioStats` and `GpuEnhancement` stored;
- `Error(Call | Audio | NotInCall)` shown as a call notice, not as a generic error.

Also: `GetAudioSettings` is sent once after login. The dependency checks reject a fake `twicecat-audio` dependency (checked by a mutation, then reverted).

### Task 2: Room list and room header (phase6.18)
Files: `screens/room_list_pane.rs`, `screens/main_view.rs`, `calls.rs`, `room_list.rs`, PROGRESS.md, ledger.

Acceptance criteria:
- Voice rooms show a "Voice" tag.
- Rooms with `counts > 0` show a call pill with the count. A ringing room's pill uses the highlight tone.
- The header button follows Q6. Pressing it sends exactly one `JoinCall { room_id, intent: Audio }` or `LeaveCall`, and it is disabled while `Joining`. Unit tests cover each state, plus a double press sending once.
- Selecting a voice room does not auto-join (joining is always a click). This matches Element's voice rooms.

### Task 3: In-call view: call bar, participants, mute, deafen, per-user controls (phase6.19)
Files: `screens/call.rs` (new), `screens/main_view.rs`, `calls.rs`, `keys.rs` (Q8), `docs/accessibility.md`, PROGRESS.md, ledger.

Acceptance criteria:
- The call bar and participants panel follow Q4.
- Each participant row shows:
  - the avatar, from the call room's members, fetched with `ListRoomMembers` on `Joining` and refreshed when an unknown user joins, with the initials fallback;
  - the display name, falling back to the member name, then the user ID;
  - "(you)" on the local device;
  - a speaking ring around the avatar;
  - a muted mark;
  - the cleaning status (Cleaned / Skipped with its reason).
- Remote rows also have:
  - a volume slider from 0 to 200 % that sends `SetUserVolume` once on release;
  - a voice cleaning pick list (Automatic / Always / Never) that sends `SetUserCleaning`.
- The same user on two devices gets one control, keyed by user ID, because the core's settings are per user.
- Mute and Deafen toggle locally and send `SetMicMuted` / `SetDeafened`. Un-deafen restores the mute state from before the deafen, as Discord does: the core's deafen mutes the mic. Both reset on `Joining`.
- `CallDeclined` shows "<name> declined" in the bar.
- Unit tests cover each control's command, the per-user keying, the deafen/mute interplay, and the shortcuts acting only in a call.

### Task 4: Ringtone in the core (phase6.20, `feat(core)`)
Files: `crates/core/src/audio_session.rs` (a `Ring` mode), a new `crates/core/src/ringtone.rs` (the tone generator), `crates/core/src/engine/mod.rs` (start and stop at the ring events), `crates/core/src/api/{mod,audio}.rs` (`SilenceRinging`, `AudioSettings::ringtone`), the CLI (`/silence`, and the ringtone toggle in `/audio`), PROGRESS.md, ledger.

Acceptance criteria. Q3b as approved. Core tests with `AudioBackend::Virtual` show that:
- the tone starts on a DM ring and has a non-silent RMS;
- no tone plays for `ring: false`, while in a call, or with `ringtone: false`;
- it stops on `RingingStopped`, `JoinCall`, `DeclineCall`, `SilenceRinging`, expiry and logout;
- two ringing rooms give one tone;
- the old on-disk `call_settings.json` (no `ringtone` field) loads with `ringtone: true`.

Also: the ignored stack test `call_ringing.rs` gains the tone check on carol's virtual device. `scripts/check-core-api.sh` passes.

### Task 5: Incoming calls, with a stack round trip (phase6.21)
Files: `screens/call.rs`, `calls.rs`, `notifications.rs`, `state.rs` (overlay stack), `keys.rs` (`any_overlay_open`, `on_escape`), `settings.rs` (`other_modal_open`), `crates/app/tests/calls.rs` (new, `#[ignore]`, needs the stack), PROGRESS.md, ledger.

Acceptance criteria:
- `IncomingCall { ring: true }` opens a card with the caller's name and avatar, the room, and Accept (`JoinCall`) and Decline (`DeclineCall`). Each sends once. It also asks for window attention and shows a desktop notification when the window is unfocused (same `should_show` rules).
- `ring: false` gives only a quiet desktop notification and the ringing pill.
- `RingingStopped` and joining close the card. Several rings queue, oldest first.
- A ring for the room we are already in is ignored.
- Esc hides the card (Q8).
- The sound comes from the core (Task 4). Hiding the card sends `SilenceRinging`.
- Unit tests cover all of the above with `RecordingNotifier`.
- Stack test (`crates/app/tests/calls.rs`): the app runs as carol (real `Bridge`, `AudioBackend::Virtual`), with alice driven by a `twicecat-core` client the way `crates/core/tests/call_ringing.rs` does it.
  - (1) alice calls the DM, the app shows the card, Accept joins, both see 2 participants, the app leaves, and alice sees the count drop.
  - (2) alice calls, the app declines, and alice gets `CallDeclined`.

  Plus the CLI check from the issue: `twicecat-cli` `/call` against the app account, run by the agent and recorded in PROGRESS.md.

### Task 6: Voice & audio settings (phase6.22)
Files: `screens/audio_settings.rs` (new), `screens/settings.rs` (a "Voice & audio" heading with an **Open** button, Q5; the push-to-talk line stays for #15), `settings.rs` (`PUSH_TO_TALK_LINE` reworded to "Push-to-talk arrives in a later version."), `calls.rs`, `keys.rs`, `docs/accessibility.md`, PROGRESS.md, ledger.

Acceptance criteria. Opening the card sends `ListAudioDevices`, `ProbeGpuEnhancement` and `GetAudioSettings`, and the card contains:
- **Devices.** Input and output pick lists: "System default" (`None`) plus the devices. A saved device that is no longer listed shows as "<name> (not connected)" and stays selected. A hot-plug `AudioDevices` refreshes the lists without losing the choice. During a call, a change shows the core's "used from the next call" line.
- **Microphone processing.** Noise suppression: Off / WebRTC (with Low–Very high) / RNNoise / DeepFilterNet3 (with the attenuation limit, plus "Use the GPU" only when `GpuEnhancement::available` is not empty). Echo cancellation, "Also cancel other apps' sound" (disabled unless echo cancellation is on, with the core's Linux/PipeWire note), automatic gain, high-pass. The UI says what combines and what excludes, as the core's docs require.
- **Levels.** Input gain −20..20 dB, master volume 0..200 %, voice bitrate (presets 16, 24, 32, 48, 64, 96, 128 kb/s; the 510 maximum is kept if already set).
- **Input mode:** not shown (Q7, #15). An edit made in this card keeps the stored `input_mode` and `push_to_talk` unchanged, because the whole echoed value is sent back.
- **Mic test.** Start/Stop with "Hear myself" (loopback), disabled during a call with a reason. A level meter from `InputLevel` (RMS bar, peak tick, "transmitting" mark). Closing the card or joining a call sends `StopMicTest`.
- **Other voices.** Remote cleaning: on/off, attenuation limit, skip policy (Neural / Any suppressor / Never), GPU when available, and the backend in use from `AudioStats::remote_cleaning`.
- **Ringtone.** "Play a ringtone for calls" (`AudioSettings::ringtone`, Task 4).
- **Announcement.** "Tell others my microphone is cleaned", with the `CleaningAnnouncement` privacy note (seen by everyone in the call and by the call server's operator), and "My microphone is cleaned outside Twicecat".
- **Diagnostics line** (collapsed by default) from `AudioStats`: echo return loss enhancement, delay, processing µs, under- and overruns.

Every edit clones the last echoed `AudioSettings`, changes one field, and sends the whole value with `UpdateAudioSettings`. Sliders send on release. The UI always shows the echo, so a value the core clamps snaps back.

Unit tests cover:
- each control producing the right whole `AudioSettings`;
- the clamp echo winning;
- the unplugged device staying selected;
- `input_mode` and `push_to_talk` passing through unchanged;
- the hot-plug refresh;
- the mic test being refused in a call and stopped on close;
- the GPU options hidden without backends;
- the system-audio checkbox disabled without AEC.

### Task 7: Wrap-up (phase6.23, `docs(plan)` if only docs change)
- PROGRESS.md: a "Phase 6 call UI" section with the steps, notes, open issues and the human checkpoint below.
- `docs/accessibility.md` updated.
- The full gate and the ignored app stack tests, with output recorded.
- Issue #14's checklist mapped to commits.

## Human checkpoint (collected in PROGRESS.md, never ticked by the agent; rule 9)
- [x] Plan questions answered and plan approved (2026-10-12).
- [ ] A real call with an Element Call user in an encrypted room, both directions: join and leave from the header, participants with speaking and muted marks, mute and deafen heard on the other side, per-user volume and cleaning by ear.
- [ ] Ringing with Element Web / Element X: a DM call plays the ringtone on the chosen output device, shows the card and asks for attention; Esc silences it, declining stops it on the other side, answering on another device closes it here; a group call notifies quietly.
- [ ] Device pickers with a USB and a Bluetooth headset, plugged in and out while the card is open and during a call.
- [ ] The mic test meter and loopback on headphones; each processing option by ear.
- [ ] Rendering of the call bar, panel, ring card and Voice & audio card in light and dark themes on Linux, Windows and macOS; keyboard-only use (Tab order, Esc, Ctrl+Shift+M/D).
