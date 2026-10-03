# Phase 6 Calls Plan: MatrixRTC, LiveKit SFU, native audio
Status: APPROVED (all questions answered by the human, 2026-10-02)

## Goal
Native voice calls and Discord-style voice rooms, with no Element Call widget. `tessera-core` joins MatrixRTC sessions (MSC4143 `m.call.member` with a LiveKit focus, MSC4140 delayed leave events, MSC4195 LiveKit transport discovery), gets a LiveKit JWT through the homeserver's `lk-jwt-service`, connects to the SFU with the LiveKit Rust SDK, publishes the microphone, and plays remote participants. All audio runs through our own pipeline: device capture and playback, echo cancellation, noise suppression (WebRTC NS, RNNoise, and a DeepFilterNet-class option), automatic gain, a voice-activity gate or push-to-talk, a per-participant mixer with per-user volume, and level meters. Media is end-to-end encrypted with MatrixRTC per-sender keys, so we interoperate with Element Call in encrypted rooms. Every setting is a command in the core API, so the UI phase can put it all on screen. This phase is headless: it is driven by the core API and the CLI, and tested against a local Synapse plus a local LiveKit SFU.

## Decisions (human, 2026-10-02)
1. **New phase, Phase 6.** Plan file `docs/plans/phase6-calls.md`, branch `feat/calls-matrixrtc` (rebased on `main` after the Phase 1 merge). Ledger: `docs/plans/phase6-calls.progress.md` plus a "Phase 6" section in PROGRESS.md. Commits follow Conventional Commits (AGENTS.md rule 5, adopted 2026-10-02): one `feat(<scope>): …` commit per task with a `Step: phase6.<n>` trailer, `docs(…)` for plan and ledger changes, and fixes folded into the commit they belong to.
2. **Ordering: core first, then UI.** This phase delivers the headless call engine, the audio pipeline and CLI commands. The call UI (voice room view, device and processing settings, PTT binding) comes with the Iced shell. The core/app boundary (rule 7) is unchanged.
3. **Audio stack: our own pipeline, with sonora as the APM.**
   - cpal capture and playback, then processing, then `NativeAudioSource`.
   - Remote tracks come in through `NativeAudioStream`, go into our mixer, and feed sonora's render (echo-reference) input.
   - sonora is the echo/noise/gain backend. The libwebrtc APM is kept only as a compile-time A/B fallback (see "Sonora assessment").
4. **Newest releases of every new dependency** (rule 3 approval for the list under "Dependencies"). The existing pins are already the newest releases (matrix-sdk/-ui 0.19.1, iced 0.14.0, checked 2026-10-02), so no existing pin changes.
5. **Noise suppression:** RNNoise (nnnoiseless) first. DeepFilterNet3 follows as its own task: we run its ONNX models with `tract-onnx` and write the feature extraction and deep-filter stages ourselves. The model files are pinned by SHA-256.
6. **libwebrtc fetch script with pinned SHA-256** (`scripts/fetch-libwebrtc.sh`). The build uses `LK_CUSTOM_WEBRTC` and never lets webrtc-sys download anything itself.
7. **Denormal guard: allowed now** (pre-production). A single audited `unsafe` function sets FTZ/DAZ on the processing thread. It is documented in PROGRESS.md "Known limitations", with a note to drop it once a sonora release ships upstream PR #38 (any version bump is asked first, rule 3).
8. **Call E2EE is in scope** (MatrixRTC per-sender keys over encrypted to-device messages), so we interoperate with Element Call in encrypted rooms.
9. **Local SFU in compose:** `livekit/livekit-server:v1.13.7` and `ghcr.io/element-hq/lk-jwt-service:v0.7.0`, bound to 127.0.0.1, with test-only LiveKit key/secret in `.env.test`. Synapse gets the MatrixRTC transport and delayed-event config (one `docker compose down -v` needed).
10. **Interop format: Element Call's current one.** That is `m.call.member` state events per device, a delayed leave event, and `io.element.call.encryption_keys` to-device. MSC4354 sticky `m.rtc.member` is revisited when the SDK supports it.
11. **Target platform: Steam Frame / SteamOS** in addition to desktop Linux, Windows and macOS (see "Steam Frame / SteamOS").
12. **Native PipeWire on Linux, our own client in the end** (human, 2026-10-02, after Task 8). The PulseAudio protocol (pipewire-pulse) brings nothing our targets need and costs latency, a real-time thread and graph control. Task 4 uses cpal's PipeWire backend now; Task 14 replaces it on Linux with our own client on the `pipewire` crate. cpal stays for macOS (CoreAudio) and Windows (WASAPI), ALSA stays the Linux fallback. The Steam Frame is a constrained device that also runs a standalone VR game, so the call path gets a measured CPU budget (Task 15).

## Steam Frame / SteamOS
Steam Frame runs SteamOS on ARM64 (Snapdragon). The system is Arch-based with a read-only root, so third-party apps are Flatpaks. Audio goes through PipeWire. The UI is either KDE Plasma on Wayland (desktop mode) or gamescope (game mode). The device has built-in speakers and a mic array, so open-speaker echo cancellation is the normal case, not an edge case. Consequences for this phase:
- **Native `aarch64-unknown-linux-gnu`.** CI gains a Linux ARM64 job (GitHub `ubuntu-26.04-arm` runner) for build and test. livekit publishes `webrtc-linux-arm64-release.zip` for the pinned tag (sha256 `f716b10e…6420`); sonora has a NEON backend; tract has NEON kernels.
- **PipeWire.** cpal's native `pipewire` backend on Linux (cpal 0.18.2 feature `pipewire`; CI and dev need `libpipewire-0.3-dev` and clang for bindgen). ALSA stays as a runtime fallback. Device names and hot-plug come from PipeWire. libwebrtc's weak `pw_*` stubs, which would take libpipewire's place in the same binary, are renamed by the fetch script (Task 1).
- **Flatpak-friendly builds.** No build-time network access (the fetch script plus SHA-256 maps directly onto Flatpak `sources`). rustls, not OpenSSL (livekit feature `rustls-tls-native-roots`). Flatpak packaging itself is a later phase; this phase just doesn't block it.
- **Push-to-talk without a global-shortcut portal.** gamescope has no GlobalShortcuts portal. PTT sources are therefore:
  - an in-window key;
  - a gamepad button via `gilrs` (Steam Input can map controller buttons);
  - the portal on KDE/GNOME Wayland;
  - `global-hotkey` on X11, Windows and macOS.
- **Battery.** RNNoise is the default ML option (DeepFilterNet3 is opt-in). Opus DTX is on. The processing thread sleeps between 10 ms frames. A CPU-cost unit benchmark is recorded for each NS mode on x86_64 and aarch64 (CI ARM runner).
- **Real-time audio threads** use cpal's `realtime` feature (rtkit over D-Bus, which also works inside Flatpak through the portal). This is a human checkpoint item on the device.
- **Game audio in the echo canceller.** In standalone VR the game plays through the same speakers as the call, and the AEC render reference only holds call audio, so game sound reaching the mic goes to everyone in the call. Task 14 feeds the system output mix (the sink being played) to AEC3 as the render reference, on the graph's clock.
- **CPU budget.** The whole call path (capture, chain, Opus, mixer, playback) gets a measured per-frame budget on aarch64 (Task 15), with Frame defaults chosen from it.
- **Flatpak permission for native PipeWire:** `--filesystem=xdg-run/pipewire-0` (as OBS and other audio apps request), instead of `--socket=pulseaudio`. Recorded for the packaging phase.

