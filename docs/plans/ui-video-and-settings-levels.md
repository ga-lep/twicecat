# Video in the new UI, and Settings for two levels of knowledge
Status: APPROVED by the human's request of 2026-10-15 (the rebase of `feat/ui-ux-rework` on `main`, the video and screen sharing of Phase 9 in the reworked UI, and a simple and an advanced Settings). Work happens on `feat/ui-ux-rework`, rebased on `origin/main` at `38115e8`.

## Goal
1. `feat/ui-ux-rework` sits on top of `main` (Phase 9 Tasks 2-6 are in: the core's video API, the test pattern, LiveKit's encoders).
2. The reworked UI shows and controls the call's video: Phase 9 Task 10 (renderer and call stage) and Task 11 (controls, pickers, preview, Video settings) in the new design system.
3. Settings has two levels, chosen by one switch that is saved: a simple one for people with little technical knowledge (few choices, in plain words, with good defaults) and an advanced one that exposes every knob the core and LiveKit allow, audio and video above all. This is the "expose everything behind a closed Advanced section" rule of the Phase 9 Task 11 amendment, extended to all of Settings.

## What the core can do today (and so what the UI can show)
- `CoreConfig::video = VideoBackend::TestPattern` gives a camera and a screen that are a test pattern. With `VideoBackend::System` the camera and screen commands answer `CoreError::NotSupported` until Phase 9 Tasks 8 (Linux screen sharing) and 9 (Linux camera) land. The UI works against the API, shows that answer in words, and is checked with the test pattern and with fixtures.
- The core does not report the playback latency of a person's audio, so the Task 10 audio-video delay (video shown as late as the audio path, at most 300 ms) cannot be built in the app. It is left for a core change and listed on the human checkpoint.
- `VideoSettings` has the basics only (camera, qualities, codec, encoder, integrated GPU on battery, upload cap, remembered screen). The advanced per-kind fields of Task 11 are a core change (Step 2).

## Rules
AGENTS.md as ever. In particular: rule 2 (every iced and wgpu API read in the registry source of the pinned version: `iced_widget-0.14.2/src/shader.rs`, `iced_wgpu-0.14.0/src/primitive.rs`, wgpu 27.0.1), rule 3 (no new crates, no new iced features: the shader widget is behind iced's default `wgpu` feature, which the app already builds with), rule 7 (the app reaches the core through `twicecat_core::api` only), rule 9 (rendering, sound, portal dialogs and real cameras are human checkpoint items). One `feat` commit per step.

## Steps
- [x] Step 1: video in calls, in the new UI (Phase 9 Task 10, and the buttons, picker and sharing bar of Task 11; `feat(app)`)
  - A `shader` widget that draws NV12 and I420 frames of a `VideoFeed` on the GPU (BT.601 and BT.709, limited and full range, mirrored for our own camera), reading the newest frame itself when drawn and asking for a redraw only while frames come. It reports its size in physical pixels, and `SetVideoView` follows with sizes rounded up to 64 px.
  - The stage: a slot of the room column above the timeline of the call's room (an empty space without video, so the timeline and the composer keep their place): a grid of 16:9 tiles, or with a pinned track or a remote screen share a big tile above a strip; the avatar for a person with the camera off or none; name, microphone mark, speaking ring; pin, full screen, hide.
  - The full-screen view (also the window's full-screen mode), the share picker (our own grid of screens and windows with pictures where the system has no picker, a note where it has), quality and share-sound choices, a "sharing" bar over every dialog, Camera and Share buttons in the dock, the rail and the stage, Ctrl+Shift+O and Ctrl+Shift+E (not Ctrl+Shift+V: in the composer it is paste), a **Video** button beside **Call** that joins with the camera on.
  - Acceptance: tests for the model (tiles, layout, sizes asked, pinning, joins), the update paths (every control and error path), the WGSL on a bare wgpu device and through iced's headless wgpu renderer (pixels of a known frame), the layout of the stage at several widths, clicks reaching the buttons. Rendering by eye, real window full-screen, HiDPI and the GPU-less case are on the human checkpoint.
- [x] Step 2: the advanced video settings in the core (Phase 9 Task 11, `feat(core)`)
  - `VideoSettings` gains per-kind settings for the camera and the screen share: size, frame rate, top-layer bitrate, codec and encoder, layers (simulcast on or off, 1 to 3, each with its own size, rate and bitrate), degradation preference, and for the camera the capture mode. Old `video_settings.json` files still load. The core validates and clamps them, and says what it changed. `Sending` carries them to LiveKit's `TrackPublishOptions` (`livekit-0.9.5/src/room/options.rs:117-153`, `DegradationPreference` in `libwebrtc-0.3.52/src/rtp_parameters.rs:27`).
  - Acceptance: unit tests for every field's clamp and for the options built; a stack test per option is ignored in CI and listed for the human's machine.
- [x] Step 3: Settings for two levels (`feat(app)`)
  - One saved switch, **Show advanced settings** (in `UiSettings`, off by default), in the Settings card and in the Voice & audio and Video cards.
  - Settings pages, simple: Profile, Appearance (light or dark, colours, text size), Notifications (group rooms, direct messages, desktop notifications), Voice & video (microphone, speakers, how to talk, noise reduction, camera, quality), Keyboard, Sessions, About. Advanced adds what a person must know more to use: density and room-list layout, keywords, message text in notifications, the tray badge.
  - Voice & audio card: the pages and the controls that a beginner needs (devices, talking, noise reduction with a plain Off, Standard, Strong choice, a mic test) and, with the switch, everything the core allows (processing chain, gate, gain, bitrate, GPU, other people's voices, announcements, diagnostics).
  - Video card: camera, a picture of it, quality in plain words for the camera and for sharing; and, with the switch, the encoder, codec, sizes, rates, bitrates, layers, degradation preference, the upload cap, the encoder status and live numbers of the call's video.
  - Acceptance: `update` tests for every control, gallery scenarios and layout tests for both levels, the headless gallery, the shortcuts and accessibility documents.
- [x] Step 4: wrap-up (`docs`): PROGRESS.md, the checkpoint list for the human, `docs/accessibility.md`.

## Decisions taken without asking (recommended answers, change them if wrong)
- The camera and share shortcuts are Ctrl+Shift+O and Ctrl+Shift+E (Teams' chords), because Ctrl+Shift+V is paste in the composer.
- A click on a picture pins it, a double click opens it full screen; pin and full screen are also buttons on tiles wide enough for them.
- The sharing bar sits at the bottom, in the status bar's empty middle, so the room's name and buttons are never covered.
- A video call started with **Video** turns the camera on once connected; accepting a ring never turns a camera on by itself.
- No pop-out window for a picture: a second window needs the app to move from `iced::application` to `iced::daemon`, a rework of its own. The full-screen view uses the window's own full-screen mode. Listed for the human.
