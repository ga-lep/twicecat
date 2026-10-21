# Call Sounds Plan
Status: APPROVED

Note (later change): the call sounds volume range was capped at 100 % (it was 0-200 below); a stored higher value loads as 100.

## Goal
Play short, quiet sounds for three call moments, so the user hears what happened without looking:
- **Own join:** a rising two-note chime when this device joins a call in a room. That covers both joining a call that is going and starting a new one, because both are `Command::JoinCall` ("Join (or start) the MatrixRTC session of a room", `crates/core/src/api/mod.rs:424`).
- **Own leave:** a falling two-note chime when the user leaves the call.
- **Other joins:** a softer single blip when another person joins the call we are in.

Each cue plays once per event, on the call's own output device, mixed into the call's playback, so it is part of the echo canceller's reference. It plays regardless of mute, push-to-talk and deafen. It can be turned off and its volume set on the Voice & audio "Ringtone" page.

A sound when another person leaves is not included (Q8, defaulting to no).

The work goes on the existing branch `feat/ui-ux-rework`, after the `ui.*` commits (HEAD `97c9e5a`), with `Step: snd.N` trailers. No new branch is made.

### What exists today (surveyed at `97c9e5a`)
- **`crates/core/src/engine/calls.rs` is not compiled.** `crates/core/src/engine/mod.rs` has no `mod calls;`. The file was last touched by the rename commit `b524e32`, and its `join_call`/`leave_call` are stale copies. The live call code is in `crates/core/src/engine/mod.rs`:
  - `join_call` (line 1747): when the call is already in this room it re-emits `Connected` and returns; when another call is running it calls `end_call(true)` first. Then it emits `Joining`, runs `start_call_audio()` and `CallSession::join`. On `Ok` it sets `self.call` and emits `Connected` (:1806); on `Err` it stops the audio and emits `Left { Failed }`.
  - `on_media_notice` (:1895): `Reconnecting`; then `Reconnected`, which emits `Connected` again; and `Disconnected`, which emits `Left { Failed }`.
  - `end_call(joining_next)` (:1999): `call.leave()`, `stop_call_audio()`, then `Left { Hangup }`. Its callers:
    - `Command::LeaveCall` (:1382; refused with `NotInCall` when there is no call);
    - `join_call` when switching rooms (`end_call(true)`);
    - the end of the engine loop (:1015);
    - `end_session_audio` (:3071);
    - `logout` (:3117).

  Q9 asks whether to delete the dead file in this work.
- **The ringtone** (`crates/core/src/ringtone.rs`):
  - a tone made in code (440 and 480 Hz sines, 10 ms fades, 0.15 for each sine), so there is no file or licence;
  - a separate playback-only `AudioIo` (`Ringer`), silent in a call;
  - unit tests on a virtual `Speaker`.

  "Announce" in Voice & audio is not a sound: it is `CleaningAnnouncement` (`api/audio.rs:126`). The repository has no audio assets.
- **The call's playback** (`crates/audio/src/voice/mod.rs`, `VoiceEngine::process`):
  - it mixes the sources and adds the mic-test loopback;
  - when deafened it writes `SILENCE`, otherwise `(mix × master_gain).clamp(-1, 1)`;
  - then `self.chain.render(...)` feeds the echo canceller either the device's system echo reference (`played`, only with `cancel_system_audio`) or exactly the frame just written;
  - then the mic is processed into `Outgoing`.

  So what is written into `playback` before `render` is in the echo canceller's reference, and nothing in `playback` ever reaches `Outgoing`, which is only the processed mic.
- **Participants** (`crates/core/src/call/participants.rs`, `watch_participants`):
  - it re-reads the active `m.call.member` memberships on every room-info change, on every flag change, and every 5 s;
  - each `Membership` carries `user_id`, `device_id`, `member_id`, `backend_identity` (`"{user}:{device}"`) and `joined_ms`;
  - `seen_own` notes when our own membership first appears, and `ours.gone` re-sends it if it vanishes. That re-send does not go through `join_call`;
  - it emits `CallParticipants` as a full `Reset` when the list changes.