## Facts verified while planning (2026-10-02)
Each task re-confirms the APIs it uses (rule 2) and cites file:line in PROGRESS.md. The paths below are under `~/.cargo/registry/src/index.crates.io-*/`.

**matrix-sdk 0.19.1 / ruma 0.17.0 (pinned, no feature change needed for signalling)**
- matrix-sdk already turns on ruma `unstable-msc3401`, `-msc3417` (call rooms), `-msc4075`, `-msc4140` (delayed events), `-msc4143`, `-msc4195`, `-msc4310` (matrix-sdk-0.19.1/Cargo.toml:331-344).
- Transport discovery: `Client::discover_rtc_transports() -> HttpResult<Option<Vec<RtcTransport>>>` (client/mod.rs:2898), which falls back to `well_known_rtc_transports` (:2752). `RtcTransport::LiveKit(LiveKitRtcTransport)` (ruma-client-api-0.25.0/src/rtc.rs), endpoint `/_matrix/client/unstable/org.matrix.msc4143/rtc/transports` (rtc/transports.rs:22).
- Membership event: `CallMemberEventContent::{new, new_empty(leave_reason)}` (ruma-events-0.35.0/src/call/member.rs:69, :88), `SessionMembershipData`, `CallApplicationContent::new_with_intent` (member/member_data.rs:250, :346), `CallScope`, `LivekitFocus::new(alias, service_url)` and `ActiveLivekitFocus` (member/focus.rs:23-72), `CallMemberStateKey::new(user, member_id, underscore)` (member/member_state_key.rs:29).
- Reading call state: `Room::{has_active_room_call, active_room_call_participants, is_device_in_active_room_call, active_room_call_consensus_intent}` (matrix-sdk-base-0.19.1/src/room/call.rs:46-75).
- Ringing: `RtcNotificationEventContent::new`, `NotificationType`, `CallIntent` (ruma-events rtc/notification.rs:57, :117, :133); `Room::make_decline_call_event`, `Room::subscribe_to_call_decline_events` (matrix-sdk room/calls.rs).
- Delayed events: request types in ruma-client-api-0.25.0/src/delayed_events/{delayed_state_event,update_delayed_event}.rs. They are sent with `client.send(request)`, because matrix-sdk has no high-level wrapper.
- OpenID token for the JWT service: ruma-client-api account/request_openid_token.rs.
- Call E2EE key distribution needs the matrix-sdk feature `experimental-send-custom-to-device` (Cargo.toml features list). This is a feature flag on the pinned version, approved in decision 4.

**Synapse v1.151.0 (pinned image)**
- `matrix_rtc: transports: [...]` config section (synapse/config/matrixrtc.py), `experimental_features.msc4143_enabled` (config/experimental.py:560), `max_event_delay_duration` for MSC4140 (config/server.py:911). MSC4354 sticky events exist behind `msc4354_enabled` but ruma 0.17 has no `m.rtc.member` type, so we use `m.call.member` state events, as current Element Call does.

**LiveKit Rust SDK: livekit 0.9.3 (Apache-2.0), libwebrtc 0.3.50, webrtc-sys 0.3.47**
- `Room::connect(url, token, RoomOptions) -> RoomResult<(Room, UnboundedReceiver<RoomEvent>)>` (livekit-0.9.3/src/room/mod.rs:567).
- `E2eeOptions { encryption_type, key_provider }` (room/e2ee/mod.rs:28). `KeyProvider::{new, set_key(identity, index, key), ratchet_key, get_latest_key_index}` (room/e2ee/key_provider.rs:60-117): per-participant keys, as MatrixRTC needs.
- `NativeAudioSource::new(options, sample_rate, channels, queue_size_ms)` and `capture_frame` (libwebrtc-0.3.50/src/native/audio_source.rs:49, :92). `NativeAudioStream::new(track, sample_rate, channels)` (audio_stream.rs:62).
- libwebrtc APM wrapper `AudioProcessingModule::new(aec, agc, hpf, ns)`. It takes only on/off flags (webrtc-sys-0.3.47/src/apm.cpp:60-71) and only i16 input, with no NS level and no AGC tuning.
- `PlatformAudio` (platform_audio/mod.rs:408) exists, but we don't use it (decision 2).
- **Supply chain:** webrtc-sys-build downloads a prebuilt libwebrtc zip from `github.com/livekit/rust-sdks/releases/download/webrtc-89d790b/…` at build time, with **no checksum check** (webrtc-sys-build-0.3.19/src/lib.rs:31, :101-106). `LK_CUSTOM_WEBRTC` points it at a local directory instead (:80-85). cargo-deny does not see this download. Handled by decision 6 (Task 1). Release assets carry GitHub sha256 digests (`linux-x64` `b167adad…bf70c5`, `linux-arm64` `f716b10e…706420`).

**Audio crates**
- cpal 0.18.2 (Apache-2.0, MSRV 1.85).
- nnnoiseless 0.5.2 (BSD-3, pure Rust RNNoise): `DenoiseState::new() -> Box<…>`, `process_frame(&mut out, &in) -> f32` (the return value is the VAD probability), 480-sample frames at 48 kHz (src/denoise.rs:53, :95; lib.rs:38).
- **deep_filter 0.2.5 on crates.io is DeepFilterNet's dataset/training library only.** It has no `tract` or real-time inference API (Cargo.toml features: bin, cache, dataset, transforms, flac, vorbis). Upstream github.com/Rikorose/DeepFilterNet was last pushed 2024-10-17. Handled by decision 5 (Task 13: our own DF3 pipeline on tract-onnx).
- global-hotkey 0.8.0 (Apache-2.0 OR MIT): `GlobalHotKeyManager::register`, events with `HotKeyState::{Pressed, Released}` (src/lib.rs:66-146). It works on X11, Windows and macOS. Wayland needs the xdg-desktop-portal GlobalShortcuts portal instead (a human checkpoint item, rule 9).

**Local SFU images:** `livekit/livekit-server:v1.13.7` (2026-09-14) and `ghcr.io/element-hq/lk-jwt-service:v0.7.0` (2026-09-10).

## Sonora assessment (human request)
sonora 0.2.0 (BSD-3-Clause, MSRV 1.91, github.com/dignifiedquire/sonora) is a pure-Rust port of WebRTC's AudioProcessing module at M145.

Coverage is complete for our needs:
- AEC3, with a residual echo detector;
- NS with Low/Moderate/High/VeryHigh levels;
- AGC2 (adaptive digital, fixed digital, input volume controller);
- high-pass filter;
- pre/post gain, capture level adjustment and analog mic gain emulation;
- `set_stream_delay_ms`;
- statistics (ERL/ERLE, echo likelihood) for a UI meter;
- f32 and i16 APIs at 8–384 kHz.

That is far more tunable than the livekit APM wrapper, which offers four on/off flags.

| | sonora 0.2.0 | libwebrtc APM via livekit |
|---|---|---|
| Extra dependency | 7 crates, about 51k lines, all BSD-3 | none (already linked by livekit) |
| Configurability | full (NS level, AGC2 targets, gains, stats) | on/off flags only, i16 only |
| Build | plain cargo; no C++ and no download | part of the prebuilt libwebrtc blob |
| Validation | upstream's 2400+ C++ tests pass against the Rust backend (FFI bridge); CI on Linux, macOS, Windows, Android, iOS | Google production code |
| Speed (vendor numbers, M4) | 1.07–1.24× the C++ time; 48 kHz mono full pipeline 13 µs per 10 ms | baseline |
| Maturity | created 2026-02, 2 releases, one maintainer (213 of 225 commits), AI-assisted port | very mature |

