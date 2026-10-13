# Phase 6 Push-to-Talk Plan
Status: APPROVED (questions Q1–Q6 answered and the plan approved by the human, 2026-10-13); Tasks 1–6 done 2026-10-13, waiting for the human checkpoint

## Goal
Wire the finished push-to-talk service (`crates/ptt`) into the desktop app. This covers [ga-lep/twicecat#15](https://github.com/ga-lep/twicecat/issues/15), the "Push-to-talk" part of roadmap Phase 6, which `docs/plans/phase6-calls.md` Task 11 left for the call UI and `docs/plans/phase6-call-ui.md` Q7 (c) left entirely to this issue. The work happens on branch `15-featapp-wire-push-to-talk-into-the-app`, cut from `main` at `62639d6`.

Today `crates/app` does not depend on `twicecat-ptt`, never starts the service, and never sends `Command::PushToTalk`. The Voice & audio card passes `input_mode` and `push_to_talk` through untouched, and Settings says "Push-to-talk arrives in a later version." (`PUSH_TO_TALK_LINE`, `crates/app/src/settings.rs:45`). Only the CLI uses push-to-talk (`/ptt`, as a toggle). After this plan the app:
- shows and edits the whole input mode in the Voice & audio card: voice activity (automatic, or a threshold), push-to-talk, toggle-to-talk (Q2), and the release delay;
- captures the key binding from a key press, and the gamepad button (Q3);
- runs the service on the main thread while it is needed (Q1), restarts it when the bindings change, and drops it when it is not;
- forwards `Transmit` to the core as `Command::PushToTalk` without going through Iced's `update`;
- shows what each source reports: the key the Wayland portal confirmed (`Ready`), or why a source is not available (`Unavailable`), per source;
- falls back to the key pressed in Twicecat's own window when there is no global shortcut (gamescope, compositors without the portal);
- ships a `.desktop` file with the app ID the portal knows us by, `io.github.ga_lep.Twicecat`.

Rules for every task (AGENTS.md):
- **Rule 2:** every `iced` 0.14 API the app has not used before (`keyboard::Event::KeyReleased`, `key::Physical::Code`, …) and every `twicecat-ptt` API is read in the source first and cited file:line in PROGRESS.md. `global-hotkey` 0.8.0, `ashpd` 0.13.13 and `gilrs` 0.11.2 stay behind `twicecat-ptt`: the app never names them.
- **Rule 3:** no pinned version changes and no new crate. `twicecat-ptt` is a workspace crate (Q4).
- **Rule 5:**
  - one commit per task, `feat(<scope>)` with a `Step: phase6.<n>` trailer (numbering in Q6);
  - review fixes are folded in with `--fixup` and `rebase --autosquash`;
  - PROGRESS.md and `docs/plans/phase6-push-to-talk.progress.md` are updated in the same commit.
- **Rule 7:** the app talks to the core only through `Command` and `CoreEvent`. The app may depend on `twicecat-ptt` (Q4), which has no `matrix-sdk`, `livekit` or `twicecat-audio` in its tree; the dependency check enforces that.
- **Rule 9 and the issue's constraints:** the hotkey, the portal dialog, gamepads, the macOS prompts and SteamOS go on the human checkpoint and are never ticked by the agent. Key presses are never logged; bindings are logged by name only.
- **Gate before each commit:**
  - `cargo fmt --all`;
  - `cargo clippy --workspace --all-targets -- -D warnings`, and again with `--all-features`;
  - `cargo test --workspace`;
  - `scripts/check-app-deps.sh` (also with `livekit`), `scripts/check-core-api.sh`, `scripts/check-commits.sh main`;
  - from Task 3 on, the app's ignored tests that need no homeserver (`crates/app/tests/ptt.rs`, below).

## Facts verified while planning (2026-10-13)
- **The service** (`crates/ptt/src/lib.rs`):
  - `PttService::start(PttConfig, &tokio::runtime::Handle, Sink) -> PttService` (`lib.rs:149`). `PttConfig { key: Option<KeyBinding>, gamepad: Option<GamepadButton>, keyboard: KeyboardBackend }` (`lib.rs:96-104`); `KeyboardBackend::detect()` picks `Portal` (Wayland), `Hotkey` (X11, Windows, macOS) or `WindowOnly` (gamescope, no display) (`lib.rs:58-90`, tested).
  - `Sink = Arc<dyn Fn(PttEvent) + Send + Sync>`, called from the service's threads, in order, under an internal lock (`lib.rs:48-50`): it may only send on a channel.
  - `PttEvent::Transmit(bool)`, `Ready { source, trigger }`, `Unavailable { source, reason }` (`lib.rs:36-45`). `Source` is `Hotkey | Portal | Gamepad | Window` (`merge.rs:6-16`).
  - `PttService::window_key(pressed)` feeds the window source (`lib.rs:192`). Sources are merged: any one held means transmit, and only changes are reported (`merge.rs:34-50`). Dropping the service stops the sources and reports `Transmit(false)` if it was transmitting (`lib.rs:208-225`).
  - The service must be started and dropped on the main thread: global-hotkey needs it on macOS, and on Windows a thread that runs a message loop (`lib.rs:135-138`, `hotkey.rs:4-7`).
  - Bindings are strings in the settings: `KeyBinding` parses global-hotkey syntax (`ctrl+shift+KeyT`, `F13`) and prints `shift+control+KeyT` (`binding.rs:53-67`); `GamepadButton` parses 19 position names (`south`, `left_thumb`, …; `binding.rs:131-151`). Neither exposes its key code or modifiers today (`hotkey()` and `button()` are `pub(crate)`).
  - The portal registers the host app ID `io.github.ga_lep.Twicecat` (`register_host_app`, best effort), binds `push-to-talk` with a suggested trigger, and reports the confirmed trigger description as `Ready` (`portal.rs:57-82`).
  - There is no way to learn which gamepad button was pressed outside the bound one (`gamepad.rs:46-60`).
- **The core** (`crates/core/src/api/audio.rs`, `engine/mod.rs`):
  - `InputMode::VoiceActivity { automatic, threshold_db } | PushToTalk { release_delay_ms }` (`audio.rs:322-335`); `release_delay_ms` is clamped to 0..=2000 (`limits::PTT_RELEASE_DELAY_MS`, `audio.rs:417`) and is the audio gate's release tail.
  - `PushToTalkBindings { key: Option<String>, gamepad_button: Option<String> }` with `#[serde(default)]` (`audio.rs:337-347`). The core only checks that they are short printable ASCII (`audio.rs:486-499`).
  - `Command::PushToTalk { pressed }` (`api/mod.rs:445-450`) reaches `AudioSession::push_to_talk` when an audio session runs (a call or the mic test), and is ignored otherwise (`engine/mod.rs:1418-1422`). There is no toggle mode in the core.
  - `InputLevel::transmitting` reports the gate's state (`audio.rs:537-538`), so the meter already shows the release tail.
- **The app:**
  - Iced 0.14 calls `update` from the winit event loop on the main thread (`iced_winit-0.14.1/src/lib.rs:270-281`, `run_instance` polled in the event loop; `program.update` at `lib.rs:1314`). The tray is already created there for the same reason (`crates/app/src/tray.rs:120-122`, `state.rs:758-762`). So starting and dropping the service from `update` meets global-hotkey's rule.
  - Keyboard: `iced::keyboard::Event::KeyPressed { physical_key, modifiers, repeat, .. }` and `KeyReleased { physical_key, modifiers, .. }` (`iced_core-0.14.0/src/keyboard/event.rs:12-55`). `key::Physical::Code(Code)` uses the W3C names (`KeyT`, `Digit5`, `F13`, `Backquote`; `key.rs:849-985`) that global-hotkey's parser accepts case-insensitively (`global-hotkey-0.8.0/src/hotkey.rs:233`), except that W3C `Meta*` is `Super*` (a modifier, never the bound key).
  - The app listens to window events in one `listen_with` (`state.rs:2521-2527`), already including `Focused`/`Unfocused`. Shortcuts look only at `KeyPressed` (`keys.rs:350-361`).
  - The command sink is `Box<dyn CommandSink>` (`state.rs:106-148`); the real one is `Bridge`, which owns the core's `mpsc::Sender<Command>` and the core runtime's `Handle` (`bridge.rs:132-137`).
  - `scripts/check-app-deps.sh` rejects direct dependencies named `matrix-sdk`, `ruma`, `eyeball`, `imbl`, `twicecat-audio`. `twicecat-ptt` would pass it as it stands; its own tree is not checked.
- **No `.desktop` file exists** in the repo. The app ID is used by the portal (`APP_ID`, `lib.rs:31`); the keychain service name is the same string.

## Questions for the human
Each question has a recommendation, marked (R).

**Q1. When the service runs. Answered (human, 2026-10-13): (a).** On X11, Windows and macOS the hotkey is grabbed: while it is registered, other apps do not receive that key. On Wayland, starting it shows the portal's dialog the first time.
- (a) (R) While the input mode is push-to-talk or toggle **and** one of these holds: a call is joining or connected, the mic test runs, or the Voice & audio card is open (so the user confirms the portal dialog and sees `Ready`/`Unavailable` where the binding is set). The key is free the rest of the time.
- (b) Whenever the input mode is push-to-talk or toggle and a session is logged in (Discord-like). The key is grabbed as long as Twicecat runs.

**Q2. Toggle-to-talk** (roadmap step 6, issue scope). The core has no toggle mode. **Answered (human, 2026-10-13): both push-to-talk and toggle-to-talk are offered, and chosen in the GUI.** Where the choice is saved was left to the agent: (a), so the whole input mode stays in one settings file and the CLI sees it.
- (a) (R) A core field `InputMode::PushToTalk { release_delay_ms, toggle: bool }` (`#[serde(default)]`, so older files read as `false`). The core's gate does not change: the app turns each press into "flip", and sends `PushToTalk { pressed: on }`. A `feat(core)` task; the CLI's `/audio` shows it.
- (b) The app keeps "toggle" in its own UI settings file. No core change, but the input mode is split between two files, and the CLI does not see it.
- (c) No toggle mode in the app now.

**Q3. Gamepad button. Answered (human, 2026-10-13): (a).**
- (a) (R) A menu of the 19 button names with what they are on common pads ("south (A / ✕)"). No change to `twicecat-ptt`.
- (b) Capture by pressing: a small `twicecat-ptt` addition that reports the next button pressed on any pad while the card waits for it.

**Q4. The app depends on `twicecat-ptt` directly. Answered (human, 2026-10-13): as recommended.** It cannot live in the core: the service must be started on the app's main thread. (R) yes, and `scripts/check-app-deps.sh` also checks `twicecat-ptt`'s whole tree (normal and build edges) for the banned crates and `livekit`. Small `twicecat-ptt` additions for the app (`KeyBinding` from a window key, the matching rule) are in Task 2 under scope `ptt`.

**Q5. The `.desktop` file. Answered (human, 2026-10-13): as recommended.** (R) `packaging/linux/io.github.ga_lep.Twicecat.desktop` (Name Twicecat, `Exec=twicecat %u`, Categories Network;InstantMessaging;Chat;, no icon until Phase 7), checked by `desktop-file-validate` when it is installed (skipped otherwise), and a note in `docs/building.md` on copying it to `~/.local/share/applications/` for the portal during development. Installing it system-wide is Phase 7.

**Q6. Plan, branch and step numbers. Answered (human, 2026-10-13): as recommended.** (R) Plan `docs/plans/phase6-push-to-talk.md`, ledger `docs/plans/phase6-push-to-talk.progress.md`, the existing branch `15-featapp-wire-push-to-talk-into-the-app`, steps `phase6.24` to `phase6.29`.

## Architecture
- **`crates/app/src/ptt.rs` (new)**, `PttModel` on `App`:
  - `config_for(&AudioSettings, KeyboardBackend) -> Result<PttConfig, Vec<BindingProblem>>`: the bindings mapped to `PttConfig`; a binding that does not parse is left out and reported on screen ("The saved key cannot be used: …"), never logged with its text.
  - `wanted(&App) -> Option<PttConfig>` from the input mode and Q1; `sync()` after every event that changes it (settings echo, `CallState`, mic test, card open/close). It starts the service if none runs, restarts it when the config changed, and drops it when none is wanted. Always from `update`, so on the main thread.
  - The service sits behind a `PttStarter` trait (real: `PttService::start` on the core runtime's `Handle`; tests: a fake that records configs and exposes its sink), as `TrayFactory` does for the tray.
  - **Forwarding outside `update`:** the sink sends each `PttEvent` on two unbounded channels (a send never blocks and never fails while the receivers live). (1) A forwarding task on the core runtime turns `Transmit(on)` into `Command::PushToTalk { pressed }` (with Q2a, each `Transmit(true)` flips a toggle state instead) and awaits `commands.send`, so order is kept and a release is never dropped on a full channel. (2) An Iced subscription carries all events to `update`, for the screen only.
  - `CommandSink` gains `fn ptt_forwarder(&self) -> Option<PttForwarder>` (the core's sender and runtime handle); the test sink returns a forwarder on a test runtime.
  - **Window fallback:** in `listen_with`, `KeyPressed`/`KeyReleased` whose physical code is the bound key become `Message::PttWindowKey(bool)`, also when a widget captured the key (a focused composer still talks); a press needs the binding's modifiers, a release does not (modifiers may be let go first); repeats are dropped. `Unfocused` releases the window source, so a key held while switching away never sticks. With a global source also running, the merge makes the double report harmless.
  - **State on screen:** `Ready`/`Unavailable` per source in the card; "Talking" in the call bar while transmitting (from the service's `Transmit`, so it shows at once; the meter keeps showing the gate with its tail).
- **`crates/app/src/audio_settings.rs` and `screens/audio_settings.rs`:** a new "Talking" section at the top of the card (below Devices): mode menu; voice activity: "Set automatically" and the threshold slider (−100…0 dB); push-to-talk/toggle: the key with **Set key…** and **Clear**, the gamepad button menu (Q3) with **None**, the release delay slider (0…2000 ms, step 50; switching to push-to-talk from voice activity starts at 200 ms when it was 0), and the source lines.
- **Key capture:** **Set key…** waits for the next non-modifier key with its modifiers (Esc cancels, the window losing focus cancels). It becomes `ctrl+shift+KeyT`-style text, is parsed by `KeyBinding`, and is sent with `UpdateAudioSettings`. A key global-hotkey cannot bind shows "This key cannot be used for push-to-talk." While capturing, the app's shortcuts and the window key are off.

## Tasks (one commit each)

### Task 1: Toggle-to-talk in the core (phase6.24, `feat(core)`; Q2a)
Files: `crates/core/src/api/audio.rs`, the places matching `InputMode::PushToTalk` (`audio_session.rs`, `call_settings.rs`, the CLI's `calls/`), PROGRESS.md, ledger.
- `toggle: bool` with `#[serde(default)]`; `validated` keeps it; the gate ignores it.
- CLI: `/gate ptt [ms] [toggle]`; `/audio` says "push-to-talk (toggle)". `/ptt` is unchanged (the CLI already toggles).
- Tests: serde of old files (no field → `false`) and new ones; `validated`; the CLI parse and display. `scripts/check-core-api.sh` passes.

### Task 2: The service in the app (phase6.25, `feat(app)`; small `twicecat-ptt` additions)
Files: `crates/app/Cargo.toml` (`twicecat-ptt = { path = "../ptt" }`), `crates/app/src/ptt.rs` (new), `state.rs`, `bridge.rs`, `lib.rs`, `crates/ptt/src/binding.rs`, `scripts/check-app-deps.sh` (Q4), PROGRESS.md, ledger.
Acceptance (unit tests on `App::update` with a fake starter, unless noted):
- bindings → `PttConfig`: key and button parsed; a bad key or button left out with its problem shown; no bindings → only the window source; the keyboard backend passed through;
- the service starts, restarts and stops as Q1 says (voice activity never starts it; a changed binding restarts it once; leaving the call or closing the card drops it; logout drops it);
- `Transmit(true/false)` from the fake sink reaches the command channel as `PushToTalk { pressed: true/false }`, in order, and not through `update` (the test reads the channel without calling `update`); with toggle, presses flip;
- window keys: the bound key with its modifiers presses, its release without them releases, repeats and other keys do nothing, `Unfocused` releases;
- `Ready`/`Unavailable` stored per source; one source unavailable leaves the others;
- `twicecat-ptt`: a window key to `KeyBinding`, and the match rule, table-tested (letters, digits, F-keys, numpad, a modifier alone refused);
- the dependency check fails on a fake `matrix-sdk` dependency of `twicecat-ptt` (mutation, reverted).

### Task 3: The release tail end to end (phase6.26, `feat(app)`)
Files: `crates/app/tests/ptt.rs` (new), PROGRESS.md, ledger.
- The real `Bridge` with `AudioBackend::Virtual` (no homeserver, no sound card), a mic test in push-to-talk with a 300 ms release delay: a fake service's sink presses, the meter's `InputLevel::transmitting` turns on; it releases, `transmitting` stays on for about the delay and then turns off; with voice activity, presses change nothing. `#[ignore]` only if the virtual mic test needs more than the CI runner gives (decided by running it).

### Task 4: The Talking section of the Voice & audio card (phase6.27, `feat(app)`)
Files: `audio_settings.rs`, `screens/audio_settings.rs`, `screens/call.rs` (the "Talking" mark), `settings.rs` and `screens/settings.rs` (`PUSH_TO_TALK_LINE` removed; the Voice & audio line mentions push-to-talk), `keys.rs` (capture mode), `docs/accessibility.md`, PROGRESS.md, ledger.
Acceptance (unit tests):
- each control sends the whole settings with its one change (mode both ways keeping the bindings; automatic; threshold; release delay with its steps and the 200 ms start; key set and cleared; button set and cleared);
- capture: a key with modifiers is stored in global-hotkey syntax; modifiers alone wait; Esc and losing focus cancel; an unbindable key shows the note and sends nothing; shortcuts and the window key are off while capturing;
- the source lines show `Ready`'s trigger (the portal's words on Wayland) and `Unavailable`'s reason per source;
- the view with each mode renders in a layout test; the other sections' tests no longer assert the input mode passes through (it is now edited here).

### Task 5: The desktop entry (phase6.28, `feat(app)`)
Files: `packaging/linux/io.github.ga_lep.Twicecat.desktop` (new), `docs/building.md`, a test that the file's name and `StartupWMClass`/app ID match `twicecat_ptt`'s app ID, PROGRESS.md, ledger.

### Task 6: Wrap-up (phase6.29, `docs(plan)` if only docs change)
- An independent read-only review of `main..HEAD`; fixes folded into their commits.
- PROGRESS.md: a "Phase 6 push-to-talk" section with the steps, notes, open issues and the checkpoint below; the Phase 6 checkpoint's "Global PTT hotkey" item points to it.
- Issue #15's scope mapped to commits.

## Human checkpoint (collected in PROGRESS.md, never ticked by the agent; rule 9)
- [x] Plan questions answered and plan approved (2026-10-13).
- [ ] The global key held and let go in a call (the other side hears only while held, plus the delay) on Windows, macOS, X11, GNOME Wayland and KDE Wayland; on Wayland the portal dialog appears once, and the card shows the key it reports.
- [ ] macOS: the microphone prompt; whether global-hotkey needs Input Monitoring for releases.
- [ ] A gamepad button (Xbox, PlayStation, Steam Input) held in a call; a pad unplugged while held lets go.
- [ ] SteamOS: desktop mode (portal or X11) and game mode (gamescope: the window key and the gamepad only).
- [ ] Toggle-to-talk: one press starts, the next stops; leaving the call stops.
- [ ] The Talking section and key capture in both themes, by keyboard only; the "Talking" mark in the call bar.
- [ ] With the key grabbed (Q1), the same key in another app works again once the call ends.