- **Settings:** `AudioSettings` (`api/audio.rs:357-416`) has `#[serde(default)]`, is clamped in `validated()` with `limits::VOLUME_PERCENT` = 200, and is persisted in `call_settings.json` (version 1). The engine applies a change to a running call with `call.set_cleaning(&settings)` from `store_audio_settings` (`engine/mod.rs:1722`).
- **The app:**
  - the `AudioPage::Ringtone` page has one checkbox (`AudioMessage::Ringtone`) at `screens/audio_settings.rs:787`;
  - sliders are drawn by the shared slider row at `screens/audio_settings.rs:455-480`, which already wraps `slider` in `crate::ui::guard::no_wheel` and uses −/+ buttons for keyboard steps;
  - menus use `kit::menu` with `no_wheel`;
  - the gallery shot is `audio-ringtone`.

### Decisions
- **Triggers.** Each cue is played in exactly one place:
  - **Own join:** in `join_call`'s `Ok` branch only, after `self.call = Some(session)` and before `Connected` is emitted. It does not play:
    - on the early return for the call we are already in;
    - on `MediaNotice::Reconnected`;
    - on `Err`;
    - when our membership is re-sent after `ours.gone`, because that path does not go through `join_call`.
  - **Own leave:** only in the `Command::LeaveCall` arm, when a call exists. The `end_call` signature gets a `leave_sound: bool` argument; only that arm passes `true`. It does not play:
    - on a `LeaveCall` without a call (`NotInCall`), so not on a repeated leave;
    - on `Disconnected` or a failed join;
    - when switching rooms (the join chime of the new room follows);
    - on shutdown, at the end of a session, or on logout.
  - **Other joins:** in `watch_participants`, through a pure detector `Arrivals` (unit-tested without Matrix). It does not play:
    - for the first read of the list: that read is the baseline, so people already present when we join make no sound;
    - for any membership of our own user, including our other devices;
    - before our own membership has been seen (`seen_own`), so not while `Joining`;
    - for a `member_id` that was in the call within the last 60 s and comes back, which covers re-announcements, expired-and-refreshed memberships and the other person's reconnect;
    - more than once per 3 s: arrivals inside that window are folded into the cue already played.
- **How the cues play: a one-shot cue mixed into the call's `VoiceEngine`.** It is not a second device stream like `Ringer`, so it is on the call's output device and in the echo canceller's reference.
  - **Deafen bypass (Q3):** the cue is mixed into its own frame. When deafened, playback becomes `cue × master_gain` instead of `SILENCE`; otherwise it is `(mix + cue) × master_gain`, clamped. `render` then runs on that playback frame, unchanged, so the cue stays in the echo reference while deafened too. With `cancel_system_audio`, the device's own echo reference already contains it.
  - The cue's samples are `&'static [f32]`, made once in a `OnceLock`, so the audio thread never allocates or frees.
- **Echo (Q4): echo cancellation only, with no mic hold.** Because the cue is in the echo reference, the echo canceller removes it from the mic like any remote voice. Remaining risks, documented in the core rustdoc, PROGRESS.md and the checkpoint:
  - **Echo cancellation off ("Echo cancellation" unticked):** on speakers, the mic picks the cue up and sends it to the room, exactly as it would send the other people's voices. With headphones nothing leaks.
  - **Echo cancellation on, but not yet converged:** at the very start of a call the canceller has had no far-end signal, so a faint trace of the join chime may leak on speakers.

  Short, quiet cues keep both risks small.
- **Sounds (Q5).** All are synthesised in code: no file, no licence, no crate. The frequencies stay clear of the 1 kHz test tone in `crates/core/tests/call_media.rs`.

  | Cue | Notes | Length | Peak at 100 % | Peak at the default 60 % |
  |---|---|---|---|---|
  | Own join | E5 659.25 Hz, then A5 880 Hz (rising) | about 240 ms | 0.1 (−20 dBFS) | about −24.4 dBFS |
  | Own leave | A5, then E5 (falling) | about 240 ms | 0.1 (−20 dBFS) | about −24.4 dBFS |
  | Other joins | one G5 784 Hz blip | about 90 ms | 0.05 (−26 dBFS) | about −30.4 dBFS |

  Each note has a 5 ms attack, an exponential decay (τ ≈ 50 ms) and a 15 ms fade to exact zero, so there is no click. The peaks are also scaled by the master volume.
- **Setting (Q1 approved, nested).** A new field `AudioSettings::call_sounds: CallSounds`:

  | Field | Meaning | Default |
  |---|---|---|
  | `enabled` | Master switch; on its own it gives the own-join cue | `true` |
  | `others_joining` | Also play when others join | `true` |
  | `leaving` | Also play when I leave | `true` |
  | `volume_percent` | Cue volume | 60 |

  `volume_percent` is clamped to `limits::VOLUME_PERCENT` and applied on top of the master volume. `#[serde(default)]` makes files without the field read as the defaults. When `enabled` is false, no cue plays.