Known 0.2.0 problems that matter for calls:
- **#34 (open):** AEC3 runs 10–50× slower while the far end is silent, because of denormal floats. The worst case is about 15 ms per 10 ms frame, which falls behind real time. Upstream WebRTC wraps processing in a denormal disabler, and the port does not. Silence is the normal state in a voice room.
- **#39/#40 (open PRs):** about 40 heap allocations per 10 ms frame in the AEC3 path. The fix is bit-identical.
- **#38 (open PR)** fixes the denormals. **#36 (open PR)** syncs post-M145 upstream fixes.

**Recommendation:** use sonora behind a small `EchoProcessor` trait, with two mitigations:
1. Run processing on a dedicated audio-processing thread, never inside the cpal device callback. Lock-free SPSC rings connect the callbacks to that thread, so an allocation or a slow frame cannot cause a device xrun.
2. Set flush-to-zero/denormals-are-zero on that thread (x86 MXCSR, AArch64 FPCR). This is a few lines of `unsafe` in one audited function (decision 7). It goes away once a sonora release with #38 lands; any version bump is asked first (rule 3).

Keep the libwebrtc APM as a second, compile-time backend. It costs no new dependency and gives an A/B comparison for the human checkpoint. A unit test fails if processing a steady-state frame takes over 2 ms in release mode, with silent render, to catch #34 regressions.

## Architecture
- **`crates/core`** gains a private `call/` module:
  - signalling: transport discovery, OpenID token to JWT, `m.call.member` join with a delayed leave event and a periodic restart, leave, membership tracking, ringing, E2EE key exchange;
  - the LiveKit room;
  - a `CallSession` actor owned by the engine.

  The public `api/` gains SDK-free call types. `check-core-api.sh` also rejects `livekit`, `libwebrtc`, `cpal` and `sonora` names in `api/`.
- **New `crates/audio` (`tessera-audio`)**, the media pipeline. It depends on no Matrix crate and no LiveKit crate: core adapts between it and LiveKit frames. It has:
  - device enumeration and hot-plug (cpal);
  - capture and playout threads;
  - resampling to 48 kHz;
  - the processing chain: HPF → AEC → NS (WebRTC / RNNoise / DFN-class) → AGC2 → input gain → gate (VAD threshold or PTT, with a release tail) → limiter;
  - the mixer: per-participant gain, deafen, master volume, and the AEC render tap;
  - mic test (local loopback) and RMS/peak meters.

  It is fully unit-testable offline with synthetic signals.
- **`crates/ptt`**: the global push-to-talk hotkey service (X11/Windows/macOS via global-hotkey; Wayland via the portal). It emits pressed/released events that the app forwards as core commands. It never depends on matrix-sdk.
- **Settings:** a non-secret `call_settings.json` in the data dir: devices, processing choices, input mode and key, per-user volumes. It has a version field and atomic write, like `last_account`.

### API additions (SDK-free; the final shape is approved by the human after Task 3)
As built in Task 3 (`crates/core/src/api/call.rs`, commands and events in `api/mod.rs`). Settings are one value, not a setter per field: the UI edits a copy and sends it whole.
- **Commands:**
  - `JoinCall { room_id, intent: CallIntent::Audio }`, `LeaveCall`, `DeclineCall { room_id }`;
  - `SetMicMuted(bool)`, `SetDeafened(bool)`, `PushToTalk { pressed }` (from any source: hotkey, gamepad, window key);
  - `SetUserVolume { user_id, percent }` (remembered per user);
  - `ListAudioDevices`, `GetAudioSettings`, `UpdateAudioSettings(AudioSettings)`;
  - `StartMicTest { loopback }`, `StopMicTest`.
- **`AudioSettings`** (also the `call_settings.json` format, versioned wrapper, atomic write, 0o600):
  - `input_device` and `output_device` (`None` follows the system default);
  - `processing { echo_cancellation, noise_suppression: Off | WebRtc { level } | RnNoise | DeepFilter { attenuation_limit_db }, auto_gain, high_pass }`;
  - `input_mode: VoiceActivity { automatic, threshold_db } | PushToTalk { release_delay_ms }`, and `push_to_talk { key, gamepad_button }` (Task 11);
  - `input_gain_db`, `output_volume_percent`, `voice_bitrate_kbps` (default 64), and `user_volumes`.

  `validated()` clamps every field (`api::limits`).
