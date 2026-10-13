# Phase 9 Video Plan
Status: APPROVED (2026-10-13): Q6-Q8 answered by the human (Vulkan Video for Intel, AMD and NVIDIA before Ada; NVENC/NVDEC through CUDA on Ada and newer; no VA-API), the other questions with their recommended answers (R), including the Q2 version bump and the Q11 crates (rule 3). Work happens on branch `feat/video`, cut from `fix/pre-video` (the two pre-phase fixes, waiting for review) so that Task 1 can re-run their key-logging test. Tracked in [ga-lep/twicecat#22](https://github.com/ga-lep/twicecat/issues/22): a task is ticked there when its commit lands; notes go to PROGRESS.md and `phase9-video.progress.md`.

**Before this phase (fixes outside this plan, on branch `fix/pre-video`: `fix(core): play a screen share's sound beside the sharer's voice`, `fix(core): keep call keys out of the log whatever RUST_LOG asks`):**
- Remote audio players are keyed by participant identity only (`crates/core/src/call/media.rs:602-625`), so an Element Call user's screen-share audio replaces their microphone, is voice-cleaned, and stopping the share silences them. Fix: key players by (identity, source).
- The native frame cryptor logs key material at INFO (webrtc-sdk/webrtc#308) and we set no libwebrtc log routing: check whether encrypted audio calls already leak keys into logs, and fix it if so.

## Tasks checklist
- [ ] Task 1: Bump livekit and check the logs for key material (phase9.1, `feat(core)`)
- [ ] Task 2: `twicecat-video` skeleton, frames, test pattern, ladder, API shape (phase9.2, `feat(video)` + core API)
- [ ] Task 3: Hardware probe and backend measurement spike (phase9.3, `feat(video)`)
- [ ] Task 4: Vulkan compute and Vulkan Video H.264 encoder (phase9.4, `feat(video)`)
- [ ] Task 5: Publishing and receiving in the core, software path (phase9.5, `feat(core)`)
- [ ] Task 6: Our encoder through the passthrough, with layers (phase9.6, `feat(core)` + the Q5 fork)
- [ ] Task 7: NVENC encoder through CUDA, NVIDIA Ada and newer (phase9.7, `feat(video)`)
- [ ] Task 8: Linux screen sharing (phase9.8, `feat(video)` + core)
- [ ] Task 9: Linux camera (phase9.9, `feat(video)` + core)
- [ ] Task 10: App: video renderer and call stage (phase9.10, `feat(app)`)
- [ ] Task 11: App: controls, pickers, preview, Video settings (phase9.11, `feat(app)`)
- [ ] Task 12: Windows and macOS (phase9.12, `feat(video)` + core)
- [ ] Task 13: Steam Frame (phase9.13, `feat(video)`; conditional)
- [ ] Task 14: 4K60 acceptance and tuning (phase9.14, `feat(video)` + docs)
- [ ] Task 15: Hardware decode (phase9.15, `feat(video)` + core; gated on Q5's receive side)
- [ ] Task 16: Wrap-up (phase9.16, `docs`)

## Goal
Camera video and screen sharing (with its audio) in MatrixRTC calls, up to 3840×2160 at 60 fps in both directions, interoperable with Element Call in encrypted rooms, on Linux (Wayland and X11), Windows, macOS, and later the Steam Frame.

**How we get there: we own the video pipeline in Rust and keep libwebrtc for transport.** Capture, colour conversion, scaling and encoding are our code (`crates/video`), on the hardware encoder of each machine. libwebrtc (through `livekit`) keeps doing what it already does for audio: congestion control, packetisation, retransmission, jitter buffers, SRTP and the E2EE frame cryptor. Encoded frames enter it through LiveKit's pre-encoded ("passthrough") video source, which hands us WebRTC's bitrate and framerate targets and keyframe requests. Decoding starts on libwebrtc's software decoders and moves to our own hardware decoders once LiveKit exposes encoded frames on the receive side.

**Rust-native rule for this phase.** Everything we write is Rust. FFI is allowed only to OS and driver APIs (Vulkan, the NVIDIA driver's CUDA, NVENC and NVDEC, Media Foundation, Direct3D, VideoToolbox, ScreenCaptureKit, AVFoundation, PipeWire, V4L2) and to libwebrtc, which Phase 6 already uses. No FFmpeg, no GStreamer, no OpenH264. Every `unsafe` site follows the Phase 6 decision 7 rule (SAFETY comment, listed in PROGRESS.md). A new crate is added only when writing it ourselves would not be justified, and each one is in the Q11 list for approval.

Rules for every task (AGENTS.md):
- **Rule 2:** every `livekit`, `libwebrtc`, `ash`, NVIDIA driver (CUDA driver API, NVENC, NVDEC: the Video Codec SDK headers of the version the probe requires), `ashpd`, `pipewire`, `windows`, `objc2-*` and `iced` API is read in the registry source of the pinned version before use and cited file:line in PROGRESS.md. The facts below come from planning and are re-checked by each task.
- **Rule 3:** the version bumps in Q2 and the crates in Q11 need the human's approval first. Nothing else changes.
- **Rule 5:** one `feat(<scope>)` commit per task with a `Step: phase9.<n>` trailer (scopes `video`, `core`, `app`, `dev`). Review fixes are folded in with `--fixup` and `rebase --autosquash`. PROGRESS.md is updated in the same commit, and the task is ticked in this issue.
- **Rule 6:** tests use the local stack only (Synapse plus both local SFUs, `scripts/stack.sh`) and the test accounts. Frames, window titles, device serials, encoded bytes and keys are never logged.
- **Rule 7:** the app talks to the core only through `Command`s, `CoreEvent`s and the new SDK-free `VideoFeed` handle in `crates/core/src/api/`. `scripts/check-app-deps.sh` and `scripts/check-core-api.sh` gain `twicecat-video`, `ash` and the capture crates.
- **Rule 8:** stop and ask if an upstream change in Q5 is refused and the fork would grow beyond the passthrough code, or if a hardware path misses the 4K60 budget on the human's machine after tuning.
- **Rule 9:** anything seen or heard (rendering, colours, A/V sync by eye and ear, portal and permission dialogs, interop with Element Call in a browser) is a human checkpoint item. Measured A/V offset (Task 14) is agent-verifiable.
- **Gate before each commit:** as in Phase 8, plus the ignored video stack tests from Task 5 on, and the hardware suite (`--features hw-video`, Task 4 on) on the human's machine before a hardware task is marked done.

## Facts verified while planning (2026-10-13)
`$R` = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f`, `L` = `$R/libwebrtc-0.3.50`, `W` = `$R/webrtc-sys-0.3.47`.

**Test hardware on record:** the human's laptop has an Intel Arrow Lake iGPU and an NVIDIA RTX 5080 Mobile (`lspci`; CUDA in use by Phase 6 Task 13b). It is a hybrid machine: the compositor's DMA-BUFs may live on either GPU. Phase 6 decision 11 adds the Steam Frame (Snapdragon, SteamOS). No macOS or Windows machine, and no 4K60 webcam, is on record.

**Pre-encoded publishing (pinned livekit 0.9.3 / libwebrtc 0.3.50 / webrtc-sys 0.3.47)**
- `NativeVideoSource::new_encoded(resolution)` (`L/src/video_source.rs:92`), `capture_encoded_frame(&EncodedVideoFrame)` (`:160`) with codec H264/H265/VP8/VP9/AV1, Key/Delta, timestamp (`L/src/video_frame.rs:101-114`). Selected by `TrackPublishOptions.video_encoder = PreEncoded` (`livekit-0.9.3/src/room/options.rs:137`).
- The C++ passthrough (`W/src/passthrough_video_encoder.cpp:215-321`) hands our bytes to RTP unchanged. `take_rate_control_request()` returns `{ target_bitrate_bps, framerate_fps }` from WebRTC's `SetRates` (`:323-353`; `L/src/native/video_source.rs:225-231`); `take_keyframe_request()` reports PLI/FIR (`:245-253`; `video_source.rs:219`). Both update only when we push a frame. Loss and RTT come from stats only.
- **Bug in the pinned version:** the passthrough does not declare a trusted rate controller (`:329-338`), so WebRTC's frame dropper drops delta frames when we overshoot, which breaks decoding until the next keyframe (upstream measured 24 freezes in a 1080p60 minute). **Fixed in webrtc-sys 0.3.49 / libwebrtc 0.3.52 / livekit 0.9.5 (2026-10-12)**, which also adds `PeerConnection::set_bitrate`.
- **Single layer only:** the passthrough reports `supports_simulcast=false` and index 0 (`:294,335`); AV1 accepts L1T1 only, VP9 one spatial and one temporal layer (`:145-163`, `:402-404`). H.264 is advertised as `42e01f` (`:369-373`). H.265 is advertised only when a real local H.265 encoder exists (`W/src/video_encoder_factory.cpp:396-416`).
- **E2EE still applies:** the frame cryptor is attached with `SetEncoderToPacketizerFrameTransformer`, after the encoder (`W/src/frame_cryptor.cpp:142,164,215`), and the passthrough sets the codec type it needs (`:131`).
- `RtpSender::set_parameters` (active, max bitrate, max framerate, scaling, scalability mode) is exposed (`L/src/rtp_sender.rs:79-84`, `L/src/rtp_parameters.rs:132-145`). Outbound and inbound video stats are exposed (`L/src/stats.rs:347-456`).
- **No custom encoder or decoder factory** is reachable from Rust (`W/src/peer_connection_factory.cpp:189-192`). **No encoded frames on the receive side** yet: open upstream PR livekit/rust-sdks#1515 (2026-10-11) adds a passthrough decoder, `NativeEncodedVideoStream` and `request_keyframe`.
- **Decoders in the pinned Linux prebuilt:** VP8/VP9 (libvpx), AV1 (dav1d), H.264 (FFmpeg software), no H.265; NVDEC only in a CUDA build; no VA-API or Windows hardware decode.
- **Desktop capture inside libwebrtc** exists (PipeWire, X11, WGC/DXGI, ScreenCaptureKit) but hands out CPU BGRA frames; **no camera capture** is wrapped. We use neither (Q3).
- `RoomOptions.adaptive_stream` and `dynacast` are off by default (`livekit-0.9.3/src/room/mod.rs:466-467`).

**Element Call interop** (Element Call on livekit-client 2.22.3)
- Tracks are identified by LiveKit source (`Camera`, `ScreenShare`, `ScreenShareAudio`), **one screen share per participant**, screen-share audio is played with its own volume.
- Defaults: VP8; camera 720p30 1.7 Mbps with 180p/360p simulcast; screen 1080p30 5 Mbps. Its developer settings allow up to 4K60 VP9 at 15 Mbps.
- **Under E2EE there is no backup codec** (livekit-client adds it only without encryption), so every viewer must decode exactly what we publish.
- **AV1 cannot be decrypted by livekit-client** ("av1 is not yet supported for end to end encryption"). VP8, VP9, H.264 and H.265 use the same frame format in the browser and in our native cryptor (trailer `ciphertext‖IV‖IV_LEN‖keyIndex`; H.264/H.265 clear up to the first slice NAL + 2 bytes). Firefox cannot decode H.265.
- **H.264 start codes:** the browser rewrites NAL units to 4-byte start codes before checking the tag; the native cryptor does not. A sender that writes 3-byte start codes fails decryption everywhere (inferred from code, client-sdk-js#2140).
- Element Call runs `adaptiveStream` and `dynacast`; the SFU picks the smallest layer at least 0.9 × the rendered height. The 4K layer is pulled only by a spotlight above about 1200 physical pixels tall.
- **livekit-server:** no per-track bitrate cap; all codecs enabled; congestion control with `allow_pause: true` pauses a layer a viewer cannot take, so **a single 25 Mbps layer means slow viewers get no video**; at most 3 simulcast layers; `packet_buffer_size_video` 500 packets (about 200 ms at 25 Mbps).
- **The native cryptor logs key material at INFO** (webrtc-sdk/webrtc#308). Our code sets no libwebrtc log routing today (`grep` found none). Checked on `main` before this phase (see Status); Task 1 re-checks after the bump.

**Rust ecosystem (versions as of 2026-10-13)**
- `ash 0.38.0+1.3.281` is already in `Cargo.lock` (through wgpu 27) and has `VK_KHR_video_encode_queue`, `_h264`, `_h265`; **AV1 encode needs a newer ash** (Vulkan 1.4.302+).
- `x11rb 0.13.2` and `zune-jpeg 0.5.15` are already in the lock. zune-jpeg decodes a 4K 4:2:0 JPEG in 26.7 ms on one Arrow Lake core (measured in planning), so 4K60 MJPEG needs two frames in flight.
- `ashpd 0.13.13` has `screencast`, `camera` and `pipewire` features; `windows 0.62.2` has Media Foundation, Direct3D 11/12 video and `Windows.Graphics.Capture`.
- iced 0.14 uses wgpu 27.0.1: no DMA-BUF import API. wgpu 30 adds `texture_from_dmabuf_fd` (single plane only); iced moved to wgpu 30 on master, not released.
- Reference code (not dependencies): Software Mansion's `gpu-video` (Vulkan Video, needs wgpu ≥29).
- Phase 6 already loads NVIDIA's libraries at run time with `libloading` (0.8.9 in the lock) and its own small bindings (`crates/audio/src/onnx/stateful_loop/cuda.rs`); the NVIDIA video backends follow the same pattern.

## Questions
**Q1. Numbering, order and branch.** (R) Phase 9, after the Phase 6 call UI checkpoint; independent of Phase 8 (spaces). Branch `feat/video`. Trailers `Step: phase9.<n>`.

**Q2. Version bump (rule 3).** (R) Bump `livekit` 0.9.3 → 0.9.5 (libwebrtc 0.3.52, webrtc-sys 0.3.49) in Task 1, for the passthrough frame-drop fix and `set_bitrate`, with the prebuilt libwebrtc tag and SHA-256 in `scripts/fetch-libwebrtc.sh` updated if the new `webrtc-sys-build` names a new tag. The whole audio suite and the call stack tests must pass unchanged before any video work.

**Q3. Who encodes.**
- (a) (R) We do: our encoders feed the passthrough source. libwebrtc's own encoders are used only as the no-hardware fallback (raw I420 into `NativeVideoSource`, VP8, at most 1080p30).
- (b) libwebrtc encodes; we fork webrtc-sys to add hardware encoders in C++.
- (c) Replace libwebrtc with a Rust WebRTC stack (str0m) and drive LiveKit's signalling ourselves. Most control, but it rewrites Phase 6's transport, E2EE and audio path (rule 8). Deferred; revisit after this phase.

**Q4. Codecs.** Element Call's browsers cannot decrypt AV1 and get no fallback codec under E2EE.
- (a) (R) **H.264 everywhere by default**, Constrained Baseline with levels up to 5.2 (4K60), 4-byte start codes, every hardware encoder supports it, every browser decodes it. **AV1 as an opt-in** "Better quality (twicecat and unencrypted calls only)" setting, on encoders that have it: NVENC on Ada and newer (Task 7) first; Vulkan Video AV1 needs an `ash` release with `VK_KHR_video_encode_av1` (the locked 0.38.0 has none), asked for when one exists. No VP9 (no NVIDIA/AMD/Apple hardware encode), no H.265 (Firefox, no software decoder in our prebuilt).
- (b) H.264 High profile by default (about 10-15 % fewer bits), after checking that Firefox and Chrome decode High in a stream negotiated as `42e01f`.

**Q5. Simulcast with our encoder.** The passthrough is single-layer, and the SFU pauses a layer a viewer cannot take, so one 4K layer leaves viewers on slow links with nothing.
- (a) (R) We extend the passthrough upstream (livekit/rust-sdks): accept one encoded stream per simulcast layer (index 0-2), and on the receive side adopt PR #1515. Until a release ships it, we carry the change as a pinned git fork through `[patch.crates-io]` (webrtc-sys bridge code only; the prebuilt libwebrtc is unchanged). Each layer's fork commit is reviewed like our own code.
- (b) Single layer only, with our encoder following the bitrate target (everyone gets the same picture; slow viewers stall).
- (c) Publish separate tracks per layer (Element Call would show extra screen shares; rejected).

**Q6. Hardware encode backends.** Answered by the human (2026-10-13): Vulkan for most GPUs, CUDA for modern NVIDIA; no VA-API.
- **NVIDIA Ada and newer** (RTX 40 and 50, compute capability ≥ 8.9; the human's RTX 5080): **NVENC** through the NVIDIA driver's CUDA driver API, on Linux and Windows. H.264 and AV1 (Q4 opt-in) encode.
- **Intel, AMD and NVIDIA before Ada:** **Vulkan Video** (`ash`, already in the lock), on Linux and Windows.
- **Shared by both:** capture import (DMA-BUF, D3D11), colour conversion and scaling run in our own Vulkan compute shaders. NVENC takes the result through Vulkan→CUDA external memory (opaque fd / Win32 handle), checked in Task 3.
- **Fallbacks:** an NVIDIA GPU before Ada whose driver has no Vulkan Video encode uses NVENC. On Windows, Media Foundation comes next. Then the software path (Q3). macOS uses **VideoToolbox**, the Steam Frame **V4L2 stateful**.
- **Choice per machine:** a probe makes it at run time (vendor, compute capability through `cuDeviceGetAttribute`, NVENC caps, Vulkan Video caps), and settings can override it.
- **To check in Task 3:** NVENC, NVDEC and the CUDA driver API come with the NVIDIA driver (`libcuda`, `libnvidia-encode`, `libnvcuvid`; `nvcuda.dll`, `nvEncodeAPI64.dll`, `nvcuvid.dll`), not with the CUDA runtime that `scripts/fetch-cuda-runtime.sh` downloads. If so, they work in the light build. NVENC on Linux needs the proprietary driver: with nouveau or NVK the probe offers Vulkan Video or software. GeForce drivers cap concurrent NVENC sessions. Up to 3 layers × camera and screen means 6 sessions, and Task 3 records the cap.
- Task 3 measures NVENC on the RTX 5080 and Vulkan Video on the Arrow Lake iGPU (and on the RTX 5080, for comparison) before the backend tasks start. No AMD GPU or NVIDIA GPU before Ada is on record: those paths are human checkpoint items.

**Q7. Decoding.** (R) Phase 9 renders what libwebrtc decodes (software H.264 via FFmpeg, AV1 via dav1d, VP8/VP9 via libvpx). Answered by the human (2026-10-13), with the Q6 split. Task 15 adds our own hardware decode: **NVDEC** through CUDA on Ada and newer, **Vulkan Video** decode on Intel, AMD and NVIDIA before Ada, Media Foundation as the Windows fallback, VideoToolbox on macOS through the receive-side passthrough once it is released or carried by the Q5 fork. 4K60 software H.264 decode is measured in Task 14 so we know the CPU cost until then.

**Q8. Getting frames to the screen.** Answered by the human (2026-10-13), with the Q6 split. The core hands the app a `VideoFeed` per track: a latest-frame slot (NV12 or I420 planes in `Arc<[u8]>`, size, colour info, timestamp) with a wake-up the app's subscription waits on at most once per display frame. The app draws it with an iced `shader` widget: one persistent wgpu texture per plane, `queue.write_texture` per frame, YUV→RGB (BT.709, limited/full range) in WGSL. No `image::Handle` per frame (it allocates a new texture each time and splits 4K over 4 atlas layers). A Vulkan Video or NVDEC decoder (Task 15) copies its NV12 output to host memory for the `VideoFeed` (NVDEC: `cuMemcpy2D` from the mapped frame). Zero-copy from decoder to screen (CUDA→Vulkan external memory, then a wgpu import) waits for an iced release on wgpu ≥30 (a later pin bump).

**Q9. Screen picker and capture per platform.** (R)
- Wayland: the xdg-desktop-portal ScreenCast picker (`ashpd` `screencast`) with PipeWire DMA-BUF streams (our `pipewire` client); cursor embedded; "remember this choice" stores the portal restore token, opt-in.
- X11: our own picker (screens and windows with thumbnails) and XShm capture through `x11rb`; 4K60 on X11 is best effort.
- Windows: our own picker; `Windows.Graphics.Capture` through `windows` (D3D11 textures).
- macOS: the system picker (`SCContentSharingPicker`) and ScreenCaptureKit (IOSurface frames).

**Q10. Camera capture.** (R) Linux: the Camera portal (`ashpd` `camera`) and PipeWire, so it works inside Flatpak; NV12/YUYV directly, MJPEG decoded by `zune-jpeg` with two frames in flight (4K60 needs it). Windows: Media Foundation `IMFSourceReader` (its MJPEG decoder). macOS: AVFoundation. Cameras that offer only 4K30 are offered at 4K30.

**Q11. New crates and features (rule 3).** (R) Newest releases at the start of each task, exact pins:
- `yuvutils-rs` (CPU colour conversion and scaling; pure Rust SIMD) - Task 2;
- no crate for NVENC, NVDEC or CUDA: our own bindings, loaded with `libloading` 0.8.9 (already in the lock) as a direct dependency of `twicecat-video` (Task 3);
- `objc2-video-toolbox`, `objc2-core-video`, `objc2-core-media`, `objc2-screen-capture-kit`, `objc2-av-foundation` (macOS) - Task 12;
- `v4l2r` (Steam Frame encoder; licence checked first) - Task 13;
- features on existing pins: `ashpd` `screencast`, `camera`, `pipewire`; `windows` Media Foundation, Direct3D 11, Graphics.Capture, WinRT features; `ash` (already locked) as a direct dependency; `x11rb` 0.13.2 and `zune-jpeg` 0.5.15 as direct dependencies at their locked versions.

**Q12. Quality presets and limits.** (R) A per-share choice: Auto, 720p30, 1080p60, 1440p60, 4K30, 4K60 (default Auto = the highest the encoder probe and the measured uplink allow, capped at 1440p60 until the human raises it). Screen shares keep resolution and drop framerate first; cameras keep framerate and drop resolution first. Starting bitrates for H.264 (tuned in Task 14): 4K60 20 Mbps (screen) and 25 Mbps (camera), 1440p60 12 Mbps, 1080p60 8 Mbps; lower layers 1080p30 4 Mbps and 540p15 0.8 Mbps (screen), 720p30 1.7 Mbps and 360p30 0.45 Mbps (camera). AV1 at 0.6 × these.

## Architecture
**New crate `crates/video` (`twicecat-video`)**, like `crates/audio`: no Matrix, no LiveKit, no UI.
- `frame`: `Frame` = CPU planes (NV12, I420, YUYV, BGRA with strides) or a GPU handle (`DmaBuf { planes, modifier }`, D3D11 shared handle, IOSurface), plus size, colour space, timestamp.
- `capture`: `ScreenCapturer` and `CameraCapturer` traits; backends `portal` (Wayland), `x11`, `wgc` (Windows), `sck` (macOS), `pw_camera`, `mf_camera`, `avf_camera`; `TestPattern` (moving content plus a machine-readable frame counter and send timestamp) for tests.
- `gpu`: our own Vulkan device (`ash`), DMA-BUF and D3D11 import, compute shaders for BGRA→NV12 and per-layer downscaling, shared by the Vulkan encoder.
- `encode`: `Encoder` trait (`configure(codec, layers)`, `encode(frame, force_key) -> Vec<EncodedLayerFrame>`, `set_rates(per-layer bitrate, fps)`); backends `vulkan`, `nvenc`, `mf`, `vtb`, `v4l2`; an Annex B writer that always uses 4-byte start codes; a probe that reports what each backend can do on this machine.
- `ladder`: pure functions from a bitrate target to resolution and framerate per layer, by degradation preference (tested without hardware).
- `audio_share`: system/app audio capture for screen shares (Linux through `twicecat-audio`'s PipeWire client, Windows WASAPI process loopback excluding our own process, macOS ScreenCaptureKit audio excluding our own), handed to the core as PCM for a `ScreenShareAudio` track (Opus stereo, music settings, never noise-cleaned, Phase 6 decision).

**Core** (`crates/core/src/call/video.rs`, `video_feed.rs`):
- publishes `Camera`, `ScreenShare`, `ScreenShareAudio` tracks; drives the encoder from `take_rate_control_request`/`take_keyframe_request`; codec choice per Q4;
- subscribes remote video (`RemoteTrack::Video`, ignored today at `call/media.rs:602-625`), turns `NativeVideoStream` frames into `VideoFeed`s; turns on `adaptive_stream` and `dynacast`; maps the app's tile sizes to `set_video_dimensions`;
- E2EE is unchanged: the existing `KeyProvider` already covers every track (`call/mod.rs:250-290`, `media.rs:106-121`).

**API additions** (SDK-free; the human approves the shape after Task 2):
- `CallIntent::Video`; MatrixRTC membership `m.call.intent: "video"` as matrix-js-sdk spells it.
- Commands: `ListVideoDevices`, `ListScreenSources` (X11/Windows), `StartCamera { device, quality }`, `StopCamera`, `StartScreenShare { source, quality, audio }`, `StopScreenShare`, `SetVideoView { track, width, height, visible }`, `UpdateVideoSettings(VideoSettings)`, `ProbeVideoEncoders`, `StartCameraPreview` / `StopCameraPreview`.
- Events: `VideoDevices`, `ScreenSources` (titles redacted in `Debug`), `VideoSettings`, `VideoTracks { room_id, tracks: Vec<VideoTrackView { id, participant, device, kind: Camera | Screen, width, height, fps, codec }> }`, `VideoFeed { track, feed }`, `LocalVideo { camera, screen }`, `VideoEncoders` (backend, codecs, max size and fps per codec, hardware or not), `VideoStats` (about 1 Hz: sent and received fps, bitrate, encode and decode time, quality limitation reason).

**App:** a call stage (grid of camera tiles, a spotlight for screen shares and the active speaker), fullscreen and pop-out window for a screen share, the camera and share buttons with pickers, a self-view, and a **Video** card in Settings (camera, quality, codec, encoder status like the GPU line in Voice & audio, upload cap).

## Tasks
### Task 1: Bump livekit and check the logs for key material (phase9.1, `feat(core)`)
Files: `Cargo.toml`s, `Cargo.lock`, `scripts/fetch-libwebrtc.sh`, `scripts/checksums/libwebrtc.sha256`, PROGRESS.md.
- Q2 bump. Re-run the key-logging check from the pre-phase fix against the new libwebrtc.

Acceptance: full gate, full ignored suite including the call and GPU suites, unchanged results; the key-logging regression test passes.

### Task 2: `twicecat-video` skeleton, frames, test pattern, ladder, API shape (phase9.2, `feat(video)` + core API)
- `frame`, `TestPattern`, `ladder`, the Annex B writer, CPU conversion with `yuvutils-rs`; the API types and commands above as stubs returning `NotSupported`.
- Identity: `VideoTrackView.participant` and `device` come from the existing membership mapping (`backend_identity`, `crates/core/src/call/membership.rs:169`), as for audio participants.

Acceptance: unit tests for the ladder (target bitrate → layers, both preferences), start-code writing, NV12/I420 round trips, test-pattern counter decoding; `check-core-api.sh` passes; the human approves the API shape.

### Task 3: Hardware probe and backend measurement spike (phase9.3, `feat(video)`)
- A probe for Vulkan Video (queues, H.264/H.265 encode caps, max size and rate, through the locked `ash`) and NVIDIA (the driver's CUDA, NVENC and NVDEC libraries loaded with `libloading`: compute capability, NVENC codecs and caps, session limit, Vulkan→CUDA external memory). It picks the backend per GPU as Q6 says.
- Speed reference before writing any encoder: the human's machine encodes a 4K60 test clip for 30 s with existing tools that use the same driver paths (FFmpeg's `h264_vulkan` on both GPUs, `h264_nvenc` and `av1_nvenc` on the RTX 5080), run by a script in `scripts/`. These tools are measuring instruments only, never dependencies.

Acceptance: a table in PROGRESS.md: encode time p50/p99 per frame, bitrate accuracy, CPU use, per backend and GPU, and the session cap and Vulkan→CUDA interop result. The Q6 split is confirmed by the measurements, or revisited with the human.

### Task 4: Vulkan compute and Vulkan Video H.264 encoder (phase9.4, `feat(video)`)
- Our Vulkan device; BGRA/NV12 import from CPU memory and DMA-BUF (`VK_EXT_image_drm_format_modifier`); compute conversion and downscale to up to 3 layers; H.264 encode (Constrained Baseline, levels to 5.2, IDR on request, CBR/VBR per layer from `set_rates`, intra refresh off, no B-frames), SPS/PPS from the driver.
- **Hybrid GPUs:** conversion runs on the Vulkan device that owns the captured DMA-BUF (matched by DRM render node from the PipeWire stream). When the encoder is on another GPU (for example capture on the iGPU and NVENC on the RTX 5080, Task 7), frames cross through a CPU copy, and the probe reports which. On battery the iGPU is preferred (Task 11 setting).
- `--features hw-video` gates the hardware tests (ignored without a GPU).

Acceptance: hardware tests on the Arrow Lake iGPU, and on the RTX 5080 through Vulkan Video as a check of the path that NVIDIA GPUs before Ada take (NVENC is its default there, Task 7): 4K60 test pattern for 60 s, all three layers, encode p99 under 8 ms, bitrate within ±15 % of target; the stream is written to a file and checked with `ffprobe -count_frames` when that tool is installed (test-only, skipped otherwise; the end-to-end decode check is in Task 6); unit tests on the CPU-side logic.

### Task 5: Publishing and receiving in the core, software path (phase9.5, `feat(core)`)
- Video tracks in `call/video.rs`; libwebrtc-encoded fallback (raw path, VP8 ≤1080p30); remote subscription, `VideoFeed`, adaptive stream, dynacast, `SetVideoView` (sizes in physical pixels, since the SFU picks layers by physical height); remote camera mute shows as `VideoTrackView.muted`; CLI `/video pattern <res> <fps>`, `/video stats`.
- Remote screen-share audio plays with its own volume (`SetUserVolume` gains a `source`), never cleaned, never a speaking indicator (builds on the pre-phase keying fix).

Acceptance (stack tests, CI): two CLIs in an **encrypted** room; alice publishes a 720p30 test pattern; bob's feed delivers it in order with the counter intact, ≥ 28 fps over 30 s; leaving the call stops the feeds; a `RemoteTrack::Video` from another device appears as a `VideoTrackView`.

### Task 6: Our encoder through the passthrough, with layers (phase9.6, `feat(core)` + the Q5 fork)
- The Q5 change (upstream PR plus pinned fork); the encoder loop driven by rate and keyframe requests; codec policy (Q4); `quality_limitation_reason` and freezes in `VideoStats`.
- **Encoder loss mid-call** (GPU reset, dGPU powered down, driver error): the core re-probes and moves to the next backend, then to the software path; if the codec or layer count changes it republishes the track; the app gets one `VideoEncoders` update and a notice, like the audio GPU fallback in Phase 6.

Acceptance:
- CI (no GPU): pre-encoded H.264 fixture streams (generated by Task 4's encoder and committed, small sizes) published through the passthrough in an encrypted room; bob decodes every frame; a forced keyframe request reaches the sender within 1 s; a fake encoder that fails mid-call triggers the fallback and the stream continues.
- Hardware: alice publishes 4K60 test pattern with three layers; bob asking for a small tile gets the 540p layer, a full-screen tile gets 4K; shaping alice's uplink to 8 Mbps lowers the top layer without freezes longer than 500 ms. Shaping runs inside the stack: a test-only `netem` sidecar container with `NET_ADMIN` in `docker-compose.yml` (a `dev` change in this task), never `tc` on the host.

### Task 7: NVENC encoder through CUDA, NVIDIA Ada and newer (phase9.7, `feat(video)`)
- Our bindings to the CUDA driver API and NVENC (`libloading`, each `unsafe` site listed). One NVENC session per layer. Input from the Task 4 Vulkan compute output through Vulkan→CUDA external memory. H.264 (Constrained Baseline, levels to 5.2, IDR on request, CBR per layer from `set_rates`, no B-frames) and AV1 (Q4 opt-in). SPS/PPS from NVENC, written with 4-byte start codes. The probe picks it on Ada and newer, and on an NVIDIA GPU before Ada without Vulkan Video encode.

Acceptance: the Task 4 hardware tests pass on the RTX 5080 with this backend, for H.264 and AV1. An AV1 call between two CLIs in an unencrypted room and an encrypted one (native on both ends). Losing the CUDA context (driver error, dGPU powered down) is reported to the Task 6 fallback.

### Task 8: Linux screen sharing (phase9.8, `feat(video)` + core)
- Portal ScreenCast with DMA-BUF and SHM fallback, restore tokens (opt-in), cursor embedded, stream end when the user stops it from the compositor; X11 picker data and XShm capture; screen-share audio through our PipeWire client, excluding our own call playback nodes.
- 8-bit SDR formats only: on an HDR or 10-bit output the capture asks for 8-bit BGRA/NV12 and the compositor tone-maps; HDR sharing is deferred. HiDPI and mixed-scale monitors are captured at physical resolution.

Acceptance: an ignored test on a headless Wayland compositor if one runs here (else a human item); unit tests on the portal state machine with a fake; audio-share test with a test tone through a PipeWire null sink proving our own call audio is not in the share.

### Task 9: Linux camera (phase9.9, `feat(video)` + core)
- Camera portal and PipeWire; NV12/YUYV/MJPEG; two-in-flight MJPEG decode; hot-plug; preview feed.

Acceptance: unit tests with recorded MJPEG frames (decode rate ≥ 60 fps for 4K with two workers on the human's machine, recorded); a virtual camera (`v4l2loopback` if available, else a PipeWire test source) feeds a call end to end.

### Task 10: App: video renderer and call stage (phase9.10, `feat(app)`)
- The `shader` widget (Q8), tiles, spotlight, active speaker, fullscreen, pop-out window, frame pacing to the display, `SetVideoView` from tile sizes times the window's scale factor, the avatar when a camera is muted or off, encrypted/unverified marks as for audio.
- A/V sync: video frames are presented with a delay equal to the audio path's measured latency for that participant (our audio pipeline bypasses libwebrtc's own sync), capped at 300 ms.

Acceptance: `update` tests for layout, HiDPI sizes and `SetVideoView`; the WGSL conversion tested on a bare wgpu device (iced's re-exported `wgpu`): render a known NV12 frame to a texture, read it back and check pixel colours for BT.709 limited and full range (skipped without an adapter, as on CI).

### Task 11: App: controls, pickers, preview, Video settings (phase9.11, `feat(app)`)
- Camera and Share buttons, Wayland portal flow, X11/Windows picker grid, share-audio checkbox, quality picker (Q12), a "You are sharing" bar with Stop that stays visible above every overlay and a camera-on mark, self-view, Video card (device, quality, codec, encoder status, "Prefer the integrated GPU on battery", upload cap), shortcuts (Ctrl+Shift+V camera, Ctrl+Shift+S share), `docs/accessibility.md`.

Acceptance: `update` tests for every control and error path (portal cancelled, device unplugged, encoder lost).

### Task 12: Windows and macOS (phase9.12, `feat(video)` + core)
- Windows: WGC capture, NVENC on Ada and newer and Vulkan Video elsewhere, with D3D11 shared-handle import, Media Foundation encoder fallback, Media Foundation camera, WASAPI process-loopback share audio.
- macOS: ScreenCaptureKit with the system picker, VideoToolbox H.264 (and HEVC off, Q4), AVFoundation camera, ScreenCaptureKit audio; `NSCameraUsageDescription` and screen-recording notes for Phase 7.

Acceptance: CI builds and unit tests on both OSes; hardware tests are human items until a machine is on record (rule 9).

### Task 13: Steam Frame (phase9.13, `feat(video)`; conditional)
- V4L2 stateful H.264 encode (`v4l2r`), gamescope/KDE capture through the portal; measured CPU and battery cost. Runs only when the human has the device available.

### Task 14: 4K60 acceptance and tuning (phase9.14, `feat(video)` + docs)
- The full-path hardware suite: encrypted room, 120 s, 4K60 screen share and 4K60 camera (test pattern), measuring delivered fps, freezes, capture-to-feed latency from the embedded timestamps (the last hop to the screen is a human item), encode and decode time, CPU and GPU load, uplink bitrate. Default bitrates and presets tuned from the results.
- A/V offset: the test pattern flashes white with a 1 kHz tone every 2 s; the receiver measures the offset between the flash in the feed and the tone in the mixed output.
- `packet_buffer_size_video` in `dev/livekit/livekit.yaml` raised to what the measurements need, and the value documented for server admins in `docs/building.md`.

Acceptance on the human's machine, per available backend: delivered ≥ 58 fps average, no freeze over 250 ms, p95 capture-to-feed latency under 120 ms on the local stack, encode p99 under 10 ms, A/V offset within ±45 ms; the results table in PROGRESS.md. Decoding is still software here (Q7): if software 4K60 H.264 decode cannot hold 58 fps, the shortfall is recorded and the receive-side fps gate moves to Task 15.

### Task 15: Hardware decode (phase9.15, `feat(video)` + core; gated on Q5's receive side)
- Encoded frames from the receive passthrough into our decoders (NVDEC on Ada and newer, Vulkan Video H.264 on Intel, AMD and NVIDIA before Ada, Media Foundation, VideoToolbox), software fallback kept; keyframe requests on decoder errors.

Acceptance: the Task 14 suite with hardware decode, receiver CPU cost recorded before and after.

### Task 16: Wrap-up (phase9.16, `docs`)
- Independent review of `main..HEAD` (privacy: frames, titles, keys; every `unsafe` listed), upstream status of the Q5 fork, the checkpoint list, ROADMAP status.

## Human checkpoint (never ticked by the agent; rule 9)
- [x] Plan questions Q1-Q12 answered and plan approved; crates in Q11 approved. Approved 2026-10-13 (Q6-Q8 as the human chose, the rest as recommended).
- [ ] Encrypted call with Element Call in Chrome and Firefox (and Safari if a Mac, iPhone or iPad is available): our 4K60 screen share and camera shown sharp, with sound, in spotlight; their VP8 camera and screen share, and their share audio, shown and heard here.
- [ ] Lip sync by eye and ear with a camera at 4K60 and 1080p30.
- [ ] Wayland portal picker on KDE and GNOME, restore token, stop from the compositor; X11 picker.
- [ ] A real 4K60 webcam (or the best camera on hand) at its top mode; hot-plug.
- [ ] Screen-share audio from a game and a video, without our own call audio in it.
- [ ] Colours (limited/full range), fullscreen and pop-out, HiDPI and mixed-scale monitors, the sharing bar always visible, light and dark themes, keyboard-only use.
- [ ] Unplugging power during a 4K60 share on the hybrid laptop: encoding moves to the iGPU or lowers quality without dropping the share.
- [ ] Windows and macOS: permissions, pickers, capture and encode on real hardware.
- [ ] An AMD GPU and an NVIDIA GPU before Ada encoding (and, after Task 15, decoding) through Vulkan Video; an NVIDIA GPU on nouveau or NVK falling back as the probe says.
- [ ] Steam Frame, if Task 13 runs.

## Options considered and deferred
- **Replacing libwebrtc with str0m** (Q3 c): full control of transport and E2EE in Rust; revisit after Phase 9 with the measurements from Task 14.
- **Zero-copy decode to screen:** waits for an iced release on wgpu ≥30 and multi-plane DMA-BUF import in wgpu.
- **Camera H.264 passthrough** (UVC cameras that encode themselves): single layer only, revisit with Q5's layers.
- **AV1 by default:** only once livekit-client can decrypt AV1.
</content>
</invoke>