- **App (Q6).** The page stays "Ringtone". A new "Call sounds" section on it has:
  - "Play call sounds" (master);
  - "Also when others join";
  - "Also when I leave";
  - a "Call sounds volume" slider.

  **Deviation (labels, recorded after Task 4):** the shipped control names are "Play a sound when you join a call", "Also when someone else joins the call", "Also when you leave the call" and "Sounds volume" (`crates/app/src/screens/audio_settings.rs`), to match the wording of the neighbouring ringtone controls. Behaviour is as planned.

  The two sub-boxes and the slider are disabled while the master is off. There is no preview button and no preview command (Q7).

### Rules for every task (AGENTS.md)
- **Rule 2:** this plan uses no external API that is not already in use. `rtrb` already carries `VoiceEngine`'s commands, and `OnceLock` is std. The iced `slider`/`checkbox` calls copy the ones already on the card. Anything else is checked in the registry source of the pinned version and cited file:line in PROGRESS.md.
- **Rule 3:** no pinned version changes and no new crate, asset or feature.
- **Rule 7:**
  - `crates/app` uses only `twicecat_core::api`;
  - the new `twicecat-audio` method is used only inside `crates/core`, never named in `crates/core/src/api`;
  - `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` pass after every task.
- **Rule 5:**
  - one commit per task on `feat/ui-ux-rework`, type `feat`, scope `audio`, `core` or `app`, trailer `Step: snd.N`;
  - fixes and tests are folded in with `--fixup` and `rebase --autosquash`;
  - each commit updates `PROGRESS.md` (new "Call sounds steps" and "Call sounds human checkpoint" sections) and the ledger `docs/plans/call-join-sound.progress.md`;
  - this plan is committed as `docs(plan): …` before Task 1, once approved.
- **Gate before each commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features`;
  - `cargo test --workspace`;
  - `cargo check --workspace --lib --bins --locked`;
  - `scripts/check-app-deps.sh`, `scripts/check-core-api.sh`, `scripts/check-no-webrtc-download.sh`, `scripts/check-font-assets.sh` and `scripts/check-commits.sh main`.
- **Lessons carried over from the UI rework:**
  - every new slider goes through `crate::ui::guard::no_wheel`, because Ctrl + wheel is zoom and iced's slider would step on it;
  - any new menu uses `kit::menu` inside `no_wheel`;
  - the ledger records honestly which tests were red first and which were made red afterwards by a mutation (named, then reverted);
  - no test is described as red-first unless it was seen failing.
- **Rules 6 and 9:**
  - tests use virtual audio devices, and the ignored end-to-end tests use only the local stack and `.env.test`;
  - nothing logs message bodies, tokens or user IDs; a cue logs only its kind;
  - hearing the cues, deafened behaviour, leaks with echo cancellation off, and real devices on each OS go on the human checkpoint.

### Task 1: A one-shot cue in the voice engine's playback
Language: rust
Files:
- `crates/audio/src/voice/control.rs`: `Command::Cue(&'static [f32], u16)` and `pub fn play_cue(&mut self, samples: &'static [f32], volume_percent: u16) -> bool`.
- `crates/audio/src/voice/mod.rs`:
  - the cue state on `VoiceEngine` (samples, position, gain);
  - mixing it in `process` before `render`, with the deafen bypass;
  - a `#[cfg(test)]` copy of the frame handed to `chain.render`, for the reference test.
- `crates/audio/src/voice/tests.rs`.
- `PROGRESS.md`; `docs/plans/call-join-sound.progress.md` (new).
Depends on: none
Acceptance criteria:
- The cue's samples are played in order, added to the remote mix, across frame boundaries, followed by silence. Test: a 2.5-frame ramp with one source at a constant 0.1.
- At master 50 % and cue 50 %, the played samples are 0.25 × the cue (±1e-6).
- When deafened, the played frames are exactly `cue × master_gain`: the cue plays and the remote mix does not.
- The frame handed to the echo canceller's `render` contains the cue, both deafened and not. Test: the `#[cfg(test)]` copy equals the played frame while a cue plays, with `echo_cancellation: true`.
- The cue never reaches `Outgoing`. Test: capture is silence and the VAD threshold is at the floor; every `Outgoing` frame stays silence while a cue plays.
- Mute and push-to-talk do not change what the cue plays.
- A second `play_cue` while one plays restarts with the new cue, without mixing the two.
- The cue path does not allocate in `process`. Checked with the `voice::tests` allocation counter if one exists, otherwise by reading the code; the ledger says which.
- Mutations, each recorded in PROGRESS.md and reverted:
  - mixing the cue after `render` fails the reference test;
  - letting deafen silence the cue fails the deafen test;
  - adding the cue to the mic fails the `Outgoing` test.