- **Events:**
  - `CallState(Idle | Joining | Connected | Reconnecting { attempt } | Left { reason })`;
  - `CallParticipants { room_id, diffs: Vec<ListDiff<Participant { id, user_id, device_id, display_name, is_local, muted, speaking, volume_percent }>> }`;
  - `ActiveCall { room_id, participants }` for the room list;
  - `IncomingCall { room_id, caller, intent, ring }` (`ring: false` for a group room's quiet notification; Task 10), `RingingStopped { room_id }`, `CallDeclined { room_id, user_id }` (Task 10);
  - `AudioDevices { inputs, outputs }`, `AudioSettings(..)`;
  - `InputLevel { rms_db, peak_db, transmitting }` at about 20 Hz;
  - `AudioStats { echo_return_loss_enhancement_db, residual_echo_likelihood, echo_delay_ms, processing_us }` at about 1 Hz.
- **Errors:** `CoreError::{Call(String), Audio(String), NotInCall}`.
- **Voice rooms:** an MSC3417 call room (room type `org.matrix.msc3417.call`), or any room with an active session, can be joined like a Discord channel. Creating such a room waits for the room-creation phase (Phase 5).

## Tasks (one `feat` commit each with a `Step: phase6.<n>` trailer, gated as in Phase 1)
Order (revised 2026-10-02, decision 12; again 2026-10-03 by the human): 1–8, 8b, 9, 10, 11, 12, 14, 13, then 15 and 13b.

Each task follows the full Phase 1 gate: fmt, clippy -D warnings, tests, `check-app-deps.sh`, `check-core-api.sh`, `cargo deny check`, and the ignored homeserver run, which from task 2 also needs the SFU stack.

1. **libwebrtc supply chain and livekit dependency.**
   - `scripts/fetch-libwebrtc.sh [triple]` downloads the zip for the host (or the given triple) at the pinned tag `webrtc-89d790b` (`linux-{x64,arm64}`, `mac-{x64,arm64}`, `win-{x64,arm64}`). It checks the SHA-256 committed in `scripts/libwebrtc.sha256` (GitHub release digests), unpacks into `${TESSERA_WEBRTC_CACHE:-$XDG_CACHE_HOME/tessera/libwebrtc}/<tag>/<triple>`, and prints the path for `LK_CUSTOM_WEBRTC`. It fails closed on a digest mismatch, a malformed digest, an unknown triple, a wrong archive layout or a missing tool. It is idempotent (a `.verified` marker holds the digest), and shellcheck is clean.
   - **webrtc-sys downloads into `LK_CUSTOM_WEBRTC` whenever that directory is missing** (webrtc-sys-0.3.47 build.rs:112-113), so pointing at a missing path is not safe. The committed directory `dev/libwebrtc/` (README and `.gitignore` only) is set as `LK_CUSTOM_WEBRTC` by `.cargo/config.toml` `[env]`. For a host build the fetch script links the verified entries into it. Unfetched, the build fails on the missing `webrtc.ninja`, without a download. An environment `LK_CUSTOM_WEBRTC` wins (CI, Windows, where symlinks need privileges).
   - On Linux the script then renames libwebrtc's weak `pw_*` stubs (`pipewire_stubs.o`, the dlopen shims of its PipeWire screen capture; 37 functions and 51 calls to them) to `tessera_webrtc_pw_*` with llvm-objcopy (binutils objcopy as fallback). Statically linked next to libpipewire they would replace libpipewire's functions and crash `pw_init` (found in Task 8). The result is checked by symbols (no global `pw_*` left, stubs present), not by a pinned digest: the patched bytes depend on the objcopy version, while the input zip stays pinned. A `.pw-renamed` marker keeps cached reruns at about 10 ms.
   - `scripts/check-no-webrtc-download.sh` fails if webrtc-sys's downloader ran (it creates `build/scratch-*/out/livekit_webrtc`). CI runs it after every build.
   - `tessera-core` depends on `livekit =0.9.3` with `default-features = false, features = ["native", "rustls-tls-native-roots"]` (no OpenSSL or native-tls in the tree).
   - Link/ABI smoke test `crates/core/tests/livekit_link.rs`, calling the libwebrtc APM and a UUID across the FFI.
   - CI: every compiling job runs the fetch script and exports `LK_CUSTOM_WEBRTC` (no cache action). Linux jobs run on `ubuntu-26.04` and a new `ubuntu-26.04-arm` entry with `CC=clang-22 CXX=clang++-22`: webrtc-sys needs clang >= 21 on Linux (libwebrtc uses Chromium's libc++, whose `trivial_abi` GCC ignores, build.rs:531-547, version check :598), and Ubuntu 24.04 only has clang 18. The apt line gains `libglib2.0-dev` (headers). `scripts/check-app-deps.sh livekit` runs in lint.
   - Per-target link flags in `.cargo/config.toml`, from livekit/rust-sdks' own config: `+crt-static` on Windows MSVC (libwebrtc uses the static CRT), `-ObjC` on macOS (rustflags and rustdocflags), and `-fuse-ld=lld` on aarch64 Linux (CI apt line gains `lld`).
   - deny.toml: unchanged if cargo-deny passes. Audio packages (`libasound2-dev libpipewire-0.3-dev`) come in Task 4 with cpal.
2. **Local SFU stack.**
   - docker-compose gains `livekit` (`livekit/livekit-server:v1.13.7`, config `dev/livekit/livekit.yaml`: ws 7880, ICE TCP 7881, one muxed ICE UDP port 7882, `node_ip: 127.0.0.1`, `room.auto_create: false` as lk-jwt-service requires, a webhook to lk-jwt-service) and `lk-jwt` (`ghcr.io/element-hq/lk-jwt-service:0.7.0`; the tag has no `v`).
   - Both run in **synapse's network namespace** (`network_mode: service:synapse`). All ports are published on 127.0.0.1 by the synapse service. So `127.0.0.1:7880` and `127.0.0.1:8008` mean the same thing to the host (tests) and to lk-jwt-service (room creation, delegated leave).
   - **lk-jwt-service standalone mode** (MSC4195 `POST /get_token`): Synapse 1.151 has no MSC4502/MSC4512 proxying, so the application-service mode is out. Standalone mode checks the OpenID token at `https://<server_name>:8448/_matrix/federation/v1/openid/userinfo` (always TLS; lk-jwt helper.rs:1192-1247). So `dev/synapse/start.sh` adds a federation-only TLS listener on 8448 with a self-signed `CN=localhost` certificate (generated with the image's openssl, owned by 991:991, not published), and lk-jwt runs with `LIVEKIT_INSECURE_SKIP_VERIFY_TLS` (dev only).
   - Synapse also gains `experimental_features.msc4143_enabled`, `matrix_rtc.transports: [livekit → http://127.0.0.1:8080]`, `max_event_delay_duration: 24h`, and relaxed `rc_message` / `rc_delayed_event_mgmt`. A delayed event that hits `rc_message` is **dropped by Synapse, not retried** (synapse/handlers/delayed_events.py:494-498), which a test hit.
   - `.env.test` gains test-only `LIVEKIT_KEY`, `LIVEKIT_SECRET`, `LIVEKIT_URL` and `LK_JWT_URL`. livekit builds `LIVEKIT_KEYS` from them in its entrypoint, and lk-jwt reads them directly, so the secret has one source.
   - Integration tests (`crates/core/tests/rtc_stack.rs`, plain SDK clients):
     - `discover_rtc_transports` returns exactly the local lk-jwt URL;
     - OpenID token → `/get_token` → a JWT with which `livekit::Room::connect` joins the local SFU;
     - a delayed state event is sent by the server after its timeout.
   - LiveKit identities from lk-jwt are pseudonymous, `base64_nopad(sha256(json([user_id, device_id, member_id])))`, and room names are `base64_nopad(sha256(json([room_id, slot_id])))` (lk-jwt helper.rs:198-222). Tasks 7 and 9 map participants and E2EE keys through these.
3. **Call API contract.** Types and commands as above, redacted `Debug` where needed, the `check-core-api.sh` extension with negative self-checks, and settings persistence.
4. **Audio devices and I/O threads** (`tessera-audio`, as built).
   - Internal format: 48 kHz mono `f32`, 10 ms `Frame`s.
   - `AudioIo::start(IoConfig, impl FrameProcessor)` opens the capture and/or playback device on one dedicated thread (cpal streams are not `Send` everywhere), then runs the pipeline there.
   - Device callbacks only down/up-mix and move samples through `rtrb` SPSC rings (200 ms, device rate): no allocation, no locks.
   - The processing thread resamples with rubato 5 `Async` sinc (128 taps; passthrough at 48 kHz) and calls `FrameProcessor::process(capture, playback)` every 10 ms, paced by the capture clock (by a 10 ms timer without a microphone).
   - Playback more than 60 ms ahead of the device clock drops a frame, which bounds the latency from clock drift.
   - A configured device that is missing falls back to the default (`IoEvent::FellBackToDefault`). Stream errors (unplug) arrive as `IoEvent::StreamError`. Defaults follow the system default by themselves (cpal).
   - The format prefers 48 kHz when the device supports it with its default format and channels.
   - `list_devices()` (stable IDs: cpal `DeviceId` text, the PipeWire node name; on PipeWire, sinks, which cpal also offers as inputs, are left out of the inputs) and a polling `DeviceWatcher` (2 s) for hot-plug.
   - cpal's native PipeWire backend on Linux (feature `pipewire`, ALSA fallback); CI and dev need `libasound2-dev libpipewire-0.3-dev`.
   - Device buffers are 10 ms (one frame) when the device reports its range, which cpal's PipeWire backend applies as `node.latency`; otherwise the host default, which can be far larger.
   - Core: `ListAudioDevices` answers with `AudioDevices` and starts the watcher, which re-sends `AudioDevices` on change.
   - Tests use rings directly (no hardware): bit-exact 48 kHz loopback, 44.1 kHz capture and 96 kHz playback keep a 1 kHz tone (amplitude within 0.02–0.03), drift drop, timer mode, watcher diffing. An ignored output-only device test exists.
   - cpal's `realtime` feature is not enabled: the PipeWire callbacks already run on PipeWire's RT thread. Real-time priority for the processing thread is a Task 5 item.
5. **Processing chain** (`tessera-audio::CaptureChain`, as built).
   - Order: high-pass → AEC3 → noise suppression (WebRTC NS or RNNoise) → AGC2 → input gain → limiter (-0.5 dBFS, 50 ms release) → level meter → transmit gate.
   - HPF, AEC3 and WebRTC NS run in a "front" sonora instance that also gets the played frames (`render`). AGC2 runs in a separate "back" instance, so RNNoise sees the signal before gain (denoise first, then level).
   - Gate: voice activity with a fixed threshold, or automatic (RNNoise voice probability ≥ 0.6, else noise floor + 12 dB), with a 200 ms hangover; or push-to-talk with a release tail. Ramps (2.5 ms open, 10 ms close) avoid clicks.
   - All settings change live (`configure`).
   - Denormal guard `flush_denormals_on_this_thread` (x86_64 MXCSR FTZ/DAZ, aarch64 FPCR FZ), called at the start of the I/O thread.
   - `[profile.dev.package.*] opt-level = 3` for the DSP crates: unoptimised AEC3 cannot keep up in dev builds and tests.
   - Offline tests:
     - AEC on a synthetic echo path (30 ms, -12 dB, low-pass) needs ≥ 20 dB ERLE (measured 36.9);
     - RNNoise and WebRTC NS (High) need ≥ 10 dB on stationary noise (measured 15.5 and 20.7);
     - gate, PTT tail, limiter, live switching, and `Send`.
   - Mutation checks (AEC off; RNNoise bypassed) make the tests fail.
   - Release timing test (ignored; `--release -- --ignored timing`): 3 s far-end speech, then 10 s of silence, needs < 2 ms per frame with the guard.
   - The libwebrtc APM A/B backend moves to Task 8: `tessera-audio` does not link livekit, and the core does.
6. **Mixer, playout and mic test** (as built).
   - `tessera-audio::VoiceEngine` is the `FrameProcessor`. Each 10 ms it:
     1. mixes the remote sources (one 200 ms `rtrb` ring each, fed through `SourceInput::push` by the track receiver; per-source volume; trimmed above 100 ms queued; underruns counted; per-source level for "speaking");
     2. adds the mic-test loopback (the previous processed frame);
     3. applies deafen and master volume, then clamps;
     4. passes **exactly the played frame** to `CaptureChain::render` (the AEC reference);
     5. processes the microphone, and pushes it (silence when gated or muted) to the `Outgoing` ring for the sender, with a `transmitting` flag;
     6. publishes meters and statistics through atomics.
   - The owner uses `VoiceControl`: commands over a lock-free queue (configure, PTT, mute, deafen, loopback, master and source volume, add or remove source); meters and stats come back through atomics.
   - Core `audio_session::AudioSession` opens `AudioIo` with the engine (off the async threads) and runs a reporting task: `InputLevel` at 20 Hz, `AudioStats` at 1 Hz, device problems as `Error(Audio(..))`, closing devices on stop.
   - `StartMicTest { loopback }`, `StopMicTest`, `PushToTalk`, and settings changes are applied live; a device change reopens the session. DeepFilter maps to RNNoise until Task 13.
   - Tests:
     - mix math with volumes, removal and clamping;
     - underrun silence and counting; trim of a source far ahead;
     - deafen; mute (silence out, meter still live); loopback;
     - the echo reference is exactly the played frame: a perfect echo of the mix after source and master volume gives ≥ 20 dB ERLE, and rendering silence instead fails (2.9 dB);
     - settings mapping, and device changes needing a restart.
   - Tests that open real devices are named `hardware_*`, are `#[ignore]`, and are human-run. The ignored homeserver run (agent gate and CI) is now `--ignored --skip keyring --skip hardware_`.
7. **MatrixRTC signalling** (as built, matching Element Call 0.26.1 "compatibility" mode, its default, and matrix-js-sdk v43 `MembershipManager`; read at those tags in the scratchpad).
   - **Membership:** an `org.matrix.msc3401.call.member` state event under the key `_{user}_{device}_m.call` (no leading `_` in MSC3757/3779 room versions). The content is `{application: "m.call", call_id: "", scope: "m.room", device_id, membershipID: "{user}:{device}", expires: 14400000, "m.call.intent": "audio", focus_active: {type: livekit, focus_selection: "multi_sfu"}, foci_preferred: [{type: livekit, livekit_service_url, livekit_alias: room_id}]}`. It is built as raw JSON, because ruma has no `membershipID`.
   - **Crash-safe leave:** a delayed empty membership (MSC4140, 18 s), restarted every 4 s (`update_delayed_event::unstable_v2`), and rescheduled if the server lost it. Leaving fires it at once (`send`), falling back to sending `{}`.
   - **Expiry:** the membership is refreshed 10 min before it expires, with `created_ts` and a growing `expires`.
   - **Token:** our own SFU comes from MSC4143 discovery. The token comes from `POST {service}/sfu/get {room, openid_token, device_id}` (the legacy endpoint compatibility mode uses), and the LiveKit identity is then `{user}:{device}`. Service URLs must be https (http only on loopback); LiveKit URLs must be wss (ws only on loopback). The JWT is `Zeroizing` and redacted in `Debug`.
   - **Participants:** read from room state on every room-info change and every 5 s (expiry), as `CallParticipants` resets, oldest first. A state key not owned by its sender, another application or call, or no `expires` is ignored.
   - **Power levels:** members need permission to send `org.matrix.msc3401.call.member` (Element creates rooms with it at 0). `M_FORBIDDEN` gives "you are not allowed to join calls in this room".
   - `CallState::Connected` is emitted once the membership is published and the token obtained; Task 8 moves it to after the SFU connection. Mute and deafen are stored for Task 8.
   - Multi-SFU (subscribing to other members' SFUs from their `foci_preferred[0]`) is Task 8.
   - Integration tests (`tests/call_signalling.rs`): the published content checked field by field by a separate client; the membership still present after 25 s (> 18 s delay; mutation without the restart task fails, the server removed it); leave empties it; alice and carol each see both participants, and carol leaving drops her.
8. **LiveKit media** (as built, own SFU).
   - `call/media.rs` connects `livekit::Room` with the `/sfu/get` token.
   - **Publish:** the microphone is a `NativeAudioSource` (48 kHz mono, 50 ms queue, WebRTC's own AEC/NS/AGC off because our pipeline did it) published as `TrackSource::Microphone`, Opus at the configured bitrate, DTX and RED on. A task moves `Outgoing` frames into it.
   - **Receive:** each subscribed remote audio track becomes a `NativeAudioStream` read into a mixer source at that user's volume (identity `user:device` → user). Sources are removed on unsubscribe or disconnect, and `SetUserVolume` applies live.
   - **Flags:** `ActiveSpeakersChanged` and `TrackMuted`/`TrackUnmuted` set `speaking` and `muted` on participants.
   - **Mute and deafen:** mute stops the mixer output and mutes the track (others see it); deafen also mutes (Discord semantics).
   - **Connection:** `Reconnecting`/`Reconnected` become `CallState::Reconnecting { attempt }` and `Connected`; `Disconnected` leaves the call with `Failed`. `Connected` is now emitted after the SFU connection.
   - **Audio in a call:** joining opens an `AudioSession` in `Mode::Call` (replacing a mic test). Without usable devices the call still joins, silent, and the audio error is shown.
   - **Virtual audio:** `CoreConfig.audio: AudioBackend::{System, Virtual(Arc<dyn VirtualAudio>)}` (API: 480-sample `f32` frames) runs the voice engine on a 10 ms timer against software devices: tests, bots, headless use.
   - Integration test `tests/call_media.rs`: alice's virtual mic plays a 0.3-amplitude 1 kHz tone; carol's virtual speakers receive it at 0.2858 (-0.4 dB) through the SFU and Opus; carol sees alice `speaking`, then `muted`; after mute the tone is 0.0. Mutation: without the microphone sender, the test fails.
   - Not in this task: changing devices during a call (it applies to the next call; open issue), and the libwebrtc APM A/B backend (dropped from the plan, see notes).
8b. **Multi-SFU** (added 2026-10-02, as built; Element Call 0.26.1 and matrix-js-sdk v43 read at those tags).
   - **Transports per member** (js-sdk `CallMembership.getTransport`): `multi_sfu` uses the member's own `foci_preferred[0]`, `oldest_membership` the oldest active membership's; an unknown `focus_selection` gives no SFU. Transports compare by `livekit_service_url` (`membership::transport_of`, `expected_by_sfu`).
   - **One connection per SFU** (as Element Call's `ConnectionManager`): ours publishes; for every other SFU a member publishes on, a subscribe-only connection with a token from that service: `/get_token` first (`member { id, claimed_user_id, claimed_device_id }`, slot `m.call#ROOM`), then the legacy `/sfu/get` (EC's `getSFUConfigWithOpenID` without `matrixRTCMode`). A service for another homeserver grants subscribe-only access and creates no room (lk-jwt-service v0.7.0), which works because the publisher's own service created it.
   - **Only members are played** (as EC's `MatrixLivekitMembers`): a LiveKit participant plays only if its identity is an active member whose transport is that SFU. The identity of a session membership is `sender:device_id` (js-sdk `rtcBackendIdentity`), whatever `membershipID` says. The call services hand tokens to any Matrix user, so this also keeps non-members out of the mix (it applies on our own SFU too).
   - **Lifecycle:** `manage_sfus` opens connections as members appear, closes them when nobody publishes there, and retries a failed or lost one with backoff (2 s doubling to 60 s). Only our own SFU's notices reach the engine (`Reconnecting`, `Failed`); a lost remote SFU does not end the call. Active speakers are kept per SFU.
   - **Call service override:** `CoreConfig.call_service_override` publishes on a given service instead of the homeserver's (self-hosted SFU, tests). Discovery otherwise is MSC4143 then `.well-known` `rtc_foci` (the SDK's `discover_rtc_transports`).
   - **Local test stack:** a second LiveKit and lk-jwt-service pair (`livekit-b` on 7890–7892, `lk-jwt-b` on 8081, same dev key) stands in for another homeserver's call service.
   - Integration tests (`tests/call_media.rs`): with carol publishing on service B and alice on A, each hears the other's tone; a participant who joins the LiveKit room with a token but no membership is not heard while a real member is.

9. **Call E2EE** (as built; matrix-js-sdk v43 `RTCEncryptionManager` + `ToDeviceKeyTransport`, Element Call 0.26.1 `MatrixKeyProvider`, livekit-client 2.22.3, webrtc-sdk `m150_release` frame cryptor).
   - **When:** in rooms with an `m.room.encryption` state event (as Element Call's per-participant mode); `CallState::Connected { encrypted }` tells the UI.
   - **Keys:** a random 16-byte key per join at index 0, used at once. A new key (index + 1 mod 256) when a member leaves, also with nobody left to send it to (js-sdk makes that key but never uses it), or when one joins and ours is older than 10 s; otherwise a joiner gets the current key. A new key is used 1 s after sending (`useKeyDelay`). Devices that could not be reached are retried every 5 s.
   - **Transport:** `io.element.call.encryption_keys` to-device, Olm-encrypted, to every device of the other members (`send_encrypted_to_device`, `CollectStrategy::AllDevices`, as matrix-sdk's widget driver for Element Call); content `{ keys { index, key: padded base64 }, room_id, member { claimed_device_id, id }, session { call_id: "", application: "m.call", scope: "m.room" }, sent_ts }`. Received keys count only if Olm-decrypted, for this room, and claiming the Olm session's device (stricter than js-sdk, which cannot check this yet).
   - **LiveKit:** one `KeyProvider` for all SFU connections, keys per identity `user:device`, HKDF-SHA-256 (salt `LKFrameEncryptionKey`, 128 zero bytes of info, AES-GCM-128, as livekit-client for HKDF key material), ratchet window 10, key ring 256, failure tolerance 10. The native encryptor uses the key index it was given, not the latest key set, so a rotation also sets our track's cryptor index (again after a republish).
   - Integration test (`tests/call_media.rs`, encrypted room): alice and carol hear each other with keys over Olm; a LiveKit listener with a valid token but no keys gets noise only, and alice's publication is `Gcm`; after carol leaves (alice rotates) and rejoins, both hear each other again.

10. **Ringing and voice rooms** (as built; matrix-js-sdk v43 `MatrixRTCSession`/`CallNotification`, Element Call 0.26.1).
   - **Sending:** the first member to join (nobody else active) sends `org.matrix.msc4075.rtc.notification` after its membership: `notification_type` `ring` in a DM, `notification` elsewhere; `m.mentions { room: true }`; `m.reference` to our membership event; `sender_ts`, `lifetime` 90 s, `slot_id` `m.call#ROOM`, `msc4354_sticky_key`, `m.call.intent` `audio`. Sent as a regular event (js-sdk's fallback when sticky events are unavailable). A failed send only means nobody is rung.
   - **Receiving** (`call::ringing`, js-sdk `parseCallNotificationContent` rules): not our own; not withdrawn; our slot; mentioning us, or the room when the sender's power level may notify the room; not expired (`sender_ts` unless more than 20 s ahead of the server's timestamp, lifetime capped at 120 s); the call still has members and none of them is us (on any device). **Deviation:** js-sdk also requires an open `org.matrix.msc4143.rtc.slot` state event, which Element Call 0.26.1 never creates for `m.call.member` calls, so a missing slot counts as open; a closed one still stops the ring (MSC ambiguity, recorded in PROGRESS.md).
   - **Stopping** (`RingingStopped`): answered here or on another device (our membership appears), declined here or on another device (our own decline of that notification), the call ended (no members left), or expired (checked every 2 s).
   - **Declining:** `DeclineCall` sends `org.matrix.msc4310.rtc.decline` referencing the notification; with nothing ringing it is an error. The caller gets `CallDeclined { room_id, user_id }` for declines of the notification it sent.
   - **Voice rooms:** `RoomSummary.is_call_room` for MSC3417 rooms (`org.matrix.msc3417.call` room type, the SDK's `Room::is_call`); `ActiveCall { participants }` counts the devices in every room's call while syncing (re-counted every 2 s, as memberships expire without an event).
   - API additions for approval: `IncomingCall.ring`, `CoreEvent::CallDeclined`, `RoomSummary.is_call_room`.
   - Integration tests (`tests/call_ringing.rs`): a DM call rings carol, answering stops it and counts 2; declining stops it and tells alice, a second decline is refused; a group call notifies quietly and stops when it ends; an MSC3417 room is marked.
11. **Push-to-talk service** (`tessera-ptt`, as built; global-hotkey 0.8.0, ashpd 0.13.13, gilrs 0.11.2 read in the registry).
   - **Sources, merged** (any held means transmit; transitions only on change): a global keyboard shortcut, a gamepad button on any pad (a pad that disconnects lets go), and the bound key in Tessera's own window, which the app forwards (`PttService::window_key`). The release tail stays the audio gate's (`release_delay_ms`), so releases are reported at once. Dropping the service lets go.
   - **Keyboard mechanism per session** (`KeyboardBackend::detect`): gamescope (SteamOS game mode) → window key only; Wayland (`WAYLAND_DISPLAY` or `XDG_SESSION_TYPE=wayland`) → the GlobalShortcuts portal; X11 (`DISPLAY`) → global-hotkey; Windows and macOS → global-hotkey.
   - **global-hotkey:** the key is grabbed (other applications stop receiving it while it is bound); release is polled every 50 ms on X11 and Windows; the manager must be created on the main thread (macOS) or the event-loop thread (Windows), so the service is started and dropped there. Supports one key plus modifiers; no modifier-only or mouse-button bindings.
   - **Portal:** registers the host app ID `io.github.ga_lep.Tessera` (`register_host_app`, best effort), creates a session, binds `push-to-talk` with a preferred trigger in the XDG shortcuts format (`CTRL+SHIFT+t`; the compositor may let the user change it), and maps `Activated`/`Deactivated` to press and release. The confirmed trigger is reported (`PttEvent::Ready`).
   - **Settings:** `AudioSettings.push_to_talk { key, gamepad_button }`, strings in global-hotkey syntax (`ctrl+shift+KeyT`) and by button position (`left_thumb`); the core only checks they are short printable ASCII. API addition for approval.
   - **Probe:** `cargo run -p tessera-ptt --example ptt_probe -- --key <binding> --button <name>` prints transitions, for the human checks on real desktops (keys on Linux; gamepads everywhere).
   - **Not in this task:** the app starting the service from the settings and forwarding `Transmit` comes with the call UI (the app has no call yet); the CLI's `/ptt` is Task 12.
   - Unit tests: merging and transitions, pads and disconnects, binding parsing and portal triggers, the session table, the window-key service end to end and its release on drop, settings validation and persistence.

12. **CLI** (as built; `crates/core/src/bin/tessera-cli/calls.rs`, only the public core API).
   - **Calls:** `/call [n|id]` (the open room, a `/rooms` index, any room ID, or the last ring), `/leave`, `/decline [n|id]`, `/who` (participants with you/muted/speaking/volume), `/mute` and `/deafen` (`on|off`, toggle without), `/vol <n|@user> <percent>`, `/ptt [on|off]`. A terminal sees key presses but not releases, so `/ptt` toggles transmitting instead of holding (the global hotkeys of Task 11 need the app).
   - **Audio settings:** `/audio` (summary), `/devices`, `/input <n|default>`, `/output <n|default>`, `/ns off|rnnoise|webrtc [level]|deepfilter [dB]`, `/aec`, `/agc`, `/gate auto|<dB>|ptt [ms]`, `/mictest [loopback|stop]` (a level line every second). Each edit sends the whole settings value; the core's echo is printed.
   - **Shown as it happens:** joining, connected (encrypted or not), reconnecting, left or failed; others joining and leaving; incoming rings and quiet call notifications, ringing stopped, declines; `/rooms` marks voice rooms and the number of devices in each room's call. Names, devices and reasons go through the CLI's control-character filter.
   - `--silent-audio` runs calls on silent virtual devices (tests, scripts); without it a call opens the system microphone and speakers.
   - Tests: the parser, settings edits, devices, the in-call state, push-to-talk and rings as unit tests (`calls.rs`); `tests/cli.rs` joins a call from the CLI against the local stack, mutes (and sees itself muted in `/who`), leaves, quits, and checks that the membership is gone.

13. **DeepFilterNet3 via tract-onnx.**
   - `scripts/fetch-models.sh` fetches the DeepFilterNet3 ONNX export, pinned by SHA-256 like libwebrtc, with the MIT/Apache licence recorded.
   - `tessera-audio` implements the DF3 pipeline: STFT (960/480 at 48 kHz), ERB and complex features with the normalisation, the encoder/ERB-decoder/DF-decoder graphs in tract, ERB gain application, the deep-filter stage, and ISTFT with an overlap-add hop of 480. The math is studied from upstream libDF (MIT/Apache), not copied wholesale; any copied block keeps its licence header.
   - Exposed as `NoiseSuppression::DeepFilter { attenuation_limit_db }`.
   - **Pluggable enhancer** (added 2026-10-03, for Task 13b): the noise-suppression stage sits behind one trait (`process(&mut [f32; 480])`, a reported per-frame cost, a fallback), so a GPU or vendor backend slots in without touching the chain. RNNoise, WebRTC NS and DeepFilterNet on the CPU are the first implementations.
   - **Already-processed inputs** (added 2026-10-03): an input that is known to be processed already (NVIDIA Broadcast's virtual microphone on Windows, matched by device name) skips our noise suppression by default, so the voice is not cleaned twice. Whether AEC3 stays on for it is decided by ear (human item).
   - Tests: the model loads; output on speech plus noise improves SNR by at least the RNNoise result on the same fixture; the per-frame cost is recorded on x86_64 and aarch64; the fallback is RNNoise if the model files are missing.

13b. **GPU speech enhancement, opt-in** (added 2026-10-03, human request: "like NVIDIA Broadcast", optional, depending on hardware).
   - **Off by default; offered only when the hardware is there.** A capability probe at start lists the usable GPU backends; the setting is hidden without one. The CPU path stays the default everywhere, and the Steam Frame never uses the GPU (the VR game owns it).
   - **Vendor-neutral path:** run the Task 13 models (DeepFilterNet3, and heavier ones the CPU cannot afford: dereverberation, speech restoration) through ONNX Runtime with the CUDA/TensorRT execution provider on NVIDIA, DirectML on Windows, and ROCm/MIGraphX on AMD where available. This needs a new dependency (the `ort` crate and the ONNX Runtime library, its supply chain pinned like libwebrtc); **to be approved before the task starts** (rule 3).
   - **NVIDIA Maxine Audio Effects, Windows only, later:** the SDK behind NVIDIA Broadcast (denoise, dereverb, AEC, studio voice on RTX Tensor Cores). It is free on Windows; on Linux it is only available to NVIDIA AI Enterprise customers and meant for servers, so we cannot ship it there. Loaded at run time if the user has NVIDIA's runtime installed, behind a cargo feature and the setting; never bundled. Its licence and redistribution terms are checked before any code.
   - **Real-time rules:** the GPU call runs on the processing thread with a deadline (well under 10 ms); a missed deadline, a lost device or a driver error falls back to the CPU backend for the rest of the call and reports why. Incoming streams (cleaning other members' voices, one stream each) can be batched into one GPU call, where the GPU pays off most.
   - **Tests:** the probe and fallback logic as unit tests with a fake backend; SNR and per-frame latency on the same fixture as Task 13, recorded per backend; the GPU runs are human items (no GPU in CI).
   - **Frame analogue (research only):** Qualcomm's NPU through QNN instead of the GPU; recorded under Task 15, not planned.

14. **Own PipeWire client on Linux** (decision 12; replaces cpal there).
   - A `pw_stream` per direction on the `pipewire` crate (`=0.10.1`, MIT, already in the tree through cpal), driven by PipeWire's real-time data loop. The callbacks only move samples through the existing rings; the processing thread stays as it is (sonora allocates per frame, so no DSP in the RT callback).
   - Stream properties: `media.role = Communication` (WirePlumber policy: Bluetooth headset profile, ducking), `node.latency = 480/48000`, our `application.name`, `node.rate` 48 kHz when the graph allows, so nothing resamples.
   - Enumeration from the registry by `media.class`: `Audio/Source` and `Audio/Sink` (plus the Internal variants) only, so other applications' streams are no longer listed (Task 4's known limitation). Stable IDs stay the node names (`pipewire:<node.name>`), so saved settings keep working.
   - Device and default changes from registry and metadata events instead of the 2 s poll.
   - Optional system-mix echo reference: capture the playing sink (`stream.capture.sink`) and feed it to AEC3 as the render reference, so game audio is cancelled too. Measured with a fixture (game-like noise plus speech on a loopback sink): echo suppression with and without it.
   - Tests: registry filtering and the property set as unit tests; the ignored output-only stream test from Task 4 runs against it; the real-device behaviour is a human item.
15. **Steam Frame performance.**
   - A benchmark binary measures per-10 ms-frame cost of each stage (capture conversion, HPF, AEC3, each NS mode, AGC2, gate, mixer, Opus through LiveKit's encoder, playback) on x86_64 and on the CI aarch64 runner, recorded in PROGRESS.md. Budget target: the whole call path under 5 % of one core on the Frame (to be confirmed on the device).
   - Frame defaults from the measurements: RNNoise (DeepFilterNet opt-in), AEC3 on, 10 ms buffers, Opus at 32 kb/s with DTX; a "low power" profile if the budget is not met.
   - Real-time priority for the processing thread through rtkit (D-Bus, also inside Flatpak), with a fallback to normal priority and a reported reason.
   - Human check on the device: desktop and game mode, a standalone VR game running, CPU and battery during a call, crackles under load.
   - Research note (added 2026-10-03, not planned): the Frame's counterpart to Task 13b is Qualcomm's NPU through QNN rather than the GPU, which the VR game owns. Worth measuring only if DeepFilterNet on the CPU does not fit the budget.

## Dependencies (approved 2026-10-02, newest releases, exact pins)
| Crate | Version | Licence | Used by | Added in task |
|---|---|---|---|---|
| livekit (features `rustls-tls-native-roots`) | =0.9.3 | Apache-2.0 | core | 1 |
| matrix-sdk feature `experimental-send-custom-to-device` | (=0.19.1, unchanged) | Apache-2.0 | core | 9 |
| cpal (Linux: feature `pipewire`, `realtime`) | =0.18.2 | Apache-2.0 | audio | 4 |
| rtrb | =0.4.0 | MIT OR Apache-2.0 | audio | 4 |
| rubato | =5.0.1 | MIT OR Apache-2.0 | audio | 4 |
| sonora | =0.2.0 | BSD-3-Clause | audio | 5 |
| nnnoiseless | =0.5.2 | BSD-3-Clause | audio | 5 |
| tract-onnx | =0.23.8 | MIT OR Apache-2.0 | audio | 13 |
| global-hotkey | =0.8.0 | Apache-2.0 OR MIT | ptt | 11 |
| ashpd | =0.13.13 | MIT | ptt | 11 |
| gilrs | =0.11.2 | Apache-2.0 OR MIT | ptt | 11 |
| pipewire (direct; today through cpal) | =0.10.1 | MIT | audio | 14 |
| libspa (direct, format pods) | =0.10.1 | MIT | audio | 14 |

Each crate is added in the task that first uses it, so no dependency sits unused. Each task records `cargo tree` and `cargo deny check` output. A newer release that appears mid-phase is asked about before bumping (rule 3).

## Options considered and deferred (2026-10-02)
Recorded so the reasoning is not lost; none is planned.
- **Custom audio-only libwebrtc build.** Would bring a much smaller binary and download (no video codecs, screen capture or video processing; the static lib is 82 MB today, the shipped share unmeasured), fewer system dependencies (no glib, X11 or the PipeWire capture shims), Frame-specific compile flags, a source build we can audit instead of a checksum-pinned prebuilt, and faster local builds. It cannot be done alone: webrtc-sys always compiles its video and capture bridges (`video_track`, encoder/decoder factories, `desktop_capturer`, VA-API, NVIDIA, AV1), so it needs forks of webrtc-sys (and likely `livekit`), Chromium's build setup (depot_tools, a tens-of-GB checkout, hours per platform in CI), and LiveKit's exact WebRTC revision and patches, including the frame encryptor Element Call interop needs. Revisit for release packaging, with measurements first (release binary size, memory on the Frame during a call). A middle step is building the full libwebrtc from source with LiveKit's configuration: the supply-chain benefit without forks.
- **Pure-Rust WebRTC stack** (str0m or webrtc-rs) with our own LiveKit client. Would drop the C++ toolchain, but we would rebuild what libwebrtc gives today: ICE/TURN, DTLS-SRTP, NetEQ-grade jitter buffer and loss concealment (the hard part, and a large share of call quality on bad networks), bandwidth estimation, LiveKit's signalling protocol and its E2EE frame encryption matched to Element Call; Opus would still need libopus or a less proven Rust decoder. Months of work with interop risk and, at first, worse quality. For audio only, libwebrtc's CPU cost is small (NEON-optimised Opus and NetEQ), so it would not by itself help the Frame. A separate research spike if the C++ toolchain or binary size becomes a real problem.
- **PulseAudio protocol backend** (cpal `pulseaudio`, a pure-Rust client of pipewire-pulse). Used for a few hours after Task 8 as a stopgap for the `pw_*` crash, then dropped (decision 12): its only benefits (no C library at build time, old PulseAudio-only distributions, the standard Flatpak socket) do not matter for our targets, and it adds a buffer hop, normal-priority threads and less graph control.

## Human checkpoint (collected in PROGRESS.md, never ticked by the agent; rule 9)
- [x] Questions answered 2026-10-02 (see Decisions).
- [ ] Call API additions (Task 3) approved as the UI-phase contract.
- [ ] Audio quality by ear: real mic and speakers; echo cancellation on open speakers (sonora AEC3; the libwebrtc APM A/B was dropped in Task 8); RNNoise on keyboard and fan noise; AGC level; VAD gate threshold.
- [ ] Device hot-plug (USB headset in and out, Bluetooth) on each OS you use; macOS microphone permission prompt.
- [ ] Interop with Element Call, both directions, in unencrypted and encrypted rooms, on a real homeserver with a real SFU (matrix.org / element.io call infrastructure or your own).
- [ ] Cross-homeserver call (Task 8b): members on two homeservers with their own call services hear each other (the subscribe-only token path for a non-full-access homeserver cannot be exercised with the single local Synapse).
- [ ] Double talk by ear: talking while others talk, on open speakers and headphones. In a virtual test without any echo path, AEC3 cut a member's microphone to near silence while loud audio played on their speakers (Task 8b notes).
- [ ] Ringing (Task 10) with Element Web and Element X, both directions: a DM call rings and shows the caller, a group call notifies quietly, declining on one side stops the ring on the other, answering on another device stops it here.
- [ ] GPU enhancement (Task 13b): each offered backend on real hardware (NVIDIA, and AMD where available), by ear and with the recorded latency; the fallback when the GPU is busy with a game; NVIDIA Broadcast's microphone detected on Windows (Task 13).
- [ ] Global PTT hotkey on X11, Wayland (KDE/GNOME portal), Windows and macOS; gamepad PTT.
- [ ] Steam Frame: aarch64 build runs on the device (desktop mode and game mode); built-in mic and speakers with AEC; game audio cancelled with the system-mix reference (Task 14); PipeWire device switching; CPU/battery cost of each NS mode within the Task 15 budget while a standalone VR game runs; rtkit real-time priority granted.
- [ ] CLI call against a real account; leave by crash (kill -9) clears the membership within the delayed-event timeout.
- [ ] CI green on Linux, Windows and macOS with livekit/libwebrtc in the tree (build time and disk to be watched).