- The gate passes. The commit is `feat(audio): play a one-shot cue in the call's playback, deafened too` with `Step: snd.1`.
Evidence: `cargo test -p twicecat-audio --lib voice::tests::cue` passes the new tests.

### Task 2: Settings, the cue sounds, and the own join and leave cues in the core
Language: rust
Files:
- `crates/core/src/api/audio.rs`: `pub struct CallSounds { enabled, others_joining, leaving, volume_percent }` with `Default`, `AudioSettings::call_sounds`, the clamp in `validated()`, rustdoc (including the echo-cancellation-off risk), and the serde tests.
- `crates/core/src/api/mod.rs`: re-export `CallSounds` like the other audio types.
- `crates/core/src/call_sounds.rs` (new, next to `ringtone.rs`):
  - `enum Cue { Joined, Left, OtherJoined }`;
  - `fn samples(Cue) -> &'static [f32]` through `OnceLock`;
  - `fn volume(&CallSounds, Cue) -> Option<u16>`;
  - module docs giving the decisions above.
- `crates/core/src/lib.rs`: `mod call_sounds;`, private.
- `crates/core/src/audio_session.rs`: `pub(crate) fn play_cue(&self, samples: &'static [f32], volume_percent: u16)`.
- `crates/core/src/engine/mod.rs`:
  - `join_call`'s `Ok` branch plays `Cue::Joined`;
  - `end_call` gets `leave_sound: bool`. With `true`, it takes the session out, plays `Cue::Left` after `call.leave()`, and closes the devices in a task once the cue has played (cue length + 50 ms). That task is stored as `self.closing`, and `start_call_audio`/`start_mic_test` await it first, so two sessions never overlap;
  - only the `Command::LeaveCall` arm passes `true`;
  - comments at the other `Connected` and `Left` sites say why they stay silent.
- `crates/core/src/bin/twicecat-cli.rs` and `crates/core/src/bin/twicecat-cli/calls/{parse,mod,show}.rs`: `/sounds [on|off|others on|off|leave on|off|<0-200>]`, and a line in the `/audio` summary.
- `crates/core/tests/call_sounds.rs` (new; helpers from `tests/common`), plus `call_ringing.rs` and `call_media.rs`: existing ignored tests whose virtual speaker asserts silence or measures levels turn `call_sounds.enabled` off.
- `PROGRESS.md`, ledger.
Depends on: Task 1
Acceptance criteria:
- Unit tests for each cue:
  - the length matches the table (±20 ms);
  - the peak at 100 % is within 5 % of the table;
  - the first and last samples are 0;
  - the largest step between samples is < 0.02;
  - energy at 1 kHz (Goertzel) is ≥ 30 dB below the cue's main note;
  - join rises: the first half's dominant note is lower than the second half's; leave falls;
  - `samples()` returns the same pointer twice.
- `volume()`:
  - returns `None` for every cue when `enabled` is false;
  - returns `None` for `OtherJoined` when `others_joining` is false, and for `Left` when `leaving` is false;
  - returns `Some(volume_percent)` otherwise.
- `call_settings`:
  - a version-1 file without `call_sounds` loads the defaults (all on, 60) with no problem;
  - a changed value round-trips;
  - a volume of 900 is clamped to 200.
- `scripts/check-core-api.sh` passes. PROGRESS.md records the additive API change, approved in Q1.
- Ignored end-to-end tests on the local stack (`#[ignore = "needs local homeserver: scripts/stack.sh up -d --wait"]`), alice and carol, with virtual speakers recording what they play. A cue is detected by its notes' energy in 10 ms windows.
  - Alice joins (starting the call): her speaker plays one rising cue. Carol joins the same call: her speaker plays one rising cue.
  - Alice sends `JoinCall` again for the same room: `Connected`, and no cue within 1 s.
  - Alice deafens, then leaves: her speaker still plays the falling cue before it stops, and `Left { Hangup }` arrives without waiting for the cue.
  - A second `LeaveCall` gives `NotInCall` and no cue.
  - With `enabled: false`, joining and leaving play nothing. With `leaving: false`, leaving plays nothing and joining still plays.
  - A join that fails (call service override pointing at a closed local port): `Left { Failed }` and no cue.
  - Switching from room A's call to room B's: only B's join cue, no leave cue.
  - The cue never reaches the room: carol's recording during alice's join, with alice's virtual mic silent (no echo path), has no cue energy above −60 dBFS.
- The reconnect path cannot be forced on the local stack. The ledger records that `Reconnected` has no cue call, from the single call site, and keeps a human checkpoint item for it.
- `/sounds off` then `/audio` prints "Call sounds off.". `/sounds 40` prints "Call sounds on (others joining, leaving), 40 %." There are parse tests as for `/ringtone`.
- The full ignored run of the call tests on the local stack passes. The ledger lists the existing tests changed to turn sounds off.
- Mutations, each recorded and reverted:
  - playing the join cue on the early return of the repeat join fails the repeat-join test;
  - playing the leave cue on `end_call(true)` fails the switching test.
- The gate passes. The commit is `feat(core): call sounds for joining, starting and leaving a call` with `Step: snd.2`.
Evidence: on the local stack, `cargo test -p twicecat-core --test call_sounds -- --ignored` passes. In `twicecat-cli`, `/sounds off` then `/audio` prints "Call sounds off.".

### Task 3: The cue when another person joins
Language: rust
Files:
- `crates/core/src/call/participants.rs`:
  - `struct Arrivals`, a pure detector. `fn update(&mut self, active: &[Membership], own_user: &str, own_seen: bool, now: Instant) -> bool` returns true when a cue is due. It holds the baseline flag, the member IDs present, a recently-left map (60 s), and the time of the last cue (3 s);
  - `watch_participants` calls it and plays `Cue::OtherJoined` through a handle in `Ours`.
- `crates/core/src/call/mod.rs`:
  - `Ours`/`CallSession` carry a `CueHandle`: the `Voice` and a `watch::Receiver<CallSounds>`;
  - `CallSession::set_sounds(&CallSounds)`, called from `store_audio_settings` next to `set_cleaning`.
- `crates/core/src/call/participants.rs` tests.
- `crates/core/tests/call_sounds.rs`: an other-join case.
- `PROGRESS.md`, ledger.
Depends on: Task 2
Acceptance criteria:
- Unit tests on `Arrivals`, with made-up `Membership` values on `example.org`:
  - the first update never cues, whoever is there;
  - before `own_seen` there is no cue;
  - a new member of another user cues once;
  - a new device of our own user does not cue;
  - a member that leaves and comes back within 60 s does not cue, and after 60 s it does;
  - an unchanged list, or a refresh of the same member with a new `joined_ms`, does not cue;
  - three arrivals within 3 s give one cue, and an arrival 3 s after the cue gives another.
- With `others_joining: false` (set while in the call through `set_sounds`), an arrival plays nothing.
- End-to-end on the local stack (ignored):
  - carol is in the call;
  - alice joins: carol's speaker plays one blip (G5) and alice's does not (she hears her own rising cue only);
  - bob is not used, as in the other call tests;
  - alice joins again with the repeat `JoinCall`: no blip for carol.
- Mutations, each recorded and reverted:
  - no baseline (the first update cues) fails the baseline test;
  - not excluding our own user fails the own-device test;
  - removing the 3 s window fails the burst test.
- The gate passes. The commit is `feat(core): a soft cue when someone joins the call` with `Step: snd.3`.
Evidence: `cargo test -p twicecat-core --lib call::participants::arrivals` passes. On the local stack, `cargo test -p twicecat-core --test call_sounds -- --ignored other_joins` passes.

### Task 4: The "Call sounds" section on the Ringtone page
Language: rust
Files:
- `crates/app/src/audio_settings.rs`:
  - `AudioMessage::CallSounds(bool)`, `OthersJoining(bool)` and `LeavingSound(bool)`;
  - `Slider::CallSoundsVolume` on `AudioPage::Ringtone` (0–200 %, step 5);
  - `sends()` for `Ringtone`;
  - tests.
- `crates/app/src/screens/audio_settings.rs`: `ringtone_page` gets a "Call sounds" heading with three checkboxes, a note, and the slider. The slider uses the shared row, already wrapped in `ui::guard::no_wheel` with −/+.
- `crates/app/src/settings.rs`: `VOICE_AUDIO_LINE` mentions call sounds.
- `crates/app/tests/ui_gallery.rs`: shots `audio-ringtone` (all on) and `audio-ringtone-sounds-off`.
- `PROGRESS.md`, ledger.
Depends on: Task 2
Acceptance criteria:
- Each checkbox sends one `Command::UpdateAudioSettings`, with only its own field flipped. Tests follow `(AudioMessage::Ringtone(false), |s| !s.ringtone)`.
- The slider: `Slide` then `SlideReleased` sends one update with `volume_percent` set, and `Step` moves it by 5, clamped to 0 and 200.
- With the master off, the two sub-boxes and the slider are disabled (no `on_press`/`on_toggle`), as found by the gallery's control walk.
- A wheel event with Ctrl over the new slider changes nothing. Test as for the existing sliders; red-first if written before the row is used, otherwise made red by removing `no_wheel` for the new slider.
- The page has no `pick_list` outside `kit::menu`, by the existing check in `ui_gallery.rs`.
- The `sends()` consistency test and the existing gallery layout checks (no overlap, nothing outside the card at 1280×720 and in the narrow window) pass.
- `scripts/check-app-deps.sh` passes.
- The gate passes. The commit is `feat(app): call sounds settings on the Ringtone page` with `Step: snd.4`.
Evidence: `cargo test -p twicecat-app --test ui_gallery` writes `audio-ringtone.png` and `audio-ringtone-sounds-off.png`. In the app, Voice & audio > Ringtone shows the "Call sounds" section.

## Human checkpoint (added to PROGRESS.md "Call sounds human checkpoint" by the task named)
- (Task 2) Hear the join cue when joining an existing call and when starting one, and the leave cue when leaving. Check that both are recognisable (rising and falling), quiet at the default 60 %, and not startling on headphones.
- (Task 2) Deafen, then leave: the leave cue is still heard. Deafen during a call: other voices go silent, and the cues still play.
- (Task 3) Someone joins your call: a soft blip. Several people join at once: one blip. Someone who drops and rejoins within a minute: no blip. Your own second device joins: no blip.
- (Task 2) Echo: on speakers, ask the other person whether they hear your join cue, with echo cancellation on, then with "Echo cancellation" off. With it off the cue is expected to leak; record how loud it is.
- (Task 2) Network drop and reconnect (unplug for 5–10 s): no join cue when it comes back.
- (Task 2) Windows (WASAPI) and macOS outputs; a Bluetooth headset, where the switch to the headset profile at call start may swallow part of the join cue.
- (Task 4) The Ringtone page with the new section in a real window: light and dark, Compact density, the disabled state, and Ctrl+wheel over the slider (zoom only).

## Questions
- **Q1 (resolved):** a nested `CallSounds` struct in `AudioSettings`, an additive core API change, approved.
- **Q2 (resolved):** own join and start, own leave, and others joining are in scope.
- **Q3 (resolved):** cues play always: muted, push-to-talk and deafened. The deafen bypass is described in Decisions.
- **Q4 (resolved):** echo cancellation only, no mic hold. The risk is documented, and the cue is tested to be in the echo reference.
- **Q5 (resolved):** quiet, about −24 dBFS at the default 60 %, gentle fades, a volume setting.
- **Q6 (resolved):** the page stays "Ringtone", with a "Call sounds" section on it.
- **Q7 (resolved):** no preview button and no preview command.
- **Q8 (resolved): another person leaving.** Not included; a follow-up. The human approved the plan defaults (plan approved 2026-10-14). Original question: Not included by default. Adding it is small (a `Departures` twin of `Arrivals`, a falling soft blip, an "Also when others leave" box). It needs care with expiries, because a membership that times out looks like a departure. Include it, or leave it as a follow-up?
- **Q9 (resolved): the dead `crates/core/src/engine/calls.rs`.** Deleted in Task 2, with the human's approval of the plan defaults (2026-10-14). Original question: It is not compiled and holds stale copies of `join_call` and `leave_call`. Delete it in Task 2, which is recommended so that nobody edits the wrong file, or leave it?
- **Q10 (resolved): the other-join blip when I am alone.** It plays always, with the human's approval of the plan defaults (2026-10-14). Original question: When you started a call and are waiting, the first person to join is the moment the blip helps most. The plan plays it. Should it play only when you are alone, or always (the plan's default)?
